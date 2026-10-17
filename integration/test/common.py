"""What the armo_integration tests share. armod and armo-collectd come from the other packages'
tests: accuracy.Armod (daemon/armo_daemon/test) and harness.Collectd (collect/armo_lttng/test), imported from
the source tree rather than copied."""

import os
import re
import socket
import subprocess
import sys
import time
from contextlib import contextmanager
from pathlib import Path

from ament_index_python.packages import get_package_prefix

ARMO = Path(__file__).resolve().parents[2]
sys.path[:0] = [str(ARMO / "daemon/armo_daemon/test"), str(ARMO / "collect/armo_lttng/test")]
import accuracy  # noqa: E402
import harness  # noqa: E402

Collectd = harness.Collectd

SOCKET = "/run/armo/armod.sock"
DOMAIN = "88"
UID = f"{os.getuid()}:{os.getgid()}"
HOT = (
    "ros_trace_rcl_node_init", "ros_trace_rcl_publisher_init", "ros_trace_callback_start",
    "ros_trace_callback_end", "ros_trace_rcl_publish", "ros_trace_rmw_publish",
    "ros_trace_rmw_take",
)  # fmt: skip
FLUX = ("flux_trace_claim", "flux_trace_take", "flux_trace_callback_start")
# Each run's load stays at or below this share of a core (the machine's power budget).
CAP_PCT = 70.0


@contextmanager
def armod(tmp):
    """armod on the socket armo-collectd uses. One another test already started serves as is: armod
    refuses a second instance there."""
    s = socket.socket(socket.AF_UNIX)
    try:
        s.connect(SOCKET)
        s.close()
        print(f"armod: using the one already serving {SOCKET}", flush=True)
        yield
        return
    except OSError:
        s.close()
    binary = os.path.join(get_package_prefix("armo_daemon"), "bin/armod")
    a = accuracy.Armod(binary, SOCKET, str(tmp / "armod.log"))
    try:
        yield
    finally:
        a.stop()
        subprocess.run(accuracy.SUDO + ["rm", "-f", SOCKET], check=False)


def windows(client, n, over_cap=None):
    """The next `n` window messages. `over_cap(window)` names a load over CAP_PCT, which ends the
    test."""
    out = []
    while len(out) < n:
        m = client.next_message()
        if m["type"] != "window":
            continue
        why = over_cap and over_cap(m)
        if why:
            sys.exit(f"stopped: {why} is over the {CAP_PCT:.0f} % cap")
        out.append(m)
    return out


def wait_for(check, what, seconds=60):
    deadline = time.monotonic() + seconds
    while not check():
        if time.monotonic() > deadline:
            sys.exit(f"timed out waiting for {what}")
        time.sleep(0.2)


def uprobes_on(pid, *symbols, lib="libtracetools.so"):
    """Whether armod's breakpoint is on each symbol of the library `lib` (a file name, or its
    start) in `pid`; false also while the process has not loaded it yet."""
    found = [f for f in (line.split() for line in open(f"/proc/{pid}/maps")) if len(f) > 5]
    mapped = next((f[5] for f in found if f[5].rsplit("/", 1)[-1].startswith(lib)), None)
    if mapped is None:
        return False
    file = f"/proc/{pid}/root{mapped}"
    return all(
        accuracy.brk(accuracy.instruction(pid, mapped, accuracy.symbol_offset(file, s)))
        for s in symbols
    )


class Host:
    """Processes on this machine with the sourced workspace."""

    def __init__(self, env, cwd=None):
        self.env = dict(env, ROS_DOMAIN_ID=DOMAIN, RMW_IMPLEMENTATION="rmw_cyclonedds_cpp")
        self.cwd = cwd
        self.procs = []

    def start(self, argv, cpu, extra_env=None):
        """Starts `argv` on `cpu`; returns its pid."""
        p = subprocess.Popen(
            ["taskset", "-c", str(cpu), *argv],
            env=dict(self.env, **(extra_env or {})),
            cwd=self.cwd,
            stdout=subprocess.DEVNULL,
            stderr=subprocess.DEVNULL,
            start_new_session=True,
        )
        self.procs.append(p)
        return p.pid

    def run(self, argv):
        subprocess.run(argv, env=self.env, check=True, capture_output=True)

    def lib(self, package, name):
        return os.path.join(get_package_prefix(package), "lib", name)

    def stop(self):
        for p in self.procs:
            os.killpg(p.pid, 2)
            p.wait(timeout=30)


class Docker:
    """Processes in one container of IMAGE with the Humble workspace WS at /w and this tree at
    /w/src/armo, sharing /dev/shm with the host (--ipc=host) under this user's ids; CPU-capped
    to CAP_PCT of a core per container CPU."""

    def __init__(self, image, ws, cpus, mounts=()):
        self.name = f"armo_int_{os.getpid()}"
        n = len(cpus.split(","))
        vols = ["-v", f"{ws}:/w", "-v", f"{ARMO}:/w/src/armo:ro"]
        for m in mounts:
            vols += ["-v", f"{m}:{m}"]
        subprocess.run(
            ["sudo", "-n", "docker", "run", "-d", "--rm", "--name", self.name, "--ipc=host",
             "--user", UID, "-e", "HOME=/w/home", f"--cpuset-cpus={cpus}",
             f"--cpus={n * CAP_PCT / 100:.2f}", *vols, image, "sleep", "infinity"],
            check=True, capture_output=True,
        )  # fmt: skip
        self.setup = (
            "source /opt/ros/humble/setup.bash && source /w/install/setup.bash && "
            f"export ROS_DOMAIN_ID={DOMAIN} && "
        )

    def shell(self, script, detach=False):
        return subprocess.run(
            ["sudo", "-n", "docker", "exec", *(["-d"] if detach else []), self.name, "bash",
             "-c", self.setup + script],
            check=True, capture_output=True, text=True,
        ).stdout  # fmt: skip

    def start(self, argv, cpu, extra_env=None):
        """Starts `argv` in the container; returns its host pid."""
        tag = f"ARMO_INT_TAG={time.monotonic_ns()}"
        env = " ".join(f"{k}={v}" for k, v in (extra_env or {}).items())
        self.shell(f"exec env {tag} {env} {' '.join(argv)}", detach=True)
        host = []
        wait_for(lambda: host.extend(tagged(tag)) or host, f"{argv[0]} in the container")
        return host[0]

    def run(self, argv):
        self.shell(" ".join(argv))

    def build_node(self):
        """armo_daemon's node.cpp built in the image; returns its path there."""
        self.shell(
            "mkdir -p /w/build/armo_integration_node && cd /w/build/armo_integration_node"
            " && cp /w/src/armo/daemon/armo_daemon/test/node.cpp . && printf '%s\\n' "
            "'cmake_minimum_required(VERSION 3.16)' 'project(n CXX)'"
            " 'find_package(rclcpp REQUIRED)' 'find_package(std_msgs REQUIRED)'"
            " 'find_package(std_srvs REQUIRED)' 'add_executable(node node.cpp)'"
            " 'target_link_libraries(node rclcpp::rclcpp ${std_msgs_TARGETS} ${std_srvs_TARGETS})'"
            " > CMakeLists.txt && cmake -S . -B o -DCMAKE_BUILD_TYPE=Release >/dev/null"
            " && cmake --build o -j2 >/dev/null"
        )
        return "/w/build/armo_integration_node/o/node"

    def host_pid(self, ns_pid):
        """The host pid of the container's process `ns_pid`."""
        init = subprocess.run(
            ["sudo", "-n", "docker", "inspect", "-f", "{{.State.Pid}}", self.name],
            check=True, capture_output=True, text=True,
        ).stdout.strip()  # fmt: skip
        ns = os.readlink(f"/proc/{init}/ns/pid")
        for p in os.listdir("/proc"):
            try:
                if p.isdigit() and os.readlink(f"/proc/{p}/ns/pid") == ns:
                    nspid = re.search(r"^NSpid:\s+(.*)$", open(f"/proc/{p}/status").read(), re.M)
                    if int(nspid[1].split()[-1]) == ns_pid:
                        return int(p)
            except OSError:
                continue
        raise RuntimeError(f"no host pid for {ns_pid} in {self.name}")

    def lib(self, package, name):
        return f"/w/install/{package}/lib/{name}"

    def stop(self):
        subprocess.run(["sudo", "-n", "docker", "kill", self.name], capture_output=True)


class Waiting:
    """A test process that prints its pid and waits for "go" (accuracy.Proc), here or in a
    container."""

    def __init__(self, argv, env=None, docker=None):
        if docker:
            run = " ".join(f"{k}={v}" for k, v in (env or {}).items()) + " exec " + " ".join(argv)
            argv = ["sudo", "-n", "docker", "exec", "-i", docker.name, "bash", "-c"]
            argv.append(docker.setup + run)
            env = None
        self.p = accuracy.Proc(argv, env=env)
        self.ns_pid = self.p.pid
        self.pid = docker.host_pid(self.ns_pid) if docker else self.p.pid
        self.started = False

    def go(self):
        self.p.go()
        self.started = True

    def line(self):
        return self.p.line()

    def end(self):
        self.p.end()

    def kill(self):
        self.p.p.kill()
        self.p.p.wait()


class Caller:
    """armo_daemon's flux and flux callers: print their keys, run on "go", print "done"."""

    def __init__(self, argv, env):
        self.p = subprocess.Popen(
            argv, env=env, stdin=subprocess.PIPE, stdout=subprocess.PIPE, text=True
        )
        self.p.stdout.readline()
        self.pid = self.ns_pid = self.p.pid
        self.started = False

    def go(self):
        self.p.stdin.write("go\n")
        self.p.stdin.flush()
        self.started = True

    def kill(self):
        self.p.kill()
        self.p.wait()

    def end(self):
        self.p.stdout.readline()
        self.p.stdin.close()
        self.p.wait(timeout=30)


def tagged(tag):
    """The process whose environment holds `tag` and whose parent's does not."""
    pids = set()
    for p in os.listdir("/proc"):
        try:
            if p.isdigit() and tag.encode() in open(f"/proc/{p}/environ", "rb").read().split(b"\0"):
                pids.add(int(p))
        except OSError:
            continue
    ppid = {p: int(open(f"/proc/{p}/stat").read().rsplit(")", 1)[1].split()[1]) for p in pids}
    return [p for p in pids if ppid[p] not in pids]


def build_humble_ws(image, ws):
    """armo_preload, armo_lttng, armo_rclpy and armo_integration built in IMAGE into WS, once.
    Three CPUs at CAP_PCT."""
    if (ws / "install/armo_integration").is_dir():
        return
    (ws / "home").mkdir(parents=True, exist_ok=True)
    (ws / "src").mkdir(exist_ok=True)
    subprocess.run(
        ["sudo", "-n", "docker", "run", "--rm", "--user", UID, "-e", "HOME=/w/home",
         "-e", "MAKEFLAGS=-j3", "--cpuset-cpus=9-11", f"--cpus={3 * CAP_PCT / 100:.2f}",
         "-v", f"{ws}:/w", "-v", f"{ARMO}:/w/src/armo:ro", "-w", "/w", image, "bash", "-c",
         "source /opt/ros/humble/setup.bash && colcon build --symlink-install "
         "--parallel-workers 1 --cmake-args -DBUILD_TESTING=OFF --packages-select "
         "armo_preload armo_lttng armo_rclpy armo_integration"],
        check=True,
    )  # fmt: skip
