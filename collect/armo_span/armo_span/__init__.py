"""`span(name)`: a decorator that times every call of a function as one armo span (design 20.5)."""

import ctypes
import functools
import os

# Through the global scope, where an armo shim in LD_PRELOAD comes before libarmo_span.so.
ctypes.CDLL("libarmo_span.so", mode=os.RTLD_GLOBAL)
_scope = ctypes.CDLL(None)
_begin = _scope.armo_span_begin
_end = _scope.armo_span_end
for _f in (_begin, _end):
    _f.argtypes = [ctypes.c_void_p]
    _f.restype = None


def span(name):
    # One buffer per span for the process's life: its address pairs each begin with its end.
    buf = ctypes.create_string_buffer(name.encode())
    key = ctypes.addressof(buf)

    def wrap(fn):
        @functools.wraps(fn)
        def call(*args, **kwargs):
            _begin(key)
            try:
                return fn(*args, **kwargs)
            finally:
                _end(key)

        # Keeps the buffer, and so the key, alive as long as the function.
        call._armo_span_name = buf
        return call

    return wrap
