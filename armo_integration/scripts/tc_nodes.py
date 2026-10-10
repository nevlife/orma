#!/usr/bin/env python3
"""rclpy publisher, relay and sink with the topics, payload and output of nodes.cpp.

usage: tc_nodes.py pub|relay|sink [--topics N] [--rate-hz HZ] [--in P] [--out P] [--tag T]
"""

import argparse
import array
import struct
import time

import rclpy
from rclpy.executors import ExternalShutdownException
from rclpy.node import Node
from std_msgs.msg import UInt8MultiArray


def make_pub(node, a):
    pubs, msgs, seqs = [], [], [0] * a.topics
    for i in range(a.topics):
        pubs.append(node.create_publisher(UInt8MultiArray, f"{a.out}{i}", 10))
        m = UInt8MultiArray()
        m.data = array.array("B", bytes(a.bytes))
        msgs.append(m)

    def tick(i):
        def cb():
            seqs[i] += 1
            struct.pack_into("<QQ", msgs[i].data, 0, time.monotonic_ns(), seqs[i])
            pubs[i].publish(msgs[i])

        return cb

    node.timers_ = [node.create_timer(1.0 / a.rate_hz, tick(i)) for i in range(a.topics)]


def make_relay(node, a):
    pubs = [node.create_publisher(UInt8MultiArray, f"{a.out}{i}", 10) for i in range(a.topics)]
    node.subs_ = [
        node.create_subscription(UInt8MultiArray, f"{a.in_}{i}", pubs[i].publish, 10)
        for i in range(a.topics)
    ]


def make_sink(node, a):
    rows = [[] for _ in range(a.topics)]

    def on(i):
        out = rows[i]

        def cb(msg):
            recv = time.monotonic_ns()
            pub, seq = struct.unpack_from("<QQ", msg.data, 0)
            out.append((seq, pub, recv))

        return cb

    node.subs_ = [
        node.create_subscription(UInt8MultiArray, f"{a.in_}{i}", on(i), 10) for i in range(a.topics)
    ]
    return rows


def main():
    p = argparse.ArgumentParser()
    p.add_argument("role", choices=["pub", "relay", "sink"])
    p.add_argument("--topics", type=int, default=4)
    p.add_argument("--rate-hz", type=float, default=1000.0)
    p.add_argument("--in", dest="in_", default="/tc/a")
    p.add_argument("--out", default="/tc/b")
    p.add_argument("--tag", default="sink")
    p.add_argument("--bytes", type=int, default=64)
    a, ros_args = p.parse_known_args()
    rclpy.init(args=ros_args)
    node = Node(f"tc_py_{a.role}")
    rows = {"pub": make_pub, "relay": make_relay, "sink": make_sink}[a.role](node, a)
    try:
        rclpy.spin(node)
    except (KeyboardInterrupt, ExternalShutdownException):
        pass
    for i, r in enumerate(rows or []):
        with open(f"{a.tag}_t{i}.csv", "w") as f:
            f.write("seq,pub_ns,recv_ns\n")
            f.writelines(f"{s},{p_},{t}\n" for s, p_, t in r)


if __name__ == "__main__":
    main()
