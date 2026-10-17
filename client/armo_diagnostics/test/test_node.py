import json
import os
import shutil
import socket
import struct
import subprocess
import sys
import tempfile
import threading
import time
from pathlib import Path

import armo_client
import pytest
import rclpy
from conftest import PACKAGE, window
from diagnostic_msgs.msg import DiagnosticArray, DiagnosticStatus


def serve(listener, requests):
    """armo-collectd as the node uses it: hello, then a window every 0.1 s once subscribed."""
    sock, _ = listener.accept()
    while True:
        length, _ = struct.unpack("<IB", sock.recv(5, socket.MSG_WAITALL))
        request = json.loads(sock.recv(length - 1, socket.MSG_WAITALL))
        requests.append(request)
        if request["op"] == "hello":
            sock.sendall(armo_client.encode({"type": "hello", "proto": [1, 8], "server": "py"}))
            sock.sendall(armo_client.encode({"type": "state"}))
        elif request["op"] == "subscribe":
            sock.sendall(armo_client.encode({"type": "reply", "id": request["id"], "ok": True}))
            break
    while True:
        try:
            sock.sendall(armo_client.encode(window()))
        except OSError:
            return
        time.sleep(0.1)


@pytest.fixture
def runtime(monkeypatch):
    path = Path(tempfile.mkdtemp(prefix="armo-"))
    (path / "armo").mkdir()
    monkeypatch.setenv("XDG_RUNTIME_DIR", str(path))
    monkeypatch.setenv("ROS_DOMAIN_ID", str(100 + os.getpid() % 100))
    yield path
    shutil.rmtree(path)


def test_node_publishes_diagnostics(runtime):
    listener = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
    listener.bind(str(runtime / "armo" / "collectd.sock"))
    listener.listen()
    requests = []
    threading.Thread(target=serve, args=(listener, requests), daemon=True).start()
    rules = runtime / "rules.yaml"
    rules.write_text("chain_e2e_p99_us: [{match: perception, warn: 50000, error: 100000}]\n")
    env = dict(os.environ, PYTHONPATH=f"{PACKAGE}:{os.environ.get('PYTHONPATH', '')}")
    main = "import sys; from armo_diagnostics.node import main; sys.exit(main())"
    node_process = subprocess.Popen(
        [sys.executable, "-c", main, "--ros-args", "-p", f"rules:={rules}"], env=env
    )
    rclpy.init()
    try:
        node = rclpy.create_node("diagnostics_listener")
        got = []
        node.create_subscription(DiagnosticArray, "/diagnostics", got.append, 10)
        deadline = time.monotonic() + 20
        while not got:
            assert time.monotonic() < deadline, "no /diagnostics"
            rclpy.spin_once(node, timeout_sec=0.1)
        (status,) = got[0].status
        assert status.level == DiagnosticStatus.WARN
        assert status.name == "armo chain_e2e_p99_us: perception"
        assert status.message == "60000.0 us"
        assert {kv.key: kv.value for kv in status.values}["warn"] == "50000"
        assert [r["op"] for r in requests] == ["hello", "subscribe"]
        assert requests[0]["client"] == "armo_diagnostics"
        assert requests[1]["streams"] == ["window"]
    finally:
        rclpy.shutdown()
        node_process.terminate()
        assert node_process.wait(10) in (0, -15)
