#!/usr/bin/env python3
"""Off-CPU in a callback under LTTng with armod: a node whose 20 ms timer spins 2 ms of its CPU and
sleeps 2 ms. armod reads the callback libarmo_lttng.so keeps in its backend 2 file, so the
callback's CPU leaves the sleep out (about 2 ms) while its wall-clock time keeps it (about 4 ms).
The node's CPU adds its executor thread's time outside callbacks.

Needs passwordless sudo for armod on /run/armo/armod.sock (one already serving there is used), the
installed armo-collectd, and armo_integration's burner.

    offcpu.py
"""

import os
import socket
import subprocess
import sys
import tempfile
import time
from pathlib import Path

import armo_client
import harness
import session
from ament_index_python.packages import get_package_prefix

sys.path.insert(0, str(Path(__file__).resolve().parents[3] / "daemon/armo_daemon/test"))
import accuracy  # noqa: E402

SOCKET = "/run/armo/armod.sock"
BUSY_MS, SLEEP_MS, PERIOD_MS = 2.0, 2.0, 20
CPU = 9
WARM, MEASURE = 2, 10
TOL = 0.10


def serving():
    s = socket.socket(socket.AF_UNIX)
    try:
        s.connect(SOCKET)
        return True
    except OSError:
        return False
    finally:
        s.close()


def windows(client, n):
    out = []
    while len(out) < n:
        m = client.next_message()
        if m["type"] == "window":
            out.append(m)
    return out


def main():
    tmp = Path(tempfile.mkdtemp(prefix="armo-offcpu-", dir="/tmp"))
    armod = None
    if not serving():
        binary = os.path.join(get_package_prefix("armo_daemon"), "bin/armod")
        armod = accuracy.Armod(binary, SOCKET, str(tmp / "armod.log"))
    collectd = harness.Collectd(tmp)
    burner = os.path.join(get_package_prefix("armo_integration"), "lib/armo_integration/burner")
    lib = os.path.join(get_package_prefix("armo_lttng"), "lib/libarmo_lttng.so")
    params = {"busy_ms": BUSY_MS, "sleep_ms": SLEEP_MS, "period_ms": PERIOD_MS}
    argv = [burner, "--ros-args", "-r", "__node:=burner"]
    for k, v in params.items():
        argv += ["-p", f"{k}:={v}"]
    env = dict(collectd.env, LD_PRELOAD=lib, LTTNG_HOME=str(collectd.lttng_home))
    client = armo_client.connect(start=False)
    # The session must be up before the node starts, or its init events, which name it, are lost.
    client.request("select", all=False, pids=[])
    rec = client.request("record_start", method="lttng", kind="windows", max_duration_s=300)
    session.wait_for(collectd.session_up, "the LTTng session")
    node = subprocess.Popen(
        ["taskset", "-c", str(CPU), *argv], env=env, start_new_session=True,
        stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL,
    )  # fmt: skip
    try:
        client.request("configure", window_s=1.0, sched=True)
        client.request("select", all=False, pids=[node.pid], method_by_pid={str(node.pid): "lttng"})
        client.request("subscribe", streams=["window"])
        session.wait_for(
            lambda: f"PID: {node.pid} " in (collectd.lttng("list", "-u") or ""),
            "the node to register with the session daemon",
        )
        rows = []
        deadline = time.monotonic() + 120
        while len(rows) < WARM + MEASURE and time.monotonic() < deadline:
            w = windows(client, 1)[0]
            node_row = next((n for n in w["nodes"] if n["fqn"] == "/burner" and n["cb_hz"]), None)
            mine = [c for c in w["callbacks"] if node_row and c["id"] in node_row["callbacks"]]
            cb = max(mine, key=lambda c: c["hz"] or 0, default=None)
            # From the first window where armod knows the callback: its words reach armod at
            # armo-collectd's next scan after the thread's first callback.
            attributed = cb and cb["duration"] and node_row["cpu_pct"] < 0.75 * node_row["busy_pct"]
            rows = rows + [(node_row, cb)] if attributed or rows else []
    finally:
        client.request("record_stop", record_id=rec["record_id"])
        client.close()
        os.killpg(node.pid, 2)
        node.wait(timeout=30)
        collectd.stop()
        if armod:
            armod.stop()
            subprocess.run(accuracy.SUDO + ["rm", "-f", SOCKET], check=False)
    rows = rows[WARM:]
    if len(rows) < MEASURE:
        sys.exit(f"FAILED: the node showed in {len(rows)} windows, not {MEASURE} (kept {tmp})")
    for r, c in rows:
        print(f"window: {r['cb_hz']:.1f} Hz, busy {r['busy_pct']:.2f} %, CPU {r['cpu_pct']:.2f} %")
    n = sum(c["duration"]["n"] for _, c in rows)
    wall_ms = sum(c["duration"]["mean_ns"] * c["duration"]["n"] for _, c in rows) / n / 1e6
    on_ms = sum(c["oncpu_mean_ns"] * c["duration"]["n"] for _, c in rows) / n / 1e6
    cpu = sum(r["cpu_pct"] for r, _ in rows) / len(rows)
    busy = sum(r["busy_pct"] for r, _ in rows) / len(rows)
    sources = {r["cpu_source"] for r, _ in rows}
    print(
        f"callback: wall {wall_ms:.3f} ms, on-CPU {on_ms:.3f} ms, off-CPU {wall_ms - on_ms:.3f} ms"
    )
    print(f"node: CPU {cpu:.2f} %, busy {busy:.2f} %, cpu_source {sorted(sources)}")
    failed = []
    if sources != {"sched"}:
        failed.append(f"cpu_source {sorted(sources)}")
    if abs(on_ms / BUSY_MS - 1) > TOL:
        failed.append(f"callback on-CPU {on_ms:.3f} ms against {BUSY_MS} ms spun")
    if abs((wall_ms - on_ms) / SLEEP_MS - 1) > TOL:
        failed.append(f"callback off-CPU {wall_ms - on_ms:.3f} ms against {SLEEP_MS} ms slept")
    if not BUSY_MS / PERIOD_MS * 100 <= cpu < busy:
        failed.append(
            f"node CPU {cpu:.2f} % outside [{BUSY_MS / PERIOD_MS:.0%}, busy {busy:.2f} %)"
        )
    if failed:
        sys.exit("FAILED: " + "; ".join(failed) + f" (kept {tmp})")
    subprocess.run(["rm", "-rf", tmp], check=True)
    print("ok")


main()
