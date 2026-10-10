#!/usr/bin/env python3
"""Runs the flux2 caller under LD_PRELOAD=libarmo_lttng.so in a private LTTng session and fails
unless every event the caller makes arrives with its fields: four channel_init (a publisher and a
subscriber per channel), 600 claim, commit, take and release, 600 delivered callback_end pairs
named by the segment of a subscribing channel_init, each after a callback_start naming its
endpoint.

    lttng_flux2.py LIBRARY
"""

import subprocess
import sys
import tempfile
from pathlib import Path

import session

CALLBACKS = 600


def main():
    lib = Path(sys.argv[1]).resolve()
    with tempfile.TemporaryDirectory() as tmp:
        exe = session.flux2_caller(tmp)
        trace = Path(tmp) / "trace"
        with session.daemon() as env, session.session(env, trace, "flux2:*"):
            run = subprocess.run(
                [exe],
                input="go\n",
                capture_output=True,
                text=True,
                env=dict(env, LD_PRELOAD=str(lib)),
                timeout=60,
            )
        if run.returncode != 0:
            sys.exit(f"the caller failed: {run.stderr}")
        got = [(k.split(":")[1], f) for k, f in session.events(trace) if k.startswith("flux2:")]

    failed = []
    kinds = session.count(got)
    inits = [f for k, f in got if k == "channel_init"]
    subscribed = {f["segment"] for f in inits if f["publisher"] == "0"}
    ends = [f for k, f in got if k == "callback_end" and f["delivered"] == "1"]
    want = {
        "channel_init": 4,
        "claim": CALLBACKS,
        "commit": CALLBACKS,
        "take": CALLBACKS,
        "release": CALLBACKS,
    }
    for kind, n in want.items():
        if kinds[kind] != n:
            failed.append(f"{kind}: {kinds[kind]} events, want {n}")
    if len(ends) != CALLBACKS:
        failed.append(f"delivered callback_end: {len(ends)}, want {CALLBACKS}")
    if len(subscribed) != 2 or {f["segment"] for f in ends} != subscribed:
        failed.append(f"callback_end segments {sorted({f['segment'] for f in ends})}")
        failed.append(f"  subscribing channel_init segments {sorted(subscribed)}")
    if any(f["stamp_offset"] != "-1" for f in inits):
        failed.append("channel_init stamp_offset is not -1")
    if any(f["stamp"] != "0" for k, f in got if k in ("commit", "take")):
        failed.append("a frame without a header carried a stamp")
    starts = [f for k, f in got if k == "callback_start"]
    if len(starts) < CALLBACKS or any(int(f["endpoint"], 16) == 0 for f in starts):
        failed.append(f"callback_start: {len(starts)} events, want {CALLBACKS} naming an endpoint")
    print(dict(kinds))
    if failed:
        sys.exit("FAILED:\n" + "\n".join(failed))
    print("ok")


main()
