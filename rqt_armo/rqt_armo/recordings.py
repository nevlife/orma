"""Recordings: the list, the start form, stop, and the validation table."""

import armo_client
from python_qt_binding.QtCore import QRegularExpression, Qt
from python_qt_binding.QtGui import QRegularExpressionValidator
from python_qt_binding.QtWidgets import (
    QCheckBox,
    QComboBox,
    QDialog,
    QDialogButtonBox,
    QFileDialog,
    QFormLayout,
    QGroupBox,
    QHBoxLayout,
    QLabel,
    QLineEdit,
    QListWidget,
    QListWidgetItem,
    QPushButton,
    QSplitter,
    QVBoxLayout,
    QWidget,
)

from .common import make_table, num, selected_key, set_rows, size

LIST_HEADS = ["id", "state", "kind", "methods", "targets", "elapsed", "size", "time left"]
LIST_HEADS += ["size left", "events", "lost", "owner", "end", "dir"]
ENDED_OK = ("user_stop", "max_duration", "max_size")
DEFAULT = "default"
INTEGERS = (
    ("max_duration_s", "Max duration, s (--max-duration)"),
    ("max_bytes", "Max size, bytes (--max-size)"),
    ("split_bytes", "Split size, bytes (--split-size)"),
    ("split_duration_s", "Split duration, s (--split-duration)"),
    ("cache_bytes", "Cache, bytes (--cache)"),
)


def duration(seconds):
    seconds = int(seconds)
    return f"{seconds // 3600}:{seconds // 60 % 60:02}:{seconds % 60:02}"


def targets_text(t):
    parts = ["all"] if t["all"] else [f"pids {' '.join(str(p) for p in t['pids'])}"]
    parts += [f"{k} {t[k]}" for k in ("node", "topic", "exclude") if t.get(k)]
    return ", ".join(parts)


def state_status(r):
    if r["state"] == "recording":
        return "run"
    if r["state"] == "truncated" or r.get("end_reason") == "error":
        return "error"
    return "ok" if r.get("end_reason") in ENDED_OK else "warn"


def choice(values):
    box = QComboBox()
    box.addItem(DEFAULT, None)
    for value in values:
        box.addItem(value, value)
    return box


class RecordForm(QWidget):
    """The options of `armo record`, one widget each. An untouched widget sends nothing,
    like an option not given on the command line, so equal input makes an equal record_start."""

    def __init__(self, processes, parent=None):
        super().__init__(parent)
        form = QFormLayout(self)
        self.method = choice(("auto", "uprobe", "preload", "lttng"))
        form.addRow("Method (--method)", self.method)
        self.all = QCheckBox("All processes (--all)")
        form.addRow("Targets", self.all)
        self.filter = QLineEdit()
        self.filter.setPlaceholderText("filter processes")
        self.filter.textChanged.connect(self._filter)
        self.pids = QListWidget()
        for p in processes:
            item = QListWidgetItem(f"{p['pid']} {p['comm']}")
            item.setData(Qt.UserRole, p["pid"])
            item.setFlags(item.flags() | Qt.ItemIsUserCheckable)
            item.setCheckState(Qt.Unchecked)
            self.pids.addItem(item)
        self.all.toggled.connect(lambda on: self.pids.setEnabled(not on))
        form.addRow("", self.filter)
        form.addRow("Processes (--pid)", self.pids)
        self.node = QLineEdit()
        self.topic = QLineEdit()
        self.exclude = QLineEdit()
        form.addRow("Node regex (--node)", self.node)
        form.addRow("Topic regex (--topic)", self.topic)
        form.addRow("Exclude regex (--exclude)", self.exclude)
        # armo-collectd refuses them until it applies them.
        for e in (self.node, self.topic, self.exclude):
            e.setEnabled(False)
            e.setPlaceholderText("not implemented yet")
        self.kind = choice(("events", "windows"))
        form.addRow("Kind (--kind)", self.kind)
        self.dir = QLineEdit()
        browse = QPushButton("Browse...")
        browse.clicked.connect(self._browse)
        row = QHBoxLayout()
        row.addWidget(self.dir)
        row.addWidget(browse)
        form.addRow("Directory (-o)", row)
        self.name = QLineEdit()
        form.addRow("Name (--name)", self.name)
        digits = QRegularExpressionValidator(QRegularExpression("[0-9]*"), self)
        self.integers = {}
        for field, label in INTEGERS:
            edit = QLineEdit()
            edit.setValidator(digits)
            edit.setPlaceholderText(DEFAULT)
            self.integers[field] = edit
            form.addRow(label, edit)
        self.compression = choice(("none", "zstd"))
        form.addRow("Compression (--compression)", self.compression)
        self.bandwidth = QCheckBox("--bandwidth")
        self.sched = QComboBox()
        self.sched.addItem(DEFAULT, None)
        self.sched.addItem("on (--sched)", True)
        self.sched.addItem("off (--no-sched)", False)
        self.malloc = QCheckBox("--malloc")
        form.addRow("Bandwidth", self.bandwidth)
        form.addRow("Sched", self.sched)
        form.addRow("Malloc", self.malloc)

    def _filter(self, text):
        for i in range(self.pids.count()):
            item = self.pids.item(i)
            item.setHidden(text not in item.text())

    def _browse(self):
        path = QFileDialog.getExistingDirectory(self, "Recording directory")
        if path:
            self.dir.setText(path)

    def checked_pids(self):
        items = (self.pids.item(i) for i in range(self.pids.count()))
        return [i.data(Qt.UserRole) for i in items if i.checkState() == Qt.Checked]

    def fields(self):
        """The record_start fields, built the way `armo record` builds them."""
        pids = [] if self.all.isChecked() else self.checked_pids()
        regex = {k: getattr(self, k).text() or None for k in ("node", "topic", "exclude")}
        targets = None
        if self.all.isChecked() or pids or any(regex.values()):
            targets = armo_client.targets(self.all.isChecked(), pids, **regex)
        options = {k: int(e.text()) if e.text() else None for k, e in self.integers.items()}
        options.update(
            kind=self.kind.currentData(),
            dir=self.dir.text() or None,
            name=self.name.text() or None,
            compression=self.compression.currentData(),
            bandwidth=True if self.bandwidth.isChecked() else None,
            sched=self.sched.currentData(),
            malloc=True if self.malloc.isChecked() else None,
        )
        return armo_client.record_start(self.method.currentData(), targets, **options)


class RecordDialog(QDialog):
    def __init__(self, processes, parent=None):
        super().__init__(parent)
        self.setWindowTitle("Start recording")
        self.form = RecordForm(processes)
        buttons = QDialogButtonBox(QDialogButtonBox.Ok | QDialogButtonBox.Cancel)
        buttons.button(QDialogButtonBox.Ok).setText("Start")
        buttons.accepted.connect(self.accept)
        buttons.rejected.connect(self.reject)
        layout = QVBoxLayout(self)
        layout.addWidget(self.form)
        layout.addWidget(buttons)


class ValidationView(QGroupBox):
    """The record_show table."""

    def __init__(self, parent=None):
        super().__init__("Validation", parent)
        layout = QVBoxLayout(self)
        self.entities = make_table(["entity", "events", "lost", "Hz"])
        self.warnings = make_table(["warning", "text", "entities"])
        self.totals = QLabel()
        self.end = QLabel()
        layout.addWidget(self.entities)
        layout.addWidget(self.warnings)
        layout.addWidget(self.totals)
        layout.addWidget(self.end)

    def show_table(self, title, t):
        self.setTitle(f"Validation: {title}")
        set_rows(
            self.entities,
            [
                (
                    e["id"],
                    [
                        (e["id"], "warn") if e["events"] == 0 or e["lost"] else e["id"],
                        e["events"],
                        e["lost"],
                        num(e["hz"]),
                    ],
                )
                for e in t["entities"]
            ],
        )
        set_rows(
            self.warnings,
            [
                (w["kind"], [(w["kind"], "warn"), w["text"], ", ".join(w["ids"])])
                for w in t["warnings"]
            ],
        )
        s = t["totals"]
        self.totals.setText(
            f"Total: {s['events']} events, {s['lost']} lost, {s['duration_s']:.1f} s, {size(s['bytes'])}"
        )
        detail = f" ({t['end_detail']})" if t["end_detail"] else ""
        self.end.setText(f"End: {t['end_reason'] or '-'}{detail}")


class RecordingsView(QWidget):
    def __init__(self, conn, parent=None):
        super().__init__(parent)
        self.conn = conn
        self.recordings = {}
        self.processes = []
        self.dialog = None
        self.table = make_table(LIST_HEADS)
        self.table.itemSelectionChanged.connect(self._selection)
        self.start_button = QPushButton("Start...")
        self.stop_button = QPushButton("Stop")
        self.show_button = QPushButton("Validation")
        self.open_button = QPushButton("Open directory...")
        self.start_button.clicked.connect(self.open_form)
        self.stop_button.clicked.connect(self.stop_selected)
        self.show_button.clicked.connect(
            lambda: self.request_show(selected_key(self.table), record_id=selected_key(self.table))
        )
        self.open_button.clicked.connect(self._open_dir)
        buttons = QHBoxLayout()
        for b in (self.start_button, self.stop_button, self.show_button, self.open_button):
            buttons.addWidget(b)
        buttons.addStretch()
        self.validation = ValidationView()
        top = QWidget()
        top_layout = QVBoxLayout(top)
        top_layout.setContentsMargins(0, 0, 0, 0)
        top_layout.addLayout(buttons)
        top_layout.addWidget(self.table)
        split = QSplitter(Qt.Vertical)
        split.addWidget(top)
        split.addWidget(self.validation)
        layout = QVBoxLayout(self)
        layout.setContentsMargins(0, 0, 0, 0)
        layout.addWidget(split)
        self.set_connected(False)

    def set_connected(self, connected):
        for b in (self.start_button, self.open_button):
            b.setEnabled(connected)
        self._selection()

    def show_state(self, state):
        """`state.recordings` holds the recordings in progress and the recent ended ones,
        and arrives each window while one is in progress."""
        current = {r["id"]: r for r in state["recordings"]}
        ended = [
            i
            for i, r in self.recordings.items()
            if r["state"] == "recording" and current.get(i, {"state": None})["state"] != "recording"
        ]
        self.recordings = current
        for record_id in ended:
            self.request_show(record_id, record_id=record_id)
        self.refresh()

    def refresh(self):
        rows = []
        for r in self.recordings.values():
            elapsed, limits = r["elapsed_s"], r["limits"]
            recording = r["state"] == "recording"
            rows.append(
                (
                    r["id"],
                    [
                        r["id"],
                        (r["state"], state_status(r)),
                        r["kind"],
                        ",".join(r["methods"]) or "-",
                    ]
                    + [targets_text(r["targets"]), duration(elapsed), size(r["bytes"])]
                    + [duration(max(0, limits["max_duration_s"] - elapsed)) if recording else "-"]
                    + [size(max(0, limits["max_bytes"] - r["bytes"])) if recording else "-"]
                    + [r["totals"]["events"], r["totals"]["lost"]]
                    + [f"{r['owner']['client']}:{r['owner']['pid']}@{r['owner']['host']}"]
                    + [r.get("end_reason") or "-", r["dir"]],
                )
            )
        selected = selected_key(self.table)
        set_rows(self.table, rows)
        for row, (key, _) in enumerate(rows):
            if key == selected:
                self.table.selectRow(row)
        self._selection()

    def _selection(self):
        key = selected_key(self.table)
        connected = self.conn.client is not None
        r = self.recordings.get(key)
        self.stop_button.setEnabled(connected and r is not None and r["state"] == "recording")
        self.show_button.setEnabled(connected and r is not None and r["state"] != "recording")

    def open_form(self):
        self.dialog = RecordDialog(self.processes, self)
        self.dialog.accepted.connect(lambda: self.start(self.dialog.form.fields()))
        self.dialog.open()

    def start(self, fields):
        self.conn.send("record_start", **fields)

    def stop_selected(self):
        self.conn.send("record_stop", record_id=selected_key(self.table))

    def _open_dir(self):
        path = QFileDialog.getExistingDirectory(self, "Recording directory")
        if path:
            self.request_show(path, dir=path)

    def request_show(self, title, **target):
        self.conn.send(
            "record_show",
            on_reply=lambda m: self.validation.show_table(title, m["table"]),
            **target,
        )
