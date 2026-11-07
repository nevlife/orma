import contextlib
import io
import json
import os
import shlex
import signal
import subprocess
import sys

import orma_client
import pytest
from orma_cli import cli
from orma_cli.cli import main, print_process_warnings
from conftest import FAKE_SERVER, GOLDEN, orma, golden, wait_for

STATE_RECORD = "r-20261008-153012-1"
ALL_OPTIONS = [
    "--method", "preload",
    "--pid", "4021",
    "--kind", "events",
    "-o", "/home/u/rec",
    "--name", "run1",
    "--max-duration", "600",
    "--max-size", "4294967296",
    "--split-size", "1073741824",
    "--split-duration", "60",
    "--cache", "16777216",
    "--compression", "zstd",
    "--bandwidth",
    "--no-sched",
    "--malloc",
]  # fmt: skip
COMMANDS = [
    (["record", "--no-follow"], ["record_start_default"]),
    (["record", "--no-follow", "--kind", "windows", "--max-duration", "86400"], ["record_start_windows"]),
    (["record", "--no-follow"] + ALL_OPTIONS, ["record_start_all"]),
    (["record", "list"], ["record_list"]),
    (["record", "stop", STATE_RECORD], ["record_stop"]),
    (["record", "show", STATE_RECORD], ["record_show_id"]),
    (["record", "show", "/home/u/.local/share/orma/records/orma-20261008-153012"], ["record_show_dir"]),
    (["export", "/home/u/rec/run1", "-o", "/home/u/run1.csv", "--metric", "callbacks", "--metric", "topics"], ["export"]),
    (["check", "--duration", "15"], ["selfcheck"]),
]  # fmt: skip


def without_id(message):
    return {k: v for k, v in message.items() if k != "id"}


def without_regexes(message):
    """The golden without node, topic and exclude, which orma refuses until they are applied."""
    targets = message.get("targets")
    if targets:
        message["targets"] = {k: v for k, v in targets.items() if k in ("all", "pids")}
    return message


def check_session(requests, names, pid, client="orma"):
    """hello, the named goldens, bye: each equal to its golden but for `id`, `client`, `pid`."""
    hello = dict(golden("hello"), client=client, pid=pid, proto=orma_client.PROTO)
    expected = [hello] + [without_regexes(golden(n)) for n in names] + [golden("bye")]
    assert [without_id(r) for r in requests] == [without_id(e) for e in expected]


def test_frame_matches_golden():
    hello = {"id": 1, "op": "hello", "proto": [1, 7], "client": "rqt_orma", "pid": 5555}
    assert orma_client.encode(hello) == (GOLDEN / "frames" / "hello.bin").read_bytes()


@pytest.mark.parametrize("option", ["node", "topic", "exclude"])
def test_name_targets_are_sent(fake, option):
    assert main(["record", "--no-follow", f"--{option}", "/x"]) == 0
    start = next(r for r in fake.requests() if r["op"] == "record_start")
    assert start["targets"] == {"all": False, "pids": [], option: "/x"}


@pytest.mark.parametrize("argv,names", COMMANDS, ids=lambda x: "_".join(x)[:40])
def test_request_matches_golden(fake, argv, names, capsys):
    assert main(argv) == (1 if argv[0] == "check" else 0)
    # The fake server logs the bye after the CLI has already returned.
    wait_for(lambda: fake.requests()[-1]["op"] == "bye")
    check_session(fake.requests(), names, os.getpid())
    assert capsys.readouterr().out


def lines_of(*argv):
    """Lines up to the golden's last event line, then Ctrl-C."""
    process = subprocess.Popen(orma(*argv), stdout=subprocess.PIPE, text=True)
    lines = [process.stdout.readline()]
    while not lines[-1].startswith("[warn] probe_missing"):
        assert lines[-1]
        lines.append(process.stdout.readline())
    process.send_signal(signal.SIGINT)
    assert process.wait(10) == 0
    return lines


def test_node_cb_selects_the_node(fake, env):
    argv = ["--pid", "4021", "--method", "preload", "--window", "0.5", "--gpu"]
    lines = lines_of("node", "cb", "/camera/driver", *argv)
    assert "window 381  1s\n" in lines and "callbacks\n" in lines
    requests = [without_id(r) for r in fake.requests()]
    assert requests[1:] == [
        without_id(golden("subscribe")),
        {"op": "configure", "window_s": 0.5, "gpu": True},
        {
            "op": "select",
            "all": False,
            "pids": [4021],
            "method_by_pid": {"4021": "preload"},
            "node": "^(/camera/driver)$",
        },
        {"op": "bye"},
    ]


@pytest.mark.parametrize(
    "argv,title",
    [
        (["node", "cpu"], "nodes"),
        (["node", "cb", "/camera/driver"], "callbacks"),
        (["node", "sub", "/camera/driver"], "subscriptions"),
    ],
)
def test_node_commands_print_their_table(fake, env, argv, title):
    titles = {line.strip() for line in lines_of(*argv)} & {"nodes", "callbacks", "subscriptions"}
    assert titles == {title}


def test_node_cpu_shows_rss_and_warns_of_overflow(fake, env):
    lines = lines_of("node", "cpu")
    assert any(
        line.split()[:1] == ["/camera/driver"] and line.endswith("100.0MiB\n") for line in lines
    )
    assert [line for line in lines if line.startswith("warning:")] == [
        "warning: pid 4021 (camera_node): 3 samples not aggregated (thread table full)\n",
        "warning: pid 5100 (listener): flux GPU wedged (CUresult 700); its GPU channels refuse "
        "claims, loans and takes\n",
    ]


def test_node_cb_and_sub_filter(fake, env):
    assert any("sub(/scan)" in line for line in lines_of("node", "cb", "/camera/driver", "scan"))
    assert "  none\n" in lines_of("node", "cb", "/camera/driver", "nothing")
    assert "  none\n" in lines_of("node", "sub", "/camera/driver", "/other")
    assert "  none\n" in lines_of("node", "cb", "/elsewhere")


def test_node_list_and_info(fake, env, capsys):
    assert main(["node", "list"]) == 0
    assert capsys.readouterr().out.splitlines() == ["/camera/driver"]
    assert main(["node", "info", "/camera/driver"]) == 0
    assert capsys.readouterr().out.splitlines() == [
        "/camera/driver",
        "  pid 4021 (camera_node), rss 100.0MiB",
        "  publishers: 0",
        "  subscriptions: 2",
        "    /image",
        "    /scan",
        "  callbacks: 1",
        "    sub(/scan)",
    ]
    assert main(["node", "info", "/none"]) == 1


def test_span_says_none_without_spans(fake, env):
    assert "  none\n" in lines_of("span")


def test_top_is_the_host(fake, env):
    assert "  none\n" in lines_of("top")
    requests = [without_id(r) for r in fake.requests()]
    assert requests[1:] == [{"op": "subscribe", "streams": ["window", "event_line"]}, {"op": "bye"}]


@pytest.mark.parametrize("argv", [["node"], ["cb"], ["--chain", "a=/x,/y"], ["--gpu"]])
def test_top_takes_no_view(argv):
    with pytest.raises(SystemExit):
        main(["top", *argv])


@pytest.mark.parametrize(
    "overflow,expected",
    [
        (None, []),
        ({"threads": 0, "table": 0, "depth": 0}, []),
        (
            {"threads": 2, "table": 0, "depth": 5},
            [
                "warning: pid 4021 (camera_node): 2 threads not traced (thread slots full)"
                ", 5 callbacks not timed (nested deeper than 16)"
            ],
        ),
    ],
)
def test_overflow_warning_only_when_nonzero(overflow, expected, capsys):
    window = json.loads((GOLDEN / "server" / "window.json").read_text())
    window["processes"][0]["overflow"] = overflow
    print_process_warnings(window)
    out = capsys.readouterr().out.splitlines()
    assert [line for line in out if line.startswith("warning: pid 4021")] == expected


def test_top_json(fake, env):
    window = json.loads((GOLDEN / "server" / "window.json").read_text())
    process = subprocess.Popen(orma("top", "--json"), stdout=subprocess.PIPE, text=True)
    line = process.stdout.readline()
    process.send_signal(signal.SIGINT)
    assert process.wait(10) == 0
    assert json.loads(line) == window


def start_record(fake):
    process = subprocess.Popen(orma("record"), stdout=subprocess.PIPE, text=True)
    assert process.stdout.readline() == "recording r-fake-1 in /tmp/r-fake-1\n"
    assert process.stdout.readline().startswith("r-fake-1 0s 0B events")
    return process


def recording_states(capsys):
    assert main(["record", "list"]) == 0
    rows = capsys.readouterr().out.splitlines()
    return {row.split()[0]: row.split()[1] for row in rows[2:]}


def test_record_ctrl_c_stops(fake, env, capsys):
    process = start_record(fake)
    process.send_signal(signal.SIGINT)
    out, _ = process.communicate(timeout=10)
    assert process.returncode == 0
    assert out.startswith("stopping r-fake-1\nr-fake-1 ended: user_stop\n")
    ops = [without_id(r) for r in fake.requests()]
    assert ops[1:] == [
        {"op": "record_start"},
        {"op": "subscribe", "streams": ["state", "window"]},
        {"op": "record_stop", "record_id": "r-fake-1"},
        {"op": "record_show", "record_id": "r-fake-1"},
        {"op": "bye"},
    ]
    assert recording_states(capsys)["r-fake-1"] == "finalized"


@pytest.mark.parametrize("sig", [signal.SIGHUP, signal.SIGTERM])
def test_record_hangup_keeps_recording(fake, env, sig, capsys):
    process = start_record(fake)
    process.send_signal(sig)
    assert process.wait(10) == -sig
    assert "record_stop" not in [r["op"] for r in fake.requests()]
    assert recording_states(capsys)["r-fake-1"] == "recording"


def exports(argv):
    out = subprocess.run(orma(*argv), capture_output=True, text=True, check=True).stdout
    return dict(shlex.split(line)[1].split("=", 1) for line in out.splitlines())


def test_env(env, prefix, runtime, monkeypatch):
    from ament_index_python.packages import get_package_share_directory

    autoload = os.path.join(get_package_share_directory("orma_rclpy"), "autoload")
    assert os.path.isfile(os.path.join(autoload, "usercustomize.py"))
    monkeypatch.setenv("LD_PRELOAD", "/opt/other.so")
    python_path = env["PYTHONPATH"]
    assert exports(["env"]) == {
        "LD_PRELOAD": f"{prefix}/lib/liborma_preload.so:/opt/other.so",
        "PYTHONPATH": f"{autoload}:{python_path}",
    }
    assert exports(["env", "--method", "lttng"]) == {
        "LD_PRELOAD": f"{prefix}/lib/liborma_lttng.so:/opt/other.so",
        "PYTHONPATH": f"{autoload}:{python_path}",
        "LTTNG_HOME": f"{runtime}/orma/lttng",
    }


PROBE = (
    "import json, os, sys; "
    "print(json.dumps({'env': {k: os.environ.get(k) for k in ('LD_PRELOAD', 'LTTNG_HOME')}, "
    "'path0': sys.path[1], 'orma_rclpy': 'orma_rclpy' in sys.modules, "
    "'mapped': [l.split()[-1] for l in open('/proc/self/maps') if 'liborma_' in l][:1]}))"
)


@pytest.mark.parametrize("method", ["preload", "lttng"])
def test_run(env, prefix, method):
    out = subprocess.run(
        orma("run", "--method", method, "--", sys.executable, "-c", PROBE),
        capture_output=True,
        text=True,
        check=True,
    ).stdout
    seen = json.loads(out)
    library = f"{prefix}/lib/liborma_{method}.so"
    assert seen["env"]["LD_PRELOAD"] == library
    assert (seen["env"]["LTTNG_HOME"] is not None) == (method == "lttng")
    assert seen["path0"].endswith("share/orma_rclpy/autoload")
    assert seen["orma_rclpy"]
    assert seen["mapped"] == [library]


def write_script(path, text):
    path.write_text("#!/bin/sh\n" + text)
    path.chmod(0o755)


def test_starts_collectd(env, runtime, monkeypatch):
    tools = runtime / "bin"
    tools.mkdir()
    write_script(tools / "systemctl", "echo not-found\n")
    socket_path = runtime / "orma" / "collectd.sock"
    write_script(
        tools / "orma-collectd",
        f'echo $$ > {runtime}/pid\nexec "{os.path.abspath(FAKE_SERVER)}" {socket_path} '
        f"> {runtime}/fake.log\n",
    )
    monkeypatch.setenv("PATH", f"{tools}:{os.environ['PATH']}")
    status = subprocess.run(orma("status"), capture_output=True, text=True)
    assert status.returncode == 1 and "orma-collectd is not running" in status.stderr
    assert not (runtime / "pid").exists()

    subprocess.run(orma("record", "list"), check=True, capture_output=True)
    pid = int((runtime / "pid").read_text())
    try:
        stat = open(f"/proc/{pid}/stat").read().rsplit(")", 1)[1].split()
        ppid, session = int(stat[1]), int(stat[3])
        assert session == pid and ppid != os.getpid()
        wait_for(lambda: len((runtime / "fake.log").read_text().splitlines()) == 3)
        ops = [r["op"] for r in map(json.loads, (runtime / "fake.log").read_text().splitlines())]
        assert ops == ["hello", "record_list", "bye"]
    finally:
        os.kill(pid, signal.SIGKILL)


def test_analyze_runs_orma_analyze(env, runtime, monkeypatch):
    tools = runtime / "bin"
    tools.mkdir()
    write_script(tools / "orma-analyze", f'echo "$@" > {runtime}/args\n')
    monkeypatch.setenv("PATH", f"{tools}:{os.environ['PATH']}")
    subprocess.run(orma("analyze", "rec/run1"), check=True, cwd=runtime)
    assert (runtime / "args").read_text() == f"{runtime}/rec/run1\n"


def lat_ns(p50, p99, p999=None, top=None):
    p999 = p999 or p99
    return {
        "n": 4,
        "mean_ns": p50,
        "p50_ns": p50,
        "p90_ns": p50,
        "p99_ns": p99,
        "p999_ns": p999,
        "max_ns": top or p999,
    }


def window_1_8():
    """The golden window with the fields of protocol 1.8."""
    w = json.loads((GOLDEN / "server" / "window.json").read_text())
    callback = w["callbacks"][0]["id"]
    w["callbacks"][0]["cuda"] = {
        "calls": 30,
        "launches": 20,
        "memcpy_bytes": 1 << 20,
        "api_mean_ns": 250_000,
        "sync_mean_ns": 180_000,
    }
    w["callbacks"][0]["futex_mean_ns"] = 12_000
    w["topics"][0]["pub_to_dds"] = lat_ns(3_000, 9_000)
    w["topics"][0]["dds_write"] = lat_ns(5_000, 20_000)
    w["processes"][0]["cuda"] = {
        "calls_hz": 300.0,
        "launch_hz": 200.0,
        "memcpy_bps": 10485760.0,
        "alloc_hz": 0.0,
        "errors": 0,
        "sync": lat_ns(100_000, 400_000),
    }
    w["processes"][0]["gpu_mem_bytes"] = 512 << 20
    w["spans"] = [
        {
            "id": "s:4021:771234:1",
            "name": "preprocess",
            "pid": 4021,
            "node": w["callbacks"][0]["node"],
            "callback": callback,
            "parent": None,
            "hz": 10.0,
            "duration": lat_ns(800_000, 1_200_000),
        },
        {
            "id": "s:4021:771234:2",
            "name": "resize",
            "pid": 4021,
            "node": w["callbacks"][0]["node"],
            "callback": callback,
            "parent": "s:4021:771234:1",
            "hz": 10.0,
            "duration": lat_ns(300_000, 500_000),
        },
    ]
    w["chains"] = [
        {
            "name": "perception",
            "mode": "lineage",
            "topics": ["/scan", "/detections", "/tracks"],
            "count": 10,
            "e2e": lat_ns(20_000_000, 31_000_000, 33_000_000, 35_000_000),
            "steps": [lat_ns(9_000_000, 12_000_000), None],
            "broken": 1,
        }
    ]
    w["host"] = {
        "cpu_mhz": [1200, 2200, None, 1700],
        "thermal": [{"name": "cpu-thermal", "temp_c": 61.5}],
        "throttle": {"package": 3},
        "net": [
            {
                "name": "eth0",
                "rx_bps": 2048.0,
                "tx_bps": 1048576.0,
                "rx_pps": 10.0,
                "tx_pps": 800.0,
                "rx_drops": 0,
                "tx_drops": 2,
                "rx_errors": 0,
                "tx_errors": 0,
            }
        ],
        "udp": {"in_errors": 1, "rcvbuf_errors": 1, "sndbuf_errors": 0, "no_ports": 4},
        "gpus": [
            {
                "index": 0,
                "name": "Orin",
                "util_pct": 45.0,
                "mem_used_bytes": 1 << 30,
                "mem_total_bytes": 8 << 30,
                "clock_mhz": 918,
                "temp_c": 52.0,
                "power_w": 12.25,
                "throttle": ["sw_power_cap"],
            }
        ],
    }
    return w


def printed(printer, w):
    out = io.StringIO()
    with contextlib.redirect_stdout(out):
        printer(w)
    return [line.split() for line in out.getvalue().splitlines()]


def test_host_view():
    rows = printed(cli.print_host, window_1_8())
    assert rows[0] == ["cpu", "MHz:", "min", "1200", "mean", "1700", "max", "2200", "(4", "cpus)"]
    assert ["cpu-thermal", "61.5"] in rows and ["package", "3"] in rows
    assert ["eth0", "2.0KiB/s", "1.0MiB/s", "10.0", "800.0", "0", "2", "0", "0"] in rows
    assert "udp: in errors 1, rcvbuf errors 1, sndbuf errors 0, no ports 4".split() in rows
    assert rows[-3:] == [
        ["gpus"],
        ["gpu", "name", "util%", "mem", "clock", "temp", "power", "throttle"],
        ["0", "Orin", "45.0", "1.0GiB/8.0GiB", "918MHz", "52C", "12.2W", "sw_power_cap"],
    ]
    assert (
        printed(cli.print_host, json.loads((GOLDEN / "server" / "window.json").read_text())) == []
    )


def test_span_view():
    w = window_1_8()
    assert printed(cli.print_spans, w)[2:] == [
        ["preprocess", "-", "sub(/scan)", "/camera/driver", "4021", "10.0"]
        + ["800.0us", "800.0us", "1.2ms", "1.2ms"],
        ["resize", "preprocess", "sub(/scan)", "/camera/driver", "4021", "10.0"]
        + ["300.0us", "300.0us", "500.0us", "500.0us"],
    ]


def test_one_span_name_at_two_call_sites_gets_the_key():
    spans = [
        {"id": f"span:1:0:step@{key}#c#", "name": "step", "pid": 1, "callback": "c", "parent": None}
        for key in ("7f00a1b2c3", "7f00d4e5f6")
    ]
    spans.append(dict(spans[0], id="span:2:0:step@7f00a1b2c3#c#", pid=2))
    assert list(cli.span_names(spans).values()) == ["step #a1b2c3", "step #d4e5f6", "step"]


def test_cb_view_shows_cuda_and_futex_only_when_present():
    w = window_1_8()
    head, row = printed(cli.print_callbacks, w)[1:3]
    assert head[-7:] == ["dispatch", "p99", "cuda", "api", "cuda", "sync", "futex"]
    assert row[-3:] == ["250.0us", "180.0us", "12.0us"]
    plain = printed(
        cli.print_callbacks, json.loads((GOLDEN / "server" / "window.json").read_text())
    )
    assert "cuda" not in plain[1] and "futex" not in plain[1]
    out = io.StringIO()
    with contextlib.redirect_stdout(out):
        cli.print_topic_window(w, "delay", [w["topics"][0]["name"]])
    assert out.getvalue().splitlines()[2].split()[-2:] == ["9.0us", "20.0us"]


def test_topic_path_shows_each_path_and_the_subscription_segments():
    w = json.loads((GOLDEN / "server" / "window.json").read_text())
    t = w["topics"][0]
    t["paths"] = [
        {"path": "broker", "takes": 5, "hop": lat_ns(2_000, 4_000)},
        {"path": "shm", "takes": 5, "hop": lat_ns(300_000, 700_000)},
    ]
    t["loan_hold"] = lat_ns(1_000, 3_000)
    s = next(s for s in w["subscriptions"] if s["topic"] == t["id"])
    s.update(paths=t["paths"], loaned=5, deliver=lat_ns(40_000, 90_000))
    s.update(py_convert=lat_ns(20_000, 30_000), hold=lat_ns(80_000, 100_000))
    rows = printed(lambda w: cli.print_topic_paths(w, [t["name"]]), w)
    secs = w["window_ns"] / 1e9
    hz = f"{5 / secs:.1f}"
    assert [t["name"], "broker", hz, "2.0us", "2.0us", "4.0us", "4.0us"] in rows
    assert [t["name"], "shm", hz, "300.0us", "300.0us", "700.0us", "700.0us"] in rows
    assert [t["name"], "3.0us", "-"] in rows
    sub = next(r for r in rows if r[:1] == [t["name"]] and "broker," in r)
    assert sub[-6:] == ["shm", "5", "40.0us", "90.0us", "30.0us", "100.0us"]


@pytest.mark.parametrize(
    "text,chain",
    [
        ("a=/x,/y", {"name": "a", "topics": ["/x", "/y"], "mode": "lineage"}),
        ("a=/x,/y,/z:first", {"name": "a", "topics": ["/x", "/y", "/z"], "mode": "first"}),
        ("a=/x,/goal_first", {"name": "a", "topics": ["/x", "/goal_first"], "mode": "lineage"}),
    ],
)
def test_parse_chain(text, chain):
    assert orma_client.parse_chain(text) == chain
    assert orma_client.parse_chain(orma_client.format_chain(chain)) == chain


@pytest.mark.parametrize("text", ["a", "a=/x", "=/x,/y", "a=/x,/y:last"])
def test_parse_chain_refuses(text):
    with pytest.raises(orma_client.OrmaError):
        orma_client.parse_chain(text)


def test_record_sends_the_new_options(fake):
    argv = ["record", "--no-follow", "--gpu", "--dds", "--futex", "--latest-node", "/fusion"]
    argv += ["--chain", "reaction=/obstacle,/trajectory,/control_cmd:first", "--chain", "p=/a,/b"]
    assert main(argv) == 0
    start = next(without_id(r) for r in fake.requests() if r["op"] == "record_start")
    assert start == {
        "op": "record_start",
        "gpu": True,
        "dds": True,
        "futex": True,
        "chains": [
            {"name": "reaction", "topics": ["/obstacle", "/trajectory", "/control_cmd"], "mode": "first"},
            {"name": "p", "topics": ["/a", "/b"], "mode": "lineage"},
        ],
        "latest_nodes": ["/fusion"],
    }  # fmt: skip


def test_bad_chain_is_a_usage_error(capsys):
    with pytest.raises(SystemExit):
        main(["record", "--chain", "x=/a"])
    assert "give NAME=TOPIC,TOPIC" in capsys.readouterr().err


def test_status_shows_options_and_chains(fake, capsys):
    assert main(["status"]) == 0
    out = capsys.readouterr().out.splitlines()
    assert out[2].endswith("selection 4021:preload, sched, gpu, futex")
    assert out[3:5] == [
        "chain: perception=/camera/image,/detections,/tracks",
        "latest-input nodes: /fusion",
    ]
    assert out[-3] == "executor probes: 4021 (camera_node) stock"
    assert out[-2] == "delivery probes: 4021 (camera_node) loan+zenoh_shm"
    assert out[-1].startswith("orma: collectd cpu ")


def test_export_metrics(fake):
    argv = ["export", "/r", "-o", "/o.csv", "--metric", "spans", "--metric", "chains"]
    assert main(argv + ["--metric", "host"]) == 0
    export = next(r for r in fake.requests() if r["op"] == "export")
    assert export["metrics"] == ["spans", "chains", "host"]


def test_analyze_passes_chains(env, runtime, monkeypatch):
    tools = runtime / "bin"
    tools.mkdir()
    write_script(tools / "orma-analyze", f'echo "$@" > {runtime}/args\n')
    monkeypatch.setenv("PATH", f"{tools}:{os.environ['PATH']}")
    argv = ["analyze", "rec", "--chain", "a=/x,/y:first", "--chain", "b=/p,/q"]
    subprocess.run(orma(*argv), check=True, cwd=runtime)
    args = (runtime / "args").read_text()
    assert args == f"{runtime}/rec --chain a=/x,/y:first --chain b=/p,/q\n"


def topic_lines(*argv):
    return [line.split() for line in lines_of("topic", *argv)]


def test_topic_hz(fake, env):
    rows = topic_lines("hz", "/scan", "/none", "--window", "0.5")
    assert ["/scan", "10.0", "10.0", "0"] in rows and ["/none", "-", "-", "-"] in rows
    requests = [without_id(r) for r in fake.requests()]
    assert requests[1:4] == [
        without_id(golden("subscribe")),
        {"op": "configure", "window_s": 0.5},
        {"op": "select", "all": False, "pids": [], "method_by_pid": {}, "topic": "^(/scan|/none)$"},
    ]


def test_topic_bw_and_delay(fake, env):
    assert ["/image", "178.0MiB/s"] in topic_lines("bw", "/image")
    assert ["topic", "hop", "p50", "hop", "p90", "hop", "p99", "hop", "max"] in topic_lines(
        "delay", "/scan"
    )


def test_path_names_what_it_did_not_find(fake, env, capsys):
    assert main(["path", "/camera/driver", "/nosuch"]) != 0
    assert "no path from /camera/driver to /nosuch" in capsys.readouterr().err


def test_topic_list(fake, env, capsys):
    assert main(["topic", "list"]) == 0
    assert capsys.readouterr().out.splitlines() == ["/image", "/scan"]


def test_topic_info(fake, env, capsys):
    assert main(["topic", "info", "/scan"]) == 0
    assert capsys.readouterr().out.splitlines() == [
        "/scan  transport ros",
        "publishers: 1",
        "  unlinked",
        "subscribers: 1",
        f"  {cli.view.owner_label(json.loads((GOLDEN / 'server' / 'window.json').read_text()), 'n:4021:771234:5581e0a0')}",
    ]
    assert main(["topic", "info", "/none"]) == 1
