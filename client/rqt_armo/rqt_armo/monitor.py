"""Entity views of a `window`: process > node > callback, topics, subscriptions, executors,
threads, and armo's own cost."""

from python_qt_binding.QtCore import Qt, Signal
from armo_client import view
from python_qt_binding.QtWidgets import (
    QCheckBox,
    QDialog,
    QHeaderView,
    QLineEdit,
    QTabWidget,
    QTreeWidget,
    QTreeWidgetItem,
    QVBoxLayout,
    QWidget,
)

from .common import (
    KEY,
    icon,
    lat,
    lat_heads,
    make_sortable,
    make_table,
    ns,
    num,
    row_key,
    set_rows,
    size,
    warn_if,
)

TREE_HEADS = ["entity", "kind", "Hz", "expected", "loss"] + lat_heads() + ["CPU %"]
ENTITY_WIDTH = 480
TOPIC_HEADS = ["topic", "transport", "pub Hz", "expected", "bandwidth", "drops", "record drops"]
TOPIC_HEADS += ["ambiguous"] + lat_heads("hop ") + ["age p99"]
SUB_HEADS = ["topic", "node", "recv Hz", "loss %", "lost", "intra %"] + lat_heads("hop ")
SUB_HEADS += ["wake p99", "wake max"]
EXECUTOR_HEADS = ["executor", "kind", "nodes", "callbacks", "busy %", "on-CPU %"]
EXECUTOR_HEADS += lat_heads("wait ") + lat_heads("dispatch ")
THREAD_HEADS = ["tid", "process", "name", "policy", "CPU %", "in callback %"] + lat_heads(
    "CPU wait "
)
THREAD_HEADS += ["bucket", "cpus"]
BPF_HEADS = ["program", "pid", "run time", "runs"]
MAP_HEADS = ["map", "used", "max", "memlock"]
MISSING_HEADS = ["pid", "method", "point", "reason"]


def process_of(entity_id):
    """`n:`, `c:`, `t:` and `e:` ids start with the process's `<pid>:<starttime>`."""
    return "p:" + ":".join(entity_id.split(":")[1:3])


def process_cells(p):
    comm = (
        f"{p['comm']} ({p['pid']})"
        if "ns_pid" not in p
        else f"{p['comm']} ({p['pid']}/{p['ns_pid']})"
    )
    flags = [f for f in ("exited", "cold") if p[f]]
    kind = ", ".join(([p["method"]] if p["method"] else []) + flags) or "-"
    return [comm, kind, "-", "-", "-"] + ["-"] * 4 + [num(p["cpu_pct"])]


def node_cells(n):
    return [n["fqn"], "node", num(n["cb_hz"]), "-", "-"] + ["-"] * 4 + [num(n["cpu_pct"])]


def callback_cells(c, label):
    return [label, c["kind"], num(c["hz"]), num(c["expected_hz"]), "-"] + lat(c["duration"]) + ["-"]


class MonitorView(QTabWidget):
    detail_requested = Signal(str)

    def __init__(self, parent=None):
        super().__init__(parent)
        self.tree = QTreeWidget()
        self.tree.setHeaderLabels(TREE_HEADS)
        self._fit_value_columns()
        # Callback names run to hundreds of characters of C++ type; the full name is the tooltip.
        self.tree.header().setSectionResizeMode(0, QHeaderView.Interactive)
        self.tree.header().resizeSection(0, ENTITY_WIDTH)
        self.tree.itemDoubleClicked.connect(
            lambda item, _: self.detail_requested.emit(item.data(0, KEY))
        )
        self.topics = make_table(TOPIC_HEADS)
        self.subscriptions = make_table(SUB_HEADS)
        self.executors = make_table(EXECUTOR_HEADS)
        self.threads = make_table(THREAD_HEADS)
        make_sortable(self.threads)
        self.thread_search = QLineEdit()
        self.thread_search.setPlaceholderText("Search tid, process or thread name")
        self.thread_search.setClearButtonEnabled(True)
        self.thread_search.textChanged.connect(
            lambda _: self._last and self.show_window(self._last)
        )
        threads_tab = QWidget()
        threads_layout = QVBoxLayout(threads_tab)
        threads_layout.setContentsMargins(0, 0, 0, 0)
        threads_layout.addWidget(self.thread_search)
        threads_layout.addWidget(self.threads)
        self.bpf = make_table(BPF_HEADS)
        self.maps = make_table(MAP_HEADS)
        self.missing = make_table(MISSING_HEADS)
        for table in (self.topics, self.subscriptions, self.executors, self.threads):
            table.cellDoubleClicked.connect(
                lambda row, _, t=table: self.detail_requested.emit(row_key(t, row))
            )
        self.addTab(self.tree, "Processes")
        self.addTab(self.topics, "Topics")
        self.addTab(self.subscriptions, "Subscriptions")
        self.addTab(self.executors, "Executors")
        self.addTab(threads_tab, "Threads")
        cost = QTabWidget()
        cost.addTab(self.bpf, "eBPF")
        cost.addTab(self.maps, "Maps")
        cost.addTab(self.missing, "Probes missing")
        self.addTab(cost, "armo cost")
        self.infra = QCheckBox("Internal topics and services")
        self.infra.toggled.connect(lambda _: self._last and self.show_window(self._last))
        self.setCornerWidget(self.infra)
        self.selection = None
        self.collapsed = set()
        self._items = {}
        self._last = None

    def show_window(self, raw):
        self._last = raw
        w = view.visible(raw, self.selection, self.infra.isChecked())
        topics = {t["id"]: t["name"] for t in w["topics"]}
        self._fill_tree(w)
        set_rows(
            self.topics,
            [
                (
                    t["id"],
                    [t["name"], t["transport"], num(t["pub_hz"]), num(t["expected_hz"])]
                    + ["-" if t["bw_bps"] is None else size(t["bw_bps"]) + "/s"]
                    + [warn_if(t["drops"]["publish"]), warn_if(t["drops"]["record"])]
                    + [warn_if(t["drops"]["ambiguous"])]
                    + lat(t["hop"])
                    + [lat(t["age_pub"])[0]],
                )
                for t in w["topics"]
            ],
        )
        set_rows(
            self.subscriptions,
            [
                (
                    s["id"],
                    [topics[s["topic"]], view.owner_label(w, s["node"])]
                    + [num(s["recv_hz"]), warn_if(s["loss_pct"], num(s["loss_pct"]))]
                    + [warn_if(s["lost"]), num(s["intra_pct"])]
                    + lat(s["hop"])
                    + [lat(s["wake"])[0], lat(s["wake"])[2]],
                )
                for s in w["subscriptions"]
            ],
        )
        set_rows(
            self.executors,
            [
                (
                    e["id"],
                    [e["id"], e["kind"], len(e["nodes"]), len(e["callbacks"])]
                    + [num(e["busy_pct"]), num(e["oncpu_pct"])]
                    + lat(e["wait"])
                    + lat(e["dispatch"]),
                )
                for e in w["executors"]
            ],
        )
        comms = {p["pid"]: f"{p['comm']} ({p['pid']})" for p in w["processes"]}
        search = self.thread_search.text().strip().lower()
        threads = [(t, comms.get(view.pid_of(t["id"]), "-")) for t in w["threads"]]
        threads = [
            (t, comm) for t, comm in threads if search in f"{t['tid']} {comm} {t['name']}".lower()
        ]
        set_rows(
            self.threads,
            [
                (
                    t["id"],
                    [t["tid"], comm, t["name"], f"{t['policy']} {t['prio']}", num(t["cpu_pct"])]
                    + [num(t["in_callback_cpu_pct"])]
                    + lat(t["cpu_wait"])
                    + [t["bucket"], ",".join(str(c) for c in t["cpus"])],
                )
                for t, comm in threads
            ],
        )
        a = w["agent"]
        set_rows(
            self.bpf,
            [
                (b["prog"], [b["prog"], b["pid"] or "all", ns(b["run_time_ns"]), b["run_cnt"]])
                for b in a["bpf"]
            ],
        )
        set_rows(
            self.maps,
            [
                (m["name"], [m["name"], m["used"], m["max"], size(m["memlock_bytes"])])
                for m in a["maps"]
            ],
        )
        set_rows(
            self.missing,
            [
                (m["point"], [(m["pid"], "warn"), m["method"], m["point"], m["reason"]])
                for m in a["probes_missing"]
            ],
        )

    def _fit_value_columns(self):
        for column in range(1, self.tree.columnCount()):
            self.tree.header().setSectionResizeMode(column, QHeaderView.ResizeToContents)

    def _fill_tree(self, w):
        """Rebuilt every window; expansion, selection and scroll carry over by entity id."""
        tree = self.tree
        for key, item in self._items.items():
            if item.childCount() and not item.isExpanded():
                self.collapsed.add(key)
            elif item.isExpanded():
                self.collapsed.discard(key)
        selected = tree.currentItem().data(0, KEY) if tree.currentItem() else None
        scroll = tree.verticalScrollBar().value()
        tree.header().setSectionResizeMode(QHeaderView.Interactive)
        tree.clear()
        items = {}

        def add(key, parent, cells, status=None):
            item = QTreeWidgetItem(
                tree if parent is None else items[parent], [str(c) for c in cells]
            )
            item.setData(0, KEY, key)
            item.setToolTip(0, str(cells[0]))
            if status:
                item.setIcon(0, icon(status))
            items[key] = item
            return item

        for p in w["processes"]:
            add(p["id"], None, process_cells(p), "info" if p["exited"] else None)
        for n in w["nodes"]:
            add(n["id"], process_of(n["id"]), node_cells(n))
        labels = view.callback_labels(w)
        for c in view.callbacks_in_order(w):
            parent = c["node"]
            if parent not in items:
                process = process_of(c["id"])
                parent = process + ":" + view.UNLINKED
                if parent not in items:
                    label = view.owner_label(w, process)
                    add(parent, process, [label] + ["-"] * 9)
            add(c["id"], parent, callback_cells(c, labels[c["id"]]))
        for key, item in items.items():
            item.setExpanded(key not in self.collapsed)
            if key == selected:
                tree.setCurrentItem(item)
        self._fit_value_columns()
        tree.verticalScrollBar().setValue(scroll)
        self._items = items


class DetailDialog(QDialog):
    def __init__(self, reply, parent=None):
        super().__init__(parent)
        self.setWindowTitle(f"{reply['entity']} ({reply['what']})")
        layout = QVBoxLayout(self)
        rows = []
        for field, hist in sorted(reply["hists"].items()):
            for i, count in enumerate(hist["counts"]):
                if count:
                    rows.append((field, [field, f"{ns(1 << i)} - {ns(2 << i)}", count]))
        self.table = make_table(["field", "bucket", "count"])
        set_rows(self.table, rows)
        layout.addWidget(self.table)
        self.resize(480, 360)
        self.setAttribute(Qt.WA_DeleteOnClose)
