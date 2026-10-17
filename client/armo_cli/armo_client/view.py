"""What the clients show of a window (design 7.3): callback labels, the ROS entities hidden by
default, the selection's name targets, and callbacks without a node last."""

import re

INFRA_TOPICS = re.compile(r"/rosout|/parameter_events|/?ros_discovery_info")
INFRA_SERVICES = re.compile(
    r".*/(get_parameters|get_parameter_types|set_parameters|set_parameters_atomically"
    r"|describe_parameters|list_parameters|get_type_description)"
)
UNLINKED = "unlinked"


def pid_of(entity_id):
    """`p:`, `n:`, `c:`, `t:` and `e:` ids start with `<pid>:<starttime>`."""
    return int(entity_id.split(":")[1])


def handle_of(callback_id):
    return callback_id.split(":")[4]


def _whole(pattern):
    return re.compile(f"(?:{pattern})") if pattern else None


def _infra_callback(c, topic_names):
    if c["kind"] == "service":
        return bool(c.get("service") and INFRA_SERVICES.fullmatch(c["service"]))
    name = topic_names.get(c["topic"])
    return name is not None and bool(INFRA_TOPICS.fullmatch(name))


def visible(w, selection=None, infra=False):
    """`w` without the parameter services, `/rosout`, `/parameter_events` and
    `ros_discovery_info` unless `infra`. With the selection's `node`, `topic` or `exclude`, only
    the named nodes and topics, the topics the named nodes use, the nodes on the named topics,
    their callbacks and subscriptions, and the callbacks without a node of the selected
    processes. A name `exclude` matches is left out."""
    topic_names = {t["id"]: t["name"] for t in w["topics"]}
    topics = [t for t in w["topics"] if infra or not INFRA_TOPICS.fullmatch(t["name"])]
    callbacks = [c for c in w["callbacks"] if infra or not _infra_callback(c, topic_names)]
    nodes, processes = w["nodes"], w["processes"]
    selection = selection or {}
    node_re, topic_re = _whole(selection.get("node")), _whole(selection.get("topic"))
    exclude_re = _whole(selection.get("exclude"))
    owners = None
    if node_re or topic_re or exclude_re:

        def named(regex, name):
            if exclude_re and exclude_re.fullmatch(name):
                return False
            return regex.fullmatch(name) is not None if regex else not (node_re or topic_re)

        def allowed(name):
            return not (exclude_re and exclude_re.fullmatch(name))

        fqn = {n["id"]: n["fqn"] for n in nodes}
        keep_nodes = {i for i, name in fqn.items() if named(node_re, name)}
        keep_topics = {t["id"] for t in topics if named(topic_re, t["name"])}
        for t in topics:
            ends = set(t["publishers"] + t["subscribers"]) & set(fqn)
            if t["id"] in keep_topics:
                keep_nodes |= {o for o in ends if allowed(fqn[o])}
            elif ends & keep_nodes and allowed(t["name"]):
                keep_topics.add(t["id"])
        pids = set(selection.get("pids", []))
        if selection.get("all"):
            pids = {p["pid"] for p in processes}
        nodes = [n for n in nodes if n["id"] in keep_nodes]
        topics = [t for t in topics if t["id"] in keep_topics]
        callbacks = [
            c
            for c in callbacks
            if c["node"] in keep_nodes or (c["node"] not in fqn and pid_of(c["id"]) in pids)
        ]
        shown = {pid_of(c["id"]) for c in callbacks} | {n["pid"] for n in nodes}
        processes = [p for p in processes if p["pid"] in shown]
        owners = keep_nodes | {o for o in _process_owners(w) if pid_of(o) in pids}
    shown_pids = {p["pid"] for p in processes}
    threads = [t for t in w["threads"] if pid_of(t["id"]) in shown_pids]
    shown_topics = {t["id"] for t in topics}
    subscriptions = [
        s
        for s in w["subscriptions"]
        if s["topic"] in shown_topics and (owners is None or s["node"] in owners)
    ]
    return dict(
        w,
        processes=processes,
        threads=threads,
        nodes=nodes,
        callbacks=callbacks,
        topics=topics,
        subscriptions=subscriptions,
        spans=[s for s in w.get("spans", []) if s["pid"] in shown_pids],
        chains=w.get("chains", []),
    )


def _process_owners(w):
    """Subscription owners that are a process, for subscriptions without a node."""
    return {s["node"] for s in w["subscriptions"] if s["node"].startswith("p:")}


def owner_label(w, owner_id):
    """A node's name, or `unlinked` for a process's entities without one, `(cold)` when armo
    attached after the process started."""
    for n in w["nodes"]:
        if n["id"] == owner_id:
            return n["fqn"]
    pid = pid_of(owner_id)
    cold = any(p["pid"] == pid and p["cold"] for p in w["processes"])
    return f"{UNLINKED} (cold)" if cold else UNLINKED


def callback_labels(w):
    """`sub(/chatter)`, `srv(/add)`; a timer has no target and shows its function (`timer cb_fn`),
    or without one the handle (`<timer 0xaaab...>`). The function of a sub or srv is left out: a
    std::bind callback names only its type. Labels that repeat within a node get `#` and the
    handle's last six digits."""
    topic_names = {t["id"]: t["name"] for t in w["topics"]}
    labels = {}
    for c in w["callbacks"]:
        target = topic_names.get(c["topic"]) if c["topic"] else c.get("service")
        kind = {"service": "srv", "unknown": "cb"}.get(c["kind"], c["kind"])
        if c["kind"] in ("timer", "unknown"):
            head = kind
        else:
            head = f"{kind}({target})" if target else kind
        if target:
            labels[c["id"]] = head
        elif c["symbol"]:
            labels[c["id"]] = f"{head} {c['symbol']}"
        else:
            labels[c["id"]] = f"<{head} 0x{handle_of(c['id'])}>"
    seen = {}
    for c in w["callbacks"]:
        seen.setdefault((c["node"] or pid_of(c["id"]), labels[c["id"]]), []).append(c["id"])
    for ids in seen.values():
        if len(ids) > 1:
            for i in ids:
                labels[i] += f" #{handle_of(i)[-6:]}"
    return labels


def callbacks_in_order(w):
    """Callbacks with a node first, by node and label; those without one last, by process."""
    labels = callback_labels(w)
    names = {n["id"]: n["fqn"] for n in w["nodes"]}
    return sorted(
        w["callbacks"],
        key=lambda c: (
            c["node"] not in names,
            names.get(c["node"], ""),
            pid_of(c["id"]),
            labels[c["id"]],
        ),
    )
