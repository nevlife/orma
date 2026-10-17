import os
import signal
import subprocess
import sys
import time

import armo_client
import pytest
from conftest import HERE, PACKAGE, Fake, golden, wait_for, without_id
from python_qt_binding.QtCore import Qt
from rqt_armo.graph import hops

STATE_RECORD = "r-20261008-153012-1"
CALLBACK = "c:4021:771234:ros:55a0c1d0"


def table_texts(table):
    return [
        [table.item(r, c).text() for c in range(table.columnCount())]
        for r in range(table.rowCount())
    ]


def column(table, col):
    return [row[col] for row in table_texts(table)]


def tree_texts(item, depth=0):
    out = [("  " * depth) + item.text(0)]
    for i in range(item.childCount()):
        out += tree_texts(item.child(i), depth + 1)
    return out


def last(fake, op):
    return without_id([r for r in fake.requests() if r["op"] == op][-1])


def test_initial_state(gui, fake):
    """On open the GUI shows the daemon's state: the recording in progress and the monitor."""
    ops = fake.ops()
    assert ops == ["hello", "subscribe"]
    hello = fake.requests()[0]
    assert hello["client"] == "rqt_armo" and hello["pid"] == os.getpid()
    assert without_id(fake.requests()[1]) == without_id(golden("client", "subscribe"))

    row = table_texts(gui.recordings.table)[0]
    assert row[:5] == [STATE_RECORD, "recording", "events", "uprobe,preload", "pids 4021"]
    assert row[5:9] == ["0:00:42", "100.0MiB", "0:29:18", "7.9GiB"]
    assert row[9:12] == ["1200000", "0", "rqt_armo:5555@orin"]
    assert gui.armod.text.text() == "armod: connected"
    assert gui.monitor_label.text() == (
        "monitor: running, window 1 s, 4021:preload, sched, gpu, futex, chain perception"
    )

    tree = gui.monitor.tree
    lines = []
    for i in range(tree.topLevelItemCount()):
        lines += tree_texts(tree.topLevelItem(i))
    assert lines == [
        "camera_node (4021)",
        "  /camera/driver",
        "    sub(/scan)",
        "  unlinked",
        "    flux(/image)",
        "listener (5100/12)",
    ]
    callback = tree.topLevelItem(0).child(0).child(0)
    assert [callback.text(c) for c in range(2, 13)] == [
        "10.0", "10.0", "-", "1.0ms", "1.6ms", "2.1ms", "2.0ms", "-", "-", "-", "-",
    ]  # fmt: skip
    assert column(gui.monitor.topics, 0) == ["/scan", "/image"]
    assert table_texts(gui.monitor.subscriptions)[0][:5] == [
        "/scan", "/camera/driver", "10.0", "0.0", "0",
    ]  # fmt: skip
    assert column(gui.monitor.executors, 1) == ["rclcpp"]
    assert column(gui.monitor.threads, 0) == ["4025"]
    assert column(gui.monitor.missing, 2) == ["rclcpp_intra_publish"]
    assert table_texts(gui.events)[0][1:4] == [
        "warn", "probe_missing", "rclcpp_intra_publish not exported by libtracetools.so",
    ]  # fmt: skip

    selection = gui.selection.table
    assert [selection.item(r, 0).checkState() for r in range(2)] == [Qt.Checked, Qt.Unchecked]
    assert selection.cellWidget(0, 4).currentData() == "preload"
    assert not gui.selection.all.isChecked()


FORMS = {
    "record_start_default": {},
    "record_start_windows": {"kind": "windows", "max_duration_s": "86400"},
    "record_start_all": {
        "method": "preload",
        "pids": [4021],
        "kind": "events",
        "dir": "/home/u/rec",
        "name": "run1",
        "max_duration_s": "600",
        "max_bytes": "4294967296",
        "split_bytes": "1073741824",
        "split_duration_s": "60",
        "cache_bytes": "16777216",
        "compression": "zstd",
        "bandwidth": True,
        "sched": False,
        "malloc": True,
    },
}


def fill(form, values):
    for key, value in values.items():
        if key == "pids":
            for i in range(form.pids.count()):
                item = form.pids.item(i)
                item.setCheckState(Qt.Checked if item.data(Qt.UserRole) in value else Qt.Unchecked)
        elif key in form.integers:
            form.integers[key].setText(value)
        elif key in ("node", "topic", "exclude", "dir", "name"):
            getattr(form, key).setText(value)
        elif key in ("bandwidth", "malloc"):
            getattr(form, key).setChecked(value)
        else:
            box = getattr(form, key)
            box.setCurrentIndex(box.findData(value))


def start_from_form(gui, values):
    gui.recordings.start_button.click()
    dialog = gui.recordings.dialog
    fill(dialog.form, values)
    dialog.accept()


@pytest.mark.parametrize("name", FORMS)
def test_form_matches_cli_golden(gui, fake, name):
    """The form and `armo record` send the same record_start."""
    start_from_form(gui, FORMS[name])
    wait_for(lambda: "r-fake-1" in column(gui.recordings.table, 0))
    want = without_id(golden("client", name))
    if "targets" in want:
        # The golden's node, topic and exclude, which armo refuses until they are applied.
        want["targets"] = {k: want["targets"][k] for k in ("all", "pids")}
    assert last(fake, "record_start") == want


def test_stop_then_validation(gui, fake):
    gui.recordings.table.selectRow(0)
    assert gui.recordings.stop_button.isEnabled()
    gui.recordings.stop_button.click()
    wait_for(lambda: gui.recordings.validation.title() == f"Validation: {STATE_RECORD}")
    assert last(fake, "record_stop") == without_id(golden("client", "record_stop"))
    assert last(fake, "record_show") == without_id(golden("client", "record_show_id"))
    v = gui.recordings.validation
    assert table_texts(v.entities) == [
        ["p:4021:8812345", "4200", "3", "10.0"],
        ["p:4022:8812399", "0", "0", "-"],
    ]
    assert column(v.warnings, 0) == ["empty_target", "probe_missing"]
    assert v.totals.text() == "Total: 1200000 events, 3 lost, 61.5 s, 150.0MiB"
    assert v.end.text() == "End: user_stop"
    assert table_texts(gui.recordings.table)[0][1] == "finalized"
    assert not gui.recordings.stop_button.isEnabled()


def test_reconnect_shows_state(gui, fake, runtime):
    """A new armo-collectd (its state no longer has r-fake-1) is drawn from its own state."""
    start_from_form(gui, {})
    wait_for(lambda: "r-fake-1" in column(gui.recordings.table, 0))
    fake.stop()
    wait_for(lambda: gui.collectd.text.text().startswith("armo-collectd: disconnected"))
    assert not gui.recordings.start_button.isEnabled()
    assert table_texts(gui.events)[0][1:3] == ["error", "connection"]
    again = Fake(fake.socket_path, fake.log)
    try:
        wait_for(lambda: gui.collectd.text.text().startswith("armo-collectd: connected"))
        wait_for(lambda: column(gui.recordings.table, 0) == [STATE_RECORD])
        assert fake.ops().count("hello") == 2
        assert gui.recordings.start_button.isEnabled()
    finally:
        again.stop()


def test_close_sends_no_stop(gui, fake):
    start_from_form(gui, {})
    wait_for(lambda: "r-fake-1" in column(gui.recordings.table, 0))
    gui.shutdown()
    wait_for(lambda: fake.ops()[-1] == "bye")
    assert "record_stop" not in fake.ops()
    with armo_client.connect("test", start=False) as c:
        states = {r["id"]: r["state"] for r in c.state["recordings"]}
    assert states == {STATE_RECORD: "recording", "r-fake-1": "recording"}


def test_kill_keeps_recording(fake, runtime):
    env = dict(os.environ, PYTHONPATH=f"{PACKAGE}:{os.environ.get('PYTHONPATH', '')}")
    gui = subprocess.Popen(
        [sys.executable, str(HERE / "gui_process.py")], stdout=subprocess.PIPE, text=True, env=env
    )
    assert gui.stdout.readline() == "recording r-fake-1\n"
    gui.send_signal(signal.SIGKILL)
    gui.wait()
    assert "record_stop" not in fake.ops() and "bye" not in fake.ops()
    with armo_client.connect("test", start=False) as c:
        assert {r["id"]: r["state"] for r in c.state["recordings"]}["r-fake-1"] == "recording"


def test_detail_matches_golden(gui, fake):
    tree = gui.monitor.tree
    item = tree.topLevelItem(0).child(0).child(0)
    tree.itemDoubleClicked.emit(item, 0)
    wait_for(lambda: gui.detail is not None)
    assert last(fake, "detail") == without_id(golden("client", "detail"))
    assert column(gui.detail.table, 2) == ["3", "40", "120", "30", "2"]
    assert column(gui.detail.table, 1)[0] == "16.4us - 32.8us"


def test_select_and_configure(gui, fake):
    gui.selection.all.setChecked(True)
    gui.selection.window_s.setValue(0.5)
    assert gui.selection.edited
    gui.selection.apply_button.click()
    wait_for(lambda: "configure" in fake.ops())
    assert last(fake, "select") == without_id(golden("client", "select_all"))
    assert last(fake, "configure") == {"op": "configure", "window_s": 0.5}
    wait_for(lambda: gui.monitor_label.text().startswith("monitor: running, window 0.5 s, all"))
    assert not gui.selection.edited


def test_threads_sort_by_value_and_search_by_name(app):
    from rqt_armo.monitor import MonitorView

    w = golden("server", "window")
    base = w["threads"][0]
    w["threads"] = [
        dict(base, id=f"t:4021:771234:{tid}", tid=tid, name=name, cpu_pct=cpu)
        for tid, name, cpu in ((11, "recv", 9.5), (12, "tev", None), (13, "recv", 10.0))
    ]
    w["threads"][1]["cpu_wait"] = dict(base["cpu_wait"], p99_ns=2_000_000)
    monitor = MonitorView()
    monitor.show_window(w)
    threads = monitor.threads
    assert column(threads, 0) == ["11", "12", "13"]
    threads.sortItems(4, Qt.DescendingOrder)
    assert column(threads, 0) == ["13", "11", "12"]
    threads.sortItems(8, Qt.DescendingOrder)
    assert column(threads, 0)[0] == "12"
    monitor.show_window(w)
    assert column(threads, 0)[0] == "12"
    monitor.thread_search.setText("RECV")
    assert sorted(column(threads, 0)) == ["11", "13"]


def test_graph_walk(gui):
    graph = gui.graph
    assert not graph.scene.items()
    gui.tabs.setCurrentWidget(graph)
    assert graph.scene.items()
    publisher = "n:3900:770001:55aa0000"
    graph.start.setCurrentIndex(graph.start.findData(publisher))
    graph.start.activated.emit(graph.start.currentIndex())
    assert graph.next.count() == 1
    assert graph.next.itemText(0) == "/scan -> /camera/driver"
    graph.add_button.click()
    assert table_texts(graph.hops) == [
        ["1", "transport", "/scan -> /camera/driver", "131.1us", "327.7us", "524.3us", "400.0us", "210.0us", "100% (largest)"]
    ]  # fmt: skip
    assert graph.e2e.text() == "e2e (sum of hop means): 210.0us"
    graph.back_button.click()
    assert graph.hops.rowCount() == 0


def lat_of(mean):
    return {
        "n": 1,
        "mean_ns": mean,
        "p50_ns": mean,
        "p90_ns": mean,
        "p99_ns": mean,
        "p999_ns": mean,
        "max_ns": mean,
    }


def flow(node, a, b, kind="sync", ambiguous=0):
    return {
        "node": node,
        "input": a,
        "output": b,
        "kind": kind,
        "ambiguous": ambiguous,
        "dwell": lat_of(5),
    }


@pytest.mark.parametrize(
    "flows,work",
    [
        ([flow("B", "t1", "t2")], ("callback (sync)", 5)),
        ([flow("B", "t1", "t2", "stamp")], ("dwell (stamp)", 5)),
        ([flow("B", "t1", "t2", "blind")], ("blind", None)),
        ([flow("B", "t1", "t2", ambiguous=2)], ("ambiguous", None)),
        ([flow("B", "t1", "t2"), flow("B", "t1", "t2", "stamp")], ("ambiguous", None)),
        ([flow("B", "t0", "t2")], ("no link", None)),
    ],
)
def test_hops_never_invent_a_link(flows, work):
    w = {
        "nodes": [{"id": n, "fqn": "/" + n.lower()} for n in "ABC"],
        "topics": [{"id": "t1", "name": "/t1"}, {"id": "t2", "name": "/t2"}],
        "subscriptions": [
            {"topic": "t1", "node": "B", "hop": lat_of(1)},
            {"topic": "t2", "node": "C", "hop": None},
        ],
        "flows": flows,
    }
    rows = hops(w, [(None, "A"), ("t1", "B"), ("t2", "C")])
    assert [(r[1], r[3] and r[3]["mean_ns"]) for r in rows] == [
        ("transport", 1),
        work,
        ("transport", None),
    ]


def test_rqt_loads_plugin(app, fake, tmp_path):
    """rqt finds the plugin through its plugin.xml export, and closing rqt only says bye."""
    env = dict(os.environ, QT_QPA_PLATFORM="offscreen", XDG_CONFIG_HOME=str(tmp_path))
    rqt = subprocess.Popen(
        ["rqt", "--standalone", "rqt_armo"],
        env=env,
        stdout=subprocess.DEVNULL,
        stderr=subprocess.DEVNULL,
    )
    wait_for(lambda: "subscribe" in fake.ops(), timeout=30)
    # qt_gui's SIGINT handler closes rqt inside whatever Python code runs next. While startup still
    # paints, that can be qt_gui's own eventFilter on a paint event, and Qt then flushes the
    # detached plugin widget and dies of SIGSEGV. The test is about discovery and the bye.
    time.sleep(3)
    rqt.send_signal(signal.SIGINT)
    assert rqt.wait(30) == 0
    assert fake.ops() == ["hello", "subscribe", "bye"]
    assert fake.requests()[0]["client"] == "rqt_armo"


def test_select_new_options_and_chains(gui, fake):
    selection = gui.selection
    assert selection.extras["gpu"].isChecked() and not selection.extras["dds"].isChecked()
    assert selection.chains.text() == "perception=/camera/image,/detections,/tracks"
    selection.extras["dds"].setChecked(True)
    selection.chains.setText("x=/a")
    selection.apply_button.click()
    assert selection.status.text().startswith("chain 'x=/a'")
    assert "configure" not in fake.ops()
    selection.chains.setText("r=/obstacle,/trajectory:first")
    selection.apply_button.click()
    wait_for(lambda: "configure" in fake.ops())
    assert last(fake, "configure") == {
        "op": "configure",
        "dds": True,
        "chains": [{"name": "r", "topics": ["/obstacle", "/trajectory"], "mode": "first"}],
    }
    wait_for(lambda: gui.monitor_label.text().endswith("gpu, dds, futex, chain r"))


def test_form_sends_new_options(gui, fake):
    gui.recordings.start_button.click()
    form = gui.recordings.dialog.form
    form.switches["gpu"].setChecked(True)
    form.switches["futex"].setChecked(True)
    form.chains.setText("a=/x,/y b=/p,/q:first")
    form.latest_nodes.setText("/fusion /planner")
    gui.recordings.dialog.accept()
    wait_for(lambda: "record_start" in fake.ops())
    assert last(fake, "record_start") == {
        "op": "record_start",
        "gpu": True,
        "futex": True,
        "chains": [
            {"name": "a", "topics": ["/x", "/y"], "mode": "lineage"},
            {"name": "b", "topics": ["/p", "/q"], "mode": "first"},
        ],
        "latest_nodes": ["/fusion", "/planner"],
    }


def lat_ns(p50, p99):
    return {
        "n": 4,
        "mean_ns": p50,
        "p50_ns": p50,
        "p90_ns": p50,
        "p99_ns": p99,
        "p999_ns": p99,
        "max_ns": p99,
    }


def test_new_tabs(app):
    from rqt_armo.monitor import MonitorView

    w = golden("server", "window")
    callback = w["callbacks"][0]
    callback["cuda"] = {
        "calls": 3, "launches": 2, "memcpy_bytes": 0, "api_mean_ns": 250_000, "sync_mean_ns": 0,
    }  # fmt: skip
    callback["futex_mean_ns"] = 12_000
    w["threads"][0]["futex"] = lat_ns(1_000, 7_000)
    w["topics"][0]["dds_write"] = lat_ns(5_000, 20_000)
    w["processes"][0]["gpu_mem_bytes"] = 512 << 20
    w["spans"] = [
        {
            "id": "s:4021:771234:1", "name": "preprocess", "pid": 4021, "node": callback["node"],
            "callback": callback["id"], "parent": None, "hz": 10.0,
            "duration": lat_ns(800_000, 1_200_000),
        }
    ]  # fmt: skip
    w["chains"] = [
        {
            "name": "perception", "mode": "lineage", "topics": ["/scan", "/tracks"], "count": 10,
            "e2e": lat_ns(20_000_000, 31_000_000), "steps": [lat_ns(20_000_000, 31_000_000)],
            "broken": 0,
        }
    ]  # fmt: skip
    w["host"] = {
        "cpu_mhz": [1200, 2200],
        "thermal": [{"name": "cpu-thermal", "temp_c": 61.5}],
        "throttle": {},
        "net": [],
        "udp": None,
        "gpus": [
            {
                "index": 0, "name": "Orin", "util_pct": None, "mem_used_bytes": None,
                "mem_total_bytes": None, "clock_mhz": 918, "temp_c": None, "power_w": None,
                "throttle": [],
            }
        ],
    }  # fmt: skip
    monitor = MonitorView()
    monitor.show_window(w)
    item = monitor.tree.topLevelItem(0).child(0).child(0)
    assert [item.text(c) for c in range(10, 13)] == ["250.0us", "0ns", "12.0us"]
    assert table_texts(monitor.threads)[0][-3] == "7.0us"
    assert table_texts(monitor.topics)[0][-2:] == ["-", "20.0us"]
    assert table_texts(monitor.gpus) == [["0", "Orin", "-", "-/-", "918MHz", "-", "-", ""]]
    assert table_texts(monitor.cuda)[0][:2] == ["4021", "camera_node"]
    assert table_texts(monitor.cuda)[0][7] == "512.0MiB"
    assert monitor.host.cpu.text() == "cpu MHz: min 1200 mean 1700 max 2200 (2 cpus)"
    assert monitor.host.udp.text() == "udp: -"
    assert table_texts(monitor.host.thermal) == [["cpu-thermal", "61.5"]]
    assert table_texts(monitor.spans)[0][:4] == ["preprocess", "-", "sub(/scan)", "/camera/driver"]
    assert table_texts(monitor.chains) == [
        ["perception", "lineage", "10", "20.0ms", "20.0ms", "31.0ms", "31.0ms", "/tracks:31.0ms", "0"]
    ]  # fmt: skip
    tabs = [monitor.tabText(i) for i in range(monitor.count())]
    assert tabs[5:9] == ["GPU", "Host", "Spans", "Chains"]
