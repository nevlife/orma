#!/usr/bin/env python3
"""armod counts every callback exactly once and times it.

Needs root for the probes, so colcon test does not run it: run it by hand after a build, with
passwordless sudo or from a root shell. The probes are limited to the caller's pid, so other ROS
processes on the host are not touched.

    armo_daemon/test/accuracy.py [path/to/armod] [path/to/libtracetools.so]
"""

import collections
import os
import subprocess
import sys
import time
from pathlib import Path

HERE = Path(__file__).resolve().parent
ARMOD = sys.argv[1] if len(sys.argv) > 1 else str(HERE.parents[1] / "target/release/armod")
LIB = sys.argv[2] if len(sys.argv) > 2 else "/opt/ros/jazzy/lib/libtracetools.so"
EXPECT = {0xA0: (1000, 200), 0xB0: (300, 2000)}


def main():
    caller = subprocess.Popen(
        [sys.executable, str(HERE / "caller.py"), LIB],
        stdin=subprocess.PIPE,
        stdout=subprocess.PIPE,
        text=True,
    )
    caller.stdout.readline()
    sudo = [] if os.geteuid() == 0 else ["sudo", "-n"]
    armod = subprocess.Popen(
        sudo + [ARMOD, "--lib", LIB, "--pid", str(caller.pid), "--interval", "0.5"],
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
        text=True,
    )
    time.sleep(1.5)
    caller.stdin.write("go\n")
    caller.stdin.flush()
    assert caller.stdout.readline().strip() == "done"
    time.sleep(1.5)
    subprocess.run(sudo + ["kill", "-INT", str(armod.pid)], check=True)
    out, err = armod.communicate(timeout=10)
    caller.wait(timeout=10)
    assert "attached" in err, err

    counts = collections.Counter()
    weighted = collections.Counter()
    names = {}
    for line in out.splitlines():
        f = line.split()
        if len(f) < 7 or not f[1].startswith("0x"):
            continue
        callback, count = int(f[1], 16), int(f[2])
        counts[callback] += count
        weighted[callback] += count * float(f[4])
        if len(f) > 7:
            names[callback] = f[7]
        assert f[0] == str(caller.pid), line

    failed = []
    for callback, (n, us) in EXPECT.items():
        mean = weighted[callback] / counts[callback] if counts[callback] else 0.0
        print(f"{callback:#x}: count {counts[callback]} (want {n}), mean {mean:.1f} us (busy {us})")
        if counts[callback] != n:
            failed.append(f"{callback:#x} count")
        if not us <= mean < us * 1.5:
            failed.append(f"{callback:#x} mean")
    if names.get(0xA0) != "known_callback_a":
        failed.append("name of 0xa0")
    if set(counts) - set(EXPECT):
        failed.append(f"unexpected callbacks {sorted(set(counts) - set(EXPECT))}")
    if failed:
        print("FAILED:", ", ".join(failed))
        return 1
    print("ok")
    return 0


if __name__ == "__main__":
    sys.exit(main())
