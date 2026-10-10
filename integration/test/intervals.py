#!/usr/bin/env python3
"""Interval distributions per method (design 3.1 last item, 8 method agreement).

The same workload runs once per method, each run fresh: two node.cpp on one topic (CPUs 9 and
10), a timer at 100 Hz publishing to both, a service call every other tick. A real
armo-collectd records each run by its method and `armo-analyze` splits the recording into the
intervals of 2.5. preload and uprobe run twice, so the difference between two runs of one method
(A/A) is measured too.

The intervals may differ only by the methods' cost (8): for each interval and quantile, the
difference between two methods must stay within `tolerance` (recorded in design 18.1).

    intervals.py [--runs preload,lttng,uprobe,preload,uprobe] [--ticks N]
"""

import argparse
import csv
import os
import shutil
import subprocess
import sys
import tempfile
from pathlib import Path

import armo_client
import common
import numpy as np
from ament_index_python.packages import get_package_prefix

A = common.accuracy
QS = (50, 90)
# A method's cost (design 18.1). Inside one thread, per event: half a callback_start/end pair
# with two threads (8.1); lttng is libarmo_lttng's armo event, which 8.1 did not measure
# (dispatch, 8 events: lttng - preload 4.4 us on 2026-10-10). On a path that publishes, per hop:
# the p99 rise of 8.6 over its two hops, upper end.
EVENT_US = {"preload": 0.05, "lttng": 0.6, "uprobe": 1.77}
HOP_US = {"preload": 3.0, "lttng": 11.0, "uprobe": 40.0}
# Events a method adds inside the intervals that stay in one thread, counted in a recording.
EVENTS_IN = {"dispatch": 8, "callback": 2}
INTERVALS = ("hop", "wake", "dispatch", "callback", "callback_all")
# Run-to-run spread of one method on this machine: the largest A/A difference of lttng and uprobe
# over 21 runs on 2026-10-09, times 1.5. hop and wake switch between modes 40 to 60 us apart.
NOISE_US = {
    ("hop", 50): 85.0, ("hop", 90): 155.0,
    ("wake", 50): 90.0, ("wake", 90): 152.0,
    ("dispatch", 50): 2.5, ("dispatch", 90): 32.0,
    ("callback", 50): 0.5, ("callback", 90): 0.5,
    ("callback_all", 50): 0.5, ("callback_all", 90): 29.0,
}  # fmt: skip


def tolerance(interval, q, a, b):
    """|quantile(a) - quantile(b)| allowed: the methods' cost difference plus the run-to-run
    spread (design 18.1)."""
    if interval in EVENTS_IN:
        cost = EVENTS_IN[interval] * abs(EVENT_US[a] - EVENT_US[b])
    else:
        cost = abs(HOP_US[a] - HOP_US[b])
    return cost + NOISE_US[(interval, q)]


def run(method, collectd, client, exe, tmp, ticks, n):
    env = A.node_env("rmw_cyclonedds_cpp")
    env["ROS_DOMAIN_ID"] = common.DOMAIN
    if method in ("preload", "lttng"):
        p = "armo_preload" if method == "preload" else "armo_lttng"
        env["LD_PRELOAD"] = str(Path(get_package_prefix(p)) / "lib" / f"lib{p}.so")
    if method == "lttng":
        env["LTTNG_HOME"] = str(collectd.lttng_home)
    prefix = f"/armo_iv_{os.getpid()}_{n}"
    procs = [
        common.Waiting(["taskset", "-c", str(cpu), exe, prefix, str(ticks)], env) for cpu in (9, 10)
    ]
    pids = sorted(p.pid for p in procs)
    try:
        rec = client.request(
            "record_start",
            **armo_client.record_start(method, armo_client.targets(pids=pids), sched=False),
        )
        if method == "uprobe":
            for pid in pids:
                common.wait_for(
                    lambda: (
                        common.uprobes_on(pid, *common.HOT)
                        and common.uprobes_on(pid, "rcl_wait", lib="librcl.so")
                    ),
                    "armod's uprobes",
                    seconds=600,
                )
        if method == "lttng":
            common.wait_for(collectd.session_up, "the LTTng session")
            common.wait_for(
                lambda: all(f"PID: {p} " in (collectd.lttng("list", "-u") or "") for p in pids),
                "the nodes to register with the session daemon",
            )
        for p in procs:
            p.go()
        said = [p.line() for p in procs]
        client.request("record_stop", record_id=rec["record_id"])
    finally:
        for p in procs:
            if p.started:
                p.end()
            else:
                p.kill()
    subprocess.run(["armo-analyze", rec["dir"]], check=True, capture_output=True)
    return intervals(Path(rec["dir"]) / "analysis", prefix, pids), said


def intervals(out, prefix, pids):
    """Samples in us per interval of the workload's topic and callbacks."""
    got = {k: [] for k in INTERVALS}
    for row in csv.DictReader(open(out / "deliveries.csv")):
        if not row["topic"].endswith(prefix + "/out") or int(row["sub_pid"]) not in pids:
            continue
        for k in ("hop", "wake", "dispatch", "callback"):
            if row[f"{k}_ns"]:
                got[k].append(int(row[f"{k}_ns"]) / 1e3)
    for row in csv.DictReader(open(out / "callbacks.csv")):
        if int(row["callback"].split(":")[1]) in pids and row["duration_ns"]:
            got["callback_all"].append(int(row["duration_ns"]) / 1e3)
    return {k: np.array(v) for k, v in got.items()}


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--runs", default="preload,lttng,uprobe,preload,uprobe")
    ap.add_argument("--ticks", type=int, default=1500)
    a = ap.parse_args()
    runs = a.runs.split(",")
    tmp = Path(tempfile.mkdtemp(prefix="armo-iv-", dir="/tmp"))
    os.chmod(tmp, 0o755)
    exe = A.build_node(tmp)
    results = []
    with common.armod(tmp):
        collectd = common.Collectd(tmp)
        try:
            client = armo_client.connect(start=False)
            client.request("select", all=False, pids=[])
            for n, method in enumerate(runs):
                got, said = run(method, collectd, client, exe, tmp, a.ticks, n)
                print(f"run {n} {method}: {said}", flush=True)
                results.append((f"{method}#{n}", method, got))
            client.close()
        finally:
            collectd.stop()

    print(f"{'run':12} {'interval':13} {'n':>6}" + "".join(f" {f'p{q}':>9}" for q in QS))
    q = {}
    for name, _, got in results:
        for k, v in got.items():
            q[(name, k)] = [float(np.percentile(v, p)) if len(v) else float("nan") for p in QS]
            print(f"{name:12} {k:13} {len(v):6}" + "".join(f" {x:9.1f}" for x in q[(name, k)]))
    # Run-to-run spread, for comparison with NOISE_US.
    for i, (n1, m1, _) in enumerate(results):
        for n2, m2, _ in results[i + 1 :]:
            if m1 == m2:
                for k in INTERVALS:
                    d = [abs(x - y) for x, y in zip(q[(n1, k)], q[(n2, k)])]
                    print(
                        f"A/A  {k:13} {n1} {n2}: "
                        + ", ".join(f"p{p} {x:.1f}" for p, x in zip(QS, d))
                    )
    failed = []
    for i, (n1, m1, _) in enumerate(results):
        for n2, m2, _ in results[i + 1 :]:
            if m1 == m2:
                continue
            for k in INTERVALS:
                if np.isnan(q[(n1, k)][0]) or np.isnan(q[(n2, k)][0]):
                    print(f"FAIL {k:13} {n1} against {n2}: a run has no samples")
                    failed.append(f"{k}: no samples in {n1} or {n2}")
                    continue
                for p, x, y in zip(QS, q[(n1, k)], q[(n2, k)]):
                    tol = tolerance(k, p, m1, m2)
                    ok = abs(x - y) <= tol
                    print(
                        f"{'ok  ' if ok else 'FAIL'} {k:13} p{p} {n1} {x:.1f} {n2} {y:.1f}"
                        f" (|d| {abs(x - y):.1f} us, tol {tol:.1f})"
                    )
                    if not ok:
                        failed.append(f"{k} p{p} {n1} against {n2}")
    if failed:
        sys.exit(f"FAILED: {len(failed)} (kept {tmp})")
    shutil.rmtree(tmp)
    print("ok")


main()
