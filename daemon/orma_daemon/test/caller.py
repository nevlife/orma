"""Calls every ros_trace_* of a libtracetools.so (and librcl.so's rcl_wait, rcl_timer_fini) a
known number of times with known arguments, for accuracy.py to count. Prints its pid, waits for
"go", calls, prints the records it expects as JSON, and waits for a line before it exits.

    caller.py LIBTRACETOOLS LIBRCL
"""

import ctypes
import json
import os
import sys
import threading
import time

lib = ctypes.CDLL(sys.argv[1])
rcl = ctypes.CDLL(sys.argv[2])
V = ctypes.c_void_p
I64 = ctypes.c_int64
B = ctypes.c_bool
S = ctypes.c_char_p
SZ = ctypes.c_size_t
RMW_ABI = os.environ["ROS_DISTRO"] != "humble"
TIMES = 7
expect = {}
codes = set()
words = {}


def have(name):
    try:
        getattr(lib, name)
        return True
    except AttributeError:
        return False


def call(name, *args, n=TIMES, event=None):
    """Calls ros_trace_<name> n times; `event` is the record name when it is not `name`."""
    f = getattr(lib, "ros_trace_" + name)
    f.restype = None
    for _ in range(n):
        f(*args)
    expect[event or name] = expect.get(event or name, 0) + n


# A message: Header.stamp in its first 8 bytes.
message = (ctypes.c_uint32 * 4)(1_700_000_000, 123, 0, 0)
msg = ctypes.addressof(message)
gid = (ctypes.c_uint8 * 24)(*range(1, 25))


class RmwHandle(ctypes.Structure):
    """rmw_publisher_t and rmw_subscription_t open with these three (rmw/types.h)."""

    _fields_ = [("implementation", S), ("data", V), ("topic_name", S)]


rmw_pub = RmwHandle(b"x", None, b"/caller/pub")
rmw_sub = RmwHandle(b"x", None, b"/caller/sub")
PUB, SUB = ctypes.addressof(rmw_pub), ctypes.addressof(rmw_sub)

print(os.getpid(), flush=True)
sys.stdin.readline()

call("rcl_init", V(0x10))
call("rcl_node_init", V(0x20), V(0x21), S(b"caller"), S(b"/ns"))
call("rmw_publisher_init", V(PUB), gid)
call("rcl_publisher_init", V(0x30), V(0x20), V(PUB), S(b"/caller/pub"), SZ(10))
call("rclcpp_publish", V(0x31), V(msg))
if have("ros_trace_rclcpp_intra_publish"):
    call("rclcpp_intra_publish", V(0x31), V(msg))
call("rcl_publish", V(0x30), V(msg))
if RMW_ABI:
    call("rmw_publish", V(PUB), V(msg), I64(4242))
    expect["rmw_publisher_topic"] = 1
else:
    call("rmw_publish", V(msg), event="rmw_publish_rcl")
call("rmw_subscription_init", V(SUB), gid)
call("rcl_subscription_init", V(0x40), V(0x20), V(SUB), S(b"/caller/sub"), SZ(10))
call("rclcpp_subscription_init", V(0x40), V(0x41))
call("rclcpp_subscription_callback_added", V(0x41), V(0xA0))
codes.add(0xA0)
call("rmw_take", V(SUB), V(msg), I64(4242), B(True))
call("rmw_take", V(SUB), V(msg), I64(0), B(False), n=3)
expect["rmw_subscription_topic"] = 1
call("rcl_take", V(msg))
call("rclcpp_take", V(msg))
call("rcl_service_init", V(0x50), V(0x20), V(0x51), S(b"/caller/srv"))
call("rclcpp_service_callback_added", V(0x50), V(0xB0))
codes.add(0xB0)
call("rcl_client_init", V(0x60), V(0x20), V(0x61), S(b"/caller/srv"))
call("rcl_timer_init", V(0x70), I64(10_000_000))
call("rclcpp_timer_callback_added", V(0x70), V(0xC0))
codes.add(0xC0)
call("rclcpp_timer_link_node", V(0x70), V(0x20))
call("rclcpp_callback_register", V(0xA0), S(b"known_callback_a"))
call("rclcpp_executor_get_next_ready")
call("rclcpp_executor_wait_for_work", I64(1000))
call("rclcpp_executor_execute", V(0x41))
# The gated form: ormad does not attach to it (design 2.4), so this call leaves no record.
if have("ros_trace_do_rclcpp_callback_register"):
    getattr(lib, "ros_trace_do_rclcpp_callback_register")(V(0xB0), S(b"gated_b"))

rcl.rcl_wait.restype = ctypes.c_int
rcl.rcl_wait.argtypes = [V, I64]
rcl.rcl_timer_fini.restype = ctypes.c_int
rcl.rcl_timer_fini.argtypes = [V]
start = lib.ros_trace_callback_start
start.argtypes = [V, B]
start.restype = None
end = lib.ros_trace_callback_end
end.argtypes = [V]
end.restype = None
callbacks = {}


def cb(key, n=1):
    callbacks[key] = callbacks.get(key, 0) + n


def pattern(rounds):
    """Nested A(B) twice, recursive C(C), each round."""
    for _ in range(rounds):
        start(0xA0, False)
        start(0xB0, False)
        end(0xB0)
        start(0xB0, True)
        end(0xB0)
        end(0xA0)
        start(0xC0, False)
        start(0xC0, False)
        end(0xC0)
        end(0xC0)


THREADS, ROUNDS = 4, 200
threads = [threading.Thread(target=pattern, args=(ROUNDS,)) for _ in range(THREADS)]
for t in threads:
    t.start()
for t in threads:
    t.join()
cb(0xA0, THREADS * ROUNDS)
cb(0xB0, 2 * THREADS * ROUNDS)
cb(0xC0, 2 * THREADS * ROUNDS)
# Each wake from rcl_wait is followed by one callback: a dispatch interval each.
WAKES = 50
for _ in range(WAKES):
    rcl.rcl_wait(None, 0)
    start(0xD0, False)
    time.sleep(0.0002)
    end(0xD0)
cb(0xD0, WAKES)
# Out of order, as an rclpy coroutine can end: both are timed. E0 starts at the depth of the
# last wake, so it gets a dispatch interval too; E1 inside it does not.
start(0xE0, False)
start(0xE1, False)
end(0xE0)
end(0xE1)
cb(0xE0)
cb(0xE1)
# An end with no start in front of it is not a callback.
end(0xF0)
rcl.rcl_timer_fini(None)
codes.update(callbacks)
expect["rcl_timer_fini"] = 1
expect["rcl_wait_enter"] = WAKES
expect["rcl_wait_exit"] = WAKES
expect["callback_start"] = sum(callbacks.values())
expect["callback_end"] = sum(callbacks.values()) + 1
expect["callback_code"] = len(codes)
print(
    json.dumps(
        {
            "records": expect,
            "callbacks": {hex(k): v for k, v in callbacks.items()},
            "dispatch": {hex(0xD0): WAKES, hex(0xE0): 1},
            "wait": WAKES,
            "gid": list(gid),
            "stamp": 1_700_000_000 | (123 << 32),
        }
    ),
    flush=True,
)
sys.stdin.readline()
