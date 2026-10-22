#!/usr/bin/env python3
"""ormad on the overhaul's rcl_executor and on stock ROS 2 side by side (docs/instrumentation.md
2.12). Starts four nodes: rclcpp and rclpy on the overlay's RclExecutor, rclcpp and rclpy on the
stock install's SingleThreadedExecutor. Attaches the discovery and hot sets to all four, then
checks that each produced waits, executes and callbacks from the probe set it should have, that
the window rows time waits, dispatches and callbacks, and that the selfcheck names the set.

Needs root for ormad (sudo -n), the stock install under /opt/ros/$ROS_DISTRO and the overlay's
setup script as --overlay. `--ormad` defaults to the ormad on PATH.

    daemon/orma_daemon/test/rcl_executor.py --overlay ~/ros2_ws/install/setup.bash [--ormad PATH]
"""

import argparse
import collections
import os
import shutil
import subprocess
import sys
import tempfile
import time
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))
import accuracy as A  # noqa: E402

RCL_EXECUTOR_FLAG = 2
SOURCE = 'source "$0" >/dev/null && exec "$@"'


def build(tmp, setup, name, core):
    """rcl_executor_node.cpp with cmake under `setup`; returns the executable."""
    src = Path(tmp) / f"{name}_src"
    src.mkdir()
    shutil.copy(A.HERE / "rcl_executor_node.cpp", src)
    define = "target_compile_definitions(node PRIVATE ORMA_RCL_EXECUTOR)\n" if core else ""
    (src / "CMakeLists.txt").write_text(
        "cmake_minimum_required(VERSION 3.16)\nproject(orma_rcl_executor_node CXX)\n"
        "find_package(rclcpp REQUIRED)\nfind_package(std_msgs REQUIRED)\n"
        "add_executable(node rcl_executor_node.cpp)\n"
        "target_link_libraries(node rclcpp::rclcpp ${std_msgs_TARGETS})\n" + define
    )
    out = Path(tmp) / f"{name}_build"
    for step in (f"cmake -S {src} -B {out} -DCMAKE_BUILD_TYPE=Release", f"cmake --build {out}"):
        subprocess.run(
            ["bash", "-c", SOURCE, setup, *step.split()], check=True, capture_output=True
        )
    return out / "node"


def start(setup, argv, env):
    return A.Proc(["bash", "-c", SOURCE, setup, *map(str, argv)], env=env)


def check(f, c, pid, topic, core, rclcpp):
    recs = c.of(pid)
    n = collections.Counter(r.name for r in recs)
    flagged = collections.Counter(r.name for r in recs if r.flags & RCL_EXECUTOR_FLAG)
    for name in ("callback_start", "callback_end", "rcl_wait_enter", "rcl_wait_exit"):
        f.check(n[name] > 0, f"no {name}")
    claims = [r for r in recs if r.name == "rcl_executor_claim"]
    if core:
        f.check(flagged["rcl_wait_enter"] > 0, "no rcl_executor wait")
        enters, exits = flagged["rcl_wait_enter"], flagged["rcl_wait_exit"]
        f.check(abs(enters - exits) <= 1, f"core waits: {enters} enters, {exits} exits")
        f.check(claims and n["rcl_executor_complete"] > 0, "no claim or completion")
        executors = {r["executor"] for r in claims}
        waited = {
            r["wait_set"] for r in recs if r.name == "rcl_wait_exit" and r.flags & RCL_EXECUTOR_FLAG
        }
        f.check(waited and waited <= executors, f"waits on {waited}, claims on {executors}")
        handles = {r["timer"] for r in recs if r.name == "rcl_timer_init"}
        handles |= {
            r["handle"]
            for r in recs
            if r.name == "rcl_subscription_init" and r.strings[:1] == [topic]
        }
        claimed = {r["entity"] for r in claims}
        f.check(handles and handles <= claimed, f"handles {handles}, claimed {claimed}")
        f.eq({r["rclcpp"] for r in claims}, {int(rclcpp)}, "claim's rclcpp")
    else:
        f.eq(sum(flagged.values()) + len(claims), 0, "rcl_executor records in a stock process")
        if rclcpp:
            f.check(n["rclcpp_executor_execute"] > 0, "no rclcpp_executor_execute")
    rows = c.aggs(pid)
    for kind, what in ((A.AGG_WAIT, "wait"), (A.AGG_DISPATCH, "dispatch")):
        count = sum(v[0] for k, v in rows.items() if k[0] == kind and k[1] == A.SRC_ROS)
        f.check(count > 0, f"no {what} rows")
    f.check(any(k[0] == A.AGG_CALLBACK for k in rows), "no callback rows")
    executes = len(claims) if core else n["rclcpp_executor_execute"]
    return (
        f"waits {n['rcl_wait_exit']} (core {flagged['rcl_wait_exit']}), executes {executes}, "
        f"callbacks {n['callback_start']}"
    )


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--ormad", default=shutil.which("ormad"))
    ap.add_argument("--overlay", required=True, help="the overhaul's setup script")
    args = ap.parse_args()
    stock = str(A.OPT / "setup.bash")
    rclpy_src = A.ROOT / "collect/orma_rclpy"
    path = f"{rclpy_src / 'autoload'}:{rclpy_src}:{os.environ.get('PYTHONPATH', '')}"
    env = dict(A.node_env("rmw_cyclonedds_cpp"), PYTHONPATH=path)
    tmp = tempfile.mkdtemp(prefix="orma_rcl_executor_")
    node = A.HERE / "rcl_executor_node.py"
    prefix = f"/orma_rcl_executor_{os.getpid()}"
    # name: argv, setup, rcl_executor probes, rclcpp
    cases = {
        "rclcpp rcl_executor": ([build(tmp, args.overlay, "core", True)], args.overlay, True, True),
        "rclcpp stock": ([build(tmp, stock, "stock", False)], stock, False, True),
        "rclpy rcl_executor": ([sys.executable, node], args.overlay, True, False),
        "rclpy stock": ([sys.executable, node], stock, False, False),
    }
    sock = str(Path(tmp) / "ormad.sock")
    ormad = A.Ormad(args.ormad, sock, str(Path(tmp) / "ormad.log"))
    procs = {}
    topics = {}
    failed = []
    try:
        for i, (name, (argv, setup, core, _)) in enumerate(cases.items()):
            extra = ["rcl_executor" if core else "stock"] if argv[0] == sys.executable else []
            procs[name] = start(setup, argv + [f"{prefix}_{i}"] + extra, env)
            topics[name] = f"{prefix}_{i}/out"
        pids = [p.pid for p in procs.values()]
        c = A.Conn(sock)
        c.op("attach", all=True, pids=[], sets=["discovery"])
        c.op("attach", all=False, pids=pids, sets=["hot"])
        c.op("stream", on=True, pids=pids)
        c.op("window", window_s=0.5)
        c.op("flush")
        for p in procs.values():
            p.go()
        time.sleep(3)
        reply = c.op("selfcheck", timeout=300, duration_s=3.0, pids=pids)
        items = {ch["item"]: ch for ch in reply["checks"]}
        for p in procs.values():
            p.go("end")
            print(f"  pid {p.pid}: {p.line()}")
        c.settle()
        for name, (_, _, core, rclcpp) in cases.items():
            f = A.Fail(name)
            pid = procs[name].pid
            out = check(f, c, pid, topics[name], core, rclcpp)
            item = f"executor probes pid {pid} "
            sc = next((v for k, v in items.items() if k.startswith(item)), None)
            want = "rcl_executor (" if core else "stock:"
            f.check(sc and sc["detail"].startswith(want), f"selfcheck: {sc}")
            print(f"{'FAIL' if f.msgs else 'ok'}  {name} (pid {pid}): {out}")
            print(f"      {sc and sc['status']}: {sc and sc['detail']}")
            for m in f.msgs:
                print(f"      {m}")
            failed += [f"{name}: {m}" for m in f.msgs]
        c.close()
    finally:
        for p in procs.values():
            p.p.kill()
        ormad.stop()
    print(f"failed: {len(failed)}")
    return 1 if failed else 0


if __name__ == "__main__":
    sys.exit(main())
