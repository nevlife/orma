"""What orma_rclpy adds to one callback when no collection method is attached.

    taskset -c 4 python3 bench_overhead.py

Prints ns per callback for a direct call of the wrapper and for a timer callback dispatched by
SingleThreadedExecutor.spin_once, each with and without the wrapper, as medians of ROUNDS.
"""

import statistics
import time
import types

import orma_rclpy
import rclpy
from rclpy.executors import SingleThreadedExecutor

ROUNDS = 7
CALLS = 1_000_000
SPINS = 20_000


def per_call(function, n):
    start = time.perf_counter_ns()
    for _ in range(n):
        function()
    return (time.perf_counter_ns() - start) / n


def per_spin(executor, n):
    start = time.perf_counter_ns()
    for _ in range(n):
        executor.spin_once(timeout_sec=0)
    return (time.perf_counter_ns() - start) / n


def report(label, plain, traced):
    p, t = statistics.median(plain), statistics.median(traced)
    print(f"{label}: plain {p:.0f} ns, traced {t:.0f} ns, added {t - p:.0f} ns")


def main():
    entity = types.SimpleNamespace(callback=lambda: None)
    plain = entity.callback
    orma_rclpy._trace(entity)
    rounds = [(per_call(plain, CALLS), per_call(entity.callback, CALLS)) for _ in range(ROUNDS)]
    report("direct call", *zip(*rounds))

    rclpy.init()
    node = rclpy.create_node("orma_rclpy_bench")
    executor = SingleThreadedExecutor()
    executor.add_node(node)
    timer = node.create_timer(0.0, lambda: None)
    traced = timer.callback
    rounds = []
    for _ in range(ROUNDS):
        timer.callback = traced.__wrapped__
        plain_ns = per_spin(executor, SPINS)
        timer.callback = traced
        rounds.append((plain_ns, per_spin(executor, SPINS)))
    report("timer through spin_once", *zip(*rounds))
    node.destroy_node()
    rclpy.shutdown()


main()
