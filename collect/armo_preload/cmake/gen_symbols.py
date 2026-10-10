#!/usr/bin/env python3
"""Write the headers that bind armo_intercept to this distro's libraries (design 2.1, 2.1.2).

    gen_symbols.py NM LIBTRACETOOLS LIBRCLCPP RCLCPP_MAJOR OUTDIR

armo_tracepoints.h: ARMO_TP_<name> for each ros_trace_<name> the library exports, and
ARMO_TP_GATES when it exports the gated enabled_/do_ forms.

armo_rclcpp_symbols.h: ARMO_RCLCPP_SYMBOLS(X) with X(id, mangled name, event, argument count, kind,
index) for each caught rclcpp symbol, and ARMO_RCLCPP_UNBOUND(X) with the events of the points
this librclcpp has no single symbol for. A symbol is found by the front of its mangled name; a
front that matches two functions binds neither (constructor C1/C2 aliases count as one function).
"""

import re
import subprocess
import sys
from pathlib import Path

# (id, front of the mangled name, argument count with `this`, event, kind, index of the object
# argument). Kinds: ARG_NONE, ARG_VALUE (an integer), ARG_DEREF (a shared_ptr's object), ARG_IPM.
COMMON = [
    (
        "publisher_setup_intra",
        "_ZN6rclcpp13PublisherBase19setup_intra_processE",
        3,
        "PUBLISHER_SETUP_INTRA",
        "ARG_VALUE",
        1,
    ),
    (
        "subscription_setup_intra",
        "_ZN6rclcpp16SubscriptionBase19setup_intra_processE",
        3,
        "SUBSCRIPTION_SETUP_INTRA",
        "ARG_VALUE",
        1,
    ),
    (
        "ipm_insert_sub_id_for_pub",
        "_ZN6rclcpp12experimental19IntraProcessManager21insert_sub_id_for_pubE",
        4,
        "IPM_INSERT_SUB_ID_FOR_PUB",
        "ARG_IPM",
        0,
    ),
    (
        "group_add_subscription",
        "_ZN6rclcpp13CallbackGroup16add_subscriptionE",
        2,
        "GROUP_ADD_SUBSCRIPTION",
        "ARG_DEREF",
        1,
    ),
    (
        "group_add_timer",
        "_ZN6rclcpp13CallbackGroup9add_timerE",
        2,
        "GROUP_ADD_TIMER",
        "ARG_DEREF",
        1,
    ),
]

COLLECTOR = "_ZN6rclcpp9executors25ExecutorEntitiesCollector"

# rclcpp major -> points. 16 is Humble; 28 and later (Jazzy on) have the executor collector.
SETS = {
    16: COMMON
    + [
        ("publisher_base", "_ZN6rclcpp13PublisherBaseC", 5, "PUBLISHER_BASE", "ARG_NONE", 0),
        ("executor", "_ZN6rclcpp8ExecutorC", 2, "EXECUTOR", "ARG_NONE", 0),
        (
            "executor_add_group",
            "_ZN6rclcpp8Executor25add_callback_group_to_mapE",
            5,
            "EXECUTOR_ADD_GROUP",
            "ARG_DEREF",
            1,
        ),
    ],
    28: COMMON
    + [
        ("publisher_base", "_ZN6rclcpp13PublisherBaseC", 7, "PUBLISHER_BASE", "ARG_NONE", 0),
        ("executor", "_ZN6rclcpp8ExecutorC", 2, "EXECUTOR", "ARG_NONE", 0),
        ("collector", COLLECTOR + "C", 2, "COLLECTOR", "ARG_DEREF", 1),
        (
            "collector_add_group_to_collection",
            COLLECTOR + "32add_callback_group_to_collectionE",
            3,
            "COLLECTOR_ADD_GROUP_TO_COLLECTION",
            "ARG_DEREF",
            1,
        ),
        (
            "collector_add_group",
            COLLECTOR + "18add_callback_groupE",
            2,
            "COLLECTOR_ADD_GROUP",
            "ARG_DEREF",
            1,
        ),
    ],
}

# Both Executor constructors (options, context) take one reference: the same wrapper fits each.
OVERLOADS_ALLOWED = {"executor"}


def exports(nm, lib):
    out = subprocess.run(
        [nm, "-D", "--defined-only", lib], capture_output=True, text=True, check=True
    ).stdout
    return [
        f[2] for f in (line.split() for line in out.splitlines()) if len(f) == 3 and f[1] == "T"
    ]


def tracepoints(names):
    tps = sorted(
        n[len("ros_trace_") :]
        for n in names
        if n.startswith("ros_trace_") and not n.startswith(("ros_trace_do_", "ros_trace_enabled_"))
    )
    lines = [f"#define ARMO_TP_{t} 1" for t in tps]
    if any(n.startswith("ros_trace_do_") for n in names):
        lines.append("#define ARMO_TP_GATES 1")
    return lines


def rclcpp_points(names, major):
    if major != 16 and major < 28:
        sys.exit(f"gen_symbols: rclcpp {major}: armo binds rclcpp 16 (Humble) and 28 or later")
    spec = SETS[16] if major == 16 else SETS[28]
    bound, unbound = [], []
    for ident, front, nargs, event, kind, index in spec:
        hits = sorted(n for n in names if n.startswith(front))
        functions = {
            re.sub(r"^(.*?C)[12](E.*)$", r"\1\2", n) if front.endswith("C") else n for n in hits
        }
        if not hits or (len(functions) > 1 and ident not in OVERLOADS_ALLOWED):
            print(
                f"armo_preload: rclcpp {major}: {front} matches {len(functions)} functions; "
                "not caught",
                file=sys.stderr,
            )
            unbound.append(event)
            continue
        for i, name in enumerate(hits):
            bound.append(
                f'  X(cxx_{ident}_{i}, "{name}", ARMO_EV_RCLCPP_{event}, {nargs}, {kind}, {index})'
            )
    return bound, unbound


def write(path, text):
    if not path.exists() or path.read_text() != text:
        path.write_text(text)


def main():
    nm, tracetools, rclcpp, major, outdir = sys.argv[1:]
    out = Path(outdir) / "armo_preload"
    out.mkdir(parents=True, exist_ok=True)
    head = "// Generated by armo_preload/cmake/gen_symbols.py from {}.\n"

    tp = tracepoints(exports(nm, tracetools))
    write(out / "armo_tracepoints.h", head.format(tracetools) + "\n".join(tp) + "\n")

    bound, unbound = rclcpp_points(exports(nm, rclcpp), int(major))
    text = head.format(rclcpp)
    text += "#define ARMO_RCLCPP_SYMBOLS(X) \\\n" + " \\\n".join(bound) + "\n"
    text += "#define ARMO_RCLCPP_UNBOUND(X)" + "".join(f" X(ARMO_EV_RCLCPP_{e})" for e in unbound)
    write(out / "armo_rclcpp_symbols.h", text + "\n")


if __name__ == "__main__":
    main()
