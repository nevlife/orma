"""Workloads: which processes start, on which core, with which arguments.

Every process is pinned to one core with taskset and, under the fifo condition, started with
chrt -f 50 so all its threads (executor and DDS) are SCHED_FIFO 50. Bench cores are 3-11.
"""

import os
import subprocess
import sys

from ament_index_python.packages import get_package_prefix

LIB = os.path.join(get_package_prefix("armo_integration"), "lib/armo_integration")
PY = os.path.join(LIB, "tc_nodes.py")
CONTAINER = os.path.join(
    get_package_prefix("rclcpp_components"), "lib/rclcpp_components/component_container"
)
TOPICS = 4


class Proc:
    def __init__(self, role, core, cmd):
        self.role, self.core, self.cmd = role, core, cmd


class Workload:
    python = False

    def __init__(self, rate, topics=TOPICS):
        self.rate = rate
        self.topics = topics

    def procs(self):
        raise NotImplementedError

    def after_start(self, ctx, procs):
        pass


def ros(*params):
    out = ["--ros-args"]
    for p in params:
        out += ["-p", p]
    return out


class CppChain(Workload):
    name = "cpp_chain"

    def procs(self):
        r, t = f"rate_hz:={self.rate}.0", f"topics:={self.topics}"
        return [
            Proc("sink", 6, [f"{LIB}/tc_sink", *ros(t, "in:=/tc/b", "tag:=sink")]),
            Proc("relay", 5, [f"{LIB}/tc_relay", *ros(t, "in:=/tc/a", "out:=/tc/b")]),
            Proc("pub", 4, [f"{LIB}/tc_pub", *ros(t, "out:=/tc/a", r)]),
        ]


class PyChain(Workload):
    name = "py_chain"
    python = True

    def procs(self):
        r, t = str(self.rate), str(self.topics)
        py = [sys.executable, PY]
        return [
            Proc("sink", 6, [*py, "sink", "--topics", t, "--in", "/tc/b", "--tag", "sink"]),
            Proc("relay", 5, [*py, "relay", "--topics", t, "--in", "/tc/a", "--out", "/tc/b"]),
            Proc("pub", 4, [*py, "pub", "--topics", t, "--out", "/tc/a", "--rate-hz", r]),
        ]


class Container(Workload):
    """component_container (SingleThreadedExecutor) with pub -> relay -> relay -> sink."""

    name = "container"

    def procs(self):
        return [Proc("container", 4, [CONTAINER, "--ros-args", "-r", "__node:=tc_container"])]

    def after_start(self, ctx, procs):
        t = f"topics:={self.topics}"
        loads = [
            ("Sink", "tc_sink", [t, "in:=/tc/c", "tag:=sink"]),
            ("Relay", "tc_relay2", [t, "in:=/tc/b", "out:=/tc/c"]),
            ("Relay", "tc_relay1", [t, "in:=/tc/a", "out:=/tc/b"]),
            ("Pub", "tc_pub", [t, "out:=/tc/a", f"rate_hz:={self.rate}.0"]),
        ]
        for plugin, node, params in loads:
            cmd = ["ros2", "component", "load", "/tc_container", "armo_integration"]
            cmd += [f"armo_integration::{plugin}", "--node-name", node]
            for p in params:
                cmd += ["-p", p]
            subprocess.run(cmd, env=ctx.env, capture_output=True, check=True)


class Fanout(Workload):
    """One publisher, eight sink processes on the same topics: synchronized wakeups (design 8.2)."""

    name = "fanout"

    def procs(self):
        t = f"topics:={self.topics}"
        out = []
        for k in range(8):
            params = ros(t, "in:=/tc/a", f"tag:=sink{k}")
            out.append(
                Proc(f"sink{k}", 4 + k, [f"{LIB}/tc_sink", *params, "-r", f"__node:=tc_sink{k}"])
            )
        pub = ros(t, "out:=/tc/a", f"rate_hz:={self.rate}.0")
        out.append(Proc("pub", 3, [f"{LIB}/tc_pub", *pub]))
        return out


class Pair(Workload):
    """One publisher process and one sink process: one hop per message."""

    name = "pair"

    def procs(self):
        t = f"topics:={self.topics}"
        return [
            Proc("sink", 5, [f"{LIB}/tc_sink", *ros(t, "in:=/tc/a", "tag:=sink")]),
            Proc("pub", 4, [f"{LIB}/tc_pub", *ros(t, "out:=/tc/a", f"rate_hz:={self.rate}.0")]),
        ]


REGISTRY = {w.name: w for w in (CppChain, PyChain, Container, Fanout, Pair)}
