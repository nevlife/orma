"""The publisher of `armo react` (design 20.8), run under the preload method so armo sees its
publishes. It prints `ready` once the input topic has a subscriber, waits for a line on stdin,
then publishes the messages with the period after each and prints how many it sent."""

import argparse
import itertools
import sys
import time

import rclpy
import rosbag2_py
import yaml
from rosidl_runtime_py import set_message_fields
from rosidl_runtime_py.utilities import get_message

SUBSCRIBER_WAIT_S = 10.0


def from_yaml(text, msg_type, node):
    msg = msg_type()
    stamps = set_message_fields(
        msg, yaml.safe_load(text) or {}, expand_header_auto=True, expand_time_now=True
    )
    while True:
        now = node.get_clock().now().to_msg()
        for stamp in stamps:
            stamp(now)
        yield msg


def from_bag(path, topic):
    reader = rosbag2_py.SequentialReader()
    reader.open(rosbag2_py.StorageOptions(uri=path), rosbag2_py.ConverterOptions("", ""))
    reader.set_filter(rosbag2_py.StorageFilter(topics=[topic]))
    while reader.has_next():
        yield reader.read_next()[1]


def main():
    p = argparse.ArgumentParser(prog="armo-react-publisher")
    p.add_argument("--input", required=True)
    p.add_argument("--type", required=True)
    source = p.add_mutually_exclusive_group(required=True)
    source.add_argument("--msg")
    source.add_argument("--bag")
    p.add_argument("--count", type=int)
    p.add_argument("--period", type=float, required=True)
    args = p.parse_args()

    rclpy.init()
    node = rclpy.create_node("armo_react")
    msg_type = get_message(args.type)
    publisher = node.create_publisher(msg_type, args.input, 10)
    deadline = time.monotonic() + SUBSCRIBER_WAIT_S
    while publisher.get_subscription_count() == 0:
        if time.monotonic() > deadline:
            sys.exit(f"armo react: no subscriber on {args.input} after {SUBSCRIBER_WAIT_S:g}s")
        time.sleep(0.05)
    print("ready", flush=True)
    # End of input: armo react is gone.
    if not sys.stdin.readline():
        return
    if args.bag:
        messages = from_bag(args.bag, args.input)
    else:
        messages = from_yaml(args.msg, msg_type, node)
    sent = 0
    for msg in itertools.islice(messages, args.count):
        publisher.publish(msg)
        sent += 1
        time.sleep(args.period)
    print(f"published {sent}", flush=True)
    node.destroy_node()
    rclpy.shutdown()


if __name__ == "__main__":
    main()
