"""The rules of orma_diagnostics over a window (design 20.9). Each entry names its targets by a
regular expression over whole names and gives a warn and an error level; an entry whose
targets are all absent from the window is one STALE status, and so is a target not measured."""

import re

import yaml
from orma_client import view

# diagnostic_msgs/DiagnosticStatus levels.
OK, WARN, ERROR, STALE = 0, 1, 2, 3


def p99_us(lat):
    return None if lat is None else lat["p99_ns"] / 1e3


def callbacks(w):
    labels = view.callback_labels(w)
    for c in w["callbacks"]:
        node = view.owner_label(w, c["node"] or c["id"])
        yield labels[c["id"]], f"{node} {labels[c['id']]}", p99_us(c["duration"])


def topic_hz(w):
    return ((t["name"], t["name"], t["pub_hz"]) for t in w["topics"])


def topic_hop(w):
    return ((t["name"], t["name"], p99_us(t["hop"])) for t in w["topics"])


def subscriptions(w):
    names = {t["id"]: t["name"] for t in w["topics"]}
    for s in w["subscriptions"]:
        topic = names.get(s["topic"], s["topic"])
        yield topic, f"{topic} {view.owner_label(w, s['node'])}", s["loss_pct"]


def chains(w):
    return ((c["name"], c["name"], p99_us(c["e2e"])) for c in w.get("chains", []))


def host(w):
    return w.get("host") or {"gpus": [], "thermal": []}


def gpus(w):
    for g in host(w)["gpus"]:
        yield str(g["index"]), f"gpu {g['index']} {g['name']}", g["temp_c"]


def thermal(w):
    return ((t["name"], t["name"], t["temp_c"]) for t in host(w)["thermal"])


# Rule: its targets, the unit of its value, and whether a lower value is worse.
RULES = {
    "callback_p99_us": (callbacks, "us", False),
    "topic_hz_min": (topic_hz, "Hz", True),
    "topic_hop_p99_us": (topic_hop, "us", False),
    "sub_loss_pct": (subscriptions, "%", False),
    "chain_e2e_p99_us": (chains, "us", False),
    "gpu_temp_c": (gpus, "C", False),
    "thermal_c": (thermal, "C", False),
}


def load(text):
    """`{rule: [{match, warn, error}, ...]}` as (rule, regex, entry) triples."""
    entries = []
    for rule, items in (yaml.safe_load(text) or {}).items():
        if rule not in RULES:
            raise ValueError(f"unknown rule {rule!r}; rules are {', '.join(RULES)}")
        for item in items:
            missing = {"match", "warn", "error"} - set(item)
            if missing:
                raise ValueError(f"{rule}: {item} lacks {', '.join(sorted(missing))}")
            entries.append((rule, re.compile(str(item["match"])), item))
    return entries


def level(value, warn, error, lower_is_worse):
    if lower_is_worse:
        return ERROR if value < error else WARN if value < warn else OK
    return ERROR if value > error else WARN if value > warn else OK


def status(level, rule, shown, message, item, value=None):
    values = {"value": "-" if value is None else f"{value:g}"}
    values.update(warn=str(item["warn"]), error=str(item["error"]))
    return {
        "level": level,
        "name": f"orma {rule}: {shown}",
        "message": message,
        "values": values,
    }


def evaluate(entries, w):
    out = []
    for rule, regex, item in entries:
        targets, unit, lower_is_worse = RULES[rule]
        found = [(shown, v) for name, shown, v in targets(w) if regex.fullmatch(name)]
        if not found:
            out.append(status(STALE, rule, item["match"], "not in the window", item))
        for shown, value in found:
            if value is None:
                out.append(status(STALE, rule, shown, "not measured", item))
                continue
            lv = level(value, item["warn"], item["error"], lower_is_worse)
            out.append(status(lv, rule, shown, f"{value:.1f} {unit}", item, value))
    return out
