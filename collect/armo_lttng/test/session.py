"""What the LTTng tests share: a private session daemon (its own LTTNG_HOME, so a test neither
uses nor starts the user's daemon), babeltrace's events, the flux2 caller, and the agreement
workload with its counts. Homes sit under /tmp because the daemon's unix socket paths must fit in
108 bytes."""

import os
import re
import shutil
import signal
import struct
import subprocess
import sys
import tempfile
import time
from collections import Counter
from contextlib import contextmanager
from pathlib import Path

from ament_index_python.packages import get_package_prefix, get_package_share_directory

HERE = Path(__file__).resolve().parent
COUNT = 400


@contextmanager
def daemon():
    home = tempfile.mkdtemp(prefix="armo-lt-", dir="/tmp")
    env = dict(os.environ, LTTNG_HOME=home)
    subprocess.run(["lttng-sessiond", "--no-kernel", "--daemonize"], env=env, check=True)
    try:
        yield env
    finally:
        pid = open(os.path.join(home, ".lttng", "lttng-sessiond.pid")).read().strip()
        subprocess.run(["kill", pid], check=False)
        for _ in range(50):
            if not os.path.exists(f"/proc/{pid}"):
                break
            time.sleep(0.1)
        shutil.rmtree(home, ignore_errors=True)


def lttng(env, *args):
    subprocess.run(
        ["lttng", "--no-sessiond", *args], env=env, check=True, stdout=subprocess.DEVNULL
    )


@contextmanager
def session(env, out, events):
    """A started session writing to `out`, with `events` and the vpid and vtid contexts."""
    name = f"armo_test_{os.getpid()}"
    lttng(env, "create", name, "--output", str(out))
    try:
        lttng(env, "enable-event", "--userspace", events, "--session", name)
        lttng(env, "add-context", "--userspace", "-t", "vpid", "-t", "vtid", "--session", name)
        lttng(env, "start", name)
        yield
        lttng(env, "stop", name)
    finally:
        lttng(env, "destroy", name)


def events(trace):
    """(name, fields) of each event; fields are text, arrays as lists of text."""
    run = subprocess.run(
        ["babeltrace", "--no-delta", "-n", "payload,context", str(trace)],
        capture_output=True,
        text=True,
    )
    if "[error]" in run.stderr:
        raise RuntimeError(f"babeltrace: {run.stderr}")
    for line in run.stdout.splitlines():
        m = re.search(r" (\w+:\w+): (.*)$", line)
        if not m:
            continue
        fields = {}
        for key, array, value in re.findall(
            r'(\w+) = (?:\[ ((?:\[\d+\] = \w+(?:, )?)*) ?\]|("[^"]*"|-?\w+))', m[2]
        ):
            fields[key] = re.findall(r"\] = (\w+)", array) if array else value
        yield m[1], fields


def count(evs):
    return Counter(name for name, _ in evs)


CALLER = Path(__file__).resolve().parents[3] / "daemon/armo_daemon/test/flux2_caller.c"


def flux2_caller(out):
    """Builds armo_daemon's flux2 caller against the workspace's flux2_ffi into `out`."""
    for p in os.environ.get("CMAKE_PREFIX_PATH", "").split(":"):
        if p and (Path(p) / "include/flux2.h").is_file():
            prefix = Path(p)
            break
    else:
        raise RuntimeError("flux2_ffi is not under CMAKE_PREFIX_PATH: source the workspace")
    exe = Path(out) / "flux2_caller"
    subprocess.run(
        ["cc", "-O2", "-I", prefix / "include", CALLER, "-L", prefix / "lib", "-lflux2_ffi"]
        + ["-Wl,-rpath," + str(prefix / "lib"), "-o", exe],
        check=True,
    )
    return exe


def wait_for(check, what, seconds=10):
    deadline = time.monotonic() + seconds
    while not check():
        if time.monotonic() > deadline:
            sys.exit(f"timed out waiting for {what}")
        time.sleep(0.05)


@contextmanager
def workload(node, env, lttng_home, started, flux2_caller=None):
    """Both libraries in each process (LD_PRELOAD=libarmo_preload.so:libarmo_lttng.so): a C++
    publisher and subscriber, an rclpy pair with armo_rclpy and, if given, the flux2 caller, on
    CPUs 8 to 11. Calls `started` with their pids once they run, so that a recording can target
    them; yields the pids and a window that starts 2.5 s later, when armo-collectd has armed their
    shared memory files, and ends after the publishing; ends them on exit."""
    libs = [
        Path(get_package_prefix(p)) / "lib" / f"lib{p}.so" for p in ("armo_preload", "armo_lttng")
    ]
    autoload = Path(get_package_share_directory("armo_rclpy")) / "autoload"
    wenv = dict(
        env,
        LD_PRELOAD=":".join(map(str, libs)),
        LTTNG_HOME=str(lttng_home),
        PYTHONPATH=f"{autoload}:{env.get('PYTHONPATH', '')}",
    )
    cmds = [
        ([node, "sub"], 8),
        ([node, "pub", str(COUNT)], 9),
        ([sys.executable, HERE / "node.py", "sub"], 10),
        ([sys.executable, HERE / "node.py", "pub", str(COUNT)], 11),
    ]
    procs = [
        subprocess.Popen(
            ["taskset", "-c", str(cpu), *cmd],
            env=wenv,
            start_new_session=True,
            stdout=subprocess.DEVNULL,
            stderr=subprocess.DEVNULL,
        )
        for cmd, cpu in cmds
    ]
    flux2 = None
    if flux2_caller:
        flux2 = subprocess.Popen(
            ["taskset", "-c", "11", flux2_caller],
            env=wenv,
            stdin=subprocess.PIPE,
            stdout=subprocess.DEVNULL,
            text=True,
        )
    pids = {p.pid for p in procs} | ({flux2.pid} if flux2 else set())
    t0 = time.monotonic_ns() + 2_500_000_000
    started(pids)
    time.sleep(max(0, t0 - time.monotonic_ns()) / 1e9)
    if flux2:
        flux2.stdin.write("go\n")
        flux2.stdin.flush()
    time.sleep(4.5)
    t1 = time.monotonic_ns()
    time.sleep(0.5)
    try:
        yield pids, (t0, t1)
    finally:
        for p in procs:
            os.killpg(p.pid, signal.SIGINT)
        for p in procs + ([flux2] if flux2 else []):
            p.wait(timeout=20)


def names():
    """armo event number to the LTTng event that carries it."""
    header = Path(get_package_prefix("armo_preload")) / "include/armo_preload/armo_preload"
    out = {}
    text = (header / "armo_events.h").read_text()
    for name, n in re.findall(r"#define ARMO_EV_(\w+) (\d+)", text):
        name = name.lower()
        out[int(n)] = f"flux2:{name[6:]}" if name.startswith("flux2_") else f"armo:{name}"
    return out


def preload_counts(rec, pids, window):
    """Events per LTTng name in a recording's event files, of `pids` and in `window`."""
    by_id, counts = names(), Counter()
    for f in sorted((rec / "events").glob("*.armoev")):
        data = f.read_bytes()
        at = 16
        while at < len(data):
            pid, _, event, length, _, t = struct.unpack_from("<IIHHIQ", data, at)
            if pid in pids and window[0] <= t < window[1]:
                counts[by_id[event]] += 1
            at += 8 + length
    return counts


def lttng_counts(ctf, pids, window):
    """armo and flux2 events per name in CTF, of `pids` and in `window`."""
    run = subprocess.run(
        ["babeltrace", "--clock-cycles", "--no-delta", "-n", "payload,context", str(ctf)],
        capture_output=True,
        text=True,
        check=True,
    )
    counts = Counter()
    for line in run.stdout.splitlines():
        m = re.match(r"\[(\d+)\] \S+ ((?:armo|flux2):\w+): .*vpid = (\d+)", line)
        if m and int(m[3]) in pids and window[0] <= int(m[1]) < window[1]:
            if m[2] != "armo:missing":
                counts[m[2]] += 1
    return counts


def print_counts(a, b):
    print(f"{'event':48} {'preload':>8} {'lttng':>8}")
    for event in sorted(set(a) | set(b)):
        print(f"{event:48} {a[event]:8} {b[event]:8}")
