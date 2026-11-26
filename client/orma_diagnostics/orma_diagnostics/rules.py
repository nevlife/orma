"""The rules of orma_diagnostics over a window (design 20.9). Each entry names its targets by a
regular expression over whole names and gives a warn and an error level; an entry whose
targets are all absent from the window is one STALE status, and so is a target not measured."""

import re

import yaml
from orma_client import view

# diagnostic_msgs/DiagnosticStatus levels.
OK, WARN, ERROR, STALE = 0, 1, 2, 3


# The statistics of a latency an entry may pick with `stat`; p99 when it names none.
STATS = ("mean", "p50", "p90", "p99", "p999", "max")


def callbacks(w):
    labels = view.callback_labels(w)
    for c in w["callbacks"]:
        node = view.owner_label(w, c["node"] or c["id"])
        yield labels[c["id"]], f"{node} {labels[c['id']]}", c["duration"]


def topic_hz(w):
    return ((t["name"], t["name"], t["pub_hz"]) for t in w["topics"])


def topic_hop(w):
    return ((t["name"], t["name"], t["hop"]) for t in w["topics"])


def topic_age(w):
    return ((t["name"], t["name"], t["age_take"]) for t in w["topics"])


def subscriptions(w):
    names = {t["id"]: t["name"] for t in w["topics"]}
    for s in w["subscriptions"]:
        topic = names.get(s["topic"], s["topic"])
        yield topic, f"{topic} {view.owner_label(w, s['node'])}", s["loss_pct"]


def chains(w):
    return ((c["name"], c["name"], c["e2e"]) for c in w.get("chains", []))


def host(w):
    return w.get("host") or {"gpus": [], "thermal": []}


def gpus(w):
    for g in host(w)["gpus"]:
        yield str(g["index"]), f"gpu {g['index']} {g['name']}", g["temp_c"]


def thermal(w):
    return ((t["name"], t["name"], t["temp_c"]) for t in host(w)["thermal"])


# Rule: its targets, the unit of its value, whether a lower value is worse, and whether the
# targets give a latency (`Lat`) that an entry's `stat` reduces to one value in us.
RULES = {
    "callback_us": (callbacks, "us", False, True),
    "topic_hz_min": (topic_hz, "Hz", True, False),
    "topic_hop_us": (topic_hop, "us", False, True),
    "topic_age_us": (topic_age, "us", False, True),
    "sub_loss_pct": (subscriptions, "%", False, False),
    "chain_e2e_us": (chains, "us", False, True),
    "gpu_temp_c": (gpus, "C", False, False),
    "thermal_c": (thermal, "C", False, False),
}
# Names before `stat`, each a latency rule at p99.
RENAMED = {
    "callback_p99_us": "callback_us",
    "topic_hop_p99_us": "topic_hop_us",
    "topic_age_p99_us": "topic_age_us",
    "chain_e2e_p99_us": "chain_e2e_us",
}


def microseconds(lat, stat):
    return None if lat is None else lat[f"{stat}_ns"] / 1e3


def load(text):
    """`{orma: {rule: [{match, warn, error, stat?}, ...]}}` as (rule, regex, entry) triples. The
    file may also hold capa's chains under `capa`, which is not read here."""
    root = yaml.safe_load(text) or {}
    moved = sorted(set(root) & (set(RULES) | set(RENAMED)))
    if moved:
        raise ValueError(f"{moved[0]} moved under orma: (orma: {{{moved[0]}: [...]}})")
    for key in root:
        if key not in ("capa", "orma"):
            raise ValueError(f"unknown key {key!r} in the file root; keys are capa, orma")
    if not isinstance(root.get("orma"), dict):
        raise ValueError("the file has no orma mapping")
    entries = []
    for rule, items in root["orma"].items():
        if rule in RENAMED:
            raise ValueError(f"{rule} is now {RENAMED[rule]} with stat: p99")
        if rule not in RULES:
            raise ValueError(f"unknown rule {rule!r}; rules are {', '.join(RULES)}")
        latency = RULES[rule][3]
        for item in items:
            missing = {"match", "warn", "error"} - set(item)
            if missing:
                raise ValueError(f"{rule}: {item} lacks {', '.join(sorted(missing))}")
            if "stat" in item and not latency:
                raise ValueError(f"{rule}: {item} has a stat, which only latency rules take")
            if latency and item.setdefault("stat", "p99") not in STATS:
                raise ValueError(f"{rule}: stat {item['stat']!r} is not one of {', '.join(STATS)}")
            entries.append((rule, re.compile(str(item["match"])), item))
    return entries


def level(value, warn, error, lower_is_worse):
    if lower_is_worse:
        return ERROR if value < error else WARN if value < warn else OK
    return ERROR if value > error else WARN if value > warn else OK


def status(level, rule, shown, message, item, value=None):
    values = {"value": "-" if value is None else f"{value:g}"}
    values.update(warn=str(item["warn"]), error=str(item["error"]))
    if "stat" in item:
        values["stat"] = item["stat"]
    return {
        "level": level,
        "name": f"orma {rule}: {shown}",
        "message": message,
        "values": values,
    }


def evaluate(entries, w):
    out = []
    for rule, regex, item in entries:
        targets, unit, lower_is_worse, latency = RULES[rule]
        found = [(shown, v) for name, shown, v in targets(w) if regex.fullmatch(name)]
        if latency:
            found = [(shown, microseconds(v, item["stat"])) for shown, v in found]
        if not found:
            out.append(status(STALE, rule, item["match"], "not in the window", item))
        for shown, value in found:
            if value is None:
                out.append(status(STALE, rule, shown, "not measured", item))
                continue
            lv = level(value, item["warn"], item["error"], lower_is_worse)
            out.append(status(lv, rule, shown, f"{value:.1f} {unit}", item, value))
    return out
