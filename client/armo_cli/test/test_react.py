import csv
import json
import socket
import struct
import subprocess
import sys
import threading
import time

import armo_client
import pytest
from armo_cli.cli import merge_lat, react_selection
from conftest import armo

WINDOW_S = 0.05
OTHER_PID = 12345
SUBSCRIBER = """
import rclpy, sys
from std_msgs.msg import String
rclpy.init()
node = rclpy.create_node("react_input")
node.create_subscription(String, "/react_in", lambda m: print(m.data, flush=True), 10)
rclpy.spin(node)
"""


def lat(n, p50, p99, top):
    return {
        "n": n,
        "mean_ns": p50,
        "p50_ns": p50,
        "p90_ns": p50,
        "p99_ns": p99,
        "p999_ns": p99,
        "max_ns": top,
    }


class ReactFake:
    """armo-collectd as far as `armo react` uses it: windows every WINDOW_S carry the
    selected pids as processes and a row for each configured chain. Odd windows have the
    larger p99 and max."""

    def __init__(self, path):
        self.requests = []
        self.monitor = {
            "selection": {"all": False, "pids": [OTHER_PID]},
            "method_by_pid": {str(OTHER_PID): "uprobe"},
            "chains": [],
        }
        self.listener = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
        self.listener.bind(str(path))
        self.listener.listen()
        threading.Thread(target=self.serve, daemon=True).start()

    def send(self, sock, message):
        with self.lock:
            sock.sendall(armo_client.encode(message))

    def serve(self):
        sock, _ = self.listener.accept()
        self.lock = threading.Lock()
        while True:
            head = sock.recv(5, socket.MSG_WAITALL)
            if not head:
                return
            length, _ = struct.unpack("<IB", head)
            request = json.loads(sock.recv(length - 1, socket.MSG_WAITALL))
            self.requests.append(request)
            op, id = request["op"], request["id"]
            if op == "hello":
                self.send(sock, {"type": "hello", "proto": [1, 8], "server": "py", "version": "0"})
                self.send(sock, {"type": "state", "monitor": self.monitor})
                continue
            if op == "bye":
                return
            if op == "configure":
                self.monitor["chains"] = request["chains"]
            elif op == "select":
                self.monitor["selection"] = {"all": request["all"], "pids": request["pids"]}
            self.send(sock, {"type": "reply", "id": id, "ok": True})
            if op == "subscribe":
                threading.Thread(target=self.windows, args=(sock,), daemon=True).start()

    def windows(self, sock):
        seq = 0
        while True:
            seq += 1
            odd = seq % 2
            chains = [
                dict(
                    c,
                    count=1,
                    broken=1,
                    e2e=None,
                    steps=[lat(2, 1000, 3000 + 1000 * odd, 5000 + 1000 * odd)]
                    + [None] * (len(c["topics"]) - 2),
                )
                for c in self.monitor["chains"]
            ]
            processes = [{"pid": p} for p in self.monitor["selection"]["pids"]]
            window = {"type": "window", "seq": seq, "t_mono_ns": time.monotonic_ns()}
            window.update(processes=processes, chains=chains)
            try:
                self.send(sock, window)
            except OSError:
                return
            time.sleep(WINDOW_S)


@pytest.fixture
def react_fake(runtime):
    return ReactFake(runtime / "armo" / "collectd.sock")


@pytest.fixture
def subscriber(env):
    process = subprocess.Popen(
        [sys.executable, "-c", SUBSCRIBER], stdout=subprocess.PIPE, text=True
    )
    yield process
    process.kill()
    process.wait()


def bag(path):
    import rosbag2_py
    from rclpy.serialization import serialize_message
    from std_msgs.msg import String

    writer = rosbag2_py.SequentialWriter()
    writer.open(rosbag2_py.StorageOptions(uri=str(path)), rosbag2_py.ConverterOptions("", ""))
    for topic in ("/react_in", "/other"):
        writer.create_topic(
            rosbag2_py.TopicMetadata(
                id=0, name=topic, type="std_msgs/msg/String", serialization_format="cdr"
            )
        )
    for i in range(4):
        writer.write("/react_in", serialize_message(String(data=f"bag {i}")), i)
        writer.write("/other", serialize_message(String(data="other")), i)
    del writer


@pytest.mark.parametrize("source", ["msg", "bag"])
def test_react(react_fake, subscriber, runtime, source):
    given = ["--msg", "{data: hi}"] if source == "msg" else ["--bag", str(runtime / "bag")]
    if source == "bag":
        bag(runtime / "bag")
    out_csv = runtime / "react.csv"
    argv = ["react", "--input", "/react_in", "--type", "std_msgs/msg/String"] + given
    argv += ["--output", "/a", "--output", "/b", "--count", "3", "--period", "0.05"]
    result = subprocess.run(armo(*argv, "-o", str(out_csv)), capture_output=True, text=True)
    assert result.returncode == 0, result.stderr
    lines = result.stdout.splitlines()

    want = ["hi"] * 3 if source == "msg" else ["bag 0", "bag 1", "bag 2"]
    received = [subscriber.stdout.readline().strip() for _ in want]
    assert received == want

    ops = [{k: v for k, v in r.items() if k != "id"} for r in react_fake.requests]
    name = ops[2]["chains"][0]["name"]
    publisher = ops[3]["pids"][1]
    assert ops[1:] == [
        {"op": "subscribe", "streams": ["state", "window"]},
        {"op": "configure", "chains": [{"name": name, "topics": ["/react_in", "/a", "/b"], "mode": "first"}]},
        {"op": "select", "all": False, "pids": [OTHER_PID, publisher], "method_by_pid": {str(OTHER_PID): "uprobe", str(publisher): "preload"}},
        {"op": "configure", "chains": []},
        {"op": "select", "all": False, "pids": [OTHER_PID], "method_by_pid": {str(OTHER_PID): "uprobe"}},
        {"op": "bye"},
    ]  # fmt: skip

    assert lines[0] == f"{name} (first): /react_in -> /a -> /b, published 3"
    step1, step2 = lines[3].split(), lines[4].split()
    windows = int(step1[2]) // 2
    assert windows >= 2
    assert step1 == ["1", "/a", str(2 * windows), "1.0us", "4.0us", "6.0us"]
    assert step2 == ["2", "/b", "0", "-", "-", "-"]
    assert lines[5] == f"count {windows}, broken {windows}"
    assert lines[6].startswith("note: n and max are exact")
    rows = list(csv.reader(out_csv.open()))
    assert rows == [
        ["step", "topic", "n", "p50_ns", "p99_ns", "max_ns", "count", "broken"],
        ["1", "/a", str(2 * windows), "1000", "4000", "6000", str(windows), str(windows)],
        ["2", "/b", "0", "", "", "", str(windows), str(windows)],
    ]


def test_react_without_subscriber(react_fake, env):
    argv = ["react", "--input", "/nobody", "--type", "std_msgs/msg/String", "--msg", "{}"]
    result = subprocess.run(armo(*argv, "--output", "/a"), capture_output=True, text=True)
    assert result.returncode == 1
    assert "no subscriber on /nobody" in result.stderr
    assert [r["op"] for r in react_fake.requests][-3:] == ["configure", "select", "bye"]


def test_merge_lat():
    assert merge_lat([None, lat(0, 1, 1, 1)]) is None
    merged = merge_lat([lat(1, 100, 100, 100), lat(3, 200, 300, 900), None])
    assert merged == {"n": 4, "p50_ns": 175, "p99_ns": 300, "max_ns": 900}


@pytest.mark.parametrize(
    "selection,want",
    [
        ({"all": True, "pids": []}, None),
        ({"all": False, "pids": []}, {"all": True, "pids": [], "method_by_pid": {}}),
        (
            {"all": False, "pids": [], "node": "/x"},
            {"all": False, "pids": [7], "node": "/x", "method_by_pid": {"7": "preload"}},
        ),
    ],
)
def test_react_selection(selection, want):
    assert react_selection(selection, {}, 7) == want
