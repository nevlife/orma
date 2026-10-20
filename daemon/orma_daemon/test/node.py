"""The rclpy twin of node.cpp, for accuracy.py: known numbers of timer, subscription and service
callbacks, which orma_rclpy turns into ros_trace_* calls. Prints its pid and waits for "go".

    node.py PREFIX TICKS
"""

import os
import sys
import time

print(os.getpid(), flush=True)
sys.stdin.readline()

import rclpy  # noqa: E402
from rclpy.qos import QoSProfile, ReliabilityPolicy  # noqa: E402
from std_msgs.msg import Header  # noqa: E402
from std_srvs.srv import Trigger  # noqa: E402

prefix = sys.argv[1] if len(sys.argv) > 1 else "/orma_acc_py"
ticks = int(sys.argv[2]) if len(sys.argv) > 2 else 50
calls = ticks // 2

rclpy.init()
node = rclpy.create_node("orma_acc_py")
qos = QoSProfile(depth=1000, reliability=ReliabilityPolicy.RELIABLE)
n = {"timer": 0, "published": 0, "received": 0, "served": 0}
pub = node.create_publisher(Header, prefix + "/out", qos)


def on_message(_msg):
    n["received"] += 1


def on_trigger(_req, res):
    n["served"] += 1
    res.success = True
    return res


sub = node.create_subscription(Header, prefix + "/out", on_message, qos)
srv = node.create_service(Trigger, prefix + "/srv", on_trigger)
client = node.create_client(Trigger, prefix + "/srv")
client.wait_for_service(timeout_sec=5.0)


def on_timer():
    n["timer"] += 1
    if n["published"] >= ticks:
        return
    n["published"] += 1
    h = Header()
    h.stamp = node.get_clock().now().to_msg()
    h.frame_id = "f"
    pub.publish(h)
    if n["published"] <= calls:
        client.call_async(Trigger.Request())


timer = node.create_timer(0.01, on_timer)
deadline = time.monotonic() + 20
while time.monotonic() < deadline and (
    n["published"] < ticks or n["received"] < ticks or n["served"] < calls
):
    rclpy.spin_once(node, timeout_sec=0.02)
timer.cancel()
print(
    f"timer {n['timer']} timer2 0 received {n['received']} served {n['served']} "
    f"published {n['published']}",
    flush=True,
)
node.destroy_node()
rclpy.shutdown()
sys.stdin.readline()
