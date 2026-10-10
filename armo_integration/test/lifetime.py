#!/usr/bin/env python3
"""Recording lifetime with the real sources, armod and the shared memory files (design 14.7
items 2 to 7; the LTTng parts of 5 and 6 are armo_lttng/test/lifecycle.py).

A publisher and a subscriber of armo_integration at 1 kHz run with libarmo_preload.so on CPUs 9
and 10 for the whole test, under a real armo-collectd and the host's armod:

2. --max-duration 5 ends finalized, max_duration, within a window of 5 s, by preload and by
   uprobe; --max-size ends max_size within a window's bytes of the size.
3. On a tmpfs of 1 GiB and 96 MiB with a 32 MiB floor (collectd.toml): a start is refused while
   less than the floor and 1 GiB is free, and a recording ends disk_floor once another writer
   takes the space.
4. SIGTERM: the recordings in progress end finalized, collector_exit.
5. SIGKILL: within 2 s the guard puts `armed` to 0; the next start marks the recording truncated.
6. SIGKILL of armo-collectd and its guard: `armed` stays; the next start lowers it and marks the
   recording truncated.
7. No client and no recording: armod's hot probes come off, `armed` goes to 0, and 5 minutes
   later armo-collectd ends.

    lifetime.py [--only N]...
"""

import argparse
import glob
import json
import os
import shutil
import signal
import subprocess
import sys
import tempfile
import time
from pathlib import Path

import armo_client
import common
from ament_index_python.packages import get_package_prefix

ARMED = 48  # offset of `armed` in armo_shm_header (armo_preload/armo_shm.h)
FLOOR = 32 << 20
SIZE = 16 << 20
# A start needs the floor plus 1 GiB free (14.2).
TMPFS = FLOOR + (1 << 30) + (64 << 20)
WINDOW_S = 1.0


def armed(pid):
    with open(glob.glob(f"/dev/shm/armo.{pid}.*")[0], "rb") as f:
        f.seek(ARMED)
        return int.from_bytes(f.read(4), "little")


def meta(rec):
    return json.loads((Path(rec["dir"]) / "meta.json").read_text())


def ended(rec, seconds=60):
    common.wait_for(lambda: meta(rec)["state"] != "recording", "the recording to end", seconds)
    return meta(rec)


def take(path, size):
    """Another writer: `size` bytes of `path`, under a CPU quota (the 70 % cap)."""
    subprocess.run(
        ["sudo", "-n", "systemd-run", "--scope", "-q", "-p", f"CPUQuota={common.CAP_PCT:.0f}%",
         f"--uid={os.getuid()}", f"--gid={os.getgid()}", "fallocate", "-l", str(size), str(path)],
        check=True,
    )  # fmt: skip


class Test:
    def __init__(self, tmp, pids):
        self.tmp, self.pids, self.failed = tmp, pids, []
        self.targets = armo_client.targets(pids=pids)

    def check(self, ok, text):
        print(("ok    " if ok else "FAIL  ") + text, flush=True)
        if not ok:
            self.failed.append(text)

    def start(self, client, method, **options):
        return client.request(
            "record_start", **armo_client.record_start(method, self.targets, **options)
        )

    def wait_uprobes(self):
        for pid in self.pids:
            common.wait_for(
                lambda: common.uprobes_on(pid, *common.HOT), "armod's uprobes", seconds=600
            )

    def duration(self, client, method):
        rec = self.start(client, method, max_duration_s=5, sched=False)
        m = ended(rec, 120)
        took = (m["ended"]["mono_ns"] - m["started"]["mono_ns"]) / 1e9
        self.check(
            m["state"] == "finalized"
            and m["end_reason"] == "max_duration"
            and 5.0 <= took <= 5.0 + WINDOW_S,
            f"2: {method} --max-duration 5: {m['state']}, {m['end_reason']}, {took:.2f} s",
        )

    def size(self, client, method):
        limit = 1 << 20
        rec = self.start(client, method, max_bytes=limit, sched=False)
        m = ended(rec, 300)
        took = (m["ended"]["mono_ns"] - m["started"]["mono_ns"]) / 1e9
        per_window = m["bytes"] / took * WINDOW_S
        self.check(
            m["state"] == "finalized"
            and m["end_reason"] == "max_size"
            and limit <= m["bytes"] <= limit + per_window,
            f"2: {method} --max-size {limit}: {m['state']}, {m['end_reason']}, {m['bytes']} B "
            f"in {took:.2f} s",
        )

    def disk(self, client):
        disk = self.tmp / "disk"
        fill = disk / "other_writer"
        try:
            take(fill, 128 << 20)
            try:
                self.start(client, "preload", dir=str(disk), max_bytes=SIZE, sched=False)
                refused = "started"
            except armo_client.ArmoError as e:
                refused = str(e)
            self.check("disk floor" in refused, f"3: start refused under the floor: {refused}")
            fill.unlink()
            rec = self.start(client, "preload", dir=str(disk), max_bytes=SIZE, sched=False)
            time.sleep(2)
            take(fill, TMPFS - FLOOR + (12 << 20) - (SIZE // 2))
            t0 = time.monotonic()
            m = ended(rec)
            self.check(
                m["end_reason"] == "disk_floor",
                f"3: ended {m['state']}, {m['end_reason']} {time.monotonic() - t0:.1f} s after "
                f"the free space fell under the floor ({m['end_detail']})",
            )
        finally:
            fill.unlink(missing_ok=True)


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--only", type=int, action="append", choices=range(2, 8))
    items = set(ap.parse_args().only or range(2, 8))
    tmp = Path(tempfile.mkdtemp(prefix="armo-life-", dir="/tmp"))
    os.chmod(tmp, 0o755)
    cfg = tmp / "h/.config/armo"
    cfg.mkdir(parents=True)
    (cfg / "collectd.toml").write_text(f"disk_floor_bytes = {FLOOR}\n")
    (tmp / "disk").mkdir()
    subprocess.run(
        ["sudo", "-n", "mount", "-t", "tmpfs", "-o",
         f"size={TMPFS},uid={os.getuid()},gid={os.getgid()},mode=0700", "tmpfs", str(tmp / "disk")],
        check=True,
    )  # fmt: skip
    host = common.Host(dict(os.environ), cwd=tmp)
    lib = Path(get_package_prefix("armo_integration")) / "lib/armo_integration"
    preload = {"LD_PRELOAD": host.lib("armo_preload", "libarmo_preload.so")}
    ros = ["--ros-args", "-p", "topics:=1", "-p", "rate_hz:=1000.0"]
    pids = [
        host.start([lib / "tc_sink", *ros, "-p", "in:=/tc/a"], 9, preload),
        host.start([lib / "tc_pub", *ros, "-p", "out:=/tc/a"], 10, preload),
    ]
    t = Test(tmp, pids)
    try:
        with common.armod(tmp):
            common.wait_for(lambda: all(glob.glob(f"/dev/shm/armo.{p}.*") for p in pids), "shm")
            collectd = common.Collectd(tmp)
            try:
                run(t, collectd, items, pids)
            finally:
                if collectd.proc.poll() is None:
                    collectd.stop()
    finally:
        host.stop()
        subprocess.run(["sudo", "-n", "umount", str(tmp / "disk")], check=True)
    if t.failed:
        sys.exit(f"FAILED: {len(t.failed)} (kept {tmp})")
    shutil.rmtree(tmp)
    print("ok")


def run(t, collectd, items, pids):
    client = armo_client.connect(start=False)
    client.request("select", all=False, pids=[])
    if 2 in items:
        t.duration(client, "preload")
        t.duration(client, "uprobe")
        t.size(client, "preload")
        t.size(client, "uprobe")
    if 3 in items:
        t.disk(client)
    if 4 in items:
        recs = [t.start(client, m, sched=False) for m in ("preload", "uprobe")]
        t.wait_uprobes()
        client.sock.close()
        collectd.proc.send_signal(signal.SIGTERM)
        collectd.proc.wait(timeout=30)
        for r, m in zip(recs, ("preload", "uprobe")):
            x = meta(r)
            t.check(
                x["state"] == "finalized" and x["end_reason"] == "collector_exit",
                f"4: SIGTERM: the {m} recording {x['state']}, {x['end_reason']}",
            )
        t.check(not any(armed(p) for p in pids), "4: armed 0 after SIGTERM")
        collectd.start()
        client = armo_client.connect(start=False)
        client.request("select", all=False, pids=[])
    for item in (5, 6):
        if item not in items:
            continue
        rec = t.start(client, "preload", sched=False)
        common.wait_for(lambda: all(armed(p) for p in pids), "armed")
        client.sock.close()
        if item == 6:
            os.kill(collectd.guard(), signal.SIGKILL)
        collectd.proc.kill()
        collectd.proc.wait()
        killed = time.monotonic()
        if item == 5:
            while any(armed(p) for p in pids) and time.monotonic() - killed < 5:
                time.sleep(0.02)
            took = time.monotonic() - killed
            t.check(took <= 2, f"5: SIGKILL: armed 0 after {took:.2f} s")
        else:
            time.sleep(3)
            t.check(all(armed(p) for p in pids), "6: with the guard dead armed stays 1")
        collectd.start()
        client = armo_client.connect(start=False)
        client.request("select", all=False, pids=[])
        common.wait_for(lambda: meta(rec)["state"] != "recording", "the next start", 10)
        state = meta(rec)["state"]
        t.check(state == "truncated", f"{item}: the next start marked it {state}")
        if item == 6:
            t.check(not any(armed(p) for p in pids), "6: the next start lowered armed")
    if 7 in items:
        client.request("configure", window_s=1.0)
        client.request(
            "select",
            all=False,
            pids=pids,
            method_by_pid={str(pids[0]): "uprobe", str(pids[1]): "preload"},
        )
        client.request("subscribe", streams=["window"])
        common.wait_for(
            lambda: common.uprobes_on(pids[0], *common.HOT) and armed(pids[1]),
            "monitoring to arm and attach",
            seconds=600,
        )
        client.close()
        gone = time.monotonic()
        common.wait_for(lambda: not armed(pids[1]), "armed 0", 10)
        t.check(True, f"7: armed 0 {time.monotonic() - gone:.2f} s after the last client")
        common.wait_for(
            lambda: not common.uprobes_on(pids[0], "ros_trace_callback_start"),
            "the hot probes to come off",
            seconds=300,
        )
        t.check(True, f"7: hot probes off {time.monotonic() - gone:.1f} s after it")
        try:
            collectd.proc.wait(timeout=400)
            took = time.monotonic() - gone
            t.check(300 <= took <= 300 + 10, f"7: armo-collectd ended {took:.0f} s after it")
        except subprocess.TimeoutExpired:
            t.check(False, "7: armo-collectd still runs 400 s after the last client")


main()
