#!/usr/bin/env python3
"""Runs ormad's flux caller against the installed libflux_ffi under LD_PRELOAD with shm_reader
attached, and fails unless the shared memory holds every event of its 300 early callbacks, each
with a late callback nested inside: 600 claim, commit, take, release and flux callback pairs in
the ring, and two executors with 300 callbacks each in the tables. Needs a libflux_ffi with
flux_trace_contract_2 (WP1).

    flux_real.py READER PRELOAD EVENTS_H
"""

import os
import subprocess
import sys
import tempfile
from collections import Counter
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))
from nodes import Dump, event_numbers  # noqa: E402

CALLER = Path(__file__).resolve().parents[3] / "daemon/orma_daemon/test/flux_caller.c"
CALLBACKS = 300
FLUX_SRC = 2


def flux_prefix():
    for p in os.environ.get("CMAKE_PREFIX_PATH", "").split(":"):
        if p and (Path(p) / "include/flux.h").is_file():
            return Path(p)
    sys.exit("flux_ffi is not under CMAKE_PREFIX_PATH: source the workspace")


def main():
    reader, preload, events_h = sys.argv[1:4]
    prefix = flux_prefix()
    lib = prefix / "lib/libflux_ffi.so"
    nm = subprocess.run(["nm", "-D", "--defined-only", lib], capture_output=True, text=True).stdout
    if "flux_trace_contract_2" not in nm:
        sys.exit(f"{lib} lacks flux_trace_contract_2")
    with tempfile.TemporaryDirectory() as tmp:
        exe = Path(tmp) / "flux_caller"
        subprocess.run(
            ["cc", "-O2", "-I", prefix / "include", CALLER, "-L", prefix / "lib", "-lflux_ffi"]
            + ["-Wl,-rpath," + str(prefix / "lib"), "-o", exe],
            check=True,
        )
        caller = subprocess.Popen(
            [exe],
            stdin=subprocess.PIPE,
            stdout=subprocess.PIPE,
            text=True,
            env=dict(os.environ, LD_PRELOAD=preload),
        )
        caller.stdout.readline()
        shm = subprocess.Popen([reader, str(caller.pid)], stdout=subprocess.PIPE, text=True)
        if shm.stdout.readline() != "A\n":
            caller.kill()
            sys.exit("the reader did not arm")
        caller.stdin.write("go\n")
        caller.stdin.flush()
        caller.communicate(timeout=60)
        text, _ = shm.communicate(timeout=30)
    d = Dump(text, event_numbers(events_h))
    failed = []
    n = 2 * CALLBACKS
    for name in ("claim", "commit", "take", "release", "callback_start", "callback_end"):
        got = len(d.records[d.ev["flux_" + name]])
        if got != n:
            failed.append(f"{name} {got}")
    tickets = Counter(r[1] for r in d.get("flux_commit", "<QQ"))
    callbacks = sorted(c for (kind, src, _), c in d.tables.items() if kind == 1 and src == FLUX_SRC)
    if callbacks != [CALLBACKS, CALLBACKS]:
        failed.append(f"table callbacks {callbacks}")
    if any(d.missing) or +d.overflow or len(tickets) != CALLBACKS:
        failed.append(f"missing {d.missing}, overflow {dict(d.overflow)}, tickets {len(tickets)}")
    if failed:
        sys.exit("; ".join(failed))
    print(f"flux: ok, {sum(len(v) for v in d.records.values())} records")


if __name__ == "__main__":
    main()
