#!/usr/bin/env python3
"""Runs a test node under LD_PRELOAD=libarmo_preload.so with shm_reader attached and fails unless
the shared memory file holds exactly what the node did: each callback's count in the ring and in
the tables, its registered name and code words, the stamps of the published and taken Headers,
rcl_wait on every wait, and for C++ the rclcpp symbol events.

    nodes.py READER PRELOAD EVENTS_H cpp NODE
    nodes.py READER PRELOAD EVENTS_H py NODE_PY [--without-armo-rclpy]
"""

import json
import os
import re
import struct
import subprocess
import sys
from collections import Counter, defaultdict
from pathlib import Path

CHATTER = "/armo_test/chatter"
TRIGGER = "/armo_test/trigger"
WANT = {
    "cpp": {"t1": 100, "sub": 100, "t2": 50, "srv": 20},
    "py": {"t1": 100, "sub": 100, "srv": 20},
}
STAMPS = {7 << 32 | (1000 + i) for i in range(100)}


def event_numbers(header):
    text = Path(header).read_text()
    return {m[1].lower(): int(m[2]) for m in re.finditer(r"#define ARMO_EV_(\w+) (\d+)", text)}


def run(reader, preload, cmd, env):
    env = dict(env, LD_PRELOAD=preload)
    node = subprocess.Popen(cmd, stdin=subprocess.PIPE, stdout=subprocess.PIPE, text=True, env=env)
    shm = subprocess.Popen([reader, str(node.pid)], stdout=subprocess.PIPE, text=True)
    if shm.stdout.readline() != "A\n":
        node.kill()
        sys.exit(f"the reader did not arm: {shm.wait()}")
    node.stdin.write("go\n")
    node.stdin.flush()
    out, _ = node.communicate(timeout=120)
    dump, _ = shm.communicate(timeout=30)
    if node.returncode != 0 or shm.returncode != 0:
        sys.exit(f"node {node.returncode}, reader {shm.returncode}")
    return json.loads(out.strip().splitlines()[-1]), dump


class Dump:
    def __init__(self, text, ev):
        self.records = defaultdict(list)
        self.tables = Counter()
        self.overflow = Counter()
        for line in text.splitlines():
            f = line.split()
            if f[0] == "R":
                self.records[int(f[3])].append(
                    (int(f[2]), bytes.fromhex(f[6] if len(f) > 6 else ""))
                )
            elif f[0] == "T":
                self.tables[(int(f[4]), int(f[5]), int(f[7]))] += int(f[8])
            elif f[0] == "S":
                self.overflow["table"] += int(f[5])
                self.overflow["depth"] += int(f[6])
                self.overflow["ring"] += int(f[7])
            elif f[0] == "H":
                self.threads_overflow = int(f[1])
                self.missing = [int(x, 16) for x in f[2:6]]
        self.ev = ev

    def get(self, name, fmt, strings=0):
        n = struct.calcsize(fmt)
        out = []
        for _, body in self.records[self.ev[name]]:
            texts = body[n:].split(b"\0")[:strings]
            out.append(struct.unpack_from(fmt, body) + tuple(t.decode() for t in texts))
        return out


def main():
    reader, preload, events_h, kind, node = sys.argv[1:6]
    without = "--without-armo-rclpy" in sys.argv
    env = dict(os.environ, ROS_DOMAIN_ID=str(40 + os.getpid() % 50))
    env[
        "ROS_AUTOMATIC_DISCOVERY_RANGE"
        if env.get("ROS_DISTRO") != "humble"
        else "ROS_LOCALHOST_ONLY"
    ] = "LOCALHOST" if env.get("ROS_DISTRO") != "humble" else "1"
    if kind == "py":
        cmd = [sys.executable, node]
        if not without:
            from ament_index_python.packages import get_package_share_directory

            auto = Path(get_package_share_directory("armo_rclpy")) / "autoload"
            env["PYTHONPATH"] = f"{auto}:{env.get('PYTHONPATH', '')}"
    else:
        cmd = [node]
    counts, text = run(reader, preload, cmd, env)
    d = Dump(text, event_numbers(events_h))
    failed = []

    def check(ok, what):
        if not ok:
            failed.append(what)

    check(counts == WANT[kind], f"node counts {counts}")
    check(
        d.threads_overflow == 0 and not any(d.missing), f"header {d.threads_overflow} {d.missing}"
    )
    check(not +d.overflow, f"overflow {dict(d.overflow)}")

    waits = len(d.records[d.ev["rcl_wait_enter"]])
    check(waits > 0 and waits == len(d.records[d.ev["rcl_wait_exit"]]), f"rcl_wait {waits}")
    if without:
        check(not d.records[d.ev["callback_start"]], "callback_start without armo_rclpy")
        return report(failed, kind + " without armo_rclpy", d)

    period = dict(d.get("rcl_timer_init", "<Qq"))
    timer_cb = {period[h]: cb for h, cb in d.get("rclcpp_timer_callback_added", "<QQ")}
    rcl_sub = [r[0] for r in d.get("rcl_subscription_init", "<QQQQ", 1) if r[4] == CHATTER]
    obj = dict(d.get("rclcpp_subscription_init", "<QQ"))
    sub_cb = dict(d.get("rclcpp_subscription_callback_added", "<QQ"))
    srv = [r[0] for r in d.get("rcl_service_init", "<QQQ", 1) if r[3] == TRIGGER]
    srv_cb = dict(d.get("rclcpp_service_callback_added", "<QQ"))
    cbs = {"t1": timer_cb[2_000_000], "sub": sub_cb[obj[rcl_sub[0]]], "srv": srv_cb[srv[0]]}
    if kind == "cpp":
        cbs["t2"] = timer_cb[3_000_000]

    starts = Counter(r[0] for r in d.get("callback_start", "<QII"))
    ends = Counter(r[0] for r in d.get("callback_end", "<Q"))
    names = {r[0]: r[1] for r in d.get("rclcpp_callback_register", "<Q", 1)}
    codes = {r[0] for r in d.get("callback_code", "<Q")}
    for name, cb in cbs.items():
        n = WANT[kind][name]
        check(starts[cb] == n and ends[cb] == n, f"{name}: ring {starts[cb]}, {ends[cb]}")
        check(d.tables[(1, 1, cb)] == n, f"{name}: table {d.tables[(1, 1, cb)]}")
        check(cb in names and cb in codes, f"{name}: name {names.get(cb)}, code {cb in codes}")
    check(d.tables[(2, 1, cbs["t1"])] > 0, "t1 dispatch")
    check(any(k[0] == 3 for k in d.tables), "wait rows")

    pubs = [r for r in d.get("rcl_publisher_init", "<QQQQ", 1) if r[4] == CHATTER]
    gids = {r[0] for r in d.get("rmw_publisher_init", "<Q")}
    check(pubs[0][2] in gids, "rmw_publisher_init of chatter")
    stamps = {r[2] for r in d.get("rcl_publish", "<QQQ") if r[0] == pubs[0][0]}
    check(stamps == STAMPS, f"rcl_publish stamps {len(stamps)}")
    if d.records[d.ev["rmw_publish_rcl"]]:
        # The rcl publish ABI (Humble) names no publisher: the chatter stamps are among all.
        stamps = {r[1] for r in d.get("rmw_publish_rcl", "<QQ")}
        check(STAMPS <= stamps, "rmw_publish_rcl stamps")
    else:
        stamps = {r[3] for r in d.get("rmw_publish", "<QQqQ") if r[0] == pubs[0][2]}
        check(stamps == STAMPS, f"rmw_publish stamps {len(stamps)}")
    rmw_sub = [r[2] for r in d.get("rcl_subscription_init", "<QQQQ", 1) if r[4] == CHATTER]
    taken = {r[3] for r in d.get("rmw_take", "<QQqQII") if r[0] == rmw_sub[0] and r[4]}
    check(taken == STAMPS, f"rmw_take stamps {len(taken)}")

    if kind == "cpp":
        sub_objects = set(obj.values())
        grouped = {r[1] for r in d.get("rclcpp_group_add_subscription", "<QQ")}
        check(sub_objects <= grouped, "group_add_subscription arg")
        check(len({r[0] for r in d.get("rclcpp_executor", "<QQ")}) >= 3, "executors")
        check(d.get("rclcpp_publisher_base", "<QQ"), "publisher_base")
        joined = d.get("rclcpp_collector_add_group_to_collection", "<QQ")
        joined += d.get("rclcpp_executor_add_group", "<QQ")
        check(joined, "group to executor")
    return report(failed, kind, d)


def report(failed, what, d):
    total = sum(len(v) for v in d.records.values())
    if failed:
        sys.exit(f"{what}: " + "; ".join(failed))
    print(f"{what}: ok, {total} records")


if __name__ == "__main__":
    main()
