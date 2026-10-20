"""An rclpy node with known callback counts for nodes.py, the Python twin of node.cpp: after
rclpy.init it waits for a line on stdin, then timer t1 (2 ms) fires T1 times and publishes, the
subscription takes each, and the service answers SRV calls. Prints the counts as JSON."""

import json
import sys

import rclpy
from rclpy.executors import SingleThreadedExecutor
from std_msgs.msg import Header
from std_srvs.srv import Trigger

T1 = 100
SRV = 20


def main():
    rclpy.init()
    sys.stdin.readline()
    node = rclpy.create_node("orma_test_py")
    counts = {"t1": 0, "sub": 0, "srv": 0}
    pub = node.create_publisher(Header, "/orma_test/chatter", 10)

    def on_message(msg):
        counts["sub"] += 1

    def on_timer():
        h = Header()
        h.stamp.sec = 1000 + counts["t1"]
        h.stamp.nanosec = 7
        counts["t1"] += 1
        pub.publish(h)
        if counts["t1"] == T1:
            timer.cancel()

    def on_trigger(request, response):
        counts["srv"] += 1
        response.success = True
        return response

    node.create_subscription(Header, "/orma_test/chatter", on_message, 10)
    timer = node.create_timer(0.002, on_timer)
    node.create_service(Trigger, "/orma_test/trigger", on_trigger)
    client = node.create_client(Trigger, "/orma_test/trigger")
    executor = SingleThreadedExecutor()
    executor.add_node(node)
    while counts["t1"] < T1 or counts["sub"] < T1:
        executor.spin_once(timeout_sec=0.1)
    client.wait_for_service()
    for _ in range(SRV):
        executor.spin_until_future_complete(client.call_async(Trigger.Request()))
    print(json.dumps(counts), flush=True)
    node.destroy_node()
    rclpy.shutdown()


if __name__ == "__main__":
    main()
