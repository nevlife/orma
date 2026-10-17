#!/usr/bin/env python3
"""Run a tracing cost campaign (design 8.6).

usage: run.py OUT_DIR --plan PLAN [--reps N] [--window S] [--shield CPUS]

PLAN is a file of lines `workload rate sched method[,method...] [topics]`, e.g.
    cpp_chain 1000 fifo none,uprobe,lttng
topics defaults to 4; the rclpy chain at 1 kHz needs 1 (4 topics saturate its executor).
A run whose meta.json exists is skipped, so a stopped campaign resumes where it stopped.
Methods are run in the listed order on odd reps and reversed on even reps, block by block, so
each method's runs sit next to each other in time. Each run starts the workload fresh, waits
for discovery, attaches the method, waits, measures a window, detaches and stops the workload.
The run directory holds the sinks' latency CSVs, meta.json with the window and /proc counters,
and the method's logs. Analysis is analyze.py, run afterwards.

--shield moves every other task off CPUS (bench cores, e.g. 3-11) for the campaign and moves
them back at its end, also after a failure, SIGINT, SIGTERM or SIGHUP.

Load cap: once a second the campaign checks that no workload or tracer process used more than
70 % of a core and the machine not more than 70 % of all cores; otherwise it stops. SCHED_FIFO
threads are not throttled by a CPU quota, so the cap stops the campaign instead.
"""

import argparse
import json
import os
import re
import signal
import subprocess
import sys
import threading
import time
from contextlib import contextmanager

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
import methods as M  # noqa: E402
import workloads as W  # noqa: E402

DISCOVERY_S = 6
SETTLE_S = 4
CAP_PCT = 70.0
CLK = os.sysconf("SC_CLK_TCK")
SHIELD = os.path.join(os.path.dirname(os.path.abspath(__file__)), "shield.py")


class OverCap(Exception):
    pass


def base_env():
    return dict(
        os.environ,
        ROS_DOMAIN_ID="77",
        RMW_IMPLEMENTATION="rmw_cyclonedds_cpp",
        ROS_AUTOMATIC_DISCOVERY_RANGE="LOCALHOST",
        PYTHONUNBUFFERED="1",
    )


def proc_stat(pid):
    f = open(f"/proc/{pid}/stat").read()
    rest = f[f.rfind(")") + 2 :].split()
    return {
        "utime": int(rest[11]),
        "stime": int(rest[12]),
        "minflt": int(rest[7]),
        "majflt": int(rest[9]),
    }


def ctxsw(pid):
    vol = inv = 0
    for t in os.listdir(f"/proc/{pid}/task"):
        s = open(f"/proc/{pid}/task/{t}/status").read()
        vol += int(re.search(r"voluntary_ctxt_switches:\s+(\d+)", s).group(1))
        inv += int(re.search(r"nonvoluntary_ctxt_switches:\s+(\d+)", s).group(1))
    return {"vcsw": vol, "ivcsw": inv, "threads": len(os.listdir(f"/proc/{pid}/task"))}


def snapshot(pids):
    out = {}
    for label, pid in pids:
        d = proc_stat(pid)
        if label.startswith("w:"):
            d.update(ctxsw(pid))
        out[f"{label}:{pid}"] = d
    return out


def cpu_stat():
    out = {}
    for line in open("/proc/stat"):
        if line.startswith("cpu"):
            f = line.split()
            out[f[0]] = [int(x) for x in f[1:8]]
    return out


def mem(pid, root=False):
    cmd = (["sudo", "-n"] if root else []) + ["cat", f"/proc/{pid}/smaps_rollup"]
    s = subprocess.run(cmd, capture_output=True, text=True).stdout
    return {k.lower(): int(v) for k, v in re.findall(r"^(Rss|Pss):\s+(\d+) kB", s, re.M)}


class Guard(threading.Thread):
    """Samples the tracked processes and the machine once a second; `tripped` says why the load
    went over the cap."""

    def __init__(self):
        super().__init__(daemon=True)
        self.pids = []
        self.tripped = None
        self.done = threading.Event()

    def run(self):
        def ticks(pid):
            try:
                s = proc_stat(pid)
            except OSError:
                return None
            return s["utime"] + s["stime"]

        def busy():
            c = cpu_stat()["cpu"]
            return sum(c) - c[3] - c[4], sum(c)

        last = {pid: ticks(pid) for _, pid in self.pids}
        b0, t0, w0 = *busy(), time.monotonic()
        while not self.done.wait(1.0):
            now = {pid: ticks(pid) for _, pid in self.pids}
            b1, t1, w1 = *busy(), time.monotonic()
            for label, pid in self.pids:
                if now.get(pid) is not None and last.get(pid) is not None:
                    pct = (now[pid] - last[pid]) / CLK / (w1 - w0) * 100
                    if pct > CAP_PCT:
                        self.tripped = f"{label} {pid} used {pct:.0f} % of a core"
            if t1 > t0 and (b1 - b0) / (t1 - t0) * 100 > CAP_PCT:
                self.tripped = f"the machine was {(b1 - b0) / (t1 - t0) * 100:.0f} % busy"
            last, b0, t0, w0 = now, b1, t1, w1


def wait(seconds, guard):
    end = time.monotonic() + seconds
    while time.monotonic() < end:
        if guard.tripped:
            raise OverCap(guard.tripped)
        time.sleep(0.2)


class Ctx:
    pass


def run_one(out_dir, rep, wl_name, rate, sched, spec, window, env0, topics):
    tag = f"r{rep}_{wl_name}_{rate}_{sched}_t{topics}_{spec}"
    ctx = Ctx()
    ctx.rundir = os.path.join(out_dir, tag)
    if os.path.exists(os.path.join(ctx.rundir, "meta.json")):
        print(f"skip {tag} (done)", flush=True)
        return
    os.makedirs(ctx.rundir, exist_ok=True)
    wl = W.REGISTRY[wl_name](rate, topics)
    comps = M.build(spec)
    meta = {
        "rep": rep,
        "workload": wl_name,
        "rate": rate,
        "sched": sched,
        "method": spec,
        "window_s": window,
        "topics": topics,
    }
    try:
        for c in comps:
            c.check(wl)
    except M.Pending as e:
        meta["pending"] = str(e)
        json.dump(meta, open(os.path.join(ctx.rundir, "meta.json"), "w"), indent=1)
        print(f"pending {tag}: {e}", flush=True)
        return

    print(f"run {tag}", flush=True)
    prepared, attached, procs = [], [], []
    guard = Guard()
    try:
        env = dict(env0)
        for c in comps:
            prepared.append(c)
            for k, v in c.prepare(ctx).items():
                if k == "PYTHONPATH_PREPEND":
                    env["PYTHONPATH"] = v + os.pathsep + env.get("PYTHONPATH", "")
                elif k == "LD_PRELOAD_APPEND":
                    env["LD_PRELOAD"] = (env.get("LD_PRELOAD", "") + " " + v).strip()
                else:
                    env[k] = v
        ctx.env = env
        for p in wl.procs():
            cmd = ["taskset", "-c", str(p.core)] + (["chrt", "-f", "50"] if sched == "fifo" else [])
            log = open(os.path.join(ctx.rundir, f"{p.role}.log"), "w")
            proc = subprocess.Popen(
                cmd + p.cmd,
                env=env,
                cwd=ctx.rundir,
                stdout=log,
                stderr=subprocess.STDOUT,
                start_new_session=True,
            )
            procs.append((p, proc, log))
            time.sleep(0.3)
        wl.after_start(ctx, procs)
        tracked = [(f"w:{p.role}", proc.pid) for p, proc, _ in procs]
        guard.pids = list(tracked)
        guard.start()
        wait(DISCOVERY_S, guard)
        for c in comps:
            attached.append(c)
            c.attach(ctx, [pid for _, pid in tracked])
        for c in comps:
            tracked += c.daemons()
        guard.pids = list(tracked)
        wait(SETTLE_S, guard)

        s0, c0 = snapshot(tracked), cpu_stat()
        m0 = {c.name: c.sample() for c in comps}
        t0 = time.monotonic_ns()
        wait(window, guard)
        t1 = time.monotonic_ns()
        s1, c1 = snapshot(tracked), cpu_stat()
        m1 = {c.name: c.sample() for c in comps}

        meta.update(t0=t0, t1=t1, clk_tck=CLK)
        meta["procs"] = {p.role: {"pid": proc.pid, "core": p.core} for p, proc, _ in procs}
        meta["proc0"], meta["proc1"], meta["cpu0"], meta["cpu1"] = s0, s1, c0, c1
        meta["method0"], meta["method1"] = m0, m1
        meta["mem"] = {p.role: mem(proc.pid) for p, proc, _ in procs}
        meta["mem_daemons"] = {
            f"{n}:{pid}": mem(pid, root=(n == "armod")) for n, pid in tracked if n[:2] != "w:"
        }
        meta["sched_check"] = {
            p.role: subprocess.run(
                ["chrt", "-p", str(proc.pid)], capture_output=True, text=True
            ).stdout.strip()
            for p, proc, _ in procs
        }
        meta["method_stats"] = {}
        while attached:
            c = attached.pop()
            meta["method_stats"][c.name] = c.detach(ctx)
    finally:
        guard.done.set()
        # Stopped mid-run: no workload running and no tracer attached, in that order, so a
        # detach that fails leaves no workload behind.
        for _, proc, log in reversed(procs):
            M.stop(proc)
            log.close()
        while attached:
            attached.pop().detach(ctx)
        for c in prepared:
            c.cleanup()
    json.dump(meta, open(os.path.join(ctx.rundir, "meta.json"), "w"), indent=1)


def raise_exit(signum, frame):
    raise SystemExit(f"stopped by signal {signum}")


@contextmanager
def shielded(spec, path):
    """Every other task off `spec` until the block ends, however it ends."""
    if not spec:
        yield
        return
    if os.path.exists(path):
        # Left by a run that was killed before it could restore.
        subprocess.run(["sudo", "-n", sys.executable, SHIELD, "restore", path, spec], check=True)
    for s in (signal.SIGTERM, signal.SIGHUP):
        signal.signal(s, raise_exit)
    print(f"shield: state in {path}; `sudo {SHIELD} restore {path} {spec}` undoes it", flush=True)
    try:
        # A Ctrl-C does not cut a save or restore short: each takes well under a second.
        signal.signal(signal.SIGINT, signal.SIG_IGN)
        subprocess.run(["sudo", "-n", sys.executable, SHIELD, "save", path, spec], check=True)
        signal.signal(signal.SIGINT, signal.default_int_handler)
        yield
    finally:
        signal.signal(signal.SIGINT, signal.SIG_IGN)
        subprocess.run(["sudo", "-n", sys.executable, SHIELD, "restore", path, spec], check=True)
        signal.signal(signal.SIGINT, signal.default_int_handler)


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("out")
    ap.add_argument("--plan", required=True)
    ap.add_argument("--reps", type=int, default=3)
    ap.add_argument("--window", type=float, default=60.0)
    ap.add_argument("--shield", metavar="CPUS")
    a = ap.parse_args()
    a.out = os.path.abspath(a.out)
    os.makedirs(a.out, exist_ok=True)
    blocks = []
    for line in open(a.plan):
        line = line.split("#")[0].strip()
        if line:
            f = line.split()
            topics = int(f[4]) if len(f) > 4 else W.TOPICS
            blocks.append((f[0], int(f[1]), f[2], f[3].split(","), topics))
    env0 = base_env()
    sums = M.freeze(a.out)
    json.dump({"bin_sha256": sums}, open(os.path.join(a.out, "campaign.json"), "w"), indent=1)
    with shielded(a.shield, os.path.join(a.out, "shield_state.txt")):
        for rep in range(1, a.reps + 1):
            for wl, rate, sched, ms, topics in blocks:
                for spec in ms if rep % 2 else list(reversed(ms)):
                    run_one(a.out, rep, wl, rate, sched, spec, a.window, env0, topics)
    subprocess.run(["ros2", "daemon", "stop"], env=env0, capture_output=True)
    print("campaign done", flush=True)


if __name__ == "__main__":
    main()
