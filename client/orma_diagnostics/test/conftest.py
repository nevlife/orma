import json
import sys
from pathlib import Path

HERE = Path(__file__).resolve().parent
PACKAGE = HERE.parent
GOLDEN = PACKAGE.parents[1] / "common" / "orma_proto" / "golden"

sys.path.insert(0, str(PACKAGE))


def window():
    """The golden window with a chain, a GPU and a thermal zone."""
    w = json.loads((GOLDEN / "server" / "window.json").read_text())
    e2e = {
        "n": 9,
        "mean_ns": 1,
        "p50_ns": 1,
        "p90_ns": 1,
        "p99_ns": 60_000_000,
        "p999_ns": 1,
        "max_ns": 1,
    }
    w["chains"] = [
        {
            "name": "perception",
            "mode": "first",
            "topics": ["/scan", "/tracks"],
            "count": 9,
            "e2e": e2e,
            "steps": [e2e],
            "broken": 0,
        }
    ]
    gpu = {"index": 0, "name": "Orin", "util_pct": None, "mem_used_bytes": None}
    gpu.update(mem_total_bytes=None, clock_mhz=None, temp_c=None, power_w=None, throttle=[])
    w["host"] = {
        "cpu_mhz": [],
        "thermal": [
            {"name": "cpu-thermal", "temp_c": 96.0},
            {"name": "gpu-thermal", "temp_c": 40.0},
        ],
        "throttle": {},
        "net": [],
        "udp": None,
        "gpus": [gpu],
    }
    return w
