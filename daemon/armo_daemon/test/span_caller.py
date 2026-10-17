"""span_caller.py LIBARMO_SPAN N: for accuracy.py's span case, prints "pid outer_address
inner_address", runs N times outer { inner } on "go", prints "done", ends on the next line."""

import ctypes
import os
import sys

lib = ctypes.CDLL(sys.argv[1])
n = int(sys.argv[2])
outer = ctypes.create_string_buffer(b"outer")
inner = ctypes.create_string_buffer(b"inner")
begin, end = lib.armo_span_begin, lib.armo_span_end
begin.argtypes = end.argtypes = [ctypes.c_void_p]
print(os.getpid(), ctypes.addressof(outer), ctypes.addressof(inner), flush=True)
sys.stdin.readline()
for _ in range(n):
    begin(ctypes.addressof(outer))
    begin(ctypes.addressof(inner))
    end(ctypes.addressof(inner))
    end(ctypes.addressof(outer))
print("done", flush=True)
sys.stdin.readline()
