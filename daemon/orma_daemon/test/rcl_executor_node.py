"""The rclpy twin of rcl_executor_node.cpp: a 10 ms timer publishes to a topic the node subscribes
to, spun by rclpy.experimental.RclExecutor with `rcl_executor`, else a SingleThreadedExecutor.
Prints its pid, runs on "go", stops on the next line.

    rcl_executor_node.py PREFIX rcl_executor|stock
"""

import os
import sys
import threading

print(os.getpid(), flush=True)
sys.stdin.readline()

import rclpy  # noqa: E402
from std_msgs.msg import Header  # noqa: E402

if sys.argv[2] == "rcl_executor":
    from rclpy.experimental import RclExecutor as Executor
else:
    from rclpy.executors import SingleThreadedExecutor as Executor

prefix = sys.argv[1]
rclpy.init()
node = rclpy.create_node("orma_rcl_executor_py")
n = {"timer": 0, "received": 0}
pub = node.create_publisher(Header, prefix + "/out", 10)


def on_message(_msg):
    n["received"] += 1


def on_timer():
    n["timer"] += 1
    h = Header()
    h.stamp = node.get_clock().now().to_msg()
    pub.publish(h)


sub = node.create_subscription(Header, prefix + "/out", on_message, 10)
timer = node.create_timer(0.01, on_timer)
executor = Executor()
executor.add_node(node)
spin = threading.Thread(target=executor.spin)
spin.start()
sys.stdin.readline()
executor.shutdown()
spin.join()
print(f"timer {n['timer']} received {n['received']}", flush=True)
node.destroy_node()
rclpy.shutdown()
