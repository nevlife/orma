import json
import os
import subprocess
import sys
from pathlib import Path

import pytest
from ament_index_python.packages import get_package_share_directory

HERE = Path(__file__).resolve().parent
EVENTS = [
    "rcl_node_init",
    "rcl_subscription_init",
    "rcl_service_init",
    "rcl_client_init",
    "rcl_timer_init",
    "rclcpp_subscription_init",
    "rclcpp_subscription_callback_added",
    "rclcpp_service_callback_added",
    "rclcpp_timer_callback_added",
    "rclcpp_timer_link_node",
    "rclcpp_callback_register",
    "callback_start",
    "callback_end",
    "rcl_wait",
]
MESSAGES = 20
CALLS = 5


def run_nodes(preload, tmp_path):
    output = tmp_path / "nodes.json"
    env = dict(os.environ)
    autoload = os.path.join(get_package_share_directory("orma_rclpy"), "autoload")
    env["PYTHONPATH"] = autoload + os.pathsep + env.get("PYTHONPATH", "")
    env["ROS_DOMAIN_ID"] = str(100 + os.getpid() % 100)
    if preload is not None:
        env["LD_PRELOAD"] = str(preload)
    subprocess.run(
        [sys.executable, str(HERE / "nodes.py"), str(output)], env=env, timeout=120, check=True
    )
    return json.loads(output.read_text())


@pytest.fixture(scope="module")
def scratch(tmp_path_factory):
    return tmp_path_factory.mktemp("orma_rclpy")


@pytest.fixture(scope="module")
def counter(scratch):
    library = scratch / "trace_counter.so"
    subprocess.run(
        [
            "cc",
            "-shared",
            "-fPIC",
            "-O2",
            "-o",
            str(library),
            str(HERE / "trace_counter.c"),
            "-ldl",
        ],
        check=True,
    )
    return library


@pytest.fixture(scope="module")
def run(counter, scratch):
    out = run_nodes(counter, scratch)
    out["events"] = [
        {"event": EVENTS[e], "flag": f, "a": a, "b": b, "text": t}
        for e, f, a, b, t in out["records"]
    ]
    return out


def calls(run, event, **fields):
    return [
        r
        for r in run["events"]
        if r["event"] == event and all(r[k] == v for k, v in fields.items())
    ]


def only(run, event, **fields):
    found = calls(run, event, **fields)
    assert len(found) == 1, (event, fields, found)
    return found[0]


def test_autoloaded_and_resolved_to_preload(run, counter):
    assert run["callback_start_from"] == str(counter)


def test_rcl_init_handles_are_rclpy_handle_pointers(run):
    only(run, "rcl_node_init", a=run["node"])
    for pointer in run["subscriptions"]:
        only(run, "rcl_subscription_init", a=pointer, b=run["node"])
    only(run, "rcl_timer_init", a=run["timer"])
    only(run, "rcl_service_init", a=run["service"], b=run["node"])
    only(run, "rcl_client_init", a=run["client"], b=run["node"])


def subscription_handles(run):
    handles = []
    for pointer, rclpy_id in zip(run["subscriptions"], run["subscription_ids"]):
        only(run, "rclcpp_subscription_init", a=pointer, b=rclpy_id)
        handles.append(only(run, "rclcpp_subscription_callback_added", a=rclpy_id)["b"])
    return handles


def timer_handle(run):
    only(run, "rclcpp_timer_link_node", a=run["timer"], b=run["node"])
    return only(run, "rclcpp_timer_callback_added", a=run["timer"])["b"]


def service_handle(run):
    return only(run, "rclcpp_service_callback_added", a=run["service"])["b"]


def test_registration_names(run):
    for handle in subscription_handles(run):
        assert only(run, "rclcpp_callback_register", a=handle)["text"] == "main.<locals>.on_message"
    assert only(run, "rclcpp_callback_register", a=timer_handle(run))["text"] == (
        "main.<locals>.on_tick"
    )
    assert only(run, "rclcpp_callback_register", a=service_handle(run))["text"] == (
        "main.<locals>.on_request"
    )


def test_each_entity_has_its_own_handle(run):
    handles = subscription_handles(run) + [timer_handle(run), service_handle(run)]
    assert len(set(handles)) == len(handles)


def bracketed(run, handle):
    events = [r for r in run["events"] if r["a"] == handle and r["event"].startswith("callback_")]
    assert [r["event"] for r in events] == ["callback_start", "callback_end"] * (len(events) // 2)
    assert all(r["flag"] == 0 for r in events)
    return len(events) // 2


def test_callback_counts_match_what_ran(run):
    first, second = subscription_handles(run)
    assert bracketed(run, first) == MESSAGES
    assert bracketed(run, second) == MESSAGES
    assert run["counts"]["message"] == 2 * MESSAGES
    assert bracketed(run, timer_handle(run)) == run["counts"]["tick"] > 0
    assert bracketed(run, service_handle(run)) == run["counts"]["request"] == CALLS


def test_client_has_only_rcl_events(run):
    assert [r["event"] for r in run["events"] if run["client"] in (r["a"], r["b"])] == [
        "rcl_client_init"
    ]


def test_rcl_wait_from_rclpy_reaches_preload(run):
    assert calls(run, "rcl_wait")


def test_runs_without_preload(tmp_path):
    out = run_nodes(None, tmp_path)
    assert out["records"] is None
    assert os.path.basename(out["callback_start_from"]) == "libtracetools.so"
    assert out["counts"]["request"] == CALLS
