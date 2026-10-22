#!/usr/bin/env python3
"""ormad against known workloads, through its socket as orma-collectd talks to it (design 12.8).

Every case starts processes that run a known number of trace calls or callbacks, asks ormad for
the probe sets, the stream and windows of those pids, and compares the records and window rows
with what the process says it did. Counts must match exactly.

Needs root for ormad, so colcon test does not run it: run it by hand after a build, with
passwordless sudo, in a shell that has sourced the workspace. `--ormad` defaults to the ormad on
PATH, the one colcon installs.

    daemon/orma_daemon/test/accuracy.py [--ormad PATH] [--only CASE]... [--humble]

Cases: ros_calls, flux_calls, flux_real, cpp_node, cpp_fastdds, py_node, sched, select,
gpu_calls, spans, futex, dds_cyclone, dds_fastdds, selfcheck, and with --humble humble_cpp,
humble_py (docker image flux-humble).
"""

import argparse
import collections
import json
import os
import re
import shutil
import socket
import struct
import subprocess
import sys
import tempfile
import threading
import time
from pathlib import Path

HERE = Path(__file__).resolve().parent
ROOT = HERE.parents[2]
SUDO = [] if os.geteuid() == 0 else ["sudo", "-n"]
DISTRO = os.environ.get("ROS_DISTRO", "jazzy")
OPT = Path("/opt/ros") / DISTRO
DOMAIN = "87"


def event_names():
    text = (ROOT / "common/orma_common/src/events.rs").read_text()
    out = {}
    for name, n in re.findall(r"pub const ORMA_EV_(\w+): u16 = (\d+);", text):
        out[int(n)] = name.lower()
    return out


EVENTS = event_names()
NUM = {v: k for k, v in EVENTS.items()}

# Payload layouts (orma_common/src/events.rs) of the events the cases look into.
PAYLOAD = {
    "rcl_node_init": ("<QQ", "node rmw_node"),
    "rmw_publisher_init": ("<Q24sII", "rmw_handle gid gid_len pad"),
    "rmw_subscription_init": ("<Q24sII", "rmw_handle gid gid_len pad"),
    "rcl_publisher_init": ("<QQQQ", "handle node rmw_handle queue_depth"),
    "rcl_subscription_init": ("<QQQQ", "handle node rmw_handle queue_depth"),
    "rcl_service_init": ("<QQQ", "handle node rmw_handle"),
    "rclcpp_subscription_init": ("<QQ", "subscription_handle subscription"),
    "rclcpp_subscription_callback_added": ("<QQ", "handle callback"),
    "rclcpp_service_callback_added": ("<QQ", "handle callback"),
    "rclcpp_timer_callback_added": ("<QQ", "handle callback"),
    "rcl_timer_init": ("<Qq", "timer period"),
    "rclcpp_callback_register": ("<Q", "callback"),
    "rclcpp_callback_owner": ("<QQQQII", "callback object handle node kind pad"),
    "rcl_publish": ("<QQQ", "publisher message stamp"),
    "rclcpp_publish": ("<QQQ", "publisher message stamp"),
    "rmw_publish": ("<QQqQ", "rmw_publisher message timestamp stamp"),
    "rmw_publish_rcl": ("<QQ", "message stamp"),
    "rmw_take": ("<QQqQII", "rmw_subscription message source_timestamp stamp taken pad"),
    "callback_start": ("<QII", "callback is_intra_process pad"),
    "callback_end": ("<Q", "handle"),
    "callback_code": ("<Q5Q", "callback w0 w1 w2 w3 w4"),
    "rmw_subscription_topic": ("<Q", "rmw_handle"),
    "rmw_publisher_topic": ("<Q", "rmw_handle"),
    "rclcpp_group_add_subscription": ("<QQ", "object arg"),
    "rclcpp_group_add_timer": ("<QQ", "object arg"),
    "rcl_wait_enter": ("<Qq", "wait_set timeout"),
    "rcl_wait_exit": ("<Qii", "wait_set ret pad"),
    "rcl_executor_claim": (
        "<QQQQQII",
        "executor registration entity context group entity_type rclcpp",
    ),
    "rcl_executor_complete": ("<QQ", "executor registration"),
    "dds_serialized_size": ("<Q", "size"),
    "sched_switch": ("<qII", "prev_state next_tid cpu"),
    "sched_wakeup": ("<II", "tid target_cpu"),
    "flux_channel_init": (
        "<QQQiIIi",
        "channel dev ino publisher slot_count slot_size stamp_offset",
    ),
    "flux_claim": ("<QIiiI", "channel slot result loan pad"),
    "flux_loan": ("<QII", "channel slot pad"),
    "flux_loan_end": ("<QIi", "channel slot result"),
    "flux_commit": ("<QQQQIi", "channel ticket nbytes stamp slot woke"),
    "flux_take": ("<QQQQII", "channel ticket lost stamp slot pad"),
    "flux_release": ("<QQiI", "channel ticket held pad"),
    "flux_refused": ("<QiI", "channel reason pad"),
    "flux_wait": ("<QqII", "executor timeout_ns sources pad"),
    "flux_wake": ("<QiI", "executor result entry"),
    "flux_callback_start": ("<QQII", "executor endpoint entry pad"),
    "flux_callback_end": ("<QQIi", "executor channel entry delivered"),
    "flux_endpoint_init": ("<QQii", "endpoint fingerprint role lang"),
    "flux_py_gil": ("<iI", "phase pad"),
    "flux_py_ready": ("<QQ", "channel ticket"),
    "flux_record_drop": ("<QQIi", "channel ticket count reason"),
    "flux_gpu_wedged": ("<iI", "result pad"),
    "dds_write": ("<QQ", "writer duration_ns"),
    "cuda_call": ("<QQIi", "duration_ns bytes api result"),
    "span_begin": ("<Q", "key"),
    "span_end": ("<Q", "key"),
    "futex_wait": ("<QQIi", "uaddr duration_ns op ret"),
}

AGG_CALLBACK, AGG_DISPATCH, AGG_WAIT, AGG_OFFCPU, AGG_CPU_WAIT = 1, 2, 3, 4, 5
CUDA_SYNC = (4, 5, 6)
FUTEX_WAITV = 0xFFFFFFFF
SRC_ROS, SRC_FLUX = 1, 2


class Record:
    def __init__(self, pid, tid, event, flags, t, body):
        self.pid, self.tid, self.event, self.flags, self.t = pid, tid, event, flags, t
        self.name = EVENTS.get(event, str(event))
        self.fields = {}
        size = 0
        if self.name in PAYLOAD:
            fmt, names = PAYLOAD[self.name]
            size = struct.calcsize(fmt)
            self.fields = dict(zip(names.split(), struct.unpack_from(fmt, body)))
        # A trailing empty string is lost with the padding; no case looks for one.
        tail = body[size:].rstrip(b"\0")
        self.strings = [s.decode(errors="replace") for s in tail.split(b"\0")] if tail else []

    def __getitem__(self, k):
        return self.fields[k]


def parse_batch(buf):
    out = []
    at = 0
    while at < len(buf):
        pid, tid, event, length, flags, t = struct.unpack_from("<IIHHIQ", buf, at)
        body = buf[at + 24 : at + 8 + length]
        out.append(Record(pid, tid, event, flags, t, body))
        at += 8 + length
    return out


class Conn:
    """One orma-collectd-like client."""

    def __init__(self, path):
        self.s = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
        self.s.connect(path)
        self.lock = threading.Lock()
        self.records = []
        self.windows = []
        self.replies = {}
        self.lines = []
        self.next = 1
        self.reader = threading.Thread(target=self.read, daemon=True)
        self.reader.start()
        self.op("hello", proto=[1, 0], client="accuracy", pid=os.getpid())

    def read(self):
        buf = b""
        while True:
            try:
                data = self.s.recv(1 << 20)
            except OSError:
                return
            if not data:
                return
            buf += data
            while len(buf) >= 5:
                length = struct.unpack_from("<I", buf)[0]
                if len(buf) < 4 + length:
                    break
                kind, payload = buf[4], buf[5 : 4 + length]
                buf = buf[4 + length :]
                with self.lock:
                    if kind == 2:
                        self.records.extend(parse_batch(payload))
                        continue
                    m = json.loads(payload)
                    if m["type"] == "window":
                        self.windows.append(m)
                    elif m["type"] == "event_line":
                        self.lines.append(m)
                    elif m["type"] == "hello":
                        # ormad's hello carries no id; it answers the first request.
                        self.replies[1] = m
                    else:
                        self.replies[m.get("id")] = m

    def send(self, op, **fields):
        rid = self.next
        self.next += 1
        body = json.dumps({"id": rid, "op": op, **fields}).encode()
        self.s.sendall(struct.pack("<IB", len(body) + 1, 1) + body)
        return rid

    def op(self, op, timeout=120, **fields):
        rid = self.send(op, **fields)
        deadline = time.time() + timeout
        while time.time() < deadline:
            with self.lock:
                if rid in self.replies:
                    m = self.replies.pop(rid)
                    if m["type"] == "error":
                        raise RuntimeError(f"{op}: {m}")
                    return m
            time.sleep(0.01)
        raise RuntimeError(f"no answer to {op}")

    def watch(self, pid, sets=("discovery", "hot"), window=0.5, words=None):
        self.op("attach", all=True, pids=[], sets=["discovery"])
        hot = [s for s in sets if s != "discovery"]
        if hot:
            self.op("attach", all=False, pids=[pid], sets=hot, user_words=words or {})
        self.op("stream", on=True, pids=[pid])
        self.op("window", window_s=window)

    def of(self, pid):
        with self.lock:
            return [r for r in self.records if r.pid == pid]

    def aggs(self, pid):
        """Window rows of `pid` summed: (kind, src, key, entry) -> [count, sum, max]."""
        out = collections.defaultdict(lambda: [0, 0, 0])
        with self.lock:
            for w in self.windows:
                for a in w["aggregates"]:
                    if a["pid"] != pid:
                        continue
                    row = out[(a["kind"], a["src"], a["key"], a["entry"])]
                    row[0] += a["count"]
                    row[1] += a["sum"]
                    row[2] = max(row[2], a["max"])
                    assert sum(a["hist"]) == a["count"], a
        return out

    def settle(self, windows=3):
        """Waits until `windows` more windows came, so the last records and rows are in."""
        with self.lock:
            n = len(self.windows)
        while True:
            with self.lock:
                if len(self.windows) >= n + windows:
                    return
            time.sleep(0.05)

    def close(self):
        self.s.close()


class Ormad:
    def __init__(self, binary, path, log):
        self.path = path
        self.log = open(log, "w")
        group = subprocess.run(["id", "-gn"], capture_output=True, text=True).stdout.strip()
        self.p = subprocess.Popen(
            SUDO + [binary, "--socket", path, "--group", group],
            stdout=self.log,
            stderr=subprocess.STDOUT,
        )
        deadline = time.time() + 30
        while time.time() < deadline:
            try:
                s = socket.socket(socket.AF_UNIX)
                s.connect(path)
                s.close()
                return
            except OSError:
                time.sleep(0.1)
        raise RuntimeError("ormad did not start: " + Path(log).read_text()[-2000:])

    def stop(self):
        subprocess.run(SUDO + ["kill", "-INT", str(self.p.pid)], check=False)
        try:
            self.p.wait(timeout=10)
        except subprocess.TimeoutExpired:
            subprocess.run(SUDO + ["kill", "-KILL", str(self.p.pid)], check=False)


class Proc:
    """A test process: prints its pid first, then runs on "go"."""

    def __init__(self, argv, env=None, cwd=None):
        self.p = subprocess.Popen(
            argv,
            stdin=subprocess.PIPE,
            stdout=subprocess.PIPE,
            text=True,
            env=env,
            cwd=cwd,
        )
        self.first = self.p.stdout.readline().split()
        if not self.first:
            raise RuntimeError(f"{argv} printed nothing")
        self.pid = int(self.first[0])

    def go(self, line="go"):
        self.p.stdin.write(line + "\n")
        self.p.stdin.flush()

    def line(self):
        return self.p.stdout.readline().strip()

    def end(self):
        try:
            self.go("end")
        except BrokenPipeError:
            pass
        self.p.wait(timeout=30)


class Fail:
    def __init__(self, case):
        self.case = case
        self.msgs = []

    def check(self, ok, what):
        if not ok:
            self.msgs.append(what)
        return ok

    def eq(self, got, want, what):
        return self.check(got == want, f"{what}: got {got}, want {want}")


def counts(records):
    return collections.Counter(r.name for r in records)


def compare_counts(f, recs, want, exact_names=True):
    got = counts(recs)
    for name, n in want.items():
        f.eq(got.get(name, 0), n, f"{name} records")
    if exact_names:
        extra = {k: v for k, v in got.items() if k not in want}
        f.check(not extra, f"unexpected records {extra}")
    return got


# Cases ------------------------------------------------------------------------------------------


def private_tracetools(tmp):
    """A copy of libtracetools.so of the calling case's own: a case attaches while the uprobes of
    the one before are still being removed (1.1 s each here), which must not be the same inode."""
    d = Path(tempfile.mkdtemp(dir=tmp))
    return Path(shutil.copy(OPT / "lib/libtracetools.so", d / "libtracetools.so"))


def ros_calls(sock, tmp, f):
    lib = private_tracetools(tmp)
    c = Conn(sock)
    p = Proc([sys.executable, HERE / "caller.py", lib, OPT / "lib/librcl.so"])
    c.watch(p.pid)
    p.go()
    want = json.loads(p.line())
    # flush answers after every record ormad held, so they are all in now.
    c.op("flush")
    recs = c.of(p.pid)
    got = compare_counts(f, recs, want["records"])
    c.settle()
    for r in recs:
        if r.name == "rmw_publish":
            f.eq(r["stamp"], want["stamp"], "rmw_publish stamp")
        if r.name == "rmw_take" and r["taken"]:
            f.eq(r["stamp"], want["stamp"], "rmw_take stamp")
        if r.name == "rmw_publisher_init":
            f.eq(r["gid_len"], 16, "gid_len")
            f.eq(list(r["gid"][:16]), want["gid"][:16], "gid")
            f.eq(r["gid"][16:], bytes(8), "gid tail")
        if r.name == "rmw_subscription_topic":
            f.eq(r.strings, ["/caller/sub"], "subscription topic read at offset 16")
    names = {s for r in recs if r.name == "rclcpp_callback_register" for s in r.strings}
    f.eq(names, {"known_callback_a"}, "registered names")
    a = c.aggs(p.pid)
    for key, n in want["callbacks"].items():
        f.eq(a[(AGG_CALLBACK, SRC_ROS, int(key, 16), 0)][0], n, f"callback rows {key}")
    for key, n in want["dispatch"].items():
        f.eq(a[(AGG_DISPATCH, SRC_ROS, int(key, 16), 0)][0], n, f"dispatch rows {key}")
    waits = sum(v[0] for k, v in a.items() if k[0] == AGG_WAIT)
    f.eq(waits, want["wait"], "wait rows")
    callbacks = {k: v for k, v in a.items() if k[0] == AGG_CALLBACK}
    f.eq(len(callbacks), len(want["callbacks"]), "callback keys")
    p.end()
    c.close()
    return f"{sum(got.values())} records, {sum(v[0] for v in callbacks.values())} callbacks"


def flux_lib():
    for var in ("CMAKE_PREFIX_PATH", "AMENT_PREFIX_PATH"):
        for prefix in os.environ.get(var, "").split(":"):
            p = Path(prefix)
            if prefix and (p / "include/flux.h").is_file():
                return p
    return None


def flux_calls(sock, tmp, f):
    prefix = flux_lib()
    real = (prefix / "lib/libflux_ffi.so").resolve()
    copy = Path(tmp) / real.name
    shutil.copy(real, copy)
    c = Conn(sock)
    p = Proc([sys.executable, HERE / "flux_trace_caller.py", copy])
    c.watch(p.pid)
    p.go()
    want = json.loads(p.line())
    c.settle()
    recs = c.of(p.pid)
    by = collections.defaultdict(list)
    for r in recs:
        by[r.name].append(r)
    f.eq(set(by), set(want["records"]), "flux events")
    for name, w in want["records"].items():
        f.eq(len(by[name]), w["n"], f"{name} records")
        fields = dict(w["fields"])
        strings = fields.pop("strings", [])
        for r in by[name][:1]:
            got = {k: r[k] for k in fields}
            f.eq(got, fields, f"{name} arguments")
            f.eq(r.strings, strings, f"{name} strings")
    a = c.aggs(p.pid)
    for key, n in want["callbacks"].items():
        ex, entry = key.split(":")
        f.eq(a[(AGG_CALLBACK, SRC_FLUX, int(ex, 16), int(entry))][0], n, f"flux callbacks {key}")
    for key, n in want["dispatch"].items():
        ex, entry = key.split(":")
        f.eq(a[(AGG_DISPATCH, SRC_FLUX, int(ex, 16), int(entry))][0], n, f"flux dispatch {key}")
    for ex, n in want["waits"].items():
        f.eq(a[(AGG_WAIT, SRC_FLUX, int(ex, 16), 0)][0], n, f"flux waits {ex}")
    p.end()
    c.close()
    return f"{len(recs)} records over {len(by)} functions"


def flux_real(sock, tmp, f):
    """The C caller on the real libflux_ffi (contract 2): 300 early callbacks, each with a late
    one inside on another executor."""
    prefix = flux_lib()
    real = (prefix / "lib/libflux_ffi.so").resolve()
    copy = Path(tmp) / real.name
    shutil.copy(real, copy)
    (Path(tmp) / "libflux_ffi.so").symlink_to(copy.name)
    exe = Path(tmp) / "flux_caller"
    subprocess.run(
        ["cc", "-O2", "-I", prefix / "include", HERE / "flux_caller.c"]
        + ["-L", tmp, "-lflux_ffi", "-o", exe],
        check=True,
    )
    env = dict(os.environ, LD_LIBRARY_PATH=f"{tmp}:{os.environ.get('LD_LIBRARY_PATH', '')}")
    c = Conn(sock)
    p = subprocess.Popen([exe], stdin=subprocess.PIPE, stdout=subprocess.PIPE, text=True, env=env)
    early_key, late_name = p.stdout.readline().split()
    maps = Path(f"/proc/{p.pid}/maps").read_text()
    f.check(str(copy) in maps, "the caller did not map the private copy")
    c.watch(p.pid)
    p.stdin.write("go\n")
    p.stdin.flush()
    f.eq(p.stdout.readline().strip(), "done", "caller")
    c.settle()
    recs = c.of(p.pid)
    got = counts(recs)
    for name in ("claim", "commit", "take", "release", "callback_start", "callback_end"):
        f.eq(got.get("flux_" + name, 0), 600, f"flux_{name} records")
    f.eq(got.get("flux_channel_init", 0), 2, "channel_init of the late pair")
    a = c.aggs(p.pid)
    rows = sorted(v[0] for k, v in a.items() if k[0] == AGG_CALLBACK and k[1] == SRC_FLUX)
    f.eq(rows, [300, 300], "flux callbacks per executor")
    segments = {r.strings[0] for r in recs if r.name == "flux_callback_end" and r.strings}
    f.check(any(s.startswith(late_name) for s in segments), f"late segment in {segments}")
    p.stdin.close()
    p.wait(timeout=10)
    c.close()
    return f"{sum(got.values())} records"


def build_node(tmp, image=None):
    """node.cpp with cmake against the sourced ROS; returns the executable."""
    src = Path(tmp) / "node_src"
    src.mkdir()
    shutil.copy(HERE / "node.cpp", src)
    (src / "CMakeLists.txt").write_text(
        "cmake_minimum_required(VERSION 3.16)\nproject(orma_acc_node CXX)\n"
        "find_package(rclcpp REQUIRED)\nfind_package(std_msgs REQUIRED)\n"
        "find_package(std_srvs REQUIRED)\nadd_executable(node node.cpp)\n"
        "target_link_libraries(node rclcpp::rclcpp ${std_msgs_TARGETS} ${std_srvs_TARGETS})\n"
    )
    build = Path(tmp) / "node_build"
    subprocess.run(
        ["cmake", "-S", src, "-B", build, "-DCMAKE_BUILD_TYPE=Release"],
        check=True,
        capture_output=True,
    )
    subprocess.run(["cmake", "--build", build, "-j2"], check=True, capture_output=True)
    return build / "node"


def node_checks(f, c, pid, said, prefix, rcl_abi=False, cxx=True, python=False):
    """The records and rows of node.cpp or node.py against the counts it printed."""
    n = dict(zip(said[0::2], map(int, said[1::2])))
    recs = c.of(pid)
    by = collections.defaultdict(list)
    for r in recs:
        by[r.name].append(r)
    topic = prefix + "/out"
    rcl_sub = [r for r in by["rcl_subscription_init"] if r.strings[:1] == [topic]]
    rcl_pub = [r for r in by["rcl_publisher_init"] if r.strings[:1] == [topic]]
    srv = [r for r in by["rcl_service_init"] if r.strings[:1] == [prefix + "/srv"]]
    if not (
        f.eq(len(rcl_sub), 1, "subscription init")
        and f.eq(len(rcl_pub), 1, "publisher init")
        and f.eq(len(srv), 1, "service init")
    ):
        return "inits missing"
    sub_rcl, sub_rmw = rcl_sub[0]["handle"], rcl_sub[0]["rmw_handle"]
    pub_rcl, pub_rmw = rcl_pub[0]["handle"], rcl_pub[0]["rmw_handle"]
    rclcpp_sub = [
        r["subscription"]
        for r in by["rclcpp_subscription_init"]
        if r["subscription_handle"] == sub_rcl
    ]
    f.eq(len(rclcpp_sub), 1, "rclcpp_subscription_init of the subscription")
    added = {r["handle"]: r["callback"] for r in by["rclcpp_subscription_callback_added"]}
    sub_cb = added.get(rclcpp_sub[0]) if rclcpp_sub else None
    srv_cb = {r["handle"]: r["callback"] for r in by["rclcpp_service_callback_added"]}.get(
        srv[0]["handle"]
    )
    periods = {r["timer"]: r["period"] for r in by["rcl_timer_init"]}
    timer_cb = {periods.get(r["handle"]): r["callback"] for r in by["rclcpp_timer_callback_added"]}
    # rclcpp's time source subscribes to /parameter_events; other nodes on the domain publish
    # there, so those runs are not the node's to count.
    rcl_topic = {r["handle"]: r.strings[0] for r in by["rcl_subscription_init"]}
    rclcpp_rcl = {
        r["subscription"]: r["subscription_handle"] for r in by["rclcpp_subscription_init"]
    }
    sub_topic = {
        r["callback"]: rcl_topic.get(rclcpp_rcl.get(r["handle"]))
        for r in by["rclcpp_subscription_callback_added"]
    }
    ignored = {cb for cb, t in sub_topic.items() if t == "/parameter_events"}
    starts = collections.Counter(
        r["callback"] for r in by["callback_start"] if r["callback"] not in ignored
    )
    ends = collections.Counter(
        r["handle"] for r in by["callback_end"] if r["handle"] not in ignored
    )
    a = c.aggs(pid)
    rows = {
        k[2]: v[0]
        for k, v in a.items()
        if k[0] == AGG_CALLBACK and k[1] == SRC_ROS and k[2] not in ignored
    }
    want = {
        sub_cb: n["received"],
        srv_cb: n["served"],
        timer_cb.get(10_000_000): n["timer"],
    }
    if n["timer2"]:
        want[timer_cb.get(15_000_000)] = n["timer2"]
    f.check(None not in want, f"callbacks unnamed: {want}")
    for cb in set(starts) - set(want):
        added = [
            r.name for r in recs if r.fields.get("callback") == cb and r.name != "callback_start"
        ]
        f.check(False, f"callback {cb:#x} ran {starts[cb]} times; its records: {added}")
    f.eq(dict(starts), want, "callback_start per callback")
    f.eq(dict(ends), want, "callback_end per callback")
    f.eq(rows, want, "callback rows per callback")
    pubs = [r for r in by["rcl_publish"] if r["publisher"] == pub_rcl]
    f.eq(len(pubs), n["published"], "rcl_publish of the topic")
    takes = [r for r in by["rmw_take"] if r["rmw_subscription"] == sub_rmw and r["taken"]]
    f.eq(len(takes), n["received"], "rmw_take taken")
    if rcl_abi:
        # rmw's own publishers (Cyclone's ros_discovery_info) publish without rcl_publish, so
        # every rcl_publish has its rmw_publish_rcl with the same message, not the other way.
        rmw_msgs = collections.Counter(r["message"] for r in by["rmw_publish_rcl"])
        unmatched = collections.Counter(r["message"] for r in by["rcl_publish"]) - rmw_msgs
        f.eq(sum(unmatched.values()), 0, "rcl_publish without its rmw_publish_rcl")
        sent = {r["stamp"] for r in pubs}
    else:
        rmw = [r for r in by["rmw_publish"] if r["rmw_publisher"] == pub_rmw]
        f.eq(len(rmw), n["published"], "rmw_publish of the topic")
        sent = {r["stamp"] for r in rmw}
        f.eq(
            {r["source_timestamp"] for r in takes},
            {r["timestamp"] for r in rmw},
            "take source_timestamp = publish timestamp",
        )
    f.eq({r["stamp"] for r in takes}, sent, "header stamps through")
    f.check(len(sent) == n["published"], "stamps distinct")
    gid = {r["gid_len"] for r in by["rmw_publisher_init"]}
    f.eq(gid, {24 if rcl_abi else 16}, "gid_len")
    f.eq(len(by["rcl_wait_enter"]), len(by["rcl_wait_exit"]), "rcl_wait pairs")
    waits = sum(v[0] for k, v in a.items() if k[0] == AGG_WAIT)
    f.eq(waits, len(by["rcl_wait_exit"]), "wait rows = rcl_wait returns")
    dispatch = sum(v[0] for k, v in a.items() if k[0] == AGG_DISPATCH)
    ran = len(by["callback_start"])
    f.check(0 < dispatch <= ran, f"dispatch rows {dispatch} of {ran} callbacks")
    coded = {r["callback"] for r in by["callback_code"]}
    f.check(set(want) <= coded, "callback_code for every callback that ran")
    topics = {r.strings[0] for r in by["rmw_subscription_topic"]}
    f.eq(topics & {topic}, {topic}, "subscription topic from the rmw handle")
    if python:
        names = {s for r in by["rclcpp_callback_register"] for s in r.strings}
        f.check({"on_message", "on_trigger", "on_timer"} <= names, f"rclpy names {names}")
    if cxx:
        # What ormad reads from the executor (design 2.1.4) against the init events. timer2's own
        # group holds no subscription, service or client to take the node from.
        node = [r["node"] for r in by["rcl_node_init"] if r.strings[:1] == ["orma_acc"]]
        timer_of = {p: h for h, p in periods.items()}
        want_owner = {
            sub_cb: (1, rclcpp_sub[0] if rclcpp_sub else None, sub_rcl, node[0], "/orma_acc"),
            srv_cb: (2, srv[0]["handle"], srv[0]["handle"], node[0], "/orma_acc"),
            timer_cb.get(10_000_000): (
                3,
                timer_of[10_000_000],
                timer_of[10_000_000],
                node[0],
                "/orma_acc",
            ),
            timer_cb.get(15_000_000): (3, timer_of[15_000_000], timer_of[15_000_000], 0, ""),
        }
        got_owner = {
            r["callback"]: (r["kind"], r["object"], r["handle"], r["node"], (r.strings or [""])[0])
            for r in by["rclcpp_callback_owner"]
            if r["callback"] in want_owner
        }
        f.eq(got_owner, want_owner, "callback owners from the executor")
        groups = by["rclcpp_group_add_subscription"]
        f.eq([r["arg"] for r in groups if r["arg"] in rclcpp_sub], rclcpp_sub, "group of sub")
        f.check(len(by["rclcpp_group_add_timer"]) >= 2, "group_add_timer")
        f.check(len(by["rclcpp_executor"]) >= 2, "executors")
        if rcl_abi:
            f.check(len(by["rclcpp_executor_add_group"]) >= 1, "executor_add_group (rclcpp 16)")
        else:
            f.check(len(by["rclcpp_collector"]) >= 2, "collectors")
            f.check(
                len(by["rclcpp_collector_add_group_to_collection"])
                + len(by["rclcpp_collector_add_group"])
                >= 1,
                "group to collector",
            )
    return (
        f"{len(recs)} records; callbacks {sum(want.values())} "
        f"(timer {n['timer']}, timer2 {n['timer2']}, sub {n['received']}, srv {n['served']})"
    )


def node_env(rmw):
    return dict(os.environ, ROS_DOMAIN_ID=DOMAIN, RMW_IMPLEMENTATION=rmw)


def dds_checks(f, c, pid, prefix, said):
    """Each publish on the topic has its DDS write next to it on its thread (design 20.2): the
    record after it, or with the tracepoint after the write the one before."""
    n = dict(zip(said[0::2], map(int, said[1::2])))
    recs = c.of(pid)
    topic_pub = {
        r["rmw_handle"]
        for r in recs
        if r.name == "rcl_publisher_init" and r.strings[:1] == [prefix + "/out"]
    }
    by_tid = collections.defaultdict(list)
    for r in recs:
        by_tid[r.tid].append(r)
    pairs = []
    for rs in by_tid.values():
        for i, r in enumerate(rs):
            if r.name != "rmw_publish" or r["rmw_publisher"] not in topic_pub:
                continue
            near = [x for x in rs[i + 1 : i + 2] + rs[max(i - 1, 0) : i] if x.name == "dds_write"]
            if near:
                pairs.append(near[0])
    f.eq(len(pairs), n["published"], "a DDS write next to each publish")
    writers = {w["writer"] for w in pairs}
    f.check(len(writers) == 1 and 0 not in writers, f"one writer for the topic: {writers}")
    by_writer = [r for r in recs if r.name == "dds_write" and r["writer"] in writers]
    f.eq(len(by_writer), n["published"], "DDS writes of the topic's writer")
    f.check(all(w["duration_ns"] > 0 for w in pairs), "DDS write durations")
    durations = sorted(w["duration_ns"] for w in pairs)
    return f"DDS writes {len(pairs)}, p50 {durations[len(durations) // 2] if durations else 0} ns"


def cpp_node(sock, tmp, f, rmw="rmw_cyclonedds_cpp", bandwidth=False, dds=False):
    exe = build_node(tmp)
    prefix = f"/orma_acc_{os.getpid()}"
    c = Conn(sock)
    p = Proc([exe, prefix, "60"], env=node_env(rmw))
    sets = ["discovery", "hot", "rclcpp_symbols", "names"] + (["bandwidth"] if bandwidth else [])
    sets += ["dds"] if dds else []
    c.watch(p.pid, sets=sets)
    p.go()
    said = p.line().split()
    c.settle()
    out = node_checks(f, c, p.pid, said, prefix)
    if bandwidth:
        recs = c.of(p.pid)
        by_tid = collections.defaultdict(list)
        for r in recs:
            by_tid[r.tid].append(r)
        topic_pub = [
            r["rmw_handle"]
            for r in recs
            if r.name == "rcl_publisher_init" and r.strings[:1] == [prefix + "/out"]
        ]
        sizes = []
        for rs in by_tid.values():
            for a, b in zip(rs, rs[1:]):
                if a.name == "rmw_publish" and a["rmw_publisher"] in topic_pub:
                    if b.name == "dds_serialized_size":
                        sizes.append(b["size"])
        n = dict(zip(said[0::2], map(int, said[1::2])))
        f.eq(len(sizes), n["published"], "a serialized size after each publish")
        # CDR: encapsulation 4, stamp 8, frame_id "f": length 4 + 2 bytes.
        f.eq(set(sizes), {18}, "serialized sizes")
        out += f"; serialized sizes {collections.Counter(sizes)}"
    if dds:
        out += "; " + dds_checks(f, c, p.pid, prefix, said)
    p.end()
    c.close()
    return out


def cpp_fastdds(sock, tmp, f):
    return cpp_node(sock, tmp, f, rmw="rmw_fastrtps_cpp", bandwidth=True)


def dds_cyclone(sock, tmp, f):
    return cpp_node(sock, tmp, f, dds=True)


def dds_fastdds(sock, tmp, f):
    return cpp_node(sock, tmp, f, rmw="rmw_fastrtps_cpp", dds=True)


def py_node(sock, tmp, f):
    rclpy_src = ROOT / "collect/orma_rclpy"
    path = f"{rclpy_src / 'autoload'}:{rclpy_src}:{os.environ.get('PYTHONPATH', '')}"
    env = dict(node_env("rmw_cyclonedds_cpp"), PYTHONPATH=path)
    prefix = f"/orma_acc_py_{os.getpid()}"
    c = Conn(sock)
    p = Proc([sys.executable, HERE / "node.py", prefix, "60"], env=env)
    c.watch(p.pid)
    p.go()
    said = p.line().split()
    c.settle()
    out = node_checks(f, c, p.pid, said, prefix, cxx=False, python=True)
    p.end()
    c.close()
    return out


def sched(sock, tmp, f):
    """Callbacks that sleep 2 ms and callbacks that spin 2 ms, under the uprobe stack and under a
    shim-style user word: off-CPU is the sleep, not the spin (design 15.2)."""
    lib = private_tracetools(tmp)
    n, ms = 100, 2.0
    c = Conn(sock)
    p = Proc([sys.executable, HERE / "sched_caller.py", lib, "1", str(n), str(ms)])
    tid, word_tid, word = int(p.first[1]), int(p.first[2]), int(p.first[3])
    c.watch(p.pid, sets=["discovery", "hot", "sched"], words={str(word_tid): word})
    p.go()
    wall = {int(k, 16): v for k, v in json.loads(p.line()).items()}
    c.settle()
    a = c.aggs(p.pid)
    busy = n * ms * 1e6
    off = {
        key: a[(AGG_OFFCPU, src, key, 0)][1]
        for key, src in ((0xA1, SRC_ROS), (0xA2, SRC_ROS), (0xA3, 1), (0xA4, 1))
    }
    on = {k: wall[k] - off[k] for k in off}
    for k in (0xA1, 0xA3):
        f.check(on[k] <= 0.2 * busy, f"on-CPU of the sleeping {k:#x}: {on[k]}")
        f.check(off[k] >= 0.9 * busy, f"off-CPU of the sleeping {k:#x}: {off[k]}")
    for k in (0xA2, 0xA4):
        f.check(abs(on[k] - busy) <= 0.15 * busy, f"on-CPU of the spinning {k:#x}: {on[k]}")
    cbs = a[(AGG_CALLBACK, SRC_ROS, 0xA1, 0)]
    f.check(cbs[1] >= off[0xA1], "a callback's off-CPU is inside its time")
    waits = a.get((AGG_CPU_WAIT, 0, tid, 0), [0, 0, 0])
    f.check(waits[0] >= n, f"CPU wait rows for the thread: {waits[0]}")
    c.op("flush")
    # A switch is recorded as the task leaving the CPU's; one onto the thread names it in
    # next_tid and often belongs to the idle task, so every process's records count.
    with c.lock:
        switches = [
            r for r in c.records if r.name == "sched_switch" and tid in (r.tid, r["next_tid"])
        ]
    f.check(len(switches) >= 2 * n, f"sched_switch records of the thread: {len(switches)}")
    p.end()
    c.close()
    return (
        "ms on/off-CPU: "
        + ", ".join(f"{k:#x} {on[k] / 1e6:.1f}/{off[k] / 1e6:.1f}" for k in off)
        + f" (each {n * ms:.0f}); CPU waits {waits[0]}, mean {waits[1] / max(waits[0], 1):.0f} ns"
    )


def missing(c, keep):
    """The next window's probes_missing entries that `keep` takes."""
    c.settle(1)
    with c.lock:
        return [m for m in c.windows[-1]["probes_missing"] if keep(m)]


def gpu_calls(sock, tmp, f):
    """Every driver function of the gpu set in a stand-in libcuda.so.1 (fake_cuda.c), called a
    known number of times: api, bytes and result of each call, the synchronize calls' 2 ms, and
    a second thread's calls among them (design 20.1)."""
    lib = Path(tmp) / "libcuda.so.1"
    subprocess.run(["cc", "-shared", "-fPIC", "-O0", "-o", lib, HERE / "fake_cuda.c"], check=True)
    times = 5
    c = Conn(sock)
    p = Proc([sys.executable, HERE / "cuda_caller.py", lib, str(times)])
    worker = int(p.first[1])
    c.watch(p.pid, sets=["discovery", "gpu"])
    f.eq(missing(c, lambda m: m["pid"] == p.pid), [], "probes_missing of the process")
    p.go()
    want = {(a, b, r): n for a, b, r, n in json.loads(p.line())}
    c.op("flush")
    recs = [r for r in c.of(p.pid) if r.name == "cuda_call"]
    got = collections.Counter((r["api"], r["bytes"], r["result"]) for r in recs)
    f.eq(dict(got), want, "cuda_call records by (api, bytes, result)")
    sync = [r["duration_ns"] for r in recs if r["api"] in CUDA_SYNC]
    f.check(min(sync, default=0) >= 1_900_000, f"synchronize durations from {min(sync, default=0)}")
    f.eq(sum(r.tid == worker for r in recs), times, "the second thread's calls")
    p.end()
    c.close()
    return f"{len(recs)} cuda_call records over {len(want)} (api, bytes, result)"


def spans(sock, tmp, f):
    """Nested spans through liborma_span.so (collect/orma_span when built, else fake_span.c,
    built -O0 so its two empty bodies keep two addresses): begin and end in order, keyed by the
    name's address, the name on the begin (design 20.5)."""
    real = next(
        (
            Path(p) / "lib/liborma_span.so"
            for p in os.environ.get("AMENT_PREFIX_PATH", "").split(":")
            if p and (Path(p) / "lib/liborma_span.so").is_file()
        ),
        None,
    )
    lib = Path(tmp) / "liborma_span.so"
    if real:
        shutil.copy(real, lib)
    else:
        subprocess.run(
            ["cc", "-shared", "-fPIC", "-O0", "-o", lib, HERE / "fake_span.c"], check=True
        )
    n = 200
    c = Conn(sock)
    p = Proc([sys.executable, HERE / "span_caller.py", lib, str(n)])
    outer, inner = int(p.first[1]), int(p.first[2])
    c.watch(p.pid)
    p.go()
    f.eq(p.line(), "done", "span caller")
    c.op("flush")
    got = [
        (r.name, r["key"], r.strings) for r in c.of(p.pid) if r.name in ("span_begin", "span_end")
    ]
    want = [
        ("span_begin", outer, ["outer"]),
        ("span_begin", inner, ["inner"]),
        ("span_end", inner, []),
        ("span_end", outer, []),
    ] * n
    f.eq(len(got), len(want), "span records")
    f.check(got == want, "span records in order, keyed by the name's address")
    p.end()
    c.close()
    return f"{len(got)} span records from {'collect/orma_span' if real else 'fake_span.c'}"


def futex(sock, tmp, f):
    """Futex waits of one thread (futex_caller.c): every FUTEX_WAIT on a word with its result
    and the 2 ms until the wake, a futex_waitv that times out, a condition variable wait; nothing
    of other processes (design 20.4)."""
    exe = Path(tmp) / "futex_caller"
    subprocess.run(["cc", "-O2", "-pthread", HERE / "futex_caller.c", "-o", exe], check=True)
    c = Conn(sock)
    p = Proc([exe, "50"])
    tid, word, word2 = (int(x) for x in p.first[1:4])
    c.watch(p.pid, sets=["discovery", "futex"])
    f.eq(missing(c, lambda m: m["point"] == "futex_wait"), [], "probes_missing of futex_wait")
    p.go()
    said = json.loads(p.line())
    c.op("flush")
    recs = [r for r in c.of(p.pid) if r.name == "futex_wait" and r.tid == tid]
    on_word = [r for r in recs if r["uaddr"] == word]
    f.eq(len(on_word), said["calls"], "futex_wait records on the word")
    f.eq({r["op"] for r in on_word}, {0}, "FUTEX_WAIT without the private flag")
    f.eq(sum(r["ret"] == 0 for r in on_word), said["ok"], "woken waits")
    f.eq(sum(r["ret"] == -11 for r in on_word), said["eagain"], "EAGAIN waits")
    woken = sorted(r["duration_ns"] for r in on_word if r["ret"] == 0)
    p50 = woken[len(woken) // 2] if woken else 0
    f.check(p50 >= 1_500_000, f"woken wait p50 {p50} ns")
    if said["waitv"] != -38:
        waitv = [(r["op"], r["ret"]) for r in recs if r["uaddr"] == word2]
        f.eq(waitv, [(FUTEX_WAITV, said["waitv"])], "futex_waitv")
    cond = [r for r in recs if r["uaddr"] not in (word, word2) and r["duration_ns"] >= 4_000_000]
    f.check(cond and all(r["op"] in (0, 9) for r in cond), f"condition variable wait: {len(cond)}")
    with c.lock:
        others = [r for r in c.records if r.name == "futex_wait" and r.pid != p.pid]
    f.eq(len(others), 0, "futex_wait records of other processes")
    p.end()
    c.close()
    return f"{len(recs)} waits; woken p50 {p50 / 1e6:.2f} ms; futex_waitv {said['waitv']}"


def instruction(pid, lib, symbol_offset):
    """The 4 bytes at a file offset of `lib` in `pid`'s memory, read as root."""
    code = (
        "import sys\n"
        f"pid, lib, off = {pid}, {str(lib)!r}, {symbol_offset}\n"
        "for line in open(f'/proc/{pid}/maps'):\n"
        "    f = line.split()\n"
        "    if len(f) > 5 and f[5] == lib and 'x' in f[1]:\n"
        "        lo, hi = (int(x, 16) for x in f[0].split('-'))\n"
        "        foff = int(f[2], 16)\n"
        "        if foff <= off < foff + hi - lo:\n"
        "            m = open(f'/proc/{pid}/mem', 'rb'); m.seek(lo + off - foff)\n"
        "            print(m.read(4).hex()); break\n"
    )
    out = subprocess.run(SUDO + [sys.executable, "-c", code], capture_output=True, text=True)
    return bytes.fromhex(out.stdout.strip())


def symbol_offset(lib, name):
    out = subprocess.run(["nm", "-D", "--defined-only", lib], capture_output=True, text=True)
    addr = next(int(l.split()[0], 16) for l in out.stdout.splitlines() if l.endswith(" " + name))
    hdr = subprocess.run(["readelf", "-SW", lib], capture_output=True, text=True).stdout
    for l in hdr.splitlines():
        m = re.match(r"\s*\[\s*\d+\]\s+(\S+)\s+\S+\s+([0-9a-f]+)\s+([0-9a-f]+)\s+([0-9a-f]+)", l)
        if m and m.group(1) == ".text":
            vaddr, off = int(m.group(2), 16), int(m.group(3), 16)
            return addr - vaddr + off
    raise RuntimeError("no .text")


def brk(word):
    return word and (int.from_bytes(word, "little") & 0xFFE0001F) == 0xD4200000


def select(sock, tmp, f):
    """Hot probes of one pid: another process on the same library file has no breakpoint and
    is not counted; detaching takes the breakpoint out (design 6.1, 12.8)."""
    lib = private_tracetools(tmp)
    n = 500
    off = symbol_offset(lib, "ros_trace_callback_start")
    a = Proc([sys.executable, HERE / "pid_caller.py", lib, str(n)])
    b = Proc([sys.executable, HERE / "pid_caller.py", lib, str(n)])
    before = instruction(b.pid, lib, off)
    c = Conn(sock)
    c.op("attach", all=False, pids=[a.pid], sets=["hot"])
    c.op("stream", on=True, pids=[a.pid])
    c.op("window", window_s=0.5)
    c.settle(1)
    with c.lock:
        missing = c.windows[-1]["probes_missing"]
    f.check(brk(instruction(a.pid, lib, off)), f"selected process has a breakpoint ({missing})")
    f.eq(instruction(b.pid, lib, off), before, "other process's instruction")
    f.check(not brk(before), "other process has no breakpoint")
    for p in (a, b):
        p.go()
        f.eq(p.line(), "done", "round")
    c.settle()
    f.eq(len([r for r in c.of(a.pid) if r.name == "callback_start"]), n, "selected records")
    f.eq(len(c.of(b.pid)), 0, "other process's records")
    f.eq(c.aggs(a.pid)[(AGG_CALLBACK, SRC_ROS, 0x99, 0)][0], n, "selected rows")
    f.eq(len(c.aggs(b.pid)), 0, "other process's rows")
    with c.lock:
        bpf = collections.Counter()
        for w in c.windows:
            for prog in w["bpf"]:
                bpf[prog["prog"]] += prog["run_cnt"]
                f.check(prog["pid"] is None, "bpf rows are per program")
        ns = sum(
            p["run_time_ns"] for w in c.windows for p in w["bpf"] if p["prog"] == "callback_start"
        )
    f.eq(bpf["callback_start"], n, "BPF run count of callback_start")
    f.check(ns > 0, "BPF run time of callback_start")
    c.op("detach", pids=[], sets=["hot"])
    # The kernel removes the uprobes one at a time, about 1.1 s each here.
    detached = time.monotonic()
    while instruction(a.pid, lib, off) != before and time.monotonic() - detached < 60:
        time.sleep(0.2)
    gone_s = time.monotonic() - detached
    f.eq(instruction(a.pid, lib, off), before, "instruction after detach")
    a.go()
    f.eq(a.line(), "done", "round after detach")
    c.settle()
    f.eq(len([r for r in c.of(a.pid) if r.name == "callback_start"]), n, "no records after detach")
    for p in (a, b):
        p.end()
    c.close()
    return (
        f"BPF callback_start {bpf['callback_start']} runs, {ns / max(n, 1):.0f} ns each; "
        f"breakpoint gone {gone_s:.2f} s after detach"
    )


def selfcheck(sock, tmp, f):
    """Two node.cpp processes on one topic, started inside ormad's selfcheck window."""
    exe = build_node(tmp)
    prefix = f"/orma_sc_{os.getpid()}"
    env = node_env("rmw_cyclonedds_cpp")
    a = Proc([exe, prefix, "40"], env=env)
    b = Proc([exe, prefix, "40"], env=env)
    c = Conn(sock)
    rid = c.send("selfcheck", duration_s=8.0, pids=[a.pid, b.pid])
    # Attaching every set takes tens of seconds on this RT kernel and the window opens after it;
    # ormad answers the flush only once it has.
    c.op("flush")
    a.go()
    b.go()
    a.line()
    b.line()
    # Detaching takes about 1.1 s per probe on this RT kernel, and attaches wait behind it.
    deadline = time.time() + 120
    while rid not in c.replies and time.time() < deadline:
        time.sleep(0.1)
    checks = c.replies[rid]["checks"]
    status = {ch["item"]: ch["status"] for ch in checks}
    for ch in checks:
        print(f"    {ch['status']:7} {ch['item']}: {ch['detail']}")
    for item in (
        "tracepoint layout",
        "bpf stats",
        "probes discovery",
        "probes hot",
        "probes rclcpp_symbols",
        "probes dds",
        "futex tracepoint layout",
        f"dds write pid {a.pid} (node)",
        "rmw topic_name offset 16",
        "rmw_publish timestamp = rmw_take source_timestamp",
        f"distro pid {a.pid} (node)",
        f"rclcpp symbols pid {a.pid}",
        f"header.stamp {prefix}/out",
    ):
        f.eq(status.get(item), "pass", item)
    for p in (a, b):
        p.end()
    c.close()
    return f"{len(checks)} checks, {sum(s == 'pass' for s in status.values())} pass"


def docker_node(tmp, f, sock, python):
    """A node in the flux-humble container, attached by the host's ormad through
    /proc/<pid>/root (design 2.7.3)."""
    name = f"orma_acc_{os.getpid()}_{int(python)}"
    prefix = f"/orma_h_{os.getpid()}"
    mounts = ["-v", f"{HERE}:/t:ro", "-v", f"{ROOT / 'collect/orma_rclpy'}:/orma_rclpy:ro"]
    setup = "source /opt/ros/humble/setup.bash && export ROS_DOMAIN_ID=" + DOMAIN
    if python:
        run = (
            f"{setup} && export PYTHONPATH=/orma_rclpy/autoload:/orma_rclpy:$PYTHONPATH && "
            f"echo built && exec python3 /t/node.py {prefix} 60"
        )
    else:
        run = (
            f"{setup} && mkdir -p /b/s && cp /t/node.cpp /b/s && cat > /b/s/CMakeLists.txt <<'E'\n"
            "cmake_minimum_required(VERSION 3.16)\nproject(n CXX)\nfind_package(rclcpp REQUIRED)\n"
            "find_package(std_msgs REQUIRED)\nfind_package(std_srvs REQUIRED)\n"
            "add_executable(node node.cpp)\n"
            "target_link_libraries(node rclcpp::rclcpp ${std_msgs_TARGETS} ${std_srvs_TARGETS})\nE\n"
            "cmake -S /b/s -B /b/o -DCMAKE_BUILD_TYPE=Release >/dev/null && "
            f"cmake --build /b/o -j2 >/dev/null && echo built && exec /b/o/node {prefix} 60"
        )
    p = subprocess.Popen(
        SUDO
        + ["docker", "run", "-i", "--rm", "--name", name]
        + mounts
        + ["flux-humble:latest", "bash", "-c", run],
        stdin=subprocess.PIPE,
        stdout=subprocess.PIPE,
        text=True,
    )
    if p.stdout.readline().strip() != "built":
        raise RuntimeError("the container did not build the node")
    p.stdout.readline()
    host = int(
        subprocess.run(
            SUDO + ["docker", "inspect", "-f", "{{.State.Pid}}", name],
            capture_output=True,
            text=True,
            check=True,
        ).stdout
    )
    c = Conn(sock)
    c.watch(host, sets=["discovery", "hot", "rclcpp_symbols"] + ([] if python else ["names"]))
    p.stdin.write("go\n")
    p.stdin.flush()
    said = p.stdout.readline().split()
    c.settle()
    out = node_checks(f, c, host, said, prefix, rcl_abi=True, cxx=not python, python=python)
    p.stdin.write("end\n")
    p.stdin.flush()
    p.wait(timeout=30)
    c.close()
    return out + f" (host pid {host})"


def humble_cpp(sock, tmp, f):
    return docker_node(tmp, f, sock, python=False)


def humble_py(sock, tmp, f):
    return docker_node(tmp, f, sock, python=True)


ORMAD_BIN = [""]

# Attaching takes about 0.25 s and removing about 1.1 s per uprobe on this RT kernel, one at a
# time across processes: select times its detach, so it goes before the other cases leave uprobes
# to remove.
CASES = {
    "select": select,
    "ros_calls": ros_calls,
    "flux_calls": flux_calls,
    "flux_real": flux_real,
    "cpp_node": cpp_node,
    "cpp_fastdds": cpp_fastdds,
    "py_node": py_node,
    "sched": sched,
    "gpu_calls": gpu_calls,
    "spans": spans,
    "futex": futex,
    "dds_cyclone": dds_cyclone,
    "dds_fastdds": dds_fastdds,
    "selfcheck": selfcheck,
}
HUMBLE = {"humble_cpp": humble_cpp, "humble_py": humble_py}


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--ormad", default=shutil.which("ormad"))
    ap.add_argument("--only", action="append", default=[])
    ap.add_argument("--humble", action="store_true")
    ap.add_argument("--keep-log", help="copy ormad's log here")
    args = ap.parse_args()
    ORMAD_BIN[0] = args.ormad
    cases = dict(CASES, **(HUMBLE if args.humble else {}))
    if args.only:
        cases = {k: v for k, v in {**CASES, **HUMBLE}.items() if k in args.only}
    failed = []
    with tempfile.TemporaryDirectory(prefix="orma_acc_") as tmp:
        os.chmod(tmp, 0o755)
        sock = f"{tmp}/ormad.sock"
        ormad = Ormad(args.ormad, sock, f"{tmp}/ormad.log")
        try:
            for name, case in cases.items():
                if name.startswith("flux") and not flux_lib():
                    failed.append(f"{name}: flux_ffi not under CMAKE_PREFIX_PATH")
                    continue
                f = Fail(name)
                with tempfile.TemporaryDirectory(prefix=f"orma_{name}_") as ctmp:
                    os.chmod(ctmp, 0o755)
                    t0 = time.time()
                    try:
                        summary = case(sock, ctmp, f)
                    except Exception as e:  # noqa: BLE001
                        f.msgs.append(f"raised {type(e).__name__}: {e}")
                        summary = ""
                state = "ok" if not f.msgs else "FAILED"
                print(f"{name}: {state} ({time.time() - t0:.1f} s) {summary}", flush=True)
                for m in f.msgs:
                    print(f"    {m}")
                    failed.append(f"{name}: {m}")
        finally:
            ormad.stop()
            log = Path(f"{tmp}/ormad.log").read_text()
            if args.keep_log:
                Path(args.keep_log).write_text(log)
    if failed:
        print("ormad log tail:\n" + log[-3000:])
        print(f"FAILED: {len(failed)}")
        return 1
    print("ok")
    return 0


if __name__ == "__main__":
    sys.exit(main())
