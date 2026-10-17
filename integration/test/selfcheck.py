#!/usr/bin/env python3
"""`armo check` against the real armod and armo-collectd (design 6.7, 18 WP9).

Processes that wait before they create any ROS entity are selected first, so the check attaches
only to them; `armo check` then runs and they start inside its window: two node.cpp on one
topic (one under libarmo_preload.so), node.py with armo_rclpy and, when the workspace has
flux2_ffi, armo_daemon's flux2 caller. Each row of 6.7 must come out as an item that passes;
the Cyclone m_size offset may stay unknown with its reason (it needs Cyclone's debug symbols).

    selfcheck.py [--duration S] [--humble IMAGE WS]

With --humble the nodes run in IMAGE from the Humble workspace WS (built there first if it is
not) and the container pid row (NSpid) applies as well.
"""

import argparse
import os
import shutil
import re
import subprocess
import sys
import tempfile
import time
from pathlib import Path

import armo_client
import common
from ament_index_python.packages import get_package_prefix, get_package_share_directory

A = common.accuracy
TEST = common.ARMO / "daemon/armo_daemon/test"


def host_procs(prefix, tmp):
    exe = A.build_node(tmp)
    env = A.node_env("rmw_cyclonedds_cpp")
    env["ROS_DOMAIN_ID"] = common.DOMAIN
    preload = Path(get_package_prefix("armo_preload")) / "lib/libarmo_preload.so"
    autoload = Path(get_package_share_directory("armo_rclpy")) / "autoload"
    out = {
        "cpp": common.Waiting([exe, prefix, "200"], env),
        "cpp_preload": common.Waiting([exe, prefix, "200"], dict(env, LD_PRELOAD=str(preload))),
        "py": common.Waiting(
            [sys.executable, TEST / "node.py", prefix + "_py", "200"],
            dict(env, PYTHONPATH=f"{autoload}:{env.get('PYTHONPATH', '')}"),
        ),
    }
    if A.flux2_lib():
        sys.path.insert(0, str(common.ARMO / "collect/armo_lttng/test"))
        import session

        out["flux2"] = common.Caller([session.flux2_caller(tmp)], env)
    return out


def docker_procs(prefix, docker):
    exe = docker.build_node()
    preload = {"LD_PRELOAD": "/w/install/armo_preload/lib/libarmo_preload.so"}
    autoload = {"PYTHONPATH": "/w/install/armo_rclpy/share/armo_rclpy/autoload:$PYTHONPATH"}
    node_py = ["python3", "/w/src/armo/daemon/armo_daemon/test/node.py", prefix + "_py", "200"]
    return {
        "cpp": common.Waiting([exe, prefix, "200"], docker=docker),
        "cpp_preload": common.Waiting([exe, prefix, "200"], preload, docker),
        "py": common.Waiting(node_py, autoload, docker),
    }


# Design 6.7 row -> the item patterns that answer it; {cpp} and the like are those pids.
ROWS = [
    ("probes and interception",
     [r"probes discovery", r"probes hot", r"intercept p:{cpp_preload}:"]),
    ("C++ symbol coverage", [r"rclcpp symbols pid {cpp}"]),
    ("rmw_publisher_t::topic_name offset 16", [r"rmw topic_name offset 16"]),
    ("Cyclone serdata_rmw::m_size offset", [r"cyclone serdata_rmw::m_size offset"]),
    ("rmw_publish timestamp and rmw_take source_timestamp",
     [r"rmw_publish timestamp = rmw_take source_timestamp|source_timestamp after rmw_publish"]),
    ("header.stamp", [r"header\.stamp {prefix}/out"]),
    ("rclpy handle", [r"rclpy handle"]),
    ("flux2 contract", [r"flux2 contract .*libflux2_ffi"]),
    ("tracepoint argument layout", [r"tracepoint layout"]),
    ("distro", [r"distro pid {cpp} ", r"distro pid {py} "]),
    ("container pid", [r"container pid|nspid"]),
]  # fmt: skip
MAY_BE_UNKNOWN = {"cyclone serdata_rmw::m_size offset"}


def parse(out):
    """Rows of `armo check`'s table: item -> (status, detail)."""
    lines = out.splitlines()
    head = next(i for i, line in enumerate(lines) if line.split()[:2] == ["item", "status"])
    status_at = lines[head].index("status")
    detail_at = lines[head].index("detail")
    rows = {}
    for line in lines[head + 1 :]:
        rows[line[:status_at].strip()] = (
            line[status_at:detail_at].strip(),
            line[detail_at:].strip(),
        )
    return rows


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--duration", type=float, default=20.0)
    ap.add_argument("--humble", nargs=2, metavar=("IMAGE", "WS"))
    a = ap.parse_args()
    tmp = Path(tempfile.mkdtemp(prefix="armo-sc-", dir="/tmp"))
    os.chmod(tmp, 0o755)
    prefix = f"/armo_sc_{os.getpid()}"
    docker = None
    if a.humble:
        ws = Path(a.humble[1]).resolve()
        common.build_humble_ws(a.humble[0], ws)
        docker = common.Docker(a.humble[0], ws, "8-10")
    failed = []
    with common.armod(tmp):
        collectd = common.Collectd(tmp)
        procs = {}
        try:
            procs = docker_procs(prefix, docker) if docker else host_procs(prefix, tmp)
            pids = {k: p.pid for k, p in procs.items()}
            client = armo_client.connect(start=False)
            client.request("select", all=False, pids=sorted(pids.values()))
            check = subprocess.Popen(
                ["armo", "check", "--duration", str(a.duration)],
                env=collectd.env,
                stdout=subprocess.PIPE,
                stderr=subprocess.STDOUT,
                text=True,
            )
            # armod opens the window once every set is attached, tens of seconds on this kernel,
            # one probe after the other: the nodes start once the last ones they hit are on.
            common.wait_for(
                lambda: (
                    check.poll() is not None
                    or all(common.uprobes_on(pids[k], *common.HOT) for k in ("cpp", "cpp_preload"))
                ),
                "the selfcheck's uprobes",
                seconds=300,
            )
            time.sleep(3)
            t0 = time.monotonic()
            for p in procs.values():
                p.go()
            out = check.communicate(timeout=600)[0]
            print(f"armo check ended {time.monotonic() - t0:.0f} s after the nodes started")
            print(out)
            client.close()
        finally:
            for p in procs.values():
                if p.started:
                    p.end()
                else:
                    p.kill()
            collectd.stop()
            if docker:
                docker.stop()
    rows = parse(out)
    fill = dict(pids, prefix=prefix)
    if docker:
        fill.update({f"ns_{k}": p.ns_pid for k, p in procs.items()})
    for row, patterns in ROWS:
        # No container on the host; the Humble image has no Cyclone DDS.
        if (row == "container pid" and not docker) or (row.startswith("Cyclone") and docker):
            continue
        patterns = [
            p
            for p in patterns
            if not (p.startswith("flux2") and "flux2" not in procs)
        ]
        for pat in patterns:
            rx = re.compile(pat.format(**fill))
            hits = {k: v for k, v in rows.items() if rx.search(k)}
            ok = bool(hits) and all(
                s == "pass" or (s == "unknown" and k in MAY_BE_UNKNOWN)
                for k, (s, _) in hits.items()
            )
            what = "; ".join(f"{k}: {s} ({d})" for k, (s, d) in hits.items()) or "no such item"
            print(("ok    " if ok else "FAIL  ") + f"{row}: {what}")
            if not ok:
                failed.append(f"{row} /{rx.pattern}/")
    if failed:
        sys.exit(f"FAILED: {len(failed)}: " + "; ".join(failed))
    shutil.rmtree(tmp)
    print("ok")


main()
