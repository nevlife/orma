"""Calls every flux2_trace_* of a libflux2_ffi.so a known number of times with known arguments,
for accuracy.py to check that each program reads each argument (design 11.2). Prints its pid,
waits for "go", calls, prints what it expects as JSON, and waits for a line before it exits.

    flux2_trace_caller.py LIBFLUX2_FFI
"""

import ctypes
import json
import os
import sys

lib = ctypes.CDLL(sys.argv[1])
V, U32, I32, U64, I64, S = (
    ctypes.c_void_p,
    ctypes.c_uint32,
    ctypes.c_int32,
    ctypes.c_uint64,
    ctypes.c_int64,
    ctypes.c_char_p,
)
CH, EX, EP = 0xC4A1, 0xE7EC, 0xE4D0
# Each call: function, arguments, how many times, the payload fields armod must report.
CALLS = [
    (
        "channel_init",
        [V(CH), U64(11), U64(22), S(b"/flux2.v6..k.00ab.e1"), I32(1), U32(4), U32(64), I32(-1)],
        2,
        {
            "channel": CH,
            "dev": 11,
            "ino": 22,
            "publisher": 1,
            "slot_count": 4,
            "slot_size": 64,
            "stamp_offset": -1,
            "strings": ["/flux2.v6..k.00ab.e1"],
        },
    ),
    (
        "claim",
        [V(CH), U32(3), I32(2), I32(1)],
        3,
        {"channel": CH, "slot": 3, "result": 2, "loan": 1},
    ),
    ("loan", [V(CH), U32(3)], 4, {"channel": CH, "slot": 3}),
    ("loan_end", [V(CH), U32(3), I32(1)], 5, {"channel": CH, "slot": 3, "result": 1}),
    (
        "commit",
        [V(CH), U64(77), U32(3), U64(4096), I32(1), U64(0x1234_0000_5678)],
        6,
        {
            "channel": CH,
            "ticket": 77,
            "nbytes": 4096,
            "stamp": 0x1234_0000_5678,
            "slot": 3,
            "woke": 1,
        },
    ),
    (
        "take",
        [V(CH), U64(77), U32(3), U64(2), U64(0x1234_0000_5678)],
        7,
        {"channel": CH, "ticket": 77, "lost": 2, "stamp": 0x1234_0000_5678, "slot": 3},
    ),
    ("release", [V(CH), U64(77), I32(1)], 8, {"channel": CH, "ticket": 77, "held": 1}),
    ("refused", [V(CH), I32(4)], 9, {"channel": CH, "reason": 4}),
    ("wait", [V(EX), U32(5), I64(-1)], 10, {"executor": EX, "timeout_ns": -1, "sources": 5}),
    ("wake", [V(EX), I32(1), U32(2)], 10, {"executor": EX, "result": 1, "entry": 2}),
    ("callback_start", [V(EX), U32(2), V(EP)], 11, {"executor": EX, "endpoint": EP, "entry": 2}),
    (
        "callback_end",
        [V(EX), U32(2), I32(1), V(CH), S(b"/flux2.v6..k.00ab.e1")],
        11,
        {
            "executor": EX,
            "channel": CH,
            "entry": 2,
            "delivered": 1,
            "strings": ["/flux2.v6..k.00ab.e1"],
        },
    ),
    (
        "endpoint_init",
        [
            V(EP),
            S(b"/caller_node"),
            S(b"/cam"),
            S(b"/flux2.v6..cam.00ab"),
            U64(0xAB),
            I32(1),
            I32(0),
        ],
        2,
        {
            "endpoint": EP,
            "fingerprint": 0xAB,
            "role": 1,
            "lang": 0,
            "strings": ["/caller_node", "/cam", "/flux2.v6..cam.00ab"],
        },
    ),
    ("py_gil", [I32(1)], 12, {"phase": 1}),
    ("py_ready", [V(CH), U64(77)], 13, {"channel": CH, "ticket": 77}),
    (
        "record_drop",
        [V(CH), U64(0), U32(5), I32(0)],
        14,
        {"channel": CH, "ticket": 0, "count": 5, "reason": 0},
    ),
    ("gpu_wedged", [I32(700)], 1, {"result": 700}),
]
# A flux2 build from before GPU wedging has no gpu_wedged.
CALLS = [c for c in CALLS if hasattr(lib, "flux2_trace_" + c[0])]

print(os.getpid(), flush=True)
sys.stdin.readline()
funcs = {}
for name, args, _, _ in CALLS:
    funcs[name] = getattr(lib, "flux2_trace_" + name)
    funcs[name].restype = None
for name, args, n, _ in CALLS:
    if name == "wake":
        continue
    for _ in range(n):
        funcs[name](*args)
        # Each wait ends in a wake: the wait interval is timed per executor.
        if name == "wait":
            funcs["wake"](*{c[0]: c[1] for c in CALLS}["wake"])
expect = {f"flux2_{name}": {"n": n, "fields": fields} for name, _, n, fields in CALLS}
print(
    json.dumps(
        {
            "records": expect,
            "callbacks": {f"{EX:#x}:2": 11},
            # The 11 starts come before the 11 ends: nested, so only the first is at the depth
            # of the last wake.
            "dispatch": {f"{EX:#x}:2": 1},
            "waits": {f"{EX:#x}": 10},
        }
    ),
    flush=True,
)
sys.stdin.readline()
