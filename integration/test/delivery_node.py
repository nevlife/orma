"""rclpy twin of delivery_node.cpp: delivery_node.py NAME [pub=TOPIC:TYPE:BYTES:HZ[:loan]]...
[sub=TOPIC:TYPE]... [seconds=S] [executor=rcl]. TYPE: image or plain (4 MiB, overhaul only)."""

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


def plain_type():
    from test_rmw_zenoh_cpp.msg import PlainPayload

    return PlainPayload


def stamp(msg, ns):
    if isinstance(msg, Image):
        msg.header.stamp.sec, msg.header.stamp.nanosec = divmod(ns, 1_000_000_000)
    else:
        msg.stamp = ns


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
            loan = len(f) > 4 and f[4] == "loan"
            if f[1] == "image":
                pub = node.create_publisher(Image, f[0], qos)
                msg = Image(height=1, width=int(f[2]), step=int(f[2]), encoding="mono8")
                msg.data = bytes(int(f[2]))
            else:
                pub = node.create_publisher(plain_type(), f[0], qos)
                msg = None if loan else plain_type()()
            pubs.append([f[0], pub, msg, float(f[3]), 0])
        elif key == "sub":
            got[f[0]] = 0

            def cb(m, topic=f[0]):
                got[topic] += 1

            node.create_subscription(Image if f[1] == "image" else plain_type(), f[0], cb, qos)
    timers = []
    for p in pubs:

        def tick(p=p):
            if p[2] is None:
                loaned = p[1].borrow_loaned_message()
                stamp(loaned.msg, time.time_ns())
                p[1].publish(loaned)
            else:
                stamp(p[2], time.time_ns())
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
