"""The rqt_armo main widget: a client of armo-collectd that shows `state`, `window`,
`event_line` and requested `detail` messages and nothing else."""

import datetime

from python_qt_binding.QtCore import Qt, QTimer
from python_qt_binding.QtWidgets import (
    QHBoxLayout,
    QLabel,
    QPushButton,
    QSplitter,
    QTableWidgetItem,
    QTabWidget,
    QVBoxLayout,
    QWidget,
)

from .common import icon, make_table, ns, num, size
from .connection import Connection
from .graph import GraphView
from .monitor import DetailDialog, MonitorView
from .recordings import RecordingsView
from .selection import SelectionView

RETRY_MS = 2000
MAX_EVENTS = 1000
ARMOD_STATUS = {"connected": "ok", "absent": "warn", "denied": "warn", "lost": "error"}


class StatusLabel(QWidget):
    """A Qt default status icon next to a text."""

    def __init__(self, parent=None):
        super().__init__(parent)
        self.icon = QLabel()
        self.text = QLabel()
        layout = QHBoxLayout(self)
        layout.setContentsMargins(0, 0, 0, 0)
        layout.addWidget(self.icon)
        layout.addWidget(self.text)

    def set(self, text, status):
        self.text.setText(text)
        side = self.text.fontMetrics().height()
        self.icon.setPixmap(icon(status).pixmap(side, side))


class ArmoWidget(QWidget):
    def __init__(self, parent=None):
        super().__init__(parent)
        self.setObjectName("ArmoWidget")
        self.setWindowTitle("armo")
        self.conn = Connection(self)
        self.closed = False
        self.state = None
        self.window_msg = None
        self.detail = None

        self.collectd = StatusLabel()
        self.armod = StatusLabel()
        self.lttng = StatusLabel()
        self.monitor_label = QLabel()
        self.cost = StatusLabel()
        self.connect_button = QPushButton("Connect")
        self.connect_button.clicked.connect(lambda: self._open(start=True))
        bar = QHBoxLayout()
        for w in (self.collectd, self.armod, self.lttng):
            bar.addWidget(w)
        bar.addStretch()
        bar.addWidget(self.connect_button)
        bar2 = QHBoxLayout()
        bar2.addWidget(self.monitor_label)
        bar2.addStretch()
        bar3 = QHBoxLayout()
        bar3.addWidget(self.cost)
        bar3.addStretch()

        self.monitor = MonitorView()
        self.monitor.detail_requested.connect(self.request_detail)
        self.graph = GraphView()
        self.recordings = RecordingsView(self.conn)
        self.selection = SelectionView(self.conn)
        self.tabs = QTabWidget()
        self.tabs.addTab(self.monitor, "Monitor")
        self.tabs.addTab(self.graph, "Graph")
        self.tabs.addTab(self.recordings, "Recordings")
        self.tabs.addTab(self.selection, "Selection")
        self.events = make_table(["time", "level", "kind", "text", "entities"])
        split = QSplitter(Qt.Vertical)
        split.addWidget(self.tabs)
        split.addWidget(self.events)
        split.setStretchFactor(0, 4)
        layout = QVBoxLayout(self)
        layout.addLayout(bar)
        layout.addLayout(bar2)
        layout.addLayout(bar3)
        layout.addWidget(split)

        self.retry = QTimer(self)
        self.retry.setSingleShot(True)
        self.retry.setInterval(RETRY_MS)
        self.retry.timeout.connect(lambda: self.closed or self._open(start=False))
        # Signals from the reader thread are queued; one may arrive after shutdown.
        self.conn.connected.connect(lambda server: self.closed or self._connected(server))
        self.conn.message.connect(lambda m: self.closed or self._message(m))
        self.conn.lost.connect(lambda reason: self.closed or self._lost(reason))
        self._set_connected(False)
        self._open(start=True)

    def _open(self, start):
        self.retry.stop()
        self.connect_button.setEnabled(False)
        self.collectd.set("armo-collectd: connecting", "info")
        self.conn.open(start)

    def _set_connected(self, connected):
        self.connect_button.setEnabled(not connected)
        self.selection.setEnabled(connected)
        self.recordings.set_connected(connected)

    def _connected(self, server):
        self.collectd.set(
            f"armo-collectd: connected ({server['server']} {server['version']})", "ok"
        )
        self.conn.send("subscribe", streams=["state", "window", "event_line"])
        self._set_connected(True)

    def _lost(self, reason):
        self.conn.drop()
        self.collectd.set(f"armo-collectd: disconnected ({reason})", "error")
        self.add_event("error", "connection", reason)
        self._set_connected(False)
        self.retry.start()

    def _message(self, m):
        kind = m["type"]
        if kind == "state":
            self.show_state(m)
        elif kind == "window":
            self.show_window(m)
        elif kind == "event_line":
            self.add_event(m["level"], m["kind"], m["text"], m["ids"], m["t_real_ns"])
        elif kind == "error":
            self.add_event("error", m["code"], m["message"])

    def show_state(self, s):
        self.state = s
        src, m = s["sources"], s["monitor"]
        self.armod.set(f"armod: {src['armod']}", ARMOD_STATUS[src["armod"]])
        lttng = src["lttng"] == "available"
        self.lttng.set(
            f"LTTng: {src['lttng']}, preload processes: {src['preload_processes']}",
            "ok" if lttng else "info",
        )
        selection = m["selection"]
        if selection["all"]:
            target = "all processes"
        else:
            target = (
                " ".join(f"{p}:{m['method_by_pid'].get(str(p), 'auto')}" for p in selection["pids"])
                or "none"
            )
        target += "".join(
            f" {k} {selection[k]}" for k in ("node", "topic", "exclude") if k in selection
        )
        extras = "".join(f", {k}" for k in ("bandwidth", "sched", "malloc", "pss") if m[k])
        running = "running" if m["running"] else "idle"
        self.monitor_label.setText(
            f"monitor: {running}, window {m['window_s']:g} s, {target}{extras}"
        )
        self.recordings.show_state(s)
        self.selection.show_state(s)
        self.monitor.selection = selection

    def show_window(self, w):
        self.window_msg = w
        self.monitor.show_window(w)
        self.graph.show_window(w)
        self.recordings.processes = w["processes"]
        self.selection.show_processes(w["processes"])
        a = w["agent"]
        bpf_ns = sum(b["run_time_ns"] for b in a["bpf"])
        armod = a["armod"]
        text = (
            f"armo cost: collectd {num(a['collectd']['cpu_pct'])}% {size(a['collectd']['rss_bytes'])}"
            f", armod {num(armod and armod['cpu_pct'])}%, eBPF {ns(bpf_ns)}/window"
            f", ring drops {a['shm']['ring_drops']}, probes missing {len(a['probes_missing'])}"
        )
        if w["behind"]:
            text += ", behind"
        warn = w["behind"] or a["shm"]["ring_drops"] or a["probes_missing"]
        self.cost.set(text, "warn" if warn else "ok")

    def add_event(self, level, kind, text, ids=(), t_real_ns=None):
        when = (
            datetime.datetime.now()
            if t_real_ns is None
            else datetime.datetime.fromtimestamp(t_real_ns / 1e9)
        )
        table = self.events
        if table.rowCount() >= MAX_EVENTS:
            table.removeRow(table.rowCount() - 1)
        table.insertRow(0)
        cells = [when.strftime("%H:%M:%S.%f")[:-3], level, kind, text, ", ".join(ids)]
        for col, cell in enumerate(cells):
            item = QTableWidgetItem(cell)
            if col == 1:
                item.setIcon(icon(level))
            table.setItem(0, col, item)

    def request_detail(self, entity):
        self.conn.send("detail", on_reply=self._detail, entity=entity, what="hist")

    def _detail(self, reply):
        self.detail = DetailDialog(reply, self)
        self.detail.show()

    def shutdown(self):
        """Closing the GUI only says bye; recordings carry on in armo-collectd."""
        self.closed = True
        self.retry.stop()
        self.conn.close()
