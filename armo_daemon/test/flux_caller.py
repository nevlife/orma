"""Run flux callbacks a known number of times, for accuracy.py to count.

The early channel is opened before armod attaches, so its name has to come from the owner file.
The late one is opened after, so its name comes from the probe. Each early callback dispatches
the late channel's executor inside it, on the same thread.
"""

import os
import sys
import time

import flux
import numpy as np

N = 300
EARLY_BUSY_US = 1000
LATE_BUSY_US = 200
FP = 0x0A4C
EARLY = f"/armo_test/early_{os.getpid()}"
LATE = f"/armo_test/late_{os.getpid()}"


def busy(us):
    end = time.perf_counter_ns() + us * 1000
    while time.perf_counter_ns() < end:
        pass


frame = np.zeros(8, dtype=np.uint8)
early_pub = flux.Publisher(EARLY, fingerprint=FP, slot_size=64, slot_count=4)
early_sub = flux.Subscription(EARLY, fingerprint=FP)
early_ex = flux.Executor()
early_ex.spin_once(timeout_ns=0)
print(EARLY, LATE, flush=True)
sys.stdin.readline()

late_pub = flux.Publisher(LATE, fingerprint=FP, slot_size=64, slot_count=4)
late_sub = flux.Subscription(LATE, fingerprint=FP)
late_ex = flux.Executor()
late_ex.add(late_sub, lambda v: busy(LATE_BUSY_US))


def early(v):
    busy(EARLY_BUSY_US)
    assert late_pub.publish(frame) == flux.Published.OK
    assert late_ex.spin_once(timeout_ns=1_000_000_000) == 1


early_ex.add(early_sub, early)
for _ in range(N):
    assert early_pub.publish(frame) == flux.Published.OK
    assert early_ex.spin_once(timeout_ns=1_000_000_000) == 1
print("done", flush=True)
time.sleep(2.5)
for key in (EARLY, LATE):
    os.unlink("/dev/shm" + flux.signpost_name(key, FP))
