#!/usr/bin/env python3
"""The Humble container items 1 to 4 of design 18 (item 5 is node_cpu.py --humble), in one run.

One container of IMAGE in its own pid namespace, sharing /dev/shm (--ipc=host) under this user's
ids, runs from the Humble workspace WS (built there first if it is not), every process with
libarmo_preload.so and libarmo_lttng.so and a session daemon of its own recording ros2 events:

- node.cpp and node.py (armo_rclpy), which print what they did;
- component_container with demo_nodes_cpp's Talker and Listener and armo_integration's Pub and
  Sink, loaded once armod is attached;
- demo_nodes_py's talker and listener, which load libtracetools.so and wait before they start.

The host's armod and armo-collectd monitor them by uprobe and record them twice, by uprobe and
by preload. Checks:

1. uprobe through /proc/<pid>/root: node.cpp's and node.py's publishes and takes counted exactly;
   node and topic names in the windows.
2. preload: the shared memory files are read with the host's pids (NSpid), and every common event
   of the run is counted the same as by uprobe.
3. LTTng: the container's CTF holds as many ros2 events in a window as uprobe recorded there.
4. the rcl ABI: the processes publish by rcl ABI and the hop is timed; no intra-process publish
   records or rows; probes_missing names what Humble lacks.

    humble.py [--image IMAGE] [--ws WS]
"""

import argparse
import collections
import os
import re
import subprocess
import sys
import tempfile
import threading
import time
from pathlib import Path

import armo_client
import common

A = common.accuracy
HERE = Path(__file__).resolve().parent
# Events every method records alike (design 3.4); the others differ by method on purpose.
COMMON = (
    "callback_start", "callback_end", "rcl_publish", "rclcpp_publish", "rmw_publish_rcl",
    "rmw_take", "rcl_wait_enter", "rcl_wait_exit",
)  # fmt: skip
ROS2 = {
    "callback_start": "ros2:callback_start",
    "callback_end": "ros2:callback_end",
    "rcl_publish": "ros2:rcl_publish",
    "rmw_publish_rcl": "ros2:rmw_publish",
    "rmw_take": "ros2:rmw_take",
}
PY_WAIT = (
    "import ctypes, os, runpy, sys, time\n"
    "ctypes.CDLL('libtracetools.so')\n"
    "while not os.path.exists(sys.argv[1]):\n"
    "    time.sleep(0.05)\n"
    "sys.argv = sys.argv[2:3]\n"
    "runpy.run_module(sys.argv[0], run_name='__main__')\n"
)


def records(rec):
    data = b"".join(p.read_bytes()[16:] for p in sorted((Path(rec["dir"]) / "events").glob("*")))
    return A.parse_batch(data)


def exact(f, recs, pid, said, topic):
    """Publishes and takes of `topic` against what node.cpp or node.py said it did."""
    n = dict(zip(said[0::2], map(int, said[1::2])))
    mine = [r for r in recs if r.pid == pid]
    pub = {r["handle"] for r in mine if r.name == "rcl_publisher_init" and r.strings[:1] == [topic]}
    sub = {
        r["rmw_handle"]
        for r in mine
        if r.name == "rcl_subscription_init" and r.strings[:1] == [topic]
    }
    pubs = sum(r.name == "rcl_publish" and r["publisher"] in pub for r in mine)
    takes = sum(r.name == "rmw_take" and r["rmw_subscription"] in sub and r["taken"] for r in mine)
    starts = sum(r.name == "callback_start" for r in mine)
    ends = sum(r.name == "callback_end" for r in mine)
    return (
        f.eq(pubs, n["published"], f"pid {pid} rcl_publish of {topic}")
        and f.eq(takes, n["received"], f"pid {pid} rmw_take of {topic}")
        and f.eq(starts, ends, f"pid {pid} callback starts and ends")
    )


def in_window(recs, pids, window):
    return collections.Counter(
        (r.pid, r.name) for r in recs if r.pid in pids and window[0] <= r.t < window[1]
    )


def ros2_counts(ctf, ns_to_host, window):
    out = subprocess.run(
        ["babeltrace", "--clock-cycles", "--no-delta", "-n", "context", str(ctf)],
        capture_output=True,
        text=True,
        check=True,
    ).stdout
    counts = collections.Counter()
    for line in out.splitlines():
        m = re.match(r"\[(\d+)\] \S+ (ros2:\w+): .*vpid = (\d+)", line)
        if m and int(m[3]) in ns_to_host and window[0] <= int(m[1]) < window[1]:
            counts[(ns_to_host[int(m[3])], m[2])] += 1
    return counts


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--image", default="armo-humble")
    ap.add_argument(
        "--ws",
        default=str(Path(tempfile.gettempdir()) / "armo-humble-ws"),
        help="the Humble workspace, built in IMAGE if it holds no install/armo_integration",
    )
    a = ap.parse_args()
    ws = Path(a.ws).resolve()
    common.build_humble_ws(a.image, ws)
    tmp = Path(tempfile.mkdtemp(prefix="armo-humble-", dir="/tmp"))
    os.chmod(tmp, 0o755)
    tag = f"h{os.getpid()}"
    prefix = f"/armo_{tag}"
    f = A.Fail("humble")
    with common.armod(tmp):
        collectd = common.Collectd(tmp)
        try:
            run(a, ws, tag, prefix, f)
        finally:
            collectd.stop()
    for d in (tmp, ws / f"lt_{tag}", ws / f"ctf_{tag}", ws / f"go_{tag}"):
        subprocess.run(["rm", "-rf", d], check=True)
    print("ok")


def run(a, ws, tag, prefix, f):
    lt, ctf, go = f"/w/lt_{tag}", f"/w/ctf_{tag}", f"/w/go_{tag}"
    docker = common.Docker(a.image, ws, "8-10")
    procs, rs, windows = {}, {}, []
    try:
        docker.shell(
            f"export LTTNG_HOME={lt} && mkdir -p {lt} && lttng-sessiond --no-kernel --daemonize"
            f" && lttng create {tag} --output={ctf} >/dev/null && lttng enable-event -u 'ros2:*'"
            " >/dev/null && lttng add-context -u -t vpid -t vtid >/dev/null && lttng start"
            " >/dev/null"
        )
        libs = ":".join(f"/w/install/{p}/lib/lib{p}.so" for p in ("armo_preload", "armo_lttng"))
        autoload = "/w/install/armo_rclpy/share/armo_rclpy/autoload"
        env = {"LD_PRELOAD": libs, "LTTNG_HOME": lt, "PYTHONPATH": f"{autoload}:$PYTHONPATH"}
        exe = docker.build_node()
        procs["cpp"] = common.Waiting([exe, prefix, "40"], env, docker)
        procs["py"] = common.Waiting(
            ["python3", "/w/src/armo/armo_daemon/test/node.py", prefix + "_py", "40"], env, docker
        )
        pids = {k: p.pid for k, p in procs.items()}
        pids["container"] = docker.start(
            ["/opt/ros/humble/lib/rclcpp_components/component_container", "--ros-args", "-r",
             f"__node:=armo_{tag}"], 8, env,
        )  # fmt: skip
        for role, module in (("py_talker", "talker"), ("py_listener", "listener")):
            pids[role] = docker.start(
                ["python3", "-c", f'"{PY_WAIT}"', go, f"demo_nodes_py.topics.{module}"], 8, env
            )
        ns = {}
        for role, pid in pids.items():
            status = open(f"/proc/{pid}/status").read()
            ns[int(re.search(r"^NSpid:.*\s(\d+)$", status, re.M)[1])] = pid
        targets = armo_client.targets(pids=sorted(pids.values()))

        mon = armo_client.connect(start=False)
        mon.request("configure", window_s=1.0, sched=False)
        mon.request(
            "select",
            all=False,
            pids=sorted(pids.values()),
            method_by_pid={str(p): "uprobe" for p in pids.values()},
        )
        mon.request("subscribe", streams=["window"])

        def read():
            while True:
                try:
                    m = mon.next_message()
                except (OSError, armo_client.ArmoError):
                    return
                if m["type"] == "window":
                    windows.append(m)

        threading.Thread(target=read, daemon=True).start()
        client = armo_client.connect(start=False)
        for method in ("uprobe", "preload"):
            rs[method] = client.request(
                "record_start", **armo_client.record_start(method, targets, sched=False)
            )
        for role, pid in pids.items():
            common.wait_for(
                lambda: common.uprobes_on(
                    pid, "ros_trace_rcl_node_init", "ros_trace_callback_start"
                ),
                f"armod's uprobes on {role}",
                seconds=600,
            )
        for plugin, name in (
            ("demo_nodes_cpp", "demo_nodes_cpp::Talker"),
            ("demo_nodes_cpp", "demo_nodes_cpp::Listener"),
        ):
            docker.run(
                [
                    "ros2",
                    "component",
                    "load",
                    "--no-daemon",
                    "--spin-time",
                    "5",
                    f"/armo_{tag}",
                    plugin,
                    name,
                ]
            )
        load = [
            "ros2",
            "component",
            "load",
            "--no-daemon",
            "--spin-time",
            "5",
            f"/armo_{tag}",
            "armo_integration",
        ]
        docker.run(load + ["armo_integration::Pub", "-p", "rate_hz:=50.0", "-p", "topics:=1"])
        docker.run(load + ["armo_integration::Sink", "-p", "in:=/tc/a", "-p", "topics:=1"])
        docker.shell(f"touch {go}")
        for p in procs.values():
            p.go()
        said = {k: p.line().split() for k, p in procs.items()}
        t0 = time.monotonic_ns() + 3_000_000_000
        time.sleep(11)
        t1 = time.monotonic_ns()
        time.sleep(1)
        for method in ("uprobe", "preload"):
            client.request("record_stop", record_id=rs[method]["record_id"])
        client.close()
        mon.close()
    finally:
        for p in procs.values():
            p.end()
        docker.shell(f"export LTTNG_HOME={lt} && lttng stop >/dev/null && lttng destroy >/dev/null"
                     f" && kill $(cat {lt}/.lttng/lttng-sessiond.pid)")  # fmt: skip
        docker.stop()

    recs = {m: records(r) for m, r in rs.items()}
    exact(f, recs["uprobe"], pids["cpp"], said["cpp"], prefix + "/out")
    exact(f, recs["uprobe"], pids["py"], said["py"], prefix + "_py/out")
    # A process that starts after record_start is in a preload recording only once armo-collectd
    # has found and armed its new shared memory file; item 2 compares inside the window.
    for role in ("cpp", "py"):
        first = {m: min(r.t for r in recs[m] if r.pid == pids[role]) for m in recs}
        late = (first["preload"] - first["uprobe"]) / 1e9
        print(f"{role}: the preload recording starts {late:.2f} s after the uprobe one")
    last = windows[-3] if len(windows) > 3 else windows[-1]
    names = {(n["pid"], n["fqn"]) for n in last["nodes"]}
    topics = {t["name"] for t in last["topics"]}
    for pid, fqn in (
        (pids["container"], "/talker"),
        (pids["container"], "/listener"),
        (pids["container"], "/tc_pub"),
        (pids["py_talker"], "/talker"),
        (pids["py_listener"], "/listener"),
    ):
        f.check((pid, fqn) in names, f"1: node {fqn} of pid {pid} named in the window")
    for t in ("/chatter", "/tc/a0"):
        f.check(t in topics, f"1: topic {t} in the window")

    pset = set(pids.values())
    up, pre = in_window(recs["uprobe"], pset, (t0, t1)), in_window(recs["preload"], pset, (t0, t1))
    print(f"{'pid':>8} {'event':24} {'uprobe':>7} {'preload':>7}")
    for key in sorted(k for k in set(up) | set(pre) if k[1] in COMMON):
        print(f"{key[0]:8} {key[1]:24} {up[key]:7} {pre[key]:7}")
        f.eq(pre[key], up[key], f"2: {key[1]} of pid {key[0]}, preload against uprobe")
    procs_seen = {p["pid"]: p for p in last["processes"]}
    for role, pid in pids.items():
        p = procs_seen.get(pid)
        f.check(
            p is not None and p.get("ns_pid") not in (None, pid),
            f"2: {role} has ns_pid {p and p.get('ns_pid')}",
        )

    lt_counts = ros2_counts(ws / f"ctf_{tag}", ns, (t0, t1))
    print(f"{'pid':>8} {'event':24} {'uprobe':>7} {'ros2':>7}")
    for (pid, name), n in sorted(up.items()):
        if name in ROS2:
            print(f"{pid:8} {name:24} {n:7} {lt_counts[(pid, ROS2[name])]:7}")
            f.eq(lt_counts[(pid, ROS2[name])], n, f"3: {ROS2[name]} of pid {pid} against uprobe")

    for role, pid in pids.items():
        abi = (procs_seen.get(pid) or {}).get("ros") or {}
        f.eq(abi.get("publish_abi"), "rcl", f"4: publish ABI of {role}")
    hops = [t for t in last["topics"] if t["name"] in ("/chatter", "/tc/a0") and t["hop"]]
    f.check(len(hops) == 2, f"4: hop timed on /chatter and /tc/a0: {[t['name'] for t in hops]}")
    intra = sum(r.name == "rclcpp_intra_publish" for m in recs for r in recs[m] if r.pid in pset)
    f.eq(intra, 0, "4: intra-process publish records")
    rows = [c["id"] for w in windows for c in w["callbacks"] if (c.get("intra_pct") or 0) > 0]
    f.eq(rows, [], "4: callback rows with intra-process share")
    missing = {(m["pid"], m["point"]) for w in windows for m in w["agent"]["probes_missing"]}
    print("probes_missing:", sorted({pt for p, pt in missing if p in pset}))
    f.check(any(p in pset for p, _ in missing), "4: probes_missing names Humble's missing points")

    for m in f.msgs:
        print("FAIL ", m)
    if f.msgs:
        sys.exit(f"FAILED: {len(f.msgs)} (recordings {[r['dir'] for r in rs.values()]})")


main()
