#!/usr/bin/env python3
"""Runs the flux2 caller under LD_PRELOAD=libarmo_flux2_lttng.so in a user LTTng session and fails
unless every event the caller makes arrives with its fields: four channel_init (a publisher and a
subscriber per channel), 600 commit and take, 600 delivered callback_end pairs named by the
segment of a subscribing channel_init.

    lttng_flux2.py LIBRARY
"""

import os
import re
import subprocess
import sys
import tempfile
from collections import Counter
from pathlib import Path

CALLER = Path(__file__).resolve().parents[2] / "armo_daemon/test/flux2_caller.c"
CALLBACKS = 600


def flux2_prefix():
    for p in os.environ.get("CMAKE_PREFIX_PATH", "").split(":"):
        if p and (Path(p) / "include/flux2.h").is_file():
            return Path(p)
    sys.exit("flux2_ffi is not under CMAKE_PREFIX_PATH: source the workspace")


def events(trace):
    text = subprocess.run(["babeltrace", trace], capture_output=True, text=True, check=True).stdout
    for line in text.splitlines():
        m = re.search(r" flux2:(\w+): .*?\{ ([^}]*) \}$", line)
        if m:
            yield m[1], dict(re.findall(r'(\w+) = ("[^"]*"|\S+?)(?:,|$)', m[2]))


def main():
    lib = Path(sys.argv[1]).resolve()
    prefix = flux2_prefix()
    with tempfile.TemporaryDirectory() as tmp:
        exe = Path(tmp) / "flux2_caller"
        subprocess.run(
            ["cc", "-O2", "-I", prefix / "include", CALLER, "-L", prefix / "lib", "-lflux2_ffi"]
            + ["-Wl,-rpath," + str(prefix / "lib"), "-o", exe],
            check=True,
        )
        session = f"armo_flux2_{os.getpid()}"
        trace = Path(tmp) / "trace"

        def lttng(*args):
            subprocess.run(["lttng", *args], check=True, stdout=subprocess.DEVNULL)

        lttng("create", session, "--output", str(trace))
        try:
            lttng("enable-event", "--userspace", "flux2:*", "--session", session)
            lttng("start", session)
            run = subprocess.run(
                [exe],
                input="go\n",
                capture_output=True,
                text=True,
                env=dict(os.environ, LD_PRELOAD=str(lib)),
                timeout=60,
            )
            lttng("stop", session)
        finally:
            lttng("destroy", session)
        if run.returncode != 0:
            sys.exit(f"the caller failed: {run.stderr}")
        got = list(events(str(trace)))

    failed = []
    kinds = Counter(k for k, _ in got)
    inits = [f for k, f in got if k == "channel_init"]
    subscribed = {f["segment"] for f in inits if f["publisher"] == "0"}
    ends = [f for k, f in got if k == "callback_end" and f["delivered"] == "1"]
    want = {"channel_init": 4, "commit": CALLBACKS, "take": CALLBACKS}
    for kind, n in want.items():
        if kinds[kind] != n:
            failed.append(f"{kind}: {kinds[kind]} events, want {n}")
    if len(ends) != CALLBACKS:
        failed.append(f"delivered callback_end: {len(ends)}, want {CALLBACKS}")
    if len(subscribed) != 2 or {f["segment"] for f in ends} != subscribed:
        failed.append(f"callback_end segments {sorted({f['segment'] for f in ends})}")
        failed.append(f"  subscribing channel_init segments {sorted(subscribed)}")
    print(dict(kinds))
    if failed:
        sys.exit("FAILED:\n" + "\n".join(failed))
    print("ok")


main()
