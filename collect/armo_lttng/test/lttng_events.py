#!/usr/bin/env python3
"""Runs the caller under LD_PRELOAD=libarmo_lttng.so in a private LTTng session and fails unless
every armo and flux event the caller expects arrives exactly as often, the stamps and handles
come through, and each ROS event also arrives as a ros2 event as often: from libtracetools where it
has LTTng (Jazzy), from libarmo_lttng.so where it has not (Humble). Given the stubs, also spans
(each end keyed as its begin, the name on the begin) and the timed CUDA and DDS calls.

    lttng_events.py LIBRARY CALLER [CUDA_STUB DDS_STUB]
"""

import sys
import tempfile
from pathlib import Path

import session

# The armo events with a ros2 twin; the twin of rmw_publish_rcl is ros2:rmw_publish.
ROS2 = {
    "rcl_init", "rcl_node_init", "rmw_publisher_init", "rcl_publisher_init", "rclcpp_publish",
    "rcl_publish", "rmw_publish_rcl", "rmw_subscription_init", "rcl_subscription_init",
    "rclcpp_subscription_init", "rclcpp_subscription_callback_added", "rmw_take", "rcl_take",
    "rclcpp_take", "rcl_service_init", "rclcpp_service_callback_added", "rcl_client_init",
    "rcl_timer_init", "rclcpp_timer_callback_added", "rclcpp_timer_link_node",
    "rclcpp_callback_register", "callback_start", "callback_end",
    "rclcpp_executor_get_next_ready", "rclcpp_executor_wait_for_work", "rclcpp_executor_execute",
}  # fmt: skip
STAMP = 1700000000 | 5 << 32


def main():
    lib, caller, stubs = sys.argv[1], sys.argv[2], sys.argv[3:]
    failed = []
    with session.daemon() as env, tempfile.TemporaryDirectory() as tmp:
        trace = Path(tmp) / "trace"
        with session.session(env, trace, "armo:*,flux:*,ros2:*"):
            run = session.subprocess.run(
                [caller, *stubs], env=dict(env, LD_PRELOAD=lib), capture_output=True, text=True
            )
        if run.returncode != 0:
            sys.exit(f"the caller failed: {run.stderr}")
        got = list(session.events(trace))
    kinds = session.count(got)
    want = {e: int(n) for e, n in (line.split() for line in run.stdout.splitlines())}
    for event, n in want.items():
        if kinds[event] != n:
            failed.append(f"{event}: {kinds[event]} events, want {n}")
        name = event.split(":")[1]
        if event.startswith("armo:") and name in ROS2:
            twin = "ros2:rmw_publish" if name == "rmw_publish_rcl" else f"ros2:{name}"
            if kinds[twin] != n:
                failed.append(f"{twin}: {kinds[twin]} events, want {n}")
    unexpected = {k for k in kinds if k.split(":")[0] in ("armo", "flux")} - set(want)
    if unexpected:
        failed.append(f"unexpected events {sorted(unexpected)}")

    def words(name):
        return [[int(w, 16) for w in f["p"]] for k, f in got if k == f"armo:{name}"]

    if any(w[3] != STAMP for w in words("rmw_take")):
        failed.append(f"rmw_take stamps {[w[3] for w in words('rmw_take')]}, want {STAMP}")
    if any(w[2] != STAMP for w in words("rcl_publish")):
        failed.append("rcl_publish stamps")
    if [w[0] for w in words("callback_start")] != [0x1001, 0x1002, 0x1003]:
        failed.append(f"callback_start handles {words('callback_start')}")
    nodes = [f for k, f in got if k == "armo:rcl_node_init"]
    if any((f["s0"], f["s1"]) != ('"node"', '"/ns"') for f in nodes):
        failed.append(f"rcl_node_init strings {nodes}")
    begins = [(f["s0"], int(f["p"][0], 16)) for k, f in got if k == "armo:span_begin"]
    ends = sorted(w[0] for w in words("span_end"))
    if stubs and (
        {n for n, _ in begins} != {'"outer"', '"inner"'} or sorted(k for _, k in begins) != ends
    ):
        failed.append(f"span begins {begins}, ends {ends}")
    # duration, bytes, then api and result in one word: a 64-byte copy (2), a stream sync (4).
    cuda = sorted((w[1], w[2]) for w in words("cuda_call"))
    if stubs and cuda != sorted([(64, 2), (0, 4)] * 3):
        failed.append(f"cuda_call bytes and api {cuda}")
    if stubs and sorted(w[0] for w in words("dds_write")) != sorted([0x1234, 0xBEEF] * 3):
        failed.append(f"dds_write writers {words('dds_write')}")
    if any(f.get("vpid") is None or f.get("vtid") is None for _, f in got):
        failed.append("an event without the vpid and vtid contexts")
    print(dict(kinds))
    if failed:
        sys.exit("FAILED:\n" + "\n".join(failed))
    print("ok")


main()
