#!/usr/bin/env python3
"""The decorator's calls reach a preloaded shim with one key per span, keep the function's result
and exception, and run without any shim. argv[1]: the fake shim, preloaded into a child."""

import ctypes
import os
import subprocess
import sys

from armo_span import span


@span("fit")
def fit(x):
    return x * 2


@span("fail")
def fail():
    raise ValueError("kept")


def child():
    assert fit(21) == 42
    try:
        fail()
    except ValueError as e:
        assert str(e) == "kept"
    for _ in range(3):
        fit(1)
    out = (ctypes.c_uint64 * 4)()
    ctypes.CDLL(None).fake_shim_read(out)
    begins, ends, begin_key, end_key = out
    assert (begins, ends) == (5, 5), (begins, ends)
    assert begin_key == end_key == ctypes.addressof(fit._armo_span_name)
    assert ctypes.string_at(begin_key) == b"fit"


def main():
    if len(sys.argv) == 1:
        child()
        return
    assert fit(2) == 4, "runs on libarmo_span.so alone"
    env = dict(os.environ, LD_PRELOAD=sys.argv[1])
    subprocess.run([sys.executable, __file__], env=env, check=True)
    print("ok")


main()
