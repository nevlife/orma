"""Client of armo-collectd: frames, the connection, and starting the daemon."""

import collections
import itertools
import json
import os
import shutil
import socket
import struct
import subprocess
import time

from ament_index_python.packages import get_package_prefix, get_package_share_directory

PROTO = [1, 3]
MAX_LENGTH = 16 << 20
KIND_JSON = 1
UNIT = "armo-collectd.service"
START_TIMEOUT_S = 5.0
LIBRARIES = {
    "preload": ("armo_preload", "libarmo_preload.so"),
    "lttng": ("armo_lttng", "libarmo_lttng.so"),
}
RECORD_OPTIONS = (
    "kind",
    "dir",
    "name",
    "max_duration_s",
    "max_bytes",
    "split_bytes",
    "split_duration_s",
    "cache_bytes",
    "compression",
    "bandwidth",
    "sched",
    "malloc",
)


class ArmoError(Exception):
    pass


def encode(message):
    payload = json.dumps(message, separators=(",", ":"), ensure_ascii=False).encode()
    return struct.pack("<IB", len(payload) + 1, KIND_JSON) + payload


def runtime_dir():
    base = os.environ.get("XDG_RUNTIME_DIR")
    if not base:
        raise ArmoError("XDG_RUNTIME_DIR is not set")
    return os.path.join(base, "armo")


def socket_path():
    return os.path.join(runtime_dir(), "collectd.sock")


def targets(all=False, pids=(), node=None, topic=None, exclude=None):
    if node is not None or topic is not None or exclude is not None:
        raise ArmoError("node, topic and exclude are not implemented yet: choose processes by pid")
    return {"all": all, "pids": list(pids)}


def record_start(method=None, targets=None, **options):
    """Fields of a record_start request. None leaves a field to its default."""
    unknown = set(options) - set(RECORD_OPTIONS)
    if unknown:
        raise TypeError(f"unknown record options: {', '.join(sorted(unknown))}")
    if options.get("dir") is not None:
        options["dir"] = os.path.abspath(options["dir"])
    fields = {"method": method, "targets": targets}
    fields.update((k, options.get(k)) for k in RECORD_OPTIONS)
    return {k: v for k, v in fields.items() if v is not None}


def environment(method, env):
    """The variables `armo run` and ArmoTrace add to `env` for `method`."""
    if method == "uprobe":
        return {}
    package, library = LIBRARIES[method]
    autoload = os.path.join(get_package_share_directory("armo_rclpy"), "autoload")
    out = {
        "LD_PRELOAD": os.path.join(get_package_prefix(package), "lib", library),
        "PYTHONPATH": autoload,
    }
    for name, sep in (("LD_PRELOAD", ":"), ("PYTHONPATH", os.pathsep)):
        if env.get(name):
            out[name] += sep + env[name]
    if method == "lttng":
        out["LTTNG_HOME"] = os.path.join(runtime_dir(), "lttng")
    return out


def _unit_loaded():
    if shutil.which("systemctl") is None:
        return False
    show = ["systemctl", "--user", "show", "-p", "LoadState", "--value", UNIT]
    result = subprocess.run(show, capture_output=True, text=True)
    return result.returncode == 0 and result.stdout.strip() == "loaded"


def start_collectd():
    if _unit_loaded():
        subprocess.run(["systemctl", "--user", "start", UNIT], check=True)
        return
    exe = shutil.which("armo-collectd")
    if exe is None:
        raise ArmoError("armo-collectd is not on PATH; source the workspace")
    # setsid -f forks, so the daemon is neither our child nor in our session.
    subprocess.run(
        ["setsid", "-f", exe],
        stdin=subprocess.DEVNULL,
        stdout=subprocess.DEVNULL,
        stderr=subprocess.DEVNULL,
        cwd="/",
        check=True,
    )


def _open(path):
    sock = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
    try:
        sock.connect(path)
    except OSError:
        sock.close()
        raise
    return sock


def connect(client="armo", start=True):
    """A Client after hello. Starts armo-collectd if it is not running and `start` is set."""
    path = socket_path()
    try:
        return Client(_open(path), client)
    except (FileNotFoundError, ConnectionRefusedError):
        if not start:
            raise ArmoError(f"armo-collectd is not running ({path})") from None
    start_collectd()
    deadline = time.monotonic() + START_TIMEOUT_S
    while True:
        try:
            return Client(_open(path), client)
        except (FileNotFoundError, ConnectionRefusedError):
            if time.monotonic() > deadline:
                raise ArmoError(f"armo-collectd did not open {path}") from None
            time.sleep(0.05)


class Client:
    def __init__(self, sock, name):
        self.sock = sock
        self.ids = itertools.count(1)
        self.backlog = collections.deque()
        self.state = None
        hello = self.send("hello", proto=PROTO, client=name, pid=os.getpid())
        self.server = self._answer(hello, "hello")
        while self.state is None:
            self.backlog.append(self.receive())

    def __enter__(self):
        return self

    def __exit__(self, *exc):
        self.close()

    def close(self):
        self.send("bye")
        self.sock.close()

    def send(self, op, **fields):
        id = next(self.ids)
        self.sock.sendall(encode({"id": id, "op": op, **fields}))
        return id

    def request(self, op, **fields):
        return self._answer(self.send(op, **fields), "reply")

    def _read(self, n):
        data = b""
        while len(data) < n:
            chunk = self.sock.recv(n - len(data))
            if not chunk:
                raise ArmoError("armo-collectd closed the connection")
            data += chunk
        return data

    def receive(self):
        length, kind = struct.unpack("<IB", self._read(5))
        if not 1 <= length <= MAX_LENGTH or kind != KIND_JSON:
            raise ArmoError(f"bad frame: length {length} kind {kind}")
        message = json.loads(self._read(length - 1))
        if message["type"] == "state":
            self.state = message
        return message

    def next_message(self):
        return self.backlog.popleft() if self.backlog else self.receive()

    def _answer(self, id, kind):
        while True:
            message = self.receive()
            if message["type"] == "error" and message.get("id") in (None, id):
                raise ArmoError(f"{message['code']}: {message['message']}")
            if message["type"] == kind and message.get("id", id) == id:
                return message
            self.backlog.append(message)
