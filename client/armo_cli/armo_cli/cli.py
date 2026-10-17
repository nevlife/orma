"""The armo command."""

import argparse
import collections
import contextlib
import csv
import io
import json
import os
import re
import shlex
import subprocess
import sys
import time

import argcomplete
import armo_client
from armo_client import ArmoError, view

METHODS = ("uprobe", "preload", "lttng")
LAUNCH_METHODS = ("preload", "lttng")
EXTRAS = ("bandwidth", "sched", "malloc", "pss", "gpu", "dds", "futex")
MONITOR_OPTIONS = ("gpu", "dds", "futex", "chains", "latest_nodes")
EXPORT_METRICS = ("processes", "threads", "nodes", "executors", "callbacks", "topics")
EXPORT_METRICS += ("subscriptions", "flows", "spans", "chains", "host", "agent")


def ns(value):
    if value is None:
        return "-"
    for unit, scale in (("s", 1e9), ("ms", 1e6), ("us", 1e3)):
        if value >= scale:
            return f"{value / scale:.1f}{unit}"
    return f"{value}ns"


def size(value):
    if value is None:
        return "-"
    for unit, scale in (("GiB", 1 << 30), ("MiB", 1 << 20), ("KiB", 1 << 10)):
        if value >= scale:
            return f"{value / scale:.1f}{unit}"
    return f"{value}B"


def num(value, fmt="{:.1f}"):
    return "-" if value is None else fmt.format(value)


def lat(value):
    """p50, p90, p99, max."""
    if value is None:
        return ["-"] * 4
    return [ns(value[k]) for k in ("p50_ns", "p90_ns", "p99_ns", "max_ns")]


def p99(value):
    return "-" if value is None else ns(value["p99_ns"])


def table(title, headers, rows):
    if not rows:
        return
    print(title)
    cells = [headers] + [[str(c) for c in row] for row in rows]
    widths = [max(len(c) for c in column) for column in zip(*cells)]
    for row in cells:
        print("  " + "  ".join(c.ljust(w) for c, w in zip(row, widths)).rstrip())


OVERFLOW = [
    ("threads", "threads not traced (thread slots full)"),
    ("table", "samples not aggregated (thread table full)"),
    ("depth", "callbacks not timed (nested deeper than 16)"),
]


def print_process_warnings(w):
    for p in w["processes"]:
        overflow = p.get("overflow") or {}
        lost = [f"{overflow[k]} {what}" for k, what in OVERFLOW if overflow.get(k)]
        if lost:
            print(f"warning: pid {p['pid']} ({p['comm']}): {', '.join(lost)}")
        if p.get("flux_gpu_wedged") is not None:
            print(
                f"warning: pid {p['pid']} ({p['comm']}): flux GPU wedged (CUresult "
                f"{p['flux_gpu_wedged']}); its GPU channels refuse claims, loans and takes"
            )


def print_agent(w):
    a = w["agent"]
    armod = a["armod"]
    print(
        f"armo: collectd cpu {num(a['collectd']['cpu_pct'])}% rss {size(a['collectd']['rss_bytes'])}"
        f", armod cpu {num(armod and armod['cpu_pct'])}% rss {size(armod and armod['rss_bytes'])}"
        f", shm files {a['shm']['files']} ring drops {a['shm']['ring_drops']}"
        f", probes missing {len(a['probes_missing'])}, collect {ns(a['collect_ns'])}"
    )


def node_rss(w, n):
    """The process's RSS, marked when the process holds more than this node."""
    p = next((p for p in w["processes"] if p["pid"] == n["pid"]), None)
    if p is None:
        return "-"
    return size(p["rss_bytes"]) + (" (shared)" if len(p["nodes"]) > 1 else "")


def print_nodes(w):
    table(
        "nodes",
        ["node", "pid", "cpu%", "source", "busy%", "cb Hz", "rss"],
        [
            [n["fqn"], n["pid"], num(n["cpu_pct"]), n["cpu_source"], num(n["busy_pct"])]
            + [num(n["cb_hz"]), node_rss(w, n)]
            for n in w["nodes"]
        ],
    )
    print_process_warnings(w)


LAT_HEADS = ["p50", "p90", "p99", "max"]


def print_callbacks(w):
    labels = view.callback_labels(w)
    callbacks = view.callbacks_in_order(w)
    cuda = any(c.get("cuda") for c in callbacks)
    futex = any(c.get("futex_mean_ns") is not None for c in callbacks)
    table(
        "callbacks",
        ["callback", "node", "pid", "Hz", "expected"]
        + LAT_HEADS
        + ["dispatch p99"]
        + ["cuda api", "cuda sync"] * cuda
        + ["futex"] * futex,
        [
            [labels[c["id"]], view.owner_label(w, c["node"] or c["id"]), view.pid_of(c["id"])]
            + [num(c["hz"]), num(c["expected_hz"])]
            + lat(c["duration"])
            + [p99(c["dispatch"])]
            + cuda_cells(c.get("cuda")) * cuda
            + [ns(c.get("futex_mean_ns"))] * futex
            for c in callbacks
        ],
    )


def cuda_cells(use):
    return [ns(use["api_mean_ns"]), ns(use["sync_mean_ns"])] if use else ["-", "-"]


def rate(bps):
    return "-" if bps is None else size(bps) + "/s"


def print_subscriptions(w):
    topic = {t["id"]: t["name"] for t in w["topics"]}
    table(
        "subscriptions",
        ["topic", "node", "pid", "recv Hz", "loss%", "lost"] + ["wake " + h for h in LAT_HEADS],
        [
            [topic.get(s["topic"], s["topic"]), view.owner_label(w, s["node"])]
            + [view.pid_of(s["node"]), num(s["recv_hz"])]
            + [num(s["loss_pct"])]
            + [num(s["lost"], "{}")]
            + lat(s["wake"])
            for s in sorted(w["subscriptions"], key=lambda s: not s["node"].startswith("n:"))
        ],
    )


GPU_HEADS = ["gpu", "name", "util%", "mem", "clock", "temp", "power", "throttle"]
CUDA_HEADS = ["pid", "comm", "calls Hz", "launch Hz", "memcpy", "alloc Hz", "errors", "gpu mem"]
CUDA_HEADS += ["sync " + h for h in LAT_HEADS]
SPAN_HEADS = ["span", "parent", "callback", "node", "pid", "Hz"] + LAT_HEADS
CHAIN_HEADS = ["chain", "mode", "count"] + ["e2e " + h for h in LAT_HEADS]
CHAIN_HEADS += ["steps p99", "broken"]
THERMAL_HEADS = ["zone", "temp C"]
THROTTLE_HEADS = ["counter", "count"]
NET_HEADS = ["interface", "rx", "tx", "rx pps", "tx pps", "rx drops", "tx drops", "rx errors"]
NET_HEADS += ["tx errors"]


def unit(value, suffix, fmt="{:.0f}"):
    return "-" if value is None else fmt.format(value) + suffix


def host_of(w):
    return w.get("host") or {"cpu_mhz": [], "thermal": [], "throttle": {}, "net": [], "gpus": []}


def gpu_rows(w):
    return [
        [g["index"], g["name"], num(g["util_pct"])]
        + [f"{size(g['mem_used_bytes'])}/{size(g['mem_total_bytes'])}"]
        + [unit(g["clock_mhz"], "MHz"), unit(g["temp_c"], "C"), unit(g["power_w"], "W", "{:.1f}")]
        + [",".join(g["throttle"])]
        for g in host_of(w)["gpus"]
    ]


def cuda_rows(w):
    rows = []
    for p in w["processes"]:
        cuda = p.get("cuda")
        if cuda is None and p.get("gpu_mem_bytes") is None:
            continue
        cells = [p["pid"], p["comm"]]
        if cuda is None:
            cells += ["-"] * 5
        else:
            cells += [num(cuda["calls_hz"]), num(cuda["launch_hz"]), rate(cuda["memcpy_bps"])]
            cells += [num(cuda["alloc_hz"]), cuda["errors"]]
        rows.append(cells + [size(p.get("gpu_mem_bytes"))] + lat(cuda and cuda["sync"]))
    return rows


def span_names(spans):
    """Names that repeat in one process with the same callback and parent get `#` and the last
    six digits of their key, the address of the name string at each begin."""
    seen = collections.Counter((s["pid"], s["name"], s["callback"], s["parent"]) for s in spans)
    names = {}
    for s in spans:
        key = re.search(r"@([0-9a-f]+)#", s["id"])
        repeated = seen[(s["pid"], s["name"], s["callback"], s["parent"])] > 1
        names[s["id"]] = f"{s['name']} #{key[1][-6:]}" if repeated and key else s["name"]
    return names


def span_rows(w):
    labels = view.callback_labels(w)
    names = span_names(w.get("spans", []))
    nodes = {n["id"]: n["fqn"] for n in w["nodes"]}
    return [
        [names[s["id"]], names.get(s["parent"], "-") if s["parent"] else "-"]
        + [labels.get(s["callback"], s["callback"]) if s["callback"] else "-"]
        + [nodes.get(s["node"], view.UNLINKED), s["pid"], num(s["hz"])]
        + lat(s["duration"])
        for s in w.get("spans", [])
    ]


def chain_rows(w):
    return [
        [c["name"], c["mode"], c["count"]]
        + lat(c["e2e"])
        + [" ".join(f"{t}:{p99(step)}" for t, step in zip(c["topics"][1:], c["steps"]))]
        + [c["broken"]]
        for c in w.get("chains", [])
    ]


def thermal_rows(w):
    return [[t["name"], f"{t['temp_c']:.1f}"] for t in host_of(w)["thermal"]]


def throttle_rows(w):
    return [[name, count] for name, count in host_of(w)["throttle"].items()]


def net_rows(w):
    return [
        [n["name"], rate(n["rx_bps"]), rate(n["tx_bps"]), num(n["rx_pps"]), num(n["tx_pps"])]
        + [n["rx_drops"], n["tx_drops"], n["rx_errors"], n["tx_errors"]]
        for n in host_of(w)["net"]
    ]


def cpu_line(w):
    cpus = host_of(w)["cpu_mhz"]
    mhz = [m for m in cpus if m is not None]
    if not mhz:
        return f"cpu MHz: - ({len(cpus)} cpus)"
    mean = sum(mhz) / len(mhz)
    return f"cpu MHz: min {min(mhz)} mean {mean:.0f} max {max(mhz)} ({len(cpus)} cpus)"


def udp_line(w):
    u = host_of(w).get("udp")
    if u is None:
        return "udp: -"
    return (
        f"udp: in errors {u['in_errors']}, rcvbuf errors {u['rcvbuf_errors']}"
        f", sndbuf errors {u['sndbuf_errors']}, no ports {u['no_ports']}"
    )


def print_host(w):
    if w.get("host") is None:
        return
    print(cpu_line(w))
    table("thermal", THERMAL_HEADS, thermal_rows(w))
    table("throttle", THROTTLE_HEADS, throttle_rows(w))
    table("net", NET_HEADS, net_rows(w))
    print(udp_line(w))
    table("gpus", GPU_HEADS, gpu_rows(w))


def print_spans(w):
    table("spans", SPAN_HEADS, span_rows(w))


def print_recordings(recordings):
    if not recordings:
        print("recordings: none")
    table(
        "recordings",
        ["id", "state", "kind", "methods", "elapsed", "size", "limits", "events", "lost"]
        + ["owner", "end", "dir"],
        [
            [r["id"], r["state"], r["kind"], ",".join(r["methods"]), f"{r['elapsed_s']:.0f}s"]
            + [size(r["bytes"])]
            + [f"{r['limits']['max_duration_s']}s/{size(r['limits']['max_bytes'])}"]
            + [r["totals"]["events"], r["totals"]["lost"]]
            + [f"{r['owner']['client']}:{r['owner']['pid']}@{r['owner']['host']}"]
            + [r.get("end_reason") or "-", r["dir"]]
            for r in recordings
        ],
    )


def print_show(t):
    table(
        "entities",
        ["entity", "events", "lost", "Hz"],
        [[e["id"], e["events"], e["lost"], num(e["hz"])] for e in t["entities"]],
    )
    for w in t["warnings"]:
        ids = f" ({', '.join(w['ids'])})" if w["ids"] else ""
        print(f"warning {w['kind']}: {w['text']}{ids}")
    s = t["totals"]
    print(
        f"total: {s['events']} events, {s['lost']} lost, {s['duration_s']:.1f}s, {size(s['bytes'])}"
    )
    detail = f" ({t['end_detail']})" if t["end_detail"] else ""
    print(f"end: {t['end_reason'] or '-'}{detail}")


def cmd_status(args):
    with armo_client.connect(start=False) as c:
        src, m = c.state["sources"], c.state["monitor"]
        selection = m["selection"]
        pids = " ".join(f"{p}:{m['method_by_pid'].get(str(p), 'auto')}" for p in selection["pids"])
        extra = "".join(f", {k}" for k in EXTRAS if m.get(k))
        names = "".join(
            f" {k} {selection[k]}" for k in ("node", "topic", "exclude") if k in selection
        )
        print(
            f"armo-collectd: {c.server['server']} {c.server['version']}, {armo_client.socket_path()}"
        )
        print(
            f"armod: {src['armod']}, lttng: {src['lttng']}, "
            f"preload processes: {src['preload_processes']}"
        )
        print(
            f"monitor: {'running' if m['running'] else 'idle'}, window {m['window_s']:g}s, "
            f"selection {'all' if selection['all'] else pids or 'none'}{names}{extra}"
        )
        for chain in m.get("chains", []):
            print(f"chain: {armo_client.format_chain(chain)}")
        if m.get("latest_nodes"):
            print(f"latest-input nodes: {' '.join(m['latest_nodes'])}")
        print_recordings(c.state["recordings"])
        c.request("subscribe", streams=["window"])
        print_agent(next_window(c))
    return 0


def exact(names):
    return "^(" + "|".join(map(re.escape, names)) + ")$"


def select(c, args, options=(), **names):
    """Subscribe, configure, and select these processes, names, or else all."""
    # First, so that the event lines `select` raises reach this client.
    c.request("subscribe", streams=["state", "window", "event_line"])
    pids = getattr(args, "pid", None) or []
    methods = getattr(args, "method", None) or []
    if methods and not pids:
        raise ArmoError("--method needs --pid")
    if len(methods) not in (0, 1, len(pids)):
        raise ArmoError("give --method once, or once per --pid")
    methods = methods * len(pids) if len(methods) == 1 else methods
    configure = {"window_s": getattr(args, "window", None)}
    configure.update((k, getattr(args, k, None)) for k in options)
    configure = {k: v for k, v in configure.items() if v is not None}
    if configure:
        c.request("configure", **configure)
    names = {k: v for k, v in names.items() if v}
    c.request(
        "select",
        all=not (pids or names),
        pids=pids,
        method_by_pid={str(p): m for p, m in zip(pids, methods)},
        **names,
    )


def watch(c, show):
    """Each window's header, then what `show` prints of it or `none`, until Ctrl-C."""
    try:
        while True:
            m = c.next_message()
            if m["type"] == "window":
                behind = " behind" if m["behind"] else ""
                print(f"window {m['seq']}  {m['window_ns'] / 1e9:g}s{behind}")
                out = io.StringIO()
                with contextlib.redirect_stdout(out):
                    show(m)
                print(out.getvalue() or "  none\n")
            elif m["type"] == "event_line":
                print(f"[{m['level']}] {m['kind']}: {m['text']}")
            sys.stdout.flush()
    except KeyboardInterrupt:
        return 0


def of_nodes(w, nodes, infra=False):
    """`w` without the infrastructure entities, and with only the callbacks and subscriptions of
    these nodes."""
    w = view.visible(w, None, infra)
    ids = {n["id"] for n in w["nodes"] if n["fqn"] in nodes}
    return dict(
        w,
        nodes=[n for n in w["nodes"] if n["id"] in ids],
        callbacks=[c for c in w["callbacks"] if c["node"] in ids],
        subscriptions=[s for s in w["subscriptions"] if s["node"] in ids],
    )


def cmd_top(args):
    with armo_client.connect() as c:
        c.request("subscribe", streams=["window", "event_line"])
        if args.window is not None:
            c.request("configure", window_s=args.window)
        if args.json:
            try:
                while True:
                    print(json.dumps(next_window(c), separators=(",", ":")), flush=True)
            except KeyboardInterrupt:
                return 0
        return watch(c, print_host)


def cmd_node_list(args):
    with armo_client.connect() as c:
        select(c, args)
        w = next_window(c)
    for name in sorted({n["fqn"] for n in w["nodes"]}):
        print(name)
    return 0


def cmd_node_info(args):
    with armo_client.connect() as c:
        select(c, args, node=exact([args.node]))
        w = next_window(c)
    shown = of_nodes(w, [args.node], args.infra)
    if not shown["nodes"]:
        raise ArmoError(f"{args.node}: not an observed node")
    labels = view.callback_labels(w)
    for n in shown["nodes"]:
        p = next((p for p in w["processes"] if p["pid"] == n["pid"]), None)
        print(n["fqn"])
        print(f"  pid {n['pid']} ({p['comm'] if p else '-'}), rss {node_rss(w, n)}")
        for kind, field in (("publishers", "publishers"), ("subscriptions", "subscribers")):
            topics = sorted(t["name"] for t in shown["topics"] if n["id"] in t[field])
            print(f"  {kind}: {len(topics)}")
            for name in topics:
                print(f"    {name}")
        callbacks = [labels[c["id"]] for c in shown["callbacks"] if c["node"] == n["id"]]
        print(f"  callbacks: {len(callbacks)}")
        for label in callbacks:
            print(f"    {label}")
    return 0


def cmd_node_cpu(args):
    with armo_client.connect() as c:
        select(c, args, node=args.nodes and exact(args.nodes))

        def show(w):
            w = view.visible(w, None)
            if args.nodes:
                w = dict(w, nodes=[n for n in w["nodes"] if n["fqn"] in args.nodes])
            print_nodes(w)

        return watch(c, show)


def cmd_node_cb(args):
    with armo_client.connect() as c:
        select(c, args, ("gpu", "futex"), node=exact([args.node]))

        def show(w):
            w = of_nodes(w, [args.node], args.infra)
            if args.callback:
                labels = view.callback_labels(w)
                callbacks = [c for c in w["callbacks"] if args.callback in labels[c["id"]]]
                w = dict(w, callbacks=callbacks)
            print_callbacks(w)

        return watch(c, show)


def cmd_node_sub(args):
    with armo_client.connect() as c:
        select(c, args, node=exact([args.node]))

        def show(w):
            w = of_nodes(w, [args.node], args.infra)
            if args.topic:
                ids = {t["id"] for t in w["topics"] if t["name"] == args.topic}
                w = dict(w, subscriptions=[s for s in w["subscriptions"] if s["topic"] in ids])
            print_subscriptions(w)

        return watch(c, show)


def cmd_span(args):
    with armo_client.connect() as c:
        select(c, args)

        def show(w):
            if args.names:
                w = dict(w, spans=[s for s in w["spans"] if s["name"] in args.names])
            print_spans(w)

        return watch(c, show)


def hz_cells(t, dds):
    return [num(t["pub_hz"]), num(t["expected_hz"]), num(t["drops"]["publish"], "{}")]


def delay_cells(t, dds):
    return lat(t["hop"]) + [p99(t.get("pub_to_dds")), p99(t.get("dds_write"))] * dds


# `armo topic` command: its columns, and the cells of one topic.
TOPIC_COLUMNS = {
    "hz": (["pub Hz", "expected", "drops"], hz_cells),
    "bw": (["bw"], lambda t, dds: [rate(t["bw_bps"])]),
    "delay": (["hop " + h for h in LAT_HEADS], delay_cells),
}


def print_topic_window(w, command, topics):
    by_name = {t["name"]: t for t in w["topics"]}
    heads, cells = TOPIC_COLUMNS[command]
    dds = command == "delay" and any(t.get("dds_write") or t.get("pub_to_dds") for t in w["topics"])
    heads = heads + ["pub to dds p99", "dds write p99"] * dds
    rows = [
        [name] + (cells(by_name[name], dds) if name in by_name else ["-"] * len(heads))
        for name in topics
    ]
    table("topics", ["topic"] + heads, rows)


def cmd_topic_watch(args):
    """Counted in the publishers and subscribers, not by subscribing as `ros2 topic` does."""
    with armo_client.connect() as c:
        select(c, args, ("dds",), topic=exact(args.topics))
        return watch(c, lambda w: print_topic_window(w, args.topic_command, args.topics))


def cmd_topic_list(args):
    with armo_client.connect() as c:
        select(c, args)
        w = view.visible(next_window(c), None, args.infra)
    for name in sorted({t["name"] for t in w["topics"]}):
        print(name)
    return 0


def cmd_topic_info(args):
    with armo_client.connect() as c:
        select(c, args, topic=exact([args.topic]))
        w = next_window(c)
    topics = [t for t in w["topics"] if t["name"] == args.topic]
    if not topics:
        raise ArmoError(f"{args.topic}: no observed publisher or subscriber")
    for t in topics:
        print(f"{t['name']}  transport {t['transport']}")
        for kind in ("publishers", "subscribers"):
            print(f"{kind}: {len(t[kind])}")
            for owner in t[kind]:
                print(f"  {view.owner_label(w, owner)}")
    return 0


def recording(state, record_id):
    return next((r for r in state["recordings"] if r["id"] == record_id), None)


def wait_end(c, record_id, status):
    """Until the recording leaves `recording`, or leaves the state after being in it."""
    seen = False
    while True:
        m = c.next_message()
        if m["type"] not in ("state", "window"):
            continue
        r = recording(c.state, record_id)
        seen = seen or r is not None
        if (r is not None and r["state"] != "recording") or (r is None and seen):
            return r
        if m["type"] == "window" and status and r is not None:
            print(
                f"{record_id} {r['elapsed_s']:.0f}s {size(r['bytes'])} "
                f"events {r['totals']['events']} lost {r['totals']['lost']}",
                flush=True,
            )


def cmd_record(args):
    targets = None
    if args.all or args.pid or args.node or args.topic or args.exclude:
        targets = armo_client.targets(args.all, args.pid or (), args.node, args.topic, args.exclude)
    options = {k: getattr(args, k) for k in armo_client.RECORD_OPTIONS}
    fields = armo_client.record_start(args.method, targets, **options)
    with armo_client.connect() as c:
        reply = c.request("record_start", **fields)
        record_id = reply["record_id"]
        print(f"recording {record_id} in {reply['dir']}", flush=True)
        if args.no_follow:
            return 0
        c.request("subscribe", streams=["state", "window"])
        try:
            r = wait_end(c, record_id, True)
        except KeyboardInterrupt:
            c.request("record_stop", record_id=record_id)
            print(f"stopping {record_id}", flush=True)
            r = wait_end(c, record_id, False)
        print(f"{record_id} ended: {(r or {}).get('end_reason') or '-'}")
        print_show(c.request("record_show", record_id=record_id)["table"])
    return 0


def cmd_record_list(args):
    with armo_client.connect() as c:
        print_recordings(c.request("record_list")["recordings"])
    return 0


def cmd_record_stop(args):
    with armo_client.connect() as c:
        c.request("record_stop", record_id=args.record_id)
    print(f"stopped {args.record_id}")
    return 0


def cmd_record_show(args):
    """A record id has no `/`; anything with one, or an existing directory, is a directory."""
    target = args.target
    if os.sep in target or os.path.isdir(target):
        fields = {"dir": os.path.abspath(target)}
    else:
        fields = {"record_id": target}
    with armo_client.connect() as c:
        print_show(c.request("record_show", **fields)["table"])
    return 0


def cmd_analyze(args):
    chains = [a for spec in args.chain for a in ("--chain", spec)]
    os.execvp("armo-analyze", ["armo-analyze", os.path.abspath(args.dir)] + chains)


def cmd_export(args):
    with armo_client.connect() as c:
        c.request(
            "export",
            dir=os.path.abspath(args.dir),
            out=os.path.abspath(args.output),
            metrics=args.metric or [],
        )
    print(f"exported {args.output}")
    return 0


def cmd_check(args):
    with armo_client.connect() as c:
        checks = c.request("selfcheck", duration_s=args.duration)["checks"]
    rows = [[x["item"], x["status"], x["detail"]] for x in checks]
    table("selfcheck", ["item", "status", "detail"], rows)
    return 1 if any(x["status"] == "fail" for x in checks) else 0


def cmd_run(args):
    command = args.command[1:] if args.command[:1] == ["--"] else args.command
    if not command:
        raise ArmoError("armo run needs a command after --")
    env = dict(os.environ)
    env.update(armo_client.environment(args.method, env))
    os.execvpe(command[0], command, env)


REACT_COUNT = 10
REACT_SEEN_WINDOWS = 5
REACT_SETTLE_WINDOWS = 2
REACT_NOTE = (
    "n and max are exact; p50 is the n-weighted mean of the window p50s (an estimate), "
    "p99 the largest window p99 (an upper bound)"
)


def merge_lat(lats):
    """Window latencies pooled: no window's p99 is exceeded by more than 1% of its samples, so
    none of the pooled ones exceed the largest p99 by more than 1% either."""
    lats = [x for x in lats if x and x["n"]]
    if not lats:
        return None
    n = sum(x["n"] for x in lats)
    return {
        "n": n,
        "p50_ns": round(sum(x["p50_ns"] * x["n"] for x in lats) / n),
        "p99_ns": max(x["p99_ns"] for x in lats),
        "max_ns": max(x["max_ns"] for x in lats),
    }


def react_selection(selection, methods, pid):
    """The selection that also sees the publisher; None when the current one already does."""
    if selection["all"]:
        return None
    if not (selection["pids"] or any(k in selection for k in ("node", "topic", "exclude"))):
        return {"all": True, "pids": [], "method_by_pid": {}}
    return dict(
        selection,
        pids=selection["pids"] + [pid],
        method_by_pid=dict(methods, **{str(pid): "preload"}),
    )


def react_publisher(args, env):
    count = args.count if args.count is not None else (None if args.bag else REACT_COUNT)
    command = [sys.executable, "-m", "armo_cli.react_publisher", "--input", args.input]
    command += ["--type", args.type, "--period", str(args.period)]
    command += ["--bag", os.path.abspath(args.bag)] if args.bag else ["--msg", args.msg]
    command += [] if count is None else ["--count", str(count)]
    # Its own session: Ctrl-C reaches only armo react, which then ends it.
    return subprocess.Popen(
        command,
        env=env,
        stdin=subprocess.PIPE,
        stdout=subprocess.PIPE,
        text=True,
        start_new_session=True,
    )


def next_window(c):
    while True:
        m = c.next_message()
        if m["type"] == "window":
            return m


def react_windows(c, publisher, name):
    """The chain rows of `name` from when the publisher starts until it has ended and
    REACT_SETTLE_WINDOWS more windows have passed."""
    ready, seen = time.monotonic_ns(), 0
    while True:
        w = next_window(c)
        if w["t_mono_ns"] < ready:
            continue
        if any(p["pid"] == publisher.pid for p in w["processes"]):
            break
        seen += 1
        if seen == REACT_SEEN_WINDOWS:
            raise ArmoError(
                f"armo-collectd does not see the publisher (pid {publisher.pid}) after "
                f"{seen} windows"
            )
    publisher.stdin.write("go\n")
    publisher.stdin.flush()
    rows, settle = [], 0
    while settle < REACT_SETTLE_WINDOWS:
        w = next_window(c)
        rows += [x for x in w.get("chains", []) if x["name"] == name]
        settle += publisher.poll() is not None
    return rows


def cmd_react(args):
    name = f"react-{os.getpid()}"
    chain = {"name": name, "topics": [args.input] + args.output, "mode": "first"}
    env = dict(os.environ)
    env.update(armo_client.environment("preload", env))
    with armo_client.connect() as c:
        c.request("subscribe", streams=["state", "window"])
        monitor = c.state["monitor"]
        chains, selection = monitor.get("chains", []), monitor["selection"]
        publisher = react_publisher(args, env)
        select = react_selection(selection, monitor["method_by_pid"], publisher.pid)
        try:
            c.request("configure", chains=chains + [chain])
            if select is not None:
                c.request("select", **select)
            if publisher.stdout.readline() != "ready\n":
                raise ArmoError(f"the publisher ended with {publisher.wait()}")
            rows = react_windows(c, publisher, name)
            published = publisher.stdout.read().strip()
        finally:
            publisher.kill()
            publisher.wait()
            c.request("configure", chains=chains)
            if select is not None:
                c.request("select", method_by_pid=monitor["method_by_pid"], **selection)
    steps = [merge_lat(row["steps"][k] for row in rows) for k in range(len(chain["topics"]) - 1)]
    count, broken = sum(r["count"] for r in rows), sum(r["broken"] for r in rows)
    results = [
        [k + 1, topic, s["n"] if s else 0] + [s and s[f] for f in ("p50_ns", "p99_ns", "max_ns")]
        for k, (topic, s) in enumerate(zip(chain["topics"][1:], steps))
    ]
    print(f"{name} (first): {' -> '.join(chain['topics'])}, {published}")
    table(
        "steps",
        ["step", "topic", "n", "p50", "p99", "max"],
        [r[:3] + [ns(v) for v in r[3:]] for r in results],
    )
    print(f"count {count}, broken {broken}")
    print(f"note: {REACT_NOTE}")
    if args.output_csv:
        with open(args.output_csv, "w", newline="") as out:
            writer = csv.writer(out)
            writer.writerow(["step", "topic", "n", "p50_ns", "p99_ns", "max_ns", "count", "broken"])
            for r in results:
                writer.writerow(r[:3] + ["" if v is None else v for v in r[3:]] + [count, broken])
    return 0


def cmd_env(args):
    for name, value in armo_client.environment(args.method, os.environ).items():
        print(f"export {name}={shlex.quote(value)}")
    return 0


def chain_arg(text):
    try:
        return armo_client.parse_chain(text)
    except ArmoError as e:
        raise argparse.ArgumentTypeError(str(e)) from None


def add_monitor_options(p, chains=True):
    p.add_argument("--gpu", action="store_const", const=True, help="CUDA driver calls")
    p.add_argument("--dds", action="store_const", const=True, help="DDS write calls")
    p.add_argument("--futex", action="store_const", const=True, help="futex waits (armod)")
    if not chains:
        return
    p.add_argument(
        "--chain",
        dest="chains",
        type=chain_arg,
        action="append",
        metavar="NAME=T1,T2,...[:first]",
        help="an end-to-end chain over these topics; replaces the chains",
    )
    p.add_argument(
        "--latest-node",
        dest="latest_nodes",
        action="append",
        metavar="REGEX",
        help="link this node's outputs to its latest inputs",
    )


INFRA_HELP = "also /rosout, /parameter_events, ros_discovery_info, parameter services"


def from_window(names):
    """A completer: `names(window, parsed_args)` of armo-collectd's next window, or nothing when
    armo-collectd is not running (a completer must not fail or start it)."""

    def complete(prefix, parsed_args, **_):
        try:
            with armo_client.connect(start=False) as c:
                c.request("subscribe", streams=["window"])
                return sorted(set(names(view.visible(next_window(c)), parsed_args)))
        except (ArmoError, OSError):
            return []

    return complete


NODES = from_window(lambda w, _: (n["fqn"] for n in w["nodes"]))
TOPICS = from_window(lambda w, _: (t["name"] for t in w["topics"]))
SPANS = from_window(lambda w, _: (s["name"] for s in w["spans"]))


def node_callbacks(w, args):
    labels = view.callback_labels(w)
    return (labels[c["id"]] for c in of_nodes(w, [args.node])["callbacks"])


def node_topics(w, args):
    w = of_nodes(w, [args.node])
    ids = {s["topic"] for s in w["subscriptions"]}
    return (t["name"] for t in w["topics"] if t["id"] in ids)


def record_ids(prefix, parsed_args, **_):
    try:
        with armo_client.connect(start=False) as c:
            return [r["id"] for r in c.state["recordings"]]
    except (ArmoError, OSError):
        return []


def add_target_options(p, window=True):
    p.add_argument(
        "--pid",
        type=int,
        action="append",
        help="a process started without armo run; uprobe needs armod",
    )
    p.add_argument("--method", choices=METHODS, action="append")
    if window:
        p.add_argument("--window", type=float, metavar="S")


def parser():
    p = argparse.ArgumentParser(prog="armo", description="armo: ROS 2 and flux observation")
    sub = p.add_subparsers(dest="command", required=True)

    sub.add_parser("status", help="armo-collectd, armod, selection, recordings").set_defaults(
        run=cmd_status
    )

    m = sub.add_parser("top", help="the machine: CPU clocks, temperatures, network, GPUs")
    m.add_argument("--window", type=float, metavar="S")
    m.add_argument("--json", action="store_true", help="each window message, one per line")
    m.set_defaults(run=cmd_top)

    n = sub.add_parser("node", help="per-node values: CPU, callbacks, subscriptions")
    nsub = n.add_subparsers(dest="node_command", required=True)
    x = nsub.add_parser("list", help="the nodes of the observed processes")
    add_target_options(x, window=False)
    x.set_defaults(run=cmd_node_list)
    x = nsub.add_parser("info", help="pid, RSS, topics and callbacks of a node")
    x.add_argument("node", metavar="NODE").completer = NODES
    x.add_argument("--infra", action="store_true", help=INFRA_HELP)
    x.set_defaults(run=cmd_node_info)
    x = nsub.add_parser("cpu", help="CPU, busy time, callback rate and RSS")
    x.add_argument("nodes", nargs="*", metavar="NODE").completer = NODES
    add_target_options(x)
    x.set_defaults(run=cmd_node_cpu)
    x = nsub.add_parser("cb", help="each callback's rate, duration and dispatch wait")
    x.add_argument("node", metavar="NODE").completer = NODES
    cb = x.add_argument(
        "callback", nargs="?", metavar="CB", help="callbacks whose label contains CB"
    )
    cb.completer = from_window(node_callbacks)
    add_target_options(x)
    x.add_argument("--infra", action="store_true", help=INFRA_HELP)
    x.add_argument("--gpu", action="store_const", const=True, help="CUDA driver calls")
    x.add_argument("--futex", action="store_const", const=True, help="futex waits (armod)")
    x.set_defaults(run=cmd_node_cb)
    x = nsub.add_parser("sub", help="each subscription's receive rate, loss and wake latency")
    x.add_argument("node", metavar="NODE").completer = NODES
    x.add_argument("topic", nargs="?", metavar="TOPIC").completer = from_window(node_topics)
    add_target_options(x)
    x.add_argument("--infra", action="store_true", help=INFRA_HELP)
    x.set_defaults(run=cmd_node_sub)

    x = sub.add_parser("span", help="the spans marked in code: armo::Span, @span")
    x.add_argument("names", nargs="*", metavar="NAME").completer = SPANS
    add_target_options(x)
    x.set_defaults(run=cmd_span)

    t = sub.add_parser("topic", help="per-topic values, measured in the publishers and subscribers")
    tsub = t.add_subparsers(dest="topic_command", required=True)
    for name, text in (
        ("hz", "publish calls per second, and the publishes dropped"),
        ("bw", "bytes published per second"),
        ("delay", "publish to take, and with --dds to the DDS write"),
    ):
        x = tsub.add_parser(name, help=text)
        x.add_argument("topics", nargs="+", metavar="TOPIC").completer = TOPICS
        x.add_argument("--window", type=float, metavar="S")
        if name == "delay":
            x.add_argument("--dds", action="store_const", const=True, help="also the DDS write")
        x.set_defaults(run=cmd_topic_watch)
    x = tsub.add_parser("list", help="the topics of the observed processes")
    x.add_argument("--infra", action="store_true", help=INFRA_HELP)
    x.set_defaults(run=cmd_topic_list)
    x = tsub.add_parser("info", help="transport, publishers and subscribers")
    x.add_argument("topic", metavar="TOPIC").completer = TOPICS
    x.set_defaults(run=cmd_topic_info)

    r = sub.add_parser("record", help="start a recording; list, stop, show")
    r.add_argument("--method", choices=("auto",) + METHODS)
    who = r.add_mutually_exclusive_group()
    who.add_argument("--all", action="store_true")
    who.add_argument("--pid", type=int, action="append")
    r.add_argument("--node", metavar="REGEX").completer = NODES
    r.add_argument("--topic", metavar="REGEX").completer = TOPICS
    r.add_argument("--exclude", metavar="REGEX")
    r.add_argument("--kind", choices=("events", "windows"))
    r.add_argument("-o", dest="dir", metavar="DIR")
    r.add_argument("--name")
    r.add_argument("--max-duration", dest="max_duration_s", type=int, metavar="S")
    r.add_argument("--max-size", dest="max_bytes", type=int, metavar="BYTES")
    r.add_argument("--split-size", dest="split_bytes", type=int, metavar="BYTES")
    r.add_argument("--split-duration", dest="split_duration_s", type=int, metavar="S")
    r.add_argument("--cache", dest="cache_bytes", type=int, metavar="BYTES")
    r.add_argument("--compression", choices=("none", "zstd"))
    r.add_argument("--bandwidth", action="store_const", const=True)
    r.add_argument("--sched", action=argparse.BooleanOptionalAction)
    r.add_argument("--malloc", action="store_const", const=True)
    add_monitor_options(r)
    r.add_argument("--no-follow", action="store_true")
    r.set_defaults(run=cmd_record)
    rsub = r.add_subparsers(dest="record_command")
    rsub.add_parser("list").set_defaults(run=cmd_record_list)
    stop = rsub.add_parser("stop")
    stop.add_argument("record_id").completer = record_ids
    stop.set_defaults(run=cmd_record_stop)
    show = rsub.add_parser("show")
    show.add_argument("target", metavar="ID_OR_DIR")
    show.set_defaults(run=cmd_record_show)

    a = sub.add_parser("analyze", help="split a recording into the 2.5 intervals")
    a.add_argument("dir")
    a.add_argument("--chain", action="append", default=[], metavar="NAME=T1,T2,...[:first]")
    a.set_defaults(run=cmd_analyze)

    e = sub.add_parser("export", help="CSV of a recording")
    e.add_argument("dir")
    e.add_argument("-o", dest="output", required=True)
    e.add_argument("--metric", action="append", choices=EXPORT_METRICS)
    e.set_defaults(run=cmd_export)

    s = sub.add_parser("check", help="check this machine's assumptions (selfcheck)")
    s.add_argument("--duration", type=float, default=10.0, metavar="S")
    s.set_defaults(run=cmd_check)

    x = sub.add_parser("run", help="run a command with the method's environment")
    x.add_argument("--method", choices=LAUNCH_METHODS, default="preload")
    x.add_argument("command", nargs=argparse.REMAINDER)
    x.set_defaults(run=cmd_run)

    x = sub.add_parser("react", help="publish test messages and time the first outputs")
    x.add_argument("--input", required=True, metavar="TOPIC")
    x.add_argument("--type", required=True, metavar="PKG/msg/TYPE")
    source = x.add_mutually_exclusive_group(required=True)
    source.add_argument("--msg", metavar="YAML")
    source.add_argument("--bag", metavar="DIR", help="the bag's messages on the input topic")
    x.add_argument("--output", required=True, action="append", metavar="TOPIC")
    x.add_argument(
        "--count", type=int, metavar="N", help=f"default {REACT_COUNT}, or the whole bag"
    )
    x.add_argument("--period", type=float, default=1.0, metavar="S")
    x.add_argument("-o", dest="output_csv", metavar="CSV")
    x.set_defaults(run=cmd_react)

    x = sub.add_parser("env", help="print the method's environment as shell exports")
    x.add_argument("--method", choices=LAUNCH_METHODS, default="preload")
    x.set_defaults(run=cmd_env)
    return p


def main(argv=None):
    p = parser()
    argcomplete.autocomplete(p)
    args = p.parse_args(argv)
    try:
        return args.run(args)
    except (ArmoError, OSError) as e:
        print(f"armo: {e}", file=sys.stderr)
        return 1
    except KeyboardInterrupt:
        return 130
