#!/usr/bin/env python3
"""Delivery paths (docs/instrumentation.md 2.13) against real nodes of the ROS 2 overhaul on
rmw_zenoh with a private router: the rcl broker, zenoh shared memory, a serialized payload, rmw
loans, and rclpy. Each case runs delivery_node.cpp or .py for a few seconds under one method and
checks that every message a subscription counted was taken on the path the case makes, that
none was lost, and that the hold, take to callback and rclpy conversion segments are there.

The overlay must have the wp12 rcl, rmw_zenoh and rclpy (their anchors and the rmw_publish
timestamp fix). `--stock` adds the same zenoh cases on /opt/ros/$ROS_DISTRO, where the broker
and rclpy cases do not apply. preload and lttng need no privileges; uprobe needs root for
ormad (sudo -n), as accuracy.py does.

    integration/test/delivery.py --overlay ~/ros2_ws/install/setup.bash \
        [--method preload|lttng|uprobe] [--stock] [--only CASE]... [--port 7487]
"""

import argparse
import contextlib
import os
import shutil
import socket
import subprocess
import sys
import tempfile
import time
from pathlib import Path

import common

HERE = Path(__file__).resolve().parent
SOURCE = 'source "$0" >/dev/null && exec "$@"'
DOMAIN = "187"
T = f"/orma_delivery_{os.getpid()}"
IMG = f"{T}/img"
MIB = 1 << 20

# name: [(process, env, argv, python)], {subscribing process: expected path}, stock too
CASES = {
    "broker": (
        [("a", {"RCL_INTRA_PROCESS": "1"}, [f"pub={IMG}:image:{MIB}:50", f"sub={IMG}:image"], 0)],
        {"a": "broker"},
        False,
    ),
    "shm": (
        [("p", {}, [f"pub={IMG}:image:{MIB}:50"], 0), ("s", {}, [f"sub={IMG}:image"], 0)],
        {"s": "shm"},
        True,
    ),
    "net": (
        [("p", {}, [f"pub={IMG}:image:64:100"], 0), ("s", {}, [f"sub={IMG}:image"], 0)],
        {"s": "net"},
        True,
    ),
    "loan": (
        [
            ("p", {}, [f"pub={T}/plain:plain:0:30:loan"], 0),
            ("s", {"ROS_DISABLE_LOANED_MESSAGES": "0"}, [f"sub={T}/plain:plain"], 0),
        ],
        {"s": "loan"},
        False,
    ),
    "py_loan": (
        [
            ("p", {}, [f"pub={T}/plain:plain:0:30:loan"], 1),
            (
                "s",
                {"ROS_DISABLE_LOANED_MESSAGES": "0", "RCLPY_MESSAGE_ARRAYS": "numpy"},
                [f"sub={T}/plain:plain"],
                1,
            ),
        ],
        {"s": "loan"},
        False,
    ),
    "mixed": (
        [
            ("a", {"RCL_INTRA_PROCESS": "1"}, [f"pub={IMG}:image:{MIB}:50", f"sub={IMG}:image"], 0),
            ("s", {}, [f"sub={IMG}:image"], 0),
        ],
        {"a": "broker", "s": "shm"},
        False,
    ),
    "py_shm": (
        [
            ("p", {}, [f"pub={IMG}:image:{MIB}:50"], 0),
            ("s", {"RCLPY_MESSAGE_ARRAYS": "numpy"}, [f"sub={IMG}:image"], 1),
        ],
        {"s": "shm"},
        False,
    ),
    # rclpy's RclExecutor takes from C++, where no Python wrapper reaches.
    "py_rcl": (
        [
            ("p", {}, [f"pub={IMG}:image:{MIB}:50"], 0),
            ("s", {"RCLPY_MESSAGE_ARRAYS": "numpy"}, [f"sub={IMG}:image", "executor=rcl"], 1),
        ],
        {"s": "shm"},
        False,
    ),
    "py_broker": (
        [
            (
                "a",
                {"RCL_INTRA_PROCESS": "1", "RCLPY_MESSAGE_ARRAYS": "numpy"},
                [f"pub={IMG}:image:{MIB}:50", f"sub={IMG}:image"],
                1,
            )
        ],
        {"a": "broker"},
        False,
    ),
}


def build(tmp, setup, name):
    src = Path(tmp) / f"{name}_src"
    src.mkdir()
    shutil.copy(HERE / "delivery_node.cpp", src)
    (src / "CMakeLists.txt").write_text(
        "cmake_minimum_required(VERSION 3.16)\nproject(orma_delivery_node CXX)\n"
        "set(CMAKE_CXX_STANDARD 17)\nfind_package(rclcpp REQUIRED)\n"
        "find_package(sensor_msgs REQUIRED)\nfind_package(test_rmw_zenoh_cpp QUIET)\n"
        "add_executable(node delivery_node.cpp)\n"
        "target_link_libraries(node rclcpp::rclcpp ${sensor_msgs_TARGETS})\n"
        "if(test_rmw_zenoh_cpp_FOUND)\n"
        "  target_link_libraries(node ${test_rmw_zenoh_cpp_TARGETS})\n"
        "else()\n  target_compile_definitions(node PRIVATE DELIVERY_STOCK)\nendif()\n"
    )
    out = Path(tmp) / f"{name}_build"
    for step in (f"cmake -S {src} -B {out} -DCMAKE_BUILD_TYPE=Release", f"cmake --build {out}"):
        subprocess.run(
            ["bash", "-c", SOURCE, setup, *step.split()], check=True, capture_output=True
        )
    return out / "node"


def router(setup, port, log):
    env = dict(
        os.environ,
        ROS_DOMAIN_ID=DOMAIN,
        ZENOH_CONFIG_OVERRIDE=f'listen/endpoints=["tcp/localhost:{port}"]',
    )
    cmd = 'exec "$(ros2 pkg prefix rmw_zenoh_cpp)/lib/rmw_zenoh_cpp/rmw_zenohd"'
    p = subprocess.Popen(
        ["bash", "-c", f'source "$0" >/dev/null && {cmd}', setup],
        env=env,
        stdout=log,
        stderr=subprocess.STDOUT,
    )

    def listening():
        if p.poll() is not None:
            sys.exit("rmw_zenohd exited")
        with socket.socket() as s:
            return s.connect_ex(("127.0.0.1", port)) == 0

    common.wait_for(listening, f"rmw_zenohd on {port}", 20)
    return p


class Node:
    def __init__(self, setup, argv, env):
        self.p = subprocess.Popen(
            ["bash", "-c", SOURCE, setup, *map(str, argv)],
            env=env,
            stdin=subprocess.PIPE,
            stdout=subprocess.PIPE,
            text=True,
        )
        line = self.p.stdout.readline().strip()
        if line != "ready":
            raise RuntimeError(f"{argv[0]} printed {line!r}")

    def go(self):
        self.p.stdin.write("\n")
        self.p.stdin.flush()

    def counts(self):
        """What it published and received; it stays up, still counted in the windows."""
        out = {}
        while (f := self.p.stdout.readline().split()) != ["done"]:
            if not f:
                raise RuntimeError("the node exited before its counts")
            if len(f) == 3 and f[0] in ("pub", "sub"):
                out[f[0], f[1]] = int(f[2])
        return out

    def end(self):
        self.p.communicate("\n", timeout=60)


def run_case(name, case, setup, node, method, args, client):
    procs_spec, expect, _ = case
    env = dict(
        os.environ,
        ROS_DOMAIN_ID=DOMAIN,
        RMW_IMPLEMENTATION="rmw_zenoh_cpp",
        ZENOH_CONFIG_OVERRIDE=(
            f'connect/endpoints=["tcp/localhost:{args.port}"];transport/shared_memory/enabled=true'
        ),
    )
    import orma_client

    preloaded = orma_client.environment("preload" if method == "uprobe" else method, env)
    if method == "uprobe":
        # orma_rclpy still names rclpy's callbacks; ormad's uprobes need no preload.
        del preloaded["LD_PRELOAD"]
    env.update(preloaded)
    procs = {}
    for proc, extra, argv, python in procs_spec:
        exe = [sys.executable, HERE / "delivery_node.py"] if python else [node]
        argv = exe + [f"orma_delivery_{proc}", *argv, f"seconds={args.seconds}"]
        procs[proc] = Node(setup, argv, dict(env, **extra))
    pids = {proc: n.p.pid for proc, n in procs.items()}
    client.request(
        "select",
        all=False,
        pids=list(pids.values()),
        method_by_pid={str(p): method for p in pids.values()},
    )
    time.sleep(1.5)
    for n in procs.values():
        n.go()
    counts = {proc: n.counts() for proc, n in procs.items()}

    # A take pairs with its publish up to two windows later. LTTng live hands its events over
    # seconds late, so the windows run until every take is in or a cap.
    def taken(proc):
        return sum(p["takes"] for s in subs_of(windows, pids[proc]) for p in s.get("paths", []))

    def received(proc):
        return sum(n for (kind, _), n in counts[proc].items() if kind == "sub")

    windows = []
    while len(windows) < args.seconds + 5 or (
        len(windows) < args.seconds + 30 and any(taken(p) < received(p) for p in expect)
    ):
        m = client.next_message()
        if m["type"] == "window":
            windows.append(m)
    for n in procs.values():
        n.end()
    return check(name, expect, procs_spec, pids, counts, windows)


def subs_of(windows, pid):
    """The process's subscriptions of the test topics, not of /parameter_events."""
    return [
        s
        for w in windows
        for s in w["subscriptions"]
        if s["node"].startswith(f"n:{pid}:") and s["topic"].startswith(f"topic:ros:{T}")
    ]


def check(name, expect, procs_spec, pids, counts, windows):
    f = common.accuracy.Fail(name)
    python = {proc for proc, _, _, py in procs_spec if py}
    out = []
    for proc, path in expect.items():
        rows = subs_of(windows, pids[proc])
        got = sum(n for (kind, _), n in counts[proc].items() if kind == "sub")
        takes = {}
        for s in rows:
            for p in s.get("paths", []):
                takes[p["path"]] = takes.get(p["path"], 0) + p["takes"]
        # rmw_zenoh makes its shared memory provider on the first publish that wants it and
        # serializes into its own buffer until the provider is there.
        early = takes.pop("net", 0) if path == "shm" else 0
        f.check(early <= 3, f"{proc}: {early} takes serialized before shared memory")
        f.eq(takes, {path: got - early}, f"{proc}: takes by path, {got} received")
        f.eq(sum(s["lost"] or 0 for s in rows), 0, f"{proc}: lost")
        # The paths split the hop orma already reports, one sample per paired take.
        hops = sum((s.get("hop") or {}).get("n", 0) for s in rows)
        path_hops = sum((p.get("hop") or {}).get("n", 0) for s in rows for p in s.get("paths", []))
        f.eq(path_hops, hops, f"{proc}: hop samples by path against the subscription's hop")
        f.check(any(s.get("deliver") for s in rows), f"{proc}: no take to callback")
        if path in ("broker", "loan"):
            f.check(any(s.get("hold") for s in rows), f"{proc}: no hold")
        if path == "loan":
            f.eq(sum(s.get("loaned") or 0 for s in rows), got, f"{proc}: loaned takes")
        if proc in python:
            f.check(any(s.get("py_convert") for s in rows), f"{proc}: no rclpy conversion")
        out.append(f"{proc} {path} {got}")
    topics = [t for w in windows for t in w["topics"] if t["name"].startswith(T)]
    f.eq(
        sum((p.get("hop") or {}).get("n", 0) for t in topics for p in t.get("paths", [])),
        sum((t.get("hop") or {}).get("n", 0) for t in topics),
        "topic hop samples by path against the topic's hop",
    )
    if any(path == "loan" for path in expect.values()):
        f.check(any(t.get("loan_hold") for t in topics), "no publisher loan hold")
    if python and any("pub=" in a for proc, _, argv, py in procs_spec if py for a in argv):
        f.check(any(t.get("py_convert") for t in topics), "no rclpy publish conversion")
    print(f"{'FAIL' if f.msgs else 'ok'}  {name}: {', '.join(out)}")
    for m in f.msgs:
        print(f"      {m}")
    return [f"{name}: {m}" for m in f.msgs]


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--overlay", required=True, help="the overhaul's setup script")
    ap.add_argument("--method", choices=("preload", "lttng", "uprobe"), default="preload")
    ap.add_argument("--stock", action="store_true", help="also the zenoh cases on stock ROS")
    ap.add_argument("--only", action="append", choices=CASES)
    ap.add_argument("--port", type=int, default=7487)
    ap.add_argument("--seconds", type=float, default=3.0)
    args = ap.parse_args()
    stock = str(Path("/opt/ros") / os.environ.get("ROS_DISTRO", "jazzy") / "setup.bash")
    tmp = Path(tempfile.mkdtemp(prefix="orma_delivery_"))
    runs = [("overlay", args.overlay, build(tmp, args.overlay, "overlay"))]
    if args.stock:
        runs.append(("stock", stock, build(tmp, stock, "stock")))
    collectd = common.Collectd(tmp)
    failed = []
    try:
        import orma_client

        with common.ormad(tmp) if args.method == "uprobe" else contextlib.nullcontext():
            with orma_client.connect(start=False) as client:
                client.request("subscribe", streams=["window"])
                client.request("configure", window_s=1.0)
                for where, setup, node in runs:
                    with open(tmp / f"router_{where}.log", "w") as log:
                        r = router(setup, args.port, log)
                        try:
                            for name, case in CASES.items():
                                if (args.only and name not in args.only) or (
                                    where == "stock" and not case[2]
                                ):
                                    continue
                                failed += run_case(
                                    f"{where} {name}",
                                    case,
                                    setup,
                                    node,
                                    args.method,
                                    args,
                                    client,
                                )
                        finally:
                            r.terminate()
                            r.wait(timeout=20)
    finally:
        collectd.stop()
    print(f"failed: {len(failed)}")
    return 1 if failed else 0


if __name__ == "__main__":
    sys.exit(main())
