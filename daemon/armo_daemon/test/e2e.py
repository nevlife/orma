#!/usr/bin/env python3
"""armod with the real armo-collectd: armod serves /run/armo/armod.sock, `armo record --method
uprobe` records a running node.cpp for a few seconds, and `armo-analyze` reads the recording.
Fails unless the recording holds the node's callbacks with no loss and the analysis times them.

Needs root for armod and a sourced workspace with armo_cli and armo_collectd installed.

    daemon/armo_daemon/test/e2e.py [--armod PATH]
"""

import argparse
import collections
import os
import shutil
import subprocess
import sys
import tempfile
import time
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))
import accuracy as A  # noqa: E402

SOCKET = "/run/armo/armod.sock"


def stop_collectd(runtime):
    """SIGTERM to the armo-collectd started for `runtime`, the one whose environment names it."""
    for pid in subprocess.run(
        ["pgrep", "-x", "armo-collectd"], capture_output=True, text=True
    ).stdout.split():
        try:
            env = Path(f"/proc/{pid}/environ").read_bytes()
        except OSError:
            continue
        if f"XDG_RUNTIME_DIR={runtime}".encode() in env.split(b"\0"):
            subprocess.run(["kill", "-TERM", pid], check=False)


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--armod", default=shutil.which("armod"))
    args = ap.parse_args()
    for tool in ("armo", "armo-collectd", "armo-analyze"):
        if not shutil.which(tool):
            sys.exit(f"{tool} not on PATH: source the workspace")
    failed = []
    # The socket path has to stay under 108 bytes; collectd keeps its socket in here.
    runtime = Path(tempfile.mkdtemp(prefix="armo_e2e_", dir="/tmp"))
    env = dict(os.environ, XDG_RUNTIME_DIR=str(runtime))
    armod = A.Armod(args.armod, SOCKET, str(runtime / "armod.log"))
    node = None
    try:
        exe = A.build_node(str(runtime))
        prefix = f"/armo_e2e_{os.getpid()}"
        node = A.Proc([exe, prefix, "600"], env=A.node_env("rmw_cyclonedds_cpp"))
        node.go()
        time.sleep(1.0)
        out = runtime / "records"
        out.mkdir()
        rec = subprocess.run(
            ["armo", "record", "--method", "uprobe", "--pid", str(node.pid)]
            + ["--max-duration", "5", "-o", str(out), "--name", "e2e"],
            env=env,
            capture_output=True,
            text=True,
            timeout=120,
        )
        print(rec.stdout[-1500:])
        if rec.returncode != 0:
            failed.append(f"armo record: {rec.returncode} {rec.stderr[-500:]}")
        d = out / "e2e"
        events = sorted((d / "events").glob("*.armoev"))
        data = b"".join(p.read_bytes()[16:] for p in events)
        recs = [r for r in A.parse_batch(data) if r.pid == node.pid]
        n = collections.Counter(r.name for r in recs)
        print("records of the node:", dict(n))
        for name in ("callback_start", "callback_end", "rcl_publish", "rmw_take", "rcl_wait_exit"):
            if n[name] == 0:
                failed.append(f"no {name} in the recording")
        if abs(n["callback_start"] - n["callback_end"]) > 2:
            failed.append(f"starts {n['callback_start']} and ends {n['callback_end']} differ")
        meta = (d / "meta.json").read_text()
        print("meta:", meta[:600])
        if '"armod_ring":0' not in meta.replace(" ", ""):
            failed.append("armod lost records")
        an = subprocess.run(["armo-analyze", str(d)], capture_output=True, text=True, timeout=120)
        print(an.stdout[:3000])
        if an.returncode != 0:
            failed.append(f"armo-analyze: {an.returncode} {an.stderr[-500:]}")
        if not (d / "analysis/callbacks.csv").is_file():
            failed.append("no analysis/callbacks.csv")
        else:
            rows = (d / "analysis/callbacks.csv").read_text().splitlines()
            print(f"callbacks.csv: {len(rows) - 1} rows")
            if len(rows) - 1 < n["callback_end"] // 2:
                failed.append(f"callbacks.csv has {len(rows) - 1} rows")
    finally:
        if node:
            node.end()
        stop_collectd(runtime)
        armod.stop()
        subprocess.run(A.SUDO + ["rm", "-f", SOCKET], check=False)
    if failed:
        print("FAILED:", "; ".join(failed))
        return 1
    print("ok")
    return 0


if __name__ == "__main__":
    sys.exit(main())
