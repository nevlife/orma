"""The rclpy side of agreement.py: `node.py pub COUNT` waits 3 s, then publishes COUNT
std_msgs/Header at 200 Hz on /armo_lttng_py; `node.py sub` takes them. Both spin until SIGINT."""

import sys
import time

import rclpy
from rclpy.executors import ExternalShutdownException
from std_msgs.msg import Header


def main():
    rclpy.init()
    role = sys.argv[1]
    node = rclpy.create_node(f"armo_lttng_py_{role}")
    if role == "pub":
        pub = node.create_publisher(Header, "/armo_lttng_py", 10)
        left = [int(sys.argv[2])]
        start = time.monotonic() + 3

        def tick():
            if left[0] and time.monotonic() >= start:
                pub.publish(Header(stamp=node.get_clock().now().to_msg()))
                left[0] -= 1

        node.create_timer(0.005, tick)
    else:
        node.create_subscription(Header, "/armo_lttng_py", lambda m: None, 10)
    try:
        rclpy.spin(node)
    except (KeyboardInterrupt, ExternalShutdownException):
        pass


main()
