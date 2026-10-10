#!/usr/bin/env python3
"""Method agreement (design 8): one run, both libraries in each process
(LD_PRELOAD=libarmo_preload.so:libarmo_lttng.so), and a real armo-collectd recording the same
processes twice, by method preload into event files and by method lttng into CTF. A C++ publisher
and subscriber, an rclpy pair with armo_rclpy, and the flux2 caller run; in a window that starts
after the shared memory files are armed, every event of every workload process must be counted
the same by both recordings.

    agreement.py NODE
"""

import subprocess
import sys
import tempfile
from pathlib import Path

import armo_client
import harness
import session


def main():
    node = sys.argv[1]
    tmp = Path(tempfile.mkdtemp(prefix="armo-agree-", dir="/tmp"))
    collectd = harness.Collectd(tmp)
    env = collectd.env
    client = armo_client.connect(start=False)
    # Only the workload: a client's monitoring would arm every shared memory file of this user.
    client.request("select", all=False, pids=[])
    ltt = client.request("record_start", method="lttng", max_duration_s=120)
    session.wait_for(collectd.session_up, "the LTTng session")
    pre = {}

    def started(pids):
        targets = {"all": False, "pids": sorted(pids)}
        pre.update(client.request("record_start", method="preload", targets=targets))

    caller = session.flux2_caller(tmp)
    with session.workload(node, env, collectd.lttng_home, started, caller) as (pids, window):
        shown = {}
        for name, r in (("preload", pre), ("lttng", ltt)):
            client.request("record_stop", record_id=r["record_id"])
            shown[name] = client.request("record_show", record_id=r["record_id"])["table"]
    client.close()
    collectd.stop()

    a = session.preload_counts(Path(pre["dir"]), pids, window)
    b = session.lttng_counts(Path(ltt["dir"]) / "ctf", pids, window)
    session.print_counts(a, b)
    for name, table in shown.items():
        t = table["totals"]
        print(f"{name}: {t['events']} events, {t['lost']} lost, end {table['end_reason']}")
    failed = []
    if a != b:
        failed.append("the two methods counted differently")
    for event in ("armo:callback_start", "armo:rmw_take", "flux2:take"):
        if a[event] == 0:
            failed.append(f"no {event} in the window")
    if any(t["totals"]["lost"] for t in shown.values()):
        failed.append("a recording lost events")
    if failed:
        sys.exit("FAILED: " + "; ".join(failed) + f" (kept {tmp})")
    subprocess.run(["rm", "-rf", tmp], check=True)
    print("ok")


main()
