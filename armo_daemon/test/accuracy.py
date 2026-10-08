#!/usr/bin/env python3
"""armod counts every callback exactly once, times it and names it, for ROS and for flux.

Needs root for the probes, so colcon test does not run it: run it by hand after a build, with
passwordless sudo or from a root shell, in a shell that has sourced the workspace (the flux
caller imports flux, the ROS case runs demo_nodes_cpp). The probes are limited to the caller's
pid, so other ROS processes on the host are not touched.

armod is given no library: it has to find the ones each caller maps. The flux and flux2 callers
map a private copy of libflux_tracetools.so and libflux2_ffi.so, the way a build tree or a second
install would. The flux2 caller is C, built here against the installed flux2_ffi.

    armo_daemon/test/accuracy.py [path/to/armod] [path/to/libtracetools.so]
"""

import collections
import os
import shutil
import signal
import subprocess
import sys
import tempfile
import time
from pathlib import Path

HERE = Path(__file__).resolve().parent
ARMOD = sys.argv[1] if len(sys.argv) > 1 else str(HERE.parents[1] / "target/release/armod")
DISTRO = Path("/opt/ros") / os.environ.get("ROS_DISTRO", "jazzy")
LIB = sys.argv[2] if len(sys.argv) > 2 else str(DISTRO / "lib/libtracetools.so")
DEMO = DISTRO / "lib/demo_nodes_cpp"
SUDO = [] if os.geteuid() == 0 else ["sudo", "-n"]
EXPECT = {0xA0: (1000, 200), 0xB0: (300, 2000)}
# The early callback's time includes the late one dispatched inside it.
FLUX_EXPECT = {"early": (300, 1000), "late": (300, 200)}
FLUX2_EXPECT = FLUX_EXPECT


def find_flux_lib():
    for prefix in os.environ.get("AMENT_PREFIX_PATH", "").split(":"):
        lib = Path(prefix) / "lib/libflux_tracetools.so"
        if prefix and lib.is_file():
            return lib
    return None


def find_flux2_prefix():
    for var in ("CMAKE_PREFIX_PATH", "AMENT_PREFIX_PATH"):
        for prefix in os.environ.get(var, "").split(":"):
            p = Path(prefix)
            if (
                prefix
                and (p / "lib/libflux2_ffi.so").is_file()
                and (p / "include/flux2.h").is_file()
            ):
                return p
    return None


def start_armod(pid):
    return subprocess.Popen(
        SUDO + [ARMOD, "--pid", str(pid), "--interval", "0.5"],
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
        text=True,
    )


def stop_armod(armod):
    subprocess.run(SUDO + ["kill", "-INT", str(armod.pid)], check=True)
    return armod.communicate(timeout=10)


def watch(caller_argv, env=None):
    """Run a caller under armod. Returns the caller's first line, armod's stderr, and armod's
    rows per callback."""
    caller = subprocess.Popen(
        caller_argv, stdin=subprocess.PIPE, stdout=subprocess.PIPE, text=True, env=env
    )
    hello = caller.stdout.readline().strip()
    maps = Path(f"/proc/{caller.pid}/maps").read_text()
    armod = start_armod(caller.pid)
    time.sleep(1.5)
    caller.stdin.write("go\n")
    caller.stdin.flush()
    assert caller.stdout.readline().strip() == "done"
    time.sleep(1.5)
    out, err = stop_armod(armod)
    caller.wait(timeout=10)
    return hello, maps, err, *rows(out, caller.pid)


def rows(out, pid):

    counts = collections.Counter()
    weighted = collections.Counter()
    names = {}
    for line in out.splitlines():
        f = line.split()
        if len(f) < 8 or not f[2].startswith("0x"):
            continue
        assert f[0] == str(pid), line
        k = (f[1], f[2])
        counts[k] += int(f[3])
        weighted[k] += int(f[3]) * float(f[5])
        if len(f) > 8:
            names[k] = f[8]
    means = {k: weighted[k] / counts[k] for k in counts}
    return counts, means, names


def check(failed, label, count, mean, want_n, want_us):
    print(f"{label}: count {count} (want {want_n}), mean {mean:.1f} us (busy {want_us})")
    if count != want_n:
        failed.append(f"{label} count")
    if not want_us <= mean < want_us * 1.5:
        failed.append(f"{label} mean")


def check_named(failed, src, keys, expect, counts, means, names):
    by_name = {names.get(k): k for k in counts if k[0] == src}
    want = dict(zip(expect, keys.split()))
    if len(counts) != len(want) or set(by_name) != set(want.values()):
        failed.append(f"want {src} callbacks named {sorted(want.values())}, got {names}")
        return
    for which, (n, us) in expect.items():
        k = by_name[want[which]]
        check(failed, f"{src} {which} {want[which]}", counts[k], means[k], n, us)


def flux_case(failed, lib):
    with tempfile.TemporaryDirectory() as tmp:
        copy = Path(tmp) / "libflux_tracetools.so"
        shutil.copy(lib, copy)
        env = dict(os.environ, LD_LIBRARY_PATH=f"{tmp}:{os.environ.get('LD_LIBRARY_PATH', '')}")
        keys, maps, err, counts, means, names = watch(
            [sys.executable, str(HERE / "flux_caller.py")], env
        )
    if str(copy) not in maps:
        failed.append("the flux caller did not map the private copy; the case tests nothing")
    if f"attached to {copy}" not in err:
        failed.append(f"armod did not find the private copy: {err}")
    check_named(failed, "flux", keys, FLUX_EXPECT, counts, means, names)


def flux2_case(failed, prefix):
    """The early channel is named by its announced key, the late one by its signpost."""
    with tempfile.TemporaryDirectory() as tmp:
        # Under its soname, the name the caller's DT_NEEDED asks the loader for.
        real = (prefix / "lib/libflux2_ffi.so").resolve()
        copy = Path(tmp) / real.name
        shutil.copy(real, copy)
        (Path(tmp) / "libflux2_ffi.so").symlink_to(copy.name)
        exe = Path(tmp) / "flux2_caller"
        subprocess.run(
            ["cc", "-O2", "-I", prefix / "include", HERE / "flux2_caller.c"]
            + ["-L", tmp, "-lflux2_ffi", "-o", exe],
            check=True,
        )
        env = dict(os.environ, LD_LIBRARY_PATH=f"{tmp}:{os.environ.get('LD_LIBRARY_PATH', '')}")
        keys, maps, err, counts, means, names = watch([str(exe)], env)
    if str(copy) not in maps:
        failed.append("the flux2 caller did not map the private copy; the case tests nothing")
    if f"attached to {copy}" not in err:
        failed.append(f"armod did not find the private {copy.name}: {err}")
    check_named(failed, "flux2", keys, FLUX2_EXPECT, counts, means, names)


def ros_nodes(failed):
    """A listener that was running before armod attached is named by its topic."""
    env = dict(os.environ, ROS_DOMAIN_ID="87")
    quiet = {"stdout": subprocess.DEVNULL, "stderr": subprocess.DEVNULL}
    talker = subprocess.Popen([DEMO / "talker"], env=env, **quiet)
    listener = subprocess.Popen([DEMO / "listener"], env=env, **quiet)
    time.sleep(3)
    armod = start_armod(listener.pid)
    time.sleep(4)
    out, _ = stop_armod(armod)
    for p in (listener, talker):
        p.send_signal(signal.SIGINT)
        p.wait(timeout=10)
    counts, _, names = rows(out, listener.pid)
    named = {names.get(k) for k in counts if k[0] == "ros"}
    print(f"ros listener attached late: callbacks named {sorted(map(str, named))}")
    if "/chatter" not in named:
        failed.append(f"listener callback not named /chatter: {names}")


def main():
    failed = []

    _, _, _, counts, means, names = watch([sys.executable, str(HERE / "caller.py"), LIB])
    for callback, (n, us) in EXPECT.items():
        k = ("ros", f"{callback:#x}")
        check(failed, f"ros {callback:#x}", counts[k], means.get(k, 0.0), n, us)
    if names.get(("ros", "0xa0")) != "known_callback_a":
        failed.append("name of 0xa0")
    extra = set(counts) - {("ros", f"{c:#x}") for c in EXPECT}
    if extra:
        failed.append(f"unexpected callbacks {sorted(extra)}")

    # Each flux generation is checked when it is installed: a workspace may hold only one.
    skipped = []
    flux_lib, flux2_prefix = find_flux_lib(), find_flux2_prefix()
    if flux_lib:
        flux_case(failed, flux_lib)
    else:
        skipped.append("flux (libflux_tracetools.so not under AMENT_PREFIX_PATH)")
    if flux2_prefix:
        flux2_case(failed, flux2_prefix)
    else:
        skipped.append("flux2 (flux2_ffi not under CMAKE_PREFIX_PATH)")
    ros_nodes(failed)

    if skipped:
        print("skipped:", "; ".join(skipped))
    if failed:
        print("FAILED:", ", ".join(failed))
        return 1
    print("ok")
    return 0


if __name__ == "__main__":
    sys.exit(main())
