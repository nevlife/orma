"""pid_caller.py LIBTRACETOOLS N: for accuracy.py's selection test, prints its pid, then calls
ros_trace_callback_start/end N times and prints "done" on each "go" line; ends on any other."""

import ctypes
import os
import sys

lib = ctypes.CDLL(sys.argv[1])
n = int(sys.argv[2])
start, end = lib.ros_trace_callback_start, lib.ros_trace_callback_end
start.argtypes, end.argtypes = [ctypes.c_void_p, ctypes.c_bool], [ctypes.c_void_p]
print(os.getpid(), flush=True)
while sys.stdin.readline().strip() == "go":
    for _ in range(n):
        start(0x99, False)
        end(0x99)
    print("done", flush=True)
