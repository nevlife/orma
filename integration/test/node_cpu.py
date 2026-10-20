#!/usr/bin/env python3
"""Per-node CPU against the real ormad and orma-collectd (design 15.4).

Four Burner components in one container: node i spends i ms of CPU every 20 ms. The design's
10 ms period would load the container's core to 100 %; 20 ms keeps it at 50 % (the 70 % cap)
and the 1:2:3:4 ratio. Cases:

- shared: component_container. Node CPU in the ratio 1:2:3:4 within 5 %; per window the nodes and
  the shared part add up to the process, and over the measured windows the process matches
  /proc/<pid>/stat within 2 %.
- sleep: node 2 also sleeps 2 ms in each callback. Its CPU leaves the sleep out and its wall
  clock busy time is 2 ms per callback more.
- isolated: component_container_isolated. Each node's executor thread is in the node's bucket.

Methods (item 5): uprobe, preload (LD_PRELOAD and ormad's scheduler probes) and lttng (LTTng and
ormad's scheduler probes). Needs root for ormad (sudo -n) and a sourced workspace.

    node_cpu.py [--method M]... [--case C]... [--humble IMAGE WS]

With --humble the container runs in IMAGE, from the Humble workspace WS (built there first if
it is not), and the host's ormad and orma-collectd watch it (design 18, Humble item 5).
"""

import argparse
import json
import os
import shutil
import sys
import tempfile
import time
from pathlib import Path

import orma_client
import common
from ament_index_python.packages import get_package_prefix

PERIOD_MS = 20
SLEEP_MS = 2.0
CPU = 9
WARM, MEASURE = 4, 10
RATIO_TOL, SUM_TOL, SLEEP_TOL = 0.05, 0.02, 0.10


def proc_ticks(pid):
    f = open(f"/proc/{pid}/stat").read().rsplit(")", 1)[1].split()
    return int(f[11]) + int(f[12])


def steady(w, pid):
    """Each burner ran at its timer's rate in window `w`, within 10 %."""
    hz = 1000 / PERIOD_MS
    rates = [n["cb_hz"] for n in w["nodes"] if n["pid"] == pid and n["fqn"].startswith("/burner")]
    return len(rates) == 4 and all(abs(r / hz - 1) <= 0.1 for r in rates)


def ready(method, pid, collectd):
    if method == "uprobe":
        common.wait_for(
            lambda: common.uprobes_on(pid, "ros_trace_rcl_node_init", "ros_trace_callback_start"),
            "ormad's uprobes in the container",
            seconds=300,
        )
    elif method == "lttng":
        common.wait_for(collectd.session_up, "the LTTng session")
        common.wait_for(
            lambda: f"PID: {pid} " in (collectd.lttng("list", "-u") or ""),
            "the container to register with the session daemon",
        )


def run_case(launcher, collectd, method, case, check, tmp):
    exe = "component_container_isolated" if case == "isolated" else "component_container"
    name = f"orma_cpu_{case}"
    env = {}
    if method == "preload":
        env["LD_PRELOAD"] = launcher.lib("orma_preload", "liborma_preload.so")
    if method == "lttng":
        env["LD_PRELOAD"] = launcher.lib("orma_lttng", "liborma_lttng.so")
        env["LTTNG_HOME"] = str(collectd.lttng_home)
    pid = launcher.start([launcher.container(exe), "--ros-args", "-r", f"__node:={name}"], CPU, env)
    client = orma_client.connect(start=False)
    try:
        client.request("configure", window_s=1.0, sched=True)
        client.request("select", all=False, pids=[pid], method_by_pid={str(pid): method})
        client.request("subscribe", streams=["window"])
        ready(method, pid, collectd)

        def over_cap(w):
            p = next((p for p in w["processes"] if p["pid"] == pid), None)
            cpu = p and p["cpu_pct"]
            return cpu and cpu > common.CAP_PCT and f"the container's {cpu:.0f} %"

        common.windows(client, 2, over_cap)
        for i in (1, 2, 3, 4):
            argv = [
                "ros2",
                "component",
                "load",
                "--no-daemon",
                "--spin-time",
                "5",
                f"/{name}",
                "orma_integration",
            ]
            argv += ["orma_integration::Burner", "-n", f"burner{i}"]
            argv += ["-p", f"busy_ms:={float(i)}", "-p", f"period_ms:={PERIOD_MS}"]
            if case == "sleep" and i == 2:
                argv += ["-p", f"sleep_ms:={SLEEP_MS}"]
            launcher.run(argv)
        # Measure from WARM windows in a row that came live (not out of the backlog, so
        # /proc/<pid>/stat is read at their end) with every burner at its rate.
        run, t = 0, time.monotonic()
        for _ in range(300):
            if run == WARM:
                break
            w = common.windows(client, 1, over_cap)[0]
            live = time.monotonic() - t > 0.5
            t = time.monotonic()
            run = run + 1 if live and steady(w, pid) else 0
        else:
            sys.exit(f"{method} {case}: the burners never ran at their rate for {WARM} windows")
        ticks0 = proc_ticks(pid)
        ws = common.windows(client, MEASURE, over_cap)
        ticks1 = proc_ticks(pid)
    finally:
        client.close()
        launcher.stop()

    (tmp / f"{method}_{case}.json").write_text(json.dumps(ws))
    proc = [next(p for p in w["processes"] if p["pid"] == pid) for w in ws]
    burners = {}
    for w in ws:
        for n in w["nodes"]:
            if n["pid"] == pid and n["fqn"].startswith("/burner"):
                burners.setdefault(int(n["fqn"][7:]), []).append(n)
    tag = f"{method} {case}"
    check(sorted(burners) == [1, 2, 3, 4], f"{tag}: nodes /burner1-4 named: {sorted(burners)}")
    if sorted(burners) != [1, 2, 3, 4]:
        return
    unsteady = [w["seq"] for w in ws if not steady(w, pid)]
    check(not unsteady, f"{tag}: every burner at its rate in each window; not in {unsteady}")
    sources = {n["cpu_source"] for rows in burners.values() for n in rows}
    check(sources == {"sched"}, f"{tag}: cpu_source {sources}")
    cpu = {i: sum(n["cpu_pct"] for n in rows) / len(rows) for i, rows in burners.items()}
    total = sum(cpu.values())
    shares = {i: cpu[i] / total / (i / 10) - 1 for i in cpu}
    print(f"    {tag}: node CPU % {', '.join(f'{cpu[i]:.2f}' for i in sorted(cpu))}")
    check(
        all(abs(e) <= RATIO_TOL for e in shares.values()),
        f"{tag}: 1:2:3:4 within {RATIO_TOL:.0%}: errors "
        + ", ".join(f"{shares[i]:+.1%}" for i in sorted(shares)),
    )
    # Over the measured windows: a callback that straddles a window edge moves CPU between
    # neighbouring windows, and the shared part of a window cannot go below 0.
    nodes = shared = process = 0.0
    worst, at = 0.0, ""
    for w, p in zip(ws, proc):
        n = sum(x["cpu_pct"] for x in w["nodes"] if x["id"] in p["nodes"])
        nodes, shared, process = nodes + n, shared + p["shared_cpu_pct"], process + p["cpu_pct"]
        err = abs(n + p["shared_cpu_pct"] - p["cpu_pct"]) / p["cpu_pct"]
        if err >= worst:
            worst, at = err, f"{n:.2f} + {p['shared_cpu_pct']:.2f} against {p['cpu_pct']:.2f}"
    err = (nodes + shared) / process - 1
    check(
        abs(err) <= SUM_TOL,
        f"{tag}: nodes + shared = process within {SUM_TOL:.0%}: {err:+.2%} "
        f"(worst window {worst:.2%}: {at})",
    )
    span = sum(w["window_ns"] for w in ws) / 1e9
    orma_cpu = sum(p["cpu_pct"] * w["window_ns"] for w, p in zip(ws, proc)) / 1e9 / 100
    stat_cpu = (ticks1 - ticks0) / os.sysconf("SC_CLK_TCK")
    err = orma_cpu / stat_cpu - 1
    check(
        abs(err) <= SUM_TOL,
        f"{tag}: process CPU {orma_cpu:.3f} s against /proc/<pid>/stat {stat_cpu:.3f} s "
        f"over {span:.1f} s: {err:+.2%}",
    )
    if case == "sleep":
        rows = burners[2]
        calls = sum(r["cb_hz"] for r in rows)
        per_cb_ms = sum(r["busy_pct"] - r["cpu_pct"] for r in rows) / 100 / calls * 1e3
        check(
            abs(per_cb_ms / SLEEP_MS - 1) <= SLEEP_TOL,
            f"{tag}: node 2 wall busy - CPU per callback {per_cb_ms:.2f} ms, slept {SLEEP_MS} ms",
        )
    if case == "isolated":
        last = ws[-1]
        ids = {i: rows[-1]["id"] for i, rows in burners.items()}
        buckets = {t["bucket"] for t in last["threads"] if t["id"].startswith(f"t:{pid}:")}
        own = {i: f"node:{ids[i]}" in buckets for i in ids}
        check(all(own.values()), f"{tag}: a thread in each node's bucket: {own}")


class HostLauncher(common.Host):
    def container(self, exe):
        return os.path.join(get_package_prefix("rclcpp_components"), "lib/rclcpp_components", exe)


class DockerLauncher(common.Docker):
    def container(self, exe):
        return f"/opt/ros/humble/lib/rclcpp_components/{exe}"


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--method", action="append", choices=("uprobe", "preload", "lttng"))
    ap.add_argument("--case", action="append", choices=("shared", "sleep", "isolated"))
    ap.add_argument("--humble", nargs=2, metavar=("IMAGE", "WS"))
    a = ap.parse_args()
    methods = a.method or ["uprobe", "preload", "lttng"]
    cases = a.case or ["shared", "sleep", "isolated"]
    tmp = Path(tempfile.mkdtemp(prefix="orma-cpu-", dir="/tmp"))
    os.chmod(tmp, 0o755)
    if a.humble:
        ws = Path(a.humble[1]).resolve()
        common.build_humble_ws(a.humble[0], ws)
    failed = []

    def check(ok, text):
        print(("ok    " if ok else "FAIL  ") + text, flush=True)
        if not ok:
            failed.append(text)

    with common.ormad(tmp):
        collectd = common.Collectd(tmp)
        try:
            for method in methods:
                for case in cases:
                    if a.humble:
                        launcher = DockerLauncher(
                            a.humble[0], ws, str(CPU), mounts=[str(collectd.lttng_home.parent)]
                        )
                    else:
                        launcher = HostLauncher(collectd.env)
                    t0 = time.monotonic()
                    run_case(launcher, collectd, method, case, check, tmp)
                    print(f"  {method} {case}: {time.monotonic() - t0:.0f} s", flush=True)
        finally:
            collectd.stop()
    if failed:
        sys.exit(f"FAILED: {len(failed)} (kept {tmp})")
    shutil.rmtree(tmp)
    print("ok")


main()
