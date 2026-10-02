#!/usr/bin/env python3
"""armod counts every callback exactly once and times it, for ROS and for flux.

Needs root for the probes, so colcon test does not run it: run it by hand after a build, with
passwordless sudo or from a root shell, in a shell that has sourced the workspace (the flux
caller imports flux). The probes are limited to the caller's pid, so other ROS processes on the
host are not touched.

    armo_daemon/test/accuracy.py [path/to/armod] [path/to/libtracetools.so]
                                 [path/to/libflux_tracetools.so]
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
FLUX_LIB = sys.argv[3] if len(sys.argv) > 3 else None
EXPECT = {0xA0: (1000, 200), 0xB0: (300, 2000)}
# The early callback's time includes the late one dispatched inside it.
FLUX_EXPECT = {"early": (300, 1000), "late": (300, 200)}


def find_flux_lib():
    for prefix in os.environ.get("AMENT_PREFIX_PATH", "").split(":"):
        lib = Path(prefix) / "lib/libflux_tracetools.so"
        if prefix and lib.is_file():
            return str(lib)
    sys.exit("libflux_tracetools.so is not under AMENT_PREFIX_PATH: source the workspace")


def watch(caller_argv, flux_lib):
    """Run a caller under armod. Returns the caller's first line and armod's rows per callback."""
    caller = subprocess.Popen(caller_argv, stdin=subprocess.PIPE, stdout=subprocess.PIPE, text=True)
    hello = caller.stdout.readline().strip()
    sudo = [] if os.geteuid() == 0 else ["sudo", "-n"]
    armod = subprocess.Popen(
        sudo
        + [ARMOD, "--lib", LIB, "--flux-lib", flux_lib]
        + ["--pid", str(caller.pid), "--interval", "0.5"],
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
        if len(f) < 8 or not f[2].startswith("0x"):
            continue
        assert f[0] == str(caller.pid), line
        k = (f[1], f[2])
        counts[k] += int(f[3])
        weighted[k] += int(f[3]) * float(f[5])
        if len(f) > 8:
            names[k] = f[8]
    means = {k: weighted[k] / counts[k] for k in counts}
    return hello, counts, means, names


def check(failed, label, count, mean, want_n, want_us):
    print(f"{label}: count {count} (want {want_n}), mean {mean:.1f} us (busy {want_us})")
    if count != want_n:
        failed.append(f"{label} count")
    if not want_us <= mean < want_us * 1.5:
        failed.append(f"{label} mean")


def main():
    flux_lib = FLUX_LIB or find_flux_lib()
    failed = []

    _, counts, means, names = watch([sys.executable, str(HERE / "caller.py"), LIB], flux_lib)
    for callback, (n, us) in EXPECT.items():
        k = ("ros", f"{callback:#x}")
        check(failed, f"ros {callback:#x}", counts[k], means.get(k, 0.0), n, us)
    if names.get(("ros", "0xa0")) != "known_callback_a":
        failed.append("name of 0xa0")
    extra = set(counts) - {("ros", f"{c:#x}") for c in EXPECT}
    if extra:
        failed.append(f"unexpected callbacks {sorted(extra)}")

    keys, counts, means, names = watch([sys.executable, str(HERE / "flux_caller.py")], flux_lib)
    by_name = {names.get(k): k for k in counts if k[0] == "flux"}
    want = dict(zip(FLUX_EXPECT, keys.split()))
    if len(counts) != len(want) or set(by_name) != set(want.values()):
        failed.append(f"want flux callbacks named {sorted(want.values())}, got {names}")
    else:
        for which, (n, us) in FLUX_EXPECT.items():
            k = by_name[want[which]]
            check(failed, f"flux {which} {want[which]}", counts[k], means[k], n, us)

    if failed:
        print("FAILED:", ", ".join(failed))
        return 1
    print("ok")
    return 0


if __name__ == "__main__":
    sys.exit(main())
