import json
import shutil
import subprocess

import orma_client
import pytest
from conftest import HERE

CASES = [
    ("preload", True, True),
    ("lttng", True, False),
    ("uprobe", True, True),
    ("preload", False, True),
]


@pytest.mark.parametrize("method,record,stop", CASES)
def test_orma_trace(fake, env, prefix, runtime, monkeypatch, method, record, stop):
    out = runtime / "seen.json"
    monkeypatch.setenv("ORMA_TEST_METHOD", method)
    monkeypatch.setenv("ORMA_TEST_RECORD", "1" if record else "0")
    monkeypatch.setenv("ORMA_TEST_STOP", "1" if stop else "0")
    monkeypatch.setenv("ORMA_TEST_OUT", str(out))
    launch = [shutil.which("ros2"), "launch", str(HERE / "trace.launch.py")]
    subprocess.run(launch, check=True, timeout=60, capture_output=True)

    seen = json.loads(out.read_text())
    library = f"{prefix}/lib/liborma_{method}.so"
    instrumented = method != "uprobe"
    assert seen["env"]["LD_PRELOAD"] == (library if instrumented else None)
    assert (seen["env"]["LTTNG_HOME"] == f"{runtime}/orma/lttng") == (method == "lttng")
    assert seen["path0"].endswith("share/orma_rclpy/autoload") == instrumented
    assert seen["orma_rclpy"] == instrumented
    assert seen["mapped"] == ([library] if instrumented else [])

    requests = [{k: v for k, v in r.items() if k not in ("id", "pid")} for r in fake.requests()]
    hello = {"op": "hello", "proto": orma_client.PROTO, "client": "orma_launch"}
    start = {
        "op": "record_start",
        "method": method,
        "kind": "windows",
    }
    stopped = [hello, {"op": "record_stop", "record_id": "r-fake-1"}, {"op": "bye"}]
    expected = [hello, start, {"op": "bye"}] + (stopped if stop else [])
    assert requests == (expected if record else [])
