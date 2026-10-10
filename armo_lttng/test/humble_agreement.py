#!/usr/bin/env python3
"""Method agreement on Humble (design 2.7.1, 18 WP9 item 3), where libtracetools has no LTTng and
libarmo_lttng.so emits the ROS events itself. The host's armo-collectd records the container's
processes by method preload through the shared /dev/shm; inside the container a session daemon
under the container's own LTTNG_HOME records them into CTF, ros2 events included. Both counts of
each armo event in the window must be the same, and each ROS event must also be a ros2 event.

Run on the host with the Jazzy workspace sourced:

    humble_agreement.py IMAGE HUMBLE_WS

HUMBLE_WS is a colcon workspace whose src/armo is this tree, with armo_preload, armo_lttng (tests
on) and armo_rclpy built in IMAGE.
"""

import json
import os
import subprocess
import sys
import tempfile
from collections import Counter
from pathlib import Path

import armo_client
import harness
import session

ARMO = Path(__file__).resolve().parents[2]
INSIDE = """
import json, sys
sys.path.insert(0, "/w/src/armo/armo_lttng/test")
import session
with session.daemon() as env, session.session(env, "/t/ctf", "armo:*,flux2:*,ros2:*"):
    def started(pids):
        json.dump(sorted(pids), open("/t/pids.json", "w"))

    with session.workload("/w/build/armo_lttng/lttng_node", env, env["LTTNG_HOME"], started) as run:
        json.dump({"pids": sorted(run[0]), "window": run[1]}, open("/t/run.json", "w"))
"""


def main():
    image, ws = sys.argv[1], Path(sys.argv[2]).resolve()
    tmp = Path(tempfile.mkdtemp(prefix="armo-humble-", dir="/tmp"))
    (tmp / "t").mkdir()
    collectd = harness.Collectd(tmp)
    client = armo_client.connect(start=False)
    # Only the workload: a client's monitoring would arm every shared memory file of this user.
    client.request("select", all=False, pids=[])
    uid = f"{os.getuid()}:{os.getgid()}"
    # --pid=host: the container's pids are the host's, in the CTF and the shared memory alike.
    container = subprocess.Popen(
        ["sudo", "-n", "docker", "run", "--rm", "--pid=host", "--ipc=host", "--user", uid,
         "-e", "HOME=/w/home", "-v", f"{ws}:/w", "-v", f"{ARMO}:/w/src/armo:ro",
         "-v", f"{tmp / 't'}:/t", image, "bash", "-c",
         "source /opt/ros/humble/setup.bash && source /w/install/setup.bash && "
         f"python3 -c '{INSIDE}'"],
    )  # fmt: skip
    pids_file = tmp / "t/pids.json"
    session.wait_for(pids_file.exists, "the container's workload", seconds=60)
    session.wait_for(lambda: pids_file.read_text().endswith("]"), "its pids")
    targets = {"all": False, "pids": json.loads(pids_file.read_text())}
    pre = client.request("record_start", method="preload", targets=targets)
    if container.wait() != 0:
        sys.exit("the container failed")
    client.request("record_stop", record_id=pre["record_id"])
    client.close()
    collectd.stop()

    run = json.loads((tmp / "t/run.json").read_text())
    pids, window = set(run["pids"]), tuple(run["window"])
    a = session.preload_counts(Path(pre["dir"]), pids, window)
    b = session.lttng_counts(tmp / "t/ctf", pids, window)
    session.print_counts(a, b)
    ros2 = Counter()
    out = subprocess.run(
        ["babeltrace", "--clock-cycles", "-n", "context", str(tmp / "t/ctf")],
        capture_output=True,
        text=True,
        check=True,
    ).stdout
    for line in out.splitlines():
        fields = line.split()
        name = next((f[:-1] for f in fields if f.startswith("ros2:")), None)
        vpid = int(line.split("vpid = ")[1].split(",")[0]) if "vpid = " in line else 0
        if name and vpid in pids and window[0] <= int(fields[0][1:-1]) < window[1]:
            ros2[name] += 1
    print("ros2:", dict(ros2))
    failed = []
    if a != b:
        failed.append("the two methods counted differently")
    for event in ("callback_start", "callback_end", "rcl_publish", "rmw_take"):
        if a[f"armo:{event}"] == 0 or ros2[f"ros2:{event}"] != a[f"armo:{event}"]:
            failed.append(f"{event}: armo {a[f'armo:{event}']}, ros2 {ros2[f'ros2:{event}']}")
    if ros2["ros2:rmw_publish"] != a["armo:rmw_publish_rcl"]:
        failed.append("ros2:rmw_publish and armo:rmw_publish_rcl differ")
    if failed:
        sys.exit("FAILED: " + "; ".join(failed) + f" (kept {tmp})")
    subprocess.run(["rm", "-rf", tmp], check=True)
    print("ok")


main()
