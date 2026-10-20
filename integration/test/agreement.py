#!/usr/bin/env python3
"""Method agreement on Jazzy with all three methods in one run (design 3, 3.1, 8).

Every workload process carries both libraries (LD_PRELOAD=liborma_preload.so:liborma_lttng.so)
and waits before it creates any ROS entity: two node.cpp on one topic, node.py with orma_rclpy
and, when the workspace has flux_ffi, orma_daemon's flux caller. A real orma-collectd records
them three times, by uprobe (the host's ormad), by preload and by lttng. Once ormad's probes are
on, the processes run. In a window that starts after every method is on, each event of each
process must be counted the same by all three; the LTTng recording's ros2 events, which
libtracetools emits itself on Jazzy, must match uprobe as well. The events a method does not see
by design (3.4) are listed in ONLY and left out.

    agreement.py
"""

import os
import re
import shutil
import subprocess
import sys
import tempfile
import time
from collections import Counter
from pathlib import Path

import orma_client
import common
import session
from ament_index_python.packages import get_package_prefix, get_package_share_directory

A = common.accuracy
TEST = common.ORMA / "daemon/orma_daemon/test"
TICKS = "600"
# Design 3.4: rclcpp_callback_register is gated, so a uprobe cannot take it, and uprobe names
# callbacks by callback_code instead.
ONLY = {"orma:rclcpp_callback_register"}
ROS2 = {
    "ros2:callback_start": "orma:callback_start",
    "ros2:callback_end": "orma:callback_end",
    "ros2:rcl_publish": "orma:rcl_publish",
    "ros2:rclcpp_publish": "orma:rclcpp_publish",
    "ros2:rmw_publish": "orma:rmw_publish",
    "ros2:rmw_take": "orma:rmw_take",
}


def ros2_counts(ctf, pids, window):
    out = subprocess.run(
        ["babeltrace", "--clock-cycles", "--no-delta", "-n", "context", str(ctf)],
        capture_output=True,
        text=True,
        check=True,
    ).stdout
    counts = Counter()
    for line in out.splitlines():
        m = re.match(r"\[(\d+)\] \S+ (ros2:\w+): .*vpid = (\d+)", line)
        if m and m[2] in ROS2 and int(m[3]) in pids and window[0] <= int(m[1]) < window[1]:
            counts[ROS2[m[2]]] += 1
    return counts


def main():
    tmp = Path(tempfile.mkdtemp(prefix="orma-agree3-", dir="/tmp"))
    os.chmod(tmp, 0o755)
    failed = []
    with common.ormad(tmp):
        collectd = common.Collectd(tmp)
        procs = {}
        try:
            client = orma_client.connect(start=False)
            # Only the workload: a client's monitoring would arm every shared memory file.
            client.request("select", all=False, pids=[])
            # sched off in all three: ormad's scheduler records are not a method's events (3.4).
            rec = {
                "lttng": client.request(
                    "record_start", method="lttng", max_duration_s=600, sched=False
                )
            }
            session.wait_for(collectd.session_up, "the LTTng session")

            libs = [
                Path(get_package_prefix(p)) / "lib" / f"lib{p}.so"
                for p in ("orma_preload", "orma_lttng")
            ]
            autoload = Path(get_package_share_directory("orma_rclpy")) / "autoload"
            env = A.node_env("rmw_cyclonedds_cpp")
            env.update(
                ROS_DOMAIN_ID=common.DOMAIN,
                LD_PRELOAD=":".join(map(str, libs)),
                LTTNG_HOME=str(collectd.lttng_home),
                PYTHONPATH=f"{autoload}:{env.get('PYTHONPATH', '')}",
            )
            exe = A.build_node(tmp)
            prefix = f"/orma_agree_{os.getpid()}"
            procs["cpp_a"] = common.Waiting([exe, prefix, TICKS], env)
            procs["cpp_b"] = common.Waiting([exe, prefix, TICKS], env)
            procs["py"] = common.Waiting(
                [sys.executable, TEST / "node.py", prefix + "_py", TICKS], env
            )
            if A.flux_lib():
                procs["flux"] = common.Caller([session.flux_caller(tmp)], env)
            pids = {k: p.pid for k, p in procs.items()}
            targets = orma_client.targets(pids=sorted(pids.values()))
            for method in ("uprobe", "preload"):
                rec[method] = client.request(
                    "record_start", **orma_client.record_start(method, targets, sched=False)
                )
            for role, pid in pids.items():
                if role == "flux":
                    common.wait_for(
                        lambda: common.uprobes_on(pid, *common.FLUX, lib="libflux_ffi.so"),
                        "ormad's uprobes on the flux caller",
                        seconds=600,
                    )
                    continue
                common.wait_for(
                    lambda: (
                        common.uprobes_on(pid, *common.HOT)
                        and common.uprobes_on(pid, "rcl_wait", lib="librcl.so")
                    ),
                    f"ormad's uprobes on {role}",
                    seconds=600,
                )
            common.wait_for(
                lambda: all(
                    f"PID: {p} " in (collectd.lttng("list", "-u") or "") for p in pids.values()
                ),
                "the workload to register with the session daemon",
            )
            for role, p in procs.items():
                if role != "flux":
                    p.go()
            # The preload recording takes a new process once orma-collectd has armed its file.
            t0 = time.monotonic_ns() + 1_500_000_000
            time.sleep(2.0)
            if "flux" in procs:
                procs["flux"].go()
            time.sleep(2.5)
            t1 = time.monotonic_ns()
            said = {k: p.line() for k, p in procs.items() if k != "flux"}
            for r in rec.values():
                client.request("record_stop", record_id=r["record_id"])
            shown = {
                m: client.request("record_show", record_id=r["record_id"])["table"]
                for m, r in rec.items()
            }
            client.close()
        finally:
            for p in procs.values():
                if p.started:
                    p.end()
                else:
                    p.kill()
            collectd.stop()

    pset, window = set(pids.values()), (t0, t1)
    counts = {
        "uprobe": session.preload_counts(Path(rec["uprobe"]["dir"]), pset, window),
        "preload": session.preload_counts(Path(rec["preload"]["dir"]), pset, window),
        "lttng": session.lttng_counts(Path(rec["lttng"]["dir"]) / "ctf", pset, window),
    }
    ros2 = ros2_counts(Path(rec["lttng"]["dir"]) / "ctf", pset, window)
    print("node said:", said)
    print(f"window {(t1 - t0) / 1e9:.1f} s")
    print(f"{'event':44} {'uprobe':>7} {'preload':>7} {'lttng':>7} {'ros2':>7}")
    names = sorted(set().union(*counts.values()))
    for name in names:
        row = [counts[m][name] for m in ("uprobe", "preload", "lttng")]
        r2 = ros2[name] if name in ROS2.values() else ""
        print(f"{name:44} {row[0]:7} {row[1]:7} {row[2]:7} {r2:>7}")
        if name in ONLY:
            continue
        if len(set(row)) != 1:
            failed.append(f"{name}: uprobe {row[0]}, preload {row[1]}, lttng {row[2]}")
        if name in ROS2.values() and ros2[name] != row[0]:
            failed.append(f"{name}: ros2 {ros2[name]}, uprobe {row[0]}")
    for event in ("orma:callback_start", "orma:rmw_take", "orma:rcl_wait_exit"):
        if counts["uprobe"][event] == 0:
            failed.append(f"no {event} in the window")
    if "flux" in procs and counts["uprobe"]["flux:take"] == 0:
        failed.append("no flux:take in the window")
    for m, t in shown.items():
        print(f"{m}: {t['totals']['events']} events, {t['totals']['lost']} lost")
        if t["totals"]["lost"]:
            failed.append(f"the {m} recording lost {t['totals']['lost']} events")
    if failed:
        sys.exit("FAILED:\n  " + "\n  ".join(failed) + f"\n(kept {tmp})")
    shutil.rmtree(tmp)
    print("ok")


main()
