import pytest
from orma_diagnostics import rules
from conftest import PACKAGE, window

RULES = """
callback_p99_us:
  - {match: "sub\\\\(/scan\\\\)", warn: 1000, error: 5000}
  - {match: "timer.*", warn: 1, error: 2}
topic_hz_min:
  - {match: /scan, warn: 11, error: 5}
  - {match: /image, warn: 40, error: 35}
topic_hop_p99_us:
  - {match: /scan, warn: 1000, error: 2000}
sub_loss_pct:
  - {match: /scan, warn: 1, error: 5}
chain_e2e_p99_us:
  - {match: perception, warn: 50000, error: 100000}
  - {match: planning, warn: 1, error: 2}
gpu_temp_c:
  - {match: "0", warn: 80, error: 90}
thermal_c:
  - {match: .*-thermal, warn: 85, error: 95}
"""


def levels(statuses):
    return [(s["name"], s["level"], s["message"]) for s in statuses]


def test_evaluate():
    statuses = rules.evaluate(rules.load(RULES), window())
    assert levels(statuses) == [
        ("orma callback_p99_us: /camera/driver sub(/scan)", rules.WARN, "2097.2 us"),
        ("orma callback_p99_us: timer.*", rules.STALE, "not in the window"),
        ("orma topic_hz_min: /scan", rules.WARN, "10.0 Hz"),
        ("orma topic_hz_min: /image", rules.ERROR, "30.0 Hz"),
        ("orma topic_hop_p99_us: /scan", rules.OK, "524.3 us"),
        ("orma sub_loss_pct: /scan /camera/driver", rules.OK, "0.0 %"),
        ("orma chain_e2e_p99_us: perception", rules.WARN, "60000.0 us"),
        ("orma chain_e2e_p99_us: planning", rules.STALE, "not in the window"),
        ("orma gpu_temp_c: gpu 0 Orin", rules.STALE, "not measured"),
        ("orma thermal_c: cpu-thermal", rules.ERROR, "96.0 C"),
        ("orma thermal_c: gpu-thermal", rules.OK, "40.0 C"),
    ]
    assert statuses[0]["values"] == {"value": "2097.15", "warn": "1000", "error": "5000"}


def test_example_rules_load():
    entries = rules.load((PACKAGE / "config" / "rules.yaml").read_text())
    assert sorted({rule for rule, _, _ in entries}) == sorted(rules.RULES)


@pytest.mark.parametrize(
    "text,error",
    [
        ("topic_hz: [{match: /a, warn: 1, error: 2}]", "unknown rule 'topic_hz'"),
        ("topic_hz_min: [{match: /a, warn: 1}]", "lacks error"),
    ],
)
def test_load_refuses(text, error):
    with pytest.raises(ValueError, match=error):
        rules.load(text)
