import json
import os
import shlex
import signal
import subprocess
import sys

import armo_client
import pytest
from armo_cli.cli import main, print_processes
from conftest import FAKE_SERVER, GOLDEN, armo, golden, wait_for

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
    (["record", "show", "/home/u/.local/share/armo/records/armo-20261008-153012"], ["record_show_dir"]),
    (["export", "/home/u/rec/run1", "-o", "/home/u/run1.csv", "--metric", "callbacks", "--metric", "topics"], ["export"]),
    (["check", "--duration", "15"], ["selfcheck"]),
]  # fmt: skip


def without_id(message):
    return {k: v for k, v in message.items() if k != "id"}


def without_regexes(message):
    """The golden without node, topic and exclude, which armo refuses until they are applied."""
    targets = message.get("targets")
    if targets:
        message["targets"] = {k: v for k, v in targets.items() if k in ("all", "pids")}
    return message


def check_session(requests, names, pid, client="armo"):
    """hello, the named goldens, bye: each equal to its golden but for `id`, `client`, `pid`."""
    hello = dict(golden("hello"), client=client, pid=pid)
    expected = [hello] + [without_regexes(golden(n)) for n in names] + [golden("bye")]
    assert [without_id(r) for r in requests] == [without_id(e) for e in expected]


def test_frame_matches_golden():
    hello = {"id": 1, "op": "hello", "proto": [1, 6], "client": "rqt_armo", "pid": 5555}
    assert armo_client.encode(hello) == (GOLDEN / "frames" / "hello.bin").read_bytes()


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


@pytest.mark.parametrize(
    "argv,expected",
    [
        ([], [golden("subscribe"), golden("select_all")]),
        (
            ["--pid", "4021", "--method", "preload", "--pid", "4022", "--method", "uprobe"],
            [golden("subscribe"), golden("select_pids")],
        ),
        (
            ["--window", "0.5", "--pid", "4021", "--method", "preload"],
            [
                golden("subscribe"),
                {"op": "configure", "window_s": 0.5},
                {
                    "op": "select",
                    "all": False,
                    "pids": [4021],
                    "method_by_pid": {"4021": "preload"},
                },
            ],
        ),
    ],
)
def test_top(fake, env, argv, expected):
    lines = top_lines("cb", *argv)
    assert "window 381  1s\n" in lines and "callbacks\n" in lines
    requests = [without_id(r) for r in fake.requests()]
    assert requests[1:] == [without_id(e) for e in expected] + [{"op": "bye"}]


def top_lines(*argv):
    """Lines up to the golden's last event line, then Ctrl-C."""
    process = subprocess.Popen(armo("top", *argv), stdout=subprocess.PIPE, text=True)
    lines = [process.stdout.readline()]
    while not lines[-1].startswith("[warn] probe_missing"):
        assert lines[-1]
        lines.append(process.stdout.readline())
    process.send_signal(signal.SIGINT)
    assert process.wait(10) == 0
    return lines


TITLES = {
    "proc": "processes",
    "node": "nodes",
    "cb": "callbacks",
    "topic": "topics",
    "sub": "subscriptions",
}


@pytest.mark.parametrize("view", sorted(TITLES))
def test_top_prints_only_its_view(fake, env, view):
    titles = {line.strip() for line in top_lines(view)} & set(TITLES.values())
    assert titles == {TITLES[view]}


def test_top_proc_warns_of_overflow(fake, env):
    warnings = [line for line in top_lines("proc") if line.startswith("warning:")]
    assert warnings == [
        "warning: pid 4021 (camera_node): 3 samples not aggregated (thread table full)\n"
    ]


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
    print_processes(window)
    out = capsys.readouterr().out.splitlines()
    assert [line for line in out if line.startswith("warning:")] == expected


def test_top_needs_a_view(fake, env, capsys):
    assert main(["top"]) == 1
    assert "needs a view" in capsys.readouterr().err


def top_json(*argv):
    process = subprocess.Popen(armo("top", "--json", *argv), stdout=subprocess.PIPE, text=True)
    line = process.stdout.readline()
    process.send_signal(signal.SIGINT)
    assert process.wait(10) == 0
    return json.loads(line)


def test_top_json(fake, env):
    window = json.loads((GOLDEN.parent / "golden/server/window.json").read_text())
    assert top_json() == window
    view = {"seq": window["seq"], "window_ns": window["window_ns"], "topics": window["topics"]}
    assert top_json("topic") == view


def start_record(fake):
    process = subprocess.Popen(armo("record"), stdout=subprocess.PIPE, text=True)
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
    out = subprocess.run(armo(*argv), capture_output=True, text=True, check=True).stdout
    return dict(shlex.split(line)[1].split("=", 1) for line in out.splitlines())


def test_env(env, prefix, runtime, monkeypatch):
    from ament_index_python.packages import get_package_share_directory

    autoload = os.path.join(get_package_share_directory("armo_rclpy"), "autoload")
    assert os.path.isfile(os.path.join(autoload, "usercustomize.py"))
    monkeypatch.setenv("LD_PRELOAD", "/opt/other.so")
    python_path = env["PYTHONPATH"]
    assert exports(["env"]) == {
        "LD_PRELOAD": f"{prefix}/lib/libarmo_preload.so:/opt/other.so",
        "PYTHONPATH": f"{autoload}:{python_path}",
    }
    assert exports(["env", "--method", "lttng"]) == {
        "LD_PRELOAD": f"{prefix}/lib/libarmo_lttng.so:/opt/other.so",
        "PYTHONPATH": f"{autoload}:{python_path}",
        "LTTNG_HOME": f"{runtime}/armo/lttng",
    }


PROBE = (
    "import json, os, sys; "
    "print(json.dumps({'env': {k: os.environ.get(k) for k in ('LD_PRELOAD', 'LTTNG_HOME')}, "
    "'path0': sys.path[1], 'armo_rclpy': 'armo_rclpy' in sys.modules, "
    "'mapped': [l.split()[-1] for l in open('/proc/self/maps') if 'libarmo_' in l][:1]}))"
)


@pytest.mark.parametrize("method", ["preload", "lttng"])
def test_run(env, prefix, method):
    out = subprocess.run(
        armo("run", "--method", method, "--", sys.executable, "-c", PROBE),
        capture_output=True,
        text=True,
        check=True,
    ).stdout
    seen = json.loads(out)
    library = f"{prefix}/lib/libarmo_{method}.so"
    assert seen["env"]["LD_PRELOAD"] == library
    assert (seen["env"]["LTTNG_HOME"] is not None) == (method == "lttng")
    assert seen["path0"].endswith("share/armo_rclpy/autoload")
    assert seen["armo_rclpy"]
    assert seen["mapped"] == [library]


def write_script(path, text):
    path.write_text("#!/bin/sh\n" + text)
    path.chmod(0o755)


def test_starts_collectd(env, runtime, monkeypatch):
    tools = runtime / "bin"
    tools.mkdir()
    write_script(tools / "systemctl", "echo not-found\n")
    socket_path = runtime / "armo" / "collectd.sock"
    write_script(
        tools / "armo-collectd",
        f'echo $$ > {runtime}/pid\nexec "{os.path.abspath(FAKE_SERVER)}" {socket_path} '
        f"> {runtime}/fake.log\n",
    )
    monkeypatch.setenv("PATH", f"{tools}:{os.environ['PATH']}")
    status = subprocess.run(armo("status"), capture_output=True, text=True)
    assert status.returncode == 1 and "armo-collectd is not running" in status.stderr
    assert not (runtime / "pid").exists()

    subprocess.run(armo("record", "list"), check=True, capture_output=True)
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


def test_analyze_runs_armo_analyze(env, runtime, monkeypatch):
    tools = runtime / "bin"
    tools.mkdir()
    write_script(tools / "armo-analyze", f'echo "$@" > {runtime}/args\n')
    monkeypatch.setenv("PATH", f"{tools}:{os.environ['PATH']}")
    subprocess.run(armo("analyze", "rec/run1"), check=True, cwd=runtime)
    assert (runtime / "args").read_text() == f"{runtime}/rec/run1\n"
