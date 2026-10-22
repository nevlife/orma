"""rclpy twin of delivery_node.cpp, image only: delivery_node.py NAME
[pub=TOPIC:image:BYTES:HZ]... [sub=TOPIC:image]... [seconds=S] [executor=rcl]."""

import resource
import sys
import threading
import time

import rclpy
from rclpy.executors import ExternalShutdownException, SingleThreadedExecutor
from rclpy.node import Node
from rclpy.qos import QoSProfile, ReliabilityPolicy
from sensor_msgs.msg import Image


def cpu_us():
    u = resource.getrusage(resource.RUSAGE_SELF)
    return round((u.ru_utime + u.ru_stime) * 1e6)


def main():
    name = sys.argv[1]
    opts = [a.split("=", 1) for a in sys.argv[2:]]
    rclpy.init()
    node = Node(name)
    qos = QoSProfile(depth=10, reliability=ReliabilityPolicy.RELIABLE)
    pubs, got, seconds, executor = [], {}, 3.0, SingleThreadedExecutor
    for key, value in opts:
        f = value.split(":")
        if key == "seconds":
            seconds = float(f[0])
        elif key == "executor" and f[0] == "rcl":
            from rclpy.experimental import RclExecutor as executor
        elif key == "pub":
            pub = node.create_publisher(Image, f[0], qos)
            msg = Image(height=1, width=int(f[2]), step=int(f[2]), encoding="mono8")
            msg.data = bytes(int(f[2]))
            pubs.append([f[0], pub, msg, float(f[3]), 0])
        elif key == "sub":
            got[f[0]] = 0

            def cb(m, topic=f[0]):
                got[topic] += 1

            node.create_subscription(Image, f[0], cb, qos)
    timers = []
    for p in pubs:

        def tick(p=p):
            now = time.time_ns()
            p[2].header.stamp.sec, p[2].header.stamp.nanosec = divmod(now, 1_000_000_000)
            p[1].publish(p[2])
            p[4] += 1

        t = node.create_timer(1.0 / p[3], tick)
        t.cancel()
        timers.append(t)
    ex = executor()
    ex.add_node(node)
    def run():
        try:
            ex.spin()
        except ExternalShutdownException:
            pass

    spin = threading.Thread(target=run)
    spin.start()
    print("ready", flush=True)
    sys.stdin.readline()
    cpu0 = cpu_us()
    for t in timers:
        t.reset()
    time.sleep(seconds)
    for t in timers:
        t.cancel()
    time.sleep(0.3)
    cpu = cpu_us() - cpu0
    for p in pubs:
        print(f"pub {p[0]} {p[4]}", flush=True)
    for topic, n in got.items():
        print(f"sub {topic} {n}", flush=True)
    print(f"cpu_us {cpu}", flush=True)
    print("done", flush=True)
    sys.stdin.readline()
    rclpy.try_shutdown()
    spin.join()
    node.destroy_node()


if __name__ == "__main__":
    main()
