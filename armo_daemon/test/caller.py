"""Call the tracetools functions a known number of times, for accuracy.py to count."""

import ctypes
import sys
import time

lib = ctypes.CDLL(sys.argv[1])
print(flush=True)
sys.stdin.readline()
lib.ros_trace_rclcpp_callback_register(ctypes.c_void_p(0xA0), b"known_callback_a")


def busy(us):
    end = time.perf_counter_ns() + us * 1000
    while time.perf_counter_ns() < end:
        pass


for callback, n, us in ((0xA0, 1000, 200), (0xB0, 300, 2000)):
    for _ in range(n):
        lib.ros_trace_callback_start(ctypes.c_void_p(callback), False)
        busy(us)
        lib.ros_trace_callback_end(ctypes.c_void_p(callback))
# An end with no start in front of it is not a callback.
lib.ros_trace_callback_end(ctypes.c_void_p(0xB0))
print("done", flush=True)
time.sleep(2.5)
