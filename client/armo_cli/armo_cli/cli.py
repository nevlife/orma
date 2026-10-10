"""The armo command."""

import argparse
import json
import os
import shlex
import sys

import armo_client
from armo_client import ArmoError, view

METHODS = ("uprobe", "preload", "lttng")
LAUNCH_METHODS = ("preload", "lttng")


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
    """Tail first: p99, p99.9, max, then p50."""
    if value is None:
        return ["-"] * 4
    return [ns(value[k]) for k in ("p99_ns", "p999_ns", "max_ns", "p50_ns")]


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


def print_processes(w):
    table(
        "processes",
        ["pid", "comm", "method", "cpu%", "rss", "pss", "shared cpu%", "exited"],
        [
            [p["pid"], p["comm"], p["method"] or "-", num(p["cpu_pct"]), size(p["rss_bytes"])]
            + [size(p["pss_bytes"]), num(p["shared_cpu_pct"]), "yes" if p["exited"] else ""]
            for p in w["processes"]
        ],
    )
    for p in w["processes"]:
        overflow = p.get("overflow") or {}
        lost = [f"{overflow[k]} {what}" for k, what in OVERFLOW if overflow.get(k)]
        if lost:
            print(f"warning: pid {p['pid']} ({p['comm']}): {', '.join(lost)}")
    a = w["agent"]
    armod = a["armod"]
    print(
        f"armo: collectd cpu {num(a['collectd']['cpu_pct'])}% rss {size(a['collectd']['rss_bytes'])}"
        f", armod cpu {num(armod and armod['cpu_pct'])}% rss {size(armod and armod['rss_bytes'])}"
        f", shm files {a['shm']['files']} ring drops {a['shm']['ring_drops']}"
        f", probes missing {len(a['probes_missing'])}, collect {ns(a['collect_ns'])}"
    )


def print_nodes(w):
    table(
        "nodes",
        ["node", "pid", "cpu%", "source", "busy%", "cb Hz"],
        [
            [n["fqn"], n["pid"], num(n["cpu_pct"]), n["cpu_source"], num(n["busy_pct"])]
            + [num(n["cb_hz"])]
            for n in w["nodes"]
        ],
    )


LAT_HEADS = ["p99", "p99.9", "max", "p50"]


def print_callbacks(w):
    labels = view.callback_labels(w)
    table(
        "callbacks",
        ["callback", "node", "pid", "Hz", "expected"] + LAT_HEADS + ["dispatch p99"],
        [
            [labels[c["id"]], view.owner_label(w, c["node"] or c["id"]), view.pid_of(c["id"])]
            + [num(c["hz"]), num(c["expected_hz"])]
            + lat(c["duration"])
            + [lat(c["dispatch"])[0]]
            for c in view.callbacks_in_order(w)
        ],
    )


def print_topics(w):
    table(
        "topics",
        ["topic", "transport", "pub Hz", "expected", "bw", "drops"]
        + ["hop " + h for h in LAT_HEADS],
        [
            [t["name"], t["transport"], num(t["pub_hz"]), num(t["expected_hz"])]
            + [size(t["bw_bps"]) + "/s" if t["bw_bps"] is not None else "-"]
            + [num(t["drops"]["publish"], "{}")]
            + lat(t["hop"])
            for t in w["topics"]
        ],
    )


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


# View name: the window field it shows, and its printer.
VIEWS = {
    "proc": ("processes", print_processes),
    "node": ("nodes", print_nodes),
    "cb": ("callbacks", print_callbacks),
    "topic": ("topics", print_topics),
    "sub": ("subscriptions", print_subscriptions),
}


def print_window(w, name, selection=None, infra=False):
    behind = " behind" if w["behind"] else ""
    print(f"window {w['seq']}  {w['window_ns'] / 1e9:g}s{behind}")
    VIEWS[name][1](view.visible(w, selection, infra))


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
        extra = "".join(f", {k}" for k in ("bandwidth", "sched", "malloc") if m[k])
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
        print_recordings(c.state["recordings"])
    return 0


def cmd_top(args):
    if args.view is None and not args.json:
        raise ArmoError(f"armo top needs a view: {', '.join(VIEWS)}")
    methods = args.method or []
    if methods and not args.pid:
        raise ArmoError("--method needs --pid")
    names = {k: getattr(args, k) for k in ("node", "topic", "exclude") if getattr(args, k)}
    if len(methods) not in (0, 1, len(args.pid)):
        raise ArmoError("give --method once, or once per --pid")
    methods = methods * len(args.pid) if len(methods) == 1 else methods
    with armo_client.connect() as c:
        # First, so that the event lines `select` raises reach this client.
        c.request("subscribe", streams=["state", "window", "event_line"])
        if args.window is not None:
            c.request("configure", window_s=args.window)
        c.request(
            "select",
            all=not (args.pid or names),
            pids=args.pid,
            method_by_pid={str(p): m for p, m in zip(args.pid, methods)},
            **names,
        )
        try:
            while True:
                m = c.next_message()
                if m["type"] == "window":
                    selection = c.state["monitor"]["selection"]
                    if args.json and args.view:
                        field = VIEWS[args.view][0]
                        shown = view.visible(m, selection, args.infra)
                        m = {"seq": m["seq"], "window_ns": m["window_ns"], field: shown[field]}
                    if args.json:
                        print(json.dumps(m, separators=(",", ":")))
                    else:
                        print_window(m, args.view, selection, args.infra)
                        print()
                elif m["type"] == "event_line" and not args.json:
                    print(f"[{m['level']}] {m['kind']}: {m['text']}")
                sys.stdout.flush()
        except KeyboardInterrupt:
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
    os.execvp("armo-analyze", ["armo-analyze", os.path.abspath(args.dir)])


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


def cmd_env(args):
    for name, value in armo_client.environment(args.method, os.environ).items():
        print(f"export {name}={shlex.quote(value)}")
    return 0


def parser():
    p = argparse.ArgumentParser(prog="armo", description="armo: ROS 2 and flux observation")
    sub = p.add_subparsers(dest="command", required=True)

    sub.add_parser("status", help="armo-collectd, armod, selection, recordings").set_defaults(
        run=cmd_status
    )

    m = sub.add_parser("top", help="print one view of each window")
    m.add_argument("view", nargs="?", choices=tuple(VIEWS))
    m.add_argument("--pid", type=int, action="append", default=[])
    m.add_argument("--method", choices=METHODS, action="append")
    m.add_argument("--node", metavar="REGEX", help="the processes of the nodes it names")
    m.add_argument("--topic", metavar="REGEX", help="the publishers and subscribers")
    m.add_argument("--exclude", metavar="REGEX", help="leave out these nodes and topics")
    m.add_argument(
        "--infra",
        action="store_true",
        help="also show /rosout, /parameter_events, ros_discovery_info, parameter services",
    )
    m.add_argument("--window", type=float, metavar="S")
    m.add_argument(
        "--json", action="store_true", help="one window message per line, only the view if given"
    )
    m.set_defaults(run=cmd_top)

    r = sub.add_parser("record", help="start a recording; list, stop, show")
    r.add_argument("--method", choices=("auto",) + METHODS)
    who = r.add_mutually_exclusive_group()
    who.add_argument("--all", action="store_true")
    who.add_argument("--pid", type=int, action="append")
    r.add_argument("--node", metavar="REGEX")
    r.add_argument("--topic", metavar="REGEX")
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
    r.add_argument("--no-follow", action="store_true")
    r.set_defaults(run=cmd_record)
    rsub = r.add_subparsers(dest="record_command")
    rsub.add_parser("list").set_defaults(run=cmd_record_list)
    stop = rsub.add_parser("stop")
    stop.add_argument("record_id")
    stop.set_defaults(run=cmd_record_stop)
    show = rsub.add_parser("show")
    show.add_argument("target", metavar="ID_OR_DIR")
    show.set_defaults(run=cmd_record_show)

    a = sub.add_parser("analyze", help="split a recording into the 2.5 intervals")
    a.add_argument("dir")
    a.set_defaults(run=cmd_analyze)

    e = sub.add_parser("export", help="CSV of a recording")
    e.add_argument("dir")
    e.add_argument("-o", dest="output", required=True)
    e.add_argument("--metric", action="append")
    e.set_defaults(run=cmd_export)

    s = sub.add_parser("check", help="check this machine's assumptions (selfcheck)")
    s.add_argument("--duration", type=float, default=10.0, metavar="S")
    s.set_defaults(run=cmd_check)

    x = sub.add_parser("run", help="run a command with the method's environment")
    x.add_argument("--method", choices=LAUNCH_METHODS, default="preload")
    x.add_argument("command", nargs=argparse.REMAINDER)
    x.set_defaults(run=cmd_run)

    x = sub.add_parser("env", help="print the method's environment as shell exports")
    x.add_argument("--method", choices=LAUNCH_METHODS, default="preload")
    x.set_defaults(run=cmd_env)
    return p


def main(argv=None):
    args = parser().parse_args(argv)
    try:
        return args.run(args)
    except (ArmoError, OSError) as e:
        print(f"armo: {e}", file=sys.stderr)
        return 1
    except KeyboardInterrupt:
        return 130
