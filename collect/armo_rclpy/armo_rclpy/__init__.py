"""Calls the ros_trace_* functions rclpy does not call, with the arguments rclcpp gives them.

Importing the module patches rclpy.node.Node; autoload/usercustomize.py imports it at startup.
"""

import ctypes
import functools
import inspect

from rclpy.node import Node

# Look the functions up in the global scope, not through the library handle, so an LD_PRELOAD
# definition wins. rclpy loaded both libraries RTLD_LOCAL, where an LD_PRELOAD shim's
# dlsym(RTLD_NEXT) cannot find the functions it forwards to. PyDLL keeps the GIL across the call.
ctypes.CDLL("libtracetools.so", mode=ctypes.RTLD_GLOBAL)
ctypes.CDLL("librcl.so", mode=ctypes.RTLD_GLOBAL)
_tracetools = ctypes.PyDLL(None)


def _function(name):
    function = _tracetools["ros_trace_" + name]
    function.restype = None
    return function


_subscription_init = _function("rclcpp_subscription_init")
_subscription_callback_added = _function("rclcpp_subscription_callback_added")
_service_callback_added = _function("rclcpp_service_callback_added")
_timer_callback_added = _function("rclcpp_timer_callback_added")
_timer_link_node = _function("rclcpp_timer_link_node")
_callback_register = _function("rclcpp_callback_register")
_callback_start = _function("callback_start")
_callback_end = _function("callback_end")

_NOT_INTRA_PROCESS = ctypes.c_bool(False)
# Zeroed so a reader looking for a std::function code address at the handle finds none.
_HANDLE_BYTES = 48


def _trace(entity):
    """Replaces entity.callback with a wrapper that brackets it; returns (handle, name)."""
    callback = entity.callback
    storage = ctypes.create_string_buffer(_HANDLE_BYTES)
    handle = ctypes.c_void_p(ctypes.addressof(storage))

    if inspect.iscoroutinefunction(callback):

        @functools.wraps(callback)
        async def traced(*args):
            _callback_start(handle, _NOT_INTRA_PROCESS)
            try:
                return await callback(*args)
            finally:
                _callback_end(handle)
    else:

        @functools.wraps(callback)
        def traced(*args):
            _callback_start(handle, _NOT_INTRA_PROCESS)
            try:
                return callback(*args)
            finally:
                _callback_end(handle)

    traced.armo_handle_storage = storage
    entity.callback = traced
    name = getattr(callback, "__qualname__", None) or type(callback).__qualname__
    return handle, name.encode()


def _pointer(entity):
    return ctypes.c_void_p(entity.handle.pointer)


def _patch(method):
    original = getattr(Node, method.__name__)

    @functools.wraps(original)
    def patched(node, *args, **kwargs):
        return method(original, node, *args, **kwargs)

    setattr(Node, method.__name__, patched)
    return method


@_patch
def create_subscription(original, node, *args, **kwargs):
    subscription = original(node, *args, **kwargs)
    rclcpp_object = ctypes.c_void_p(id(subscription))
    _subscription_init(_pointer(subscription), rclcpp_object)
    handle, name = _trace(subscription)
    _subscription_callback_added(rclcpp_object, handle)
    _callback_register(handle, name)
    return subscription


@_patch
def create_service(original, node, *args, **kwargs):
    service = original(node, *args, **kwargs)
    handle, name = _trace(service)
    _service_callback_added(_pointer(service), handle)
    _callback_register(handle, name)
    return service


@_patch
def create_timer(original, node, *args, **kwargs):
    timer = original(node, *args, **kwargs)
    handle, name = _trace(timer)
    _timer_callback_added(_pointer(timer), handle)
    _callback_register(handle, name)
    _timer_link_node(_pointer(timer), _pointer(node))
    return timer
