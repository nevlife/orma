"""Runs a subscription pair, a timer, a service and a client, then writes what they did as JSON.

Started by test_events.py with orma_rclpy's autoload directory on PYTHONPATH and, optionally,
trace_counter.so in LD_PRELOAD.
"""

import collections
import ctypes
import json
import sys
import time

import rclpy
from rclpy.executors import SingleThreadedExecutor
from rclpy.qos import QoSProfile, ReliabilityPolicy
from std_msgs.msg import Int32
from std_srvs.srv import SetBool

MESSAGES = 20
CALLS = 5
TOPIC = "orma_rclpy_test"


class Record(ctypes.Structure):
    _fields_ = [
        ("event", ctypes.c_int32),
        ("flag", ctypes.c_int32),
        ("a", ctypes.c_uint64),
        ("b", ctypes.c_uint64),
        ("text", ctypes.c_char * 96),
    ]


def spin_until(executor, done, seconds=20.0):
    deadline = time.monotonic() + seconds
    while not done():
        if time.monotonic() > deadline:
            raise TimeoutError
        executor.spin_once(timeout_sec=0.1)


class DlInfo(ctypes.Structure):
    _fields_ = [
        ("fname", ctypes.c_char_p),
        ("fbase", ctypes.c_void_p),
        ("sname", ctypes.c_char_p),
        ("saddr", ctypes.c_void_p),
    ]


def defining_library(function):
    info = DlInfo()
    ctypes.CDLL(None).dladdr(ctypes.cast(function, ctypes.c_void_p), ctypes.byref(info))
    return info.fname.decode()


def recorded():
    process = ctypes.CDLL(None)
    if not hasattr(process, "trace_counter_read"):
        return None
    read = process.trace_counter_read
    read.restype = ctypes.c_uint64
    records = ctypes.POINTER(Record)()
    n = read(ctypes.byref(records))
    return [
        [records[i].event, records[i].flag, records[i].a, records[i].b, records[i].text.decode()]
        for i in range(n)
    ]


def main():
    rclpy.init()
    node = rclpy.create_node("orma_rclpy_test")
    executor = SingleThreadedExecutor()
    executor.add_node(node)
    counts = collections.Counter()

    def on_message(msg):
        counts["message"] += 1

    async def on_tick():
        counts["tick"] += 1

    def on_request(request, response):
        counts["request"] += 1
        response.success = request.data
        return response

    qos = QoSProfile(depth=MESSAGES, reliability=ReliabilityPolicy.RELIABLE)
    subscriptions = [node.create_subscription(Int32, TOPIC, on_message, qos) for _ in range(2)]
    publisher = node.create_publisher(Int32, TOPIC, qos)
    timer = node.create_timer(0.01, on_tick)
    service = node.create_service(SetBool, "orma_rclpy_set", on_request)
    client = node.create_client(SetBool, "orma_rclpy_set")

    spin_until(executor, lambda: publisher.get_subscription_count() == 2)
    for i in range(MESSAGES):
        publisher.publish(Int32(data=i))
    spin_until(executor, lambda: counts["message"] == 2 * MESSAGES)

    client.wait_for_service(timeout_sec=20.0)
    for i in range(CALLS):
        future = client.call_async(SetBool.Request(data=i % 2 == 0))
        spin_until(executor, future.done)
        assert future.result().success == (i % 2 == 0)

    timer.cancel()
    with open(sys.argv[1], "w") as out:
        json.dump(
            {
                "callback_start_from": defining_library(sys.modules["orma_rclpy"]._callback_start),
                "node": node.handle.pointer,
                "subscriptions": [s.handle.pointer for s in subscriptions],
                "subscription_ids": [id(s) for s in subscriptions],
                "timer": timer.handle.pointer,
                "service": service.handle.pointer,
                "client": client.handle.pointer,
                "counts": counts,
                "records": recorded(),
            },
            out,
        )
    node.destroy_node()
    rclpy.shutdown()


if __name__ == "__main__":
    main()
