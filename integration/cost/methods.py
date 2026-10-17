"""Tracing methods, by name. A method spec joins components with '+', e.g. 'armo_rclpy+uprobe'.

A component may start what it needs and add environment variables before the workload starts
(prepare), attach to the running workload processes (attach), name its own daemon processes so
their CPU is counted (daemons), report counters at window start and end (sample), detach
(detach, returning final stats) and end what prepare started once the workload is gone
(cleanup). A component that does not apply to a workload raises Pending from check(); the run is
then recorded as pending instead of measured.
"""

import glob
import os
import re
import shutil
import signal
import subprocess
import sys
import tempfile
import time

from ament_index_python.packages import get_package_prefix, get_package_share_directory

# Frozen copies in the campaign's bin/ (freeze()), so every rep runs one build.
BIN = {}


def freeze(out):
    """Copies armod and the two backends into out/bin; returns their sha256 by name."""
    d = os.path.join(out, "bin")
    os.makedirs(d, exist_ok=True)
    srcs = {
        "armod": os.path.join(get_package_prefix("armo_daemon"), "bin/armod"),
        "libarmo_preload.so": os.path.join(get_package_prefix("armo_preload"), "lib"),
        "libarmo_lttng.so": os.path.join(get_package_prefix("armo_lttng"), "lib"),
    }
    sums = {}
    for name, src in srcs.items():
        src = src if name == "armod" else os.path.join(src, name)
        BIN[name] = shutil.copy(os.path.realpath(src), os.path.join(d, name))
        sums[name] = sh("sha256sum", BIN[name]).stdout.split()[0]
    return sums


class Pending(Exception):
    pass


def sh(*cmd, env=None):
    return subprocess.run(cmd, check=True, capture_output=True, text=True, env=env)


def stop(proc):
    proc.send_signal(signal.SIGINT)
    proc.wait()


class Component:
    name = "?"

    def check(self, workload):
        pass

    def prepare(self, ctx):
        return {}

    def attach(self, ctx, pids):
        pass

    def daemons(self):
        return []

    def sample(self):
        return {}

    def detach(self, ctx):
        return {}

    def cleanup(self):
        pass


class ArmoRclpy(Component):
    name = "armo_rclpy"

    def check(self, workload):
        if not workload.python:
            raise Pending("armo_rclpy applies to Python workloads only")

    def prepare(self, ctx):
        autoload = os.path.join(get_package_share_directory("armo_rclpy"), "autoload")
        return {"PYTHONPATH_PREPEND": autoload}


class Preload(Component):
    """libarmo_preload.so in every workload process, nothing reading it."""

    name = "preload"

    def prepare(self, ctx):
        return {"LD_PRELOAD_APPEND": BIN["libarmo_preload.so"]}


MONITOR = """
import signal, sys, armo_client
c = armo_client.connect(start=False)
c.request("configure", window_s=1.0, sched=False)
pids = [int(p) for p in sys.argv[1:]]
c.request("select", all=False, pids=pids, method_by_pid={str(p): "preload" for p in pids})
c.request("subscribe", streams=["window"])
signal.signal(signal.SIGINT, lambda *_: sys.exit(0))
while True:
    m = c.next_message()
    if m["type"] == "window":
        print(m["agent"]["shm"]["ring_drops"], flush=True)
"""


class PreloadArmed(Preload):
    """libarmo_preload.so with a reader: a private armo-collectd monitoring every workload pid
    by method preload in 1 s windows, without armod's scheduler probes."""

    name = "preload_armed"

    def attach(self, ctx, pids):
        self.xdg = tempfile.mkdtemp(prefix="armo-cost-", dir="/tmp")
        env = dict(os.environ, XDG_RUNTIME_DIR=self.xdg, HOME=self.xdg)
        self.collectd = subprocess.Popen(
            ["armo-collectd"],
            env=env,
            cwd="/",
            stdout=open(os.path.join(ctx.rundir, "collectd.log"), "w"),
            stderr=subprocess.STDOUT,
            start_new_session=True,
        )
        while not os.path.exists(os.path.join(self.xdg, "armo", "collectd.sock")):
            time.sleep(0.05)
        self.log = os.path.join(ctx.rundir, "ring_drops.txt")
        self.monitor = subprocess.Popen(
            [sys.executable, "-c", MONITOR, *map(str, pids)],
            env=env,
            stdout=open(self.log, "w"),
            start_new_session=True,
        )
        while os.path.getsize(self.log) == 0:
            time.sleep(0.1)

    def daemons(self):
        return [("armo-collectd", self.collectd.pid), ("armo-monitor", self.monitor.pid)]

    def detach(self, ctx):
        stop(self.monitor)
        self.collectd.terminate()
        self.collectd.wait()
        shutil.rmtree(self.xdg)
        return {"preload_ring_drops": int(open(self.log).read().split()[-1])}


class Uprobe(Component):
    """One armod in its text mode per workload process (it takes one --pid); BPF run time from
    the programs' fdinfo."""

    name = "uprobe"

    def __init__(self):
        self.procs = []
        self.armods = []
        self.stats_was = sh("sysctl", "-n", "kernel.bpf_stats_enabled").stdout.strip()

    def attach(self, ctx, pids):
        sh("sudo", "-n", "sysctl", "-q", "kernel.bpf_stats_enabled=1")
        for pid in pids:
            log = open(os.path.join(ctx.rundir, f"armod_{pid}.log"), "w")
            p = subprocess.Popen(
                ["sudo", "-n", BIN["armod"], "--pid", str(pid), "--interval", "1"],
                stdout=log,
                stderr=subprocess.STDOUT,
                start_new_session=True,
            )
            self.procs.append((p, log))
        for pid in pids:
            path = os.path.join(ctx.rundir, f"armod_{pid}.log")
            while "attached to" not in open(path).read():
                time.sleep(0.1)
        for p, _ in self.procs:
            self.armods.append(int(sh("pgrep", "-P", str(p.pid)).stdout))

    def daemons(self):
        return [("armod", pid) for pid in self.armods]

    def sample(self):
        run_ns = cnt = 0
        for pid in self.armods:
            # Non-BPF fds have neither line; grep exits 0 because the BPF ones match.
            out = sh(
                "sudo", "-n", "sh", "-c",
                f"grep -h -E '^(run_time_ns|run_cnt):' /proc/{pid}/fdinfo/*",
            ).stdout  # fmt: skip
            for k, v in re.findall(r"^(run_time_ns|run_cnt):\s+(\d+)", out, re.M):
                if k == "run_time_ns":
                    run_ns += int(v)
                else:
                    cnt += int(v)
        return {"bpf_run_time_ns": run_ns, "bpf_run_cnt": cnt}

    def detach(self, ctx):
        # From the sudo processes: attach may have stopped before it knew every armod.
        for p, _ in self.procs:
            for pid in subprocess.run(
                ["pgrep", "-P", str(p.pid)], capture_output=True, text=True
            ).stdout.split():
                sh("sudo", "-n", "kill", "-INT", pid)
        for p, log in self.procs:
            p.wait()
            log.close()
        sh("sudo", "-n", "sysctl", "-q", f"kernel.bpf_stats_enabled={self.stats_was}")
        lost = 0
        for path in glob.glob(os.path.join(ctx.rundir, "armod_*.log")):
            for line in re.findall(r"^-- window .* lost: (.*)$", open(path).read(), re.M):
                lost += sum(int(n) for n in re.findall(r"\d+", line))
        return {"armod_lost": lost}


class Lttng(Component):
    """A private session daemon and a session with ros2:* enabled, tracking only the workload
    pids (ros2_tracing's events and contexts; lttng's default channel: per-user buffers,
    4 x 512 KiB, discard)."""

    name = "lttng"
    events = ["ros2:*"]

    def prepare(self, ctx):
        # Under /tmp: the daemon's socket paths must fit in 108 bytes.
        self.home = tempfile.mkdtemp(prefix="armo-cost-lt-", dir="/tmp")
        self.env = dict(os.environ, LTTNG_HOME=self.home)
        sh("lttng-sessiond", "--no-kernel", "--daemonize", env=self.env)
        return {"LTTNG_HOME": self.home}

    def lttng(self, *args):
        return sh("lttng", "--no-sessiond", *args, env=self.env).stdout

    def attach(self, ctx, pids):
        self.out = os.path.join(ctx.rundir, "ctf")
        s = "cost"
        self.lttng("create", s, f"--output={self.out}")
        for e in self.events:
            self.lttng("enable-event", "-u", "-s", s, e)
        self.lttng("add-context", "-u", "-s", s, "-t", "procname", "-t", "vpid", "-t", "vtid")
        self.lttng("untrack", "-u", "-s", s, "--vpid", "--all")
        self.lttng("track", "-u", "-s", s, "--vpid=" + ",".join(map(str, pids)))
        self.lttng("start", s)

    def sessiond(self):
        return int(open(os.path.join(self.home, ".lttng", "lttng-sessiond.pid")).read())

    def daemons(self):
        d = self.sessiond()
        kids = sh("pgrep", "-P", str(d)).stdout.split()
        return [("lttng-sessiond", d)] + [("lttng-consumerd", int(k)) for k in kids]

    def detach(self, ctx):
        self.lttng("stop", "cost")
        listing = self.lttng("list", "cost")
        self.lttng("destroy", "cost")
        open(os.path.join(ctx.rundir, "lttng_list.txt"), "w").write(listing)
        discarded = sum(int(x) for x in re.findall(r"Discarded events:\s+(\d+)", listing))
        files = glob.glob(os.path.join(self.out, "**"), recursive=True)
        size = sum(os.path.getsize(f) for f in files if os.path.isfile(f))
        shutil.rmtree(self.out)
        return {"lttng_discarded": discarded, "ctf_bytes": size}

    def cleanup(self):
        d = self.sessiond()
        os.kill(d, signal.SIGTERM)
        while os.path.exists(f"/proc/{d}"):
            time.sleep(0.05)
        shutil.rmtree(self.home)


class ArmoLttng(Lttng):
    """libarmo_lttng.so in the workload, its armo and flux providers enabled as well."""

    name = "armo_lttng"
    events = ["ros2:*", "armo:*", "flux:*"]

    def prepare(self, ctx):
        return dict(super().prepare(ctx), LD_PRELOAD_APPEND=BIN["libarmo_lttng.so"])


REGISTRY = {c.name: c for c in (ArmoRclpy, Preload, PreloadArmed, Uprobe, Lttng, ArmoLttng)}


def build(spec):
    if spec == "none":
        return []
    return [REGISTRY[name]() for name in spec.split("+")]
