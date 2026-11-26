import pytest
from orma_diagnostics import rules
from conftest import PACKAGE, window

RULES = """
orma:
  callback_us:
    - {match: "sub\\\\(/scan\\\\)", warn: 1000, error: 5000}
    - {match: "timer.*", warn: 1, error: 2}
  topic_hz_min:
    - {match: /scan, warn: 11, error: 5}
    - {match: /image, warn: 40, error: 35}
  topic_hop_us:
    - {match: /scan, warn: 1000, error: 2000}
  topic_age_us:
    - {match: /scan, warn: 100000, error: 300000}
    - {match: /image, warn: 100000, error: 300000}
    - {match: /scan, stat: p50, warn: 100000, error: 300000}
  sub_loss_pct:
    - {match: /scan, warn: 1, error: 5}
  chain_e2e_us:
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
        ("orma callback_us: /camera/driver sub(/scan)", rules.WARN, "2097.2 us"),
        ("orma callback_us: timer.*", rules.STALE, "not in the window"),
        ("orma topic_hz_min: /scan", rules.WARN, "10.0 Hz"),
        ("orma topic_hz_min: /image", rules.ERROR, "30.0 Hz"),
        ("orma topic_hop_us: /scan", rules.OK, "524.3 us"),
        ("orma topic_age_us: /scan", rules.WARN, "120000.0 us"),
        ("orma topic_age_us: /image", rules.STALE, "not measured"),
        ("orma topic_age_us: /scan", rules.OK, "0.0 us"),
        ("orma sub_loss_pct: /scan /camera/driver", rules.OK, "0.0 %"),
        ("orma chain_e2e_us: perception", rules.WARN, "60000.0 us"),
        ("orma chain_e2e_us: planning", rules.STALE, "not in the window"),
        ("orma gpu_temp_c: gpu 0 Orin", rules.STALE, "not measured"),
        ("orma thermal_c: cpu-thermal", rules.ERROR, "96.0 C"),
        ("orma thermal_c: gpu-thermal", rules.OK, "40.0 C"),
    ]
    assert statuses[0]["values"] == {
        "value": "2097.15",
        "warn": "1000",
        "error": "5000",
        "stat": "p99",
    }


def test_capas_chains_beside_the_rules_are_skipped():
    text = "capa:\n  chains: {c: {target: soft, stages: [{node: /a}]}}\n" + RULES
    assert rules.load(text) == rules.load(RULES)


def test_example_rules_load():
    entries = rules.load((PACKAGE / "config" / "rules.yaml").read_text())
    assert sorted({rule for rule, _, _ in entries}) == sorted(rules.RULES)


@pytest.mark.parametrize(
    "text,error",
    [
        ("orma: {topic_hz: [{match: /a, warn: 1, error: 2}]}", "unknown rule 'topic_hz'"),
        ("orma: {topic_hz_min: [{match: /a, warn: 1}]}", "lacks error"),
        ("topic_hz_min: [{match: /a, warn: 1, error: 2}]", "topic_hz_min moved under orma:"),
        ("orma: {}\nomra: {}", "unknown key 'omra' in the file root"),
        ("capa: {chains: {}}", "no orma mapping"),
        ("orma: {topic_age_p99_us: []}", "topic_age_p99_us is now topic_age_us with stat: p99"),
        ("orma: {topic_age_us: [{match: /a, stat: p95, warn: 1, error: 2}]}", "stat 'p95'"),
        ("orma: {thermal_c: [{match: .*, stat: max, warn: 1, error: 2}]}", "only latency rules"),
    ],
)
def test_load_refuses(text, error):
    with pytest.raises(ValueError, match=error):
        rules.load(text)
