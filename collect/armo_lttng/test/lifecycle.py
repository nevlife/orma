#!/usr/bin/env python3
"""LTTng recordings against a real armo-collectd (design 14.6, 14.7, 19):

1. A recording outlives its client killed with SIGKILL and ends with the client's events in CTF.
2. A burst over the buffers (three processes, each busy 60 % of its CPU, while the consumer
   daemon is stopped so that the buffers fill without more load): the recording's CTF events plus its counted LTTng losses are exactly
   the events the calls make, four per pair: armo and ros2 (the session takes ros2 events for an
   LTTng recording; libtracetools emits them on Jazzy, libarmo_lttng.so on Humble).
3. armo-collectd killed with SIGKILL: within 2 s its session daemon and session are gone and the
   guard marked the recording truncated. A process that registered with the old daemon registers
   with the new one by itself, and its events reach a new recording.
4. armo-collectd and its guard killed: the session daemon still ends, and the next armo-collectd
   marks the recording truncated.

    lifecycle.py LIBRARY CALLER
"""

import json
import os
import re
import signal
import subprocess
import sys
import tempfile
import time
from pathlib import Path

import armo_client
import harness
import session

BURST = 500_000
# Three CPUs, each at most 60 % busy (caller.c): the machine stays under its 70 % load cap.
BURST_CPUS = (9, 10, 11)


def meta(rec):
    return json.loads((Path(rec["dir"]) / "meta.json").read_text())


def ctf_events(rec, pids):
    """Events of any provider from `pids` in the recording's CTF."""
    run = subprocess.run(
        ["babeltrace", "-n", "context", "-f", "trace:domain", str(Path(rec["dir"]) / "ctf")],
        capture_output=True,
        text=True,
    )
    return sum(1 for m in re.finditer(r"vpid = (\d+)", run.stdout) if int(m[1]) in pids)


def consumerd(home):
    """The pids of the consumer daemons of the session daemon under `home`."""
    out = []
    for d in Path("/proc").iterdir():
        try:
            if (d / "comm").read_text().strip().startswith("lttng-consumerd") and (
                f"LTTNG_HOME={home}".encode() in (d / "environ").read_bytes().split(b"\0")
            ):
                out.append(int(d.name))
        except OSError:
            continue
    return out


def record(client):
    return client.request("record_start", method="lttng", max_duration_s=120)


def main():
    lib, caller = sys.argv[1], sys.argv[2]
    tmp = Path(tempfile.mkdtemp(prefix="armo-life-", dir="/tmp"))
    collectd = harness.Collectd(tmp)
    app_env = dict(collectd.env, LD_PRELOAD=lib, LTTNG_HOME=str(collectd.lttng_home))
    failed = []

    def check(ok, text):
        print(("ok    " if ok else "FAIL  ") + text)
        if not ok:
            failed.append(text)

    # 1. The client dies; the recording goes on.
    starter = subprocess.Popen(
        [sys.executable, "-c",
         "import armo_client, time\n"
         "c = armo_client.connect(start=False)\n"
         "r = c.request('record_start', method='lttng', max_duration_s=120)\n"
         "print(r['record_id'], flush=True)\n"
         "time.sleep(60)\n"],
        env=collectd.env, stdout=subprocess.PIPE, text=True,
    )  # fmt: skip
    rid = starter.stdout.readline().strip()
    starter.kill()
    starter.wait()
    client = armo_client.connect(start=False)
    # A client's monitoring would arm every shared memory file of this user.
    client.request("select", all=False, pids=[])
    states = {r["id"]: r["state"] for r in client.state["recordings"]}
    check(states.get(rid) == "recording", f"the recording of a killed client goes on: {states}")
    session.wait_for(collectd.session_up, "the LTTng session")
    tick = subprocess.Popen(["taskset", "-c", "8", caller, "tick"], env=app_env)
    try:
        time.sleep(2)
        client.request("record_stop", record_id=rid)
        rec = {
            "dir": next(
                r["dir"] for r in client.request("record_list")["recordings"] if r["id"] == rid
            )
        }
        n = ctf_events(rec, {tick.pid})
        check(n > 0 and meta(rec)["state"] == "finalized", f"its CTF holds the app's events: {n}")

        # 2. A burst: what LTTng kept plus what it counted as discarded is every call.
        rec = record(client)
        session.wait_for(collectd.session_up, "the LTTng session")
        stopped = consumerd(collectd.lttng_home)
        check(stopped != [], f"the consumer daemons to stop: {stopped}")
        for pid in stopped:
            os.kill(pid, signal.SIGSTOP)
        bursts = [
            subprocess.Popen(["taskset", "-c", str(c), caller, "burst", str(BURST)], env=app_env)
            for c in BURST_CPUS
        ]
        for b in bursts:
            b.wait()
        for pid in stopped:
            os.kill(pid, signal.SIGCONT)
        time.sleep(2)
        client.request("record_stop", record_id=rec["record_id"])
        kept = ctf_events(rec, {b.pid for b in bursts})
        lost = meta(rec)["drops"]["lttng"]
        want = 4 * BURST * len(bursts)
        check(lost > 0 and kept + lost == want, f"burst: {kept} kept + {lost} lost = {want}")

        # 3. armo-collectd dies with SIGKILL; the guard cleans up.
        rec = record(client)
        session.wait_for(collectd.session_up, "the LTTng session")
        client.sock.close()
        collectd.proc.kill()
        collectd.proc.wait()
        killed = time.monotonic()
        session.wait_for(
            lambda: collectd.lttng("list") is None and meta(rec)["state"] == "truncated",
            "the session daemon to end and the recording to be truncated",
            seconds=2,
        )
        check(
            True,
            f"after SIGKILL: daemon and session gone, truncated in {time.monotonic() - killed:.2f} s",
        )
        collectd.start()
        client = armo_client.connect(start=False)
        client.request("select", all=False, pids=[])
        rec = record(client)
        session.wait_for(collectd.session_up, "the LTTng session")
        session.wait_for(
            lambda: f"PID: {tick.pid} " in (collectd.lttng("list", "-u") or ""),
            "the running app to register with the new daemon",
        )
        time.sleep(2)
        client.request("record_stop", record_id=rec["record_id"])
        n = ctf_events(rec, {tick.pid})
        check(n > 0, f"a running app registered again and was recorded: {n} events")

        # 4. armo-collectd and its guard die together.
        rec = record(client)
        session.wait_for(collectd.session_up, "the LTTng session")
        client.sock.close()
        os.kill(collectd.guard(), signal.SIGKILL)
        collectd.proc.kill()
        collectd.proc.wait()
        session.wait_for(lambda: collectd.lttng("list") is None, "the session daemon to end", 2)
        check(meta(rec)["state"] == "recording", "with the guard dead nothing marked the recording")
        collectd.start()
        session.wait_for(lambda: meta(rec)["state"] == "truncated", "the next start to truncate it")
        check(True, "the next armo-collectd marked it truncated")
    finally:
        tick.kill()
        tick.wait()
        if collectd.proc.poll() is None:
            collectd.stop()
    if failed:
        sys.exit(f"FAILED: {len(failed)} (kept {tmp})")
    subprocess.run(["rm", "-rf", tmp], check=True)
    print("ok")


main()
