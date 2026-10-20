"""Callbacks that sleep and callbacks that spin, for accuracy.py's off-CPU check (design 15.2).
Two ways to tell the kernel which callback runs, each on its own thread: uprobes on
ros_trace_callback_* keep a stack, and a 16-byte word (`cur_cb`, `cur_src`, `cur_entry`) like the
LD_PRELOAD shim's, whose address goes to ormad as the thread's user word. A spinning callback
spins for MS of its own CPU time, so preemption on a busy CPU lengthens it but not its CPU use.
Prints "pid uprobe_tid word_tid word_address", waits for "go", runs, prints each callback key's
total wall time in ns as JSON.

    sched_caller.py LIBTRACETOOLS CPU N MS
"""

import ctypes
import json
import os
import sys
import threading
import time

lib = ctypes.CDLL(sys.argv[1])
cpu, n, ms = int(sys.argv[2]), int(sys.argv[3]), float(sys.argv[4])
os.sched_setaffinity(0, {cpu})
start, end = lib.ros_trace_callback_start, lib.ros_trace_callback_end
start.argtypes, end.argtypes = [ctypes.c_void_p, ctypes.c_bool], [ctypes.c_void_p]
word = (ctypes.c_uint64 * 2)()
s = ms / 1000


wall = {}


def spin(seconds):
    t = time.thread_time() + seconds
    while time.thread_time() < t:
        pass


def timed(key, f):
    t = time.perf_counter_ns()
    f(s)
    wall[key] = wall.get(key, 0) + time.perf_counter_ns() - t


def by_word(go, tids):
    tids.append(threading.get_native_id())
    go.wait()
    for key, wait in ((0xA3, time.sleep), (0xA4, spin)):
        for _ in range(n):
            word[1] = 1
            word[0] = key
            timed(key, wait)
            word[0] = 0


go, tids = threading.Event(), []
other = threading.Thread(target=by_word, args=(go, tids))
other.start()
while not tids:
    time.sleep(0.01)
print(os.getpid(), threading.get_native_id(), tids[0], ctypes.addressof(word), flush=True)
sys.stdin.readline()
for key, wait in ((0xA1, time.sleep), (0xA2, spin)):
    for _ in range(n):
        start(key, False)
        timed(key, wait)
        end(key)
go.set()
other.join()
print(json.dumps({hex(k): v for k, v in wall.items()}), flush=True)
sys.stdin.readline()
