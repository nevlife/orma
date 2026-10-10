"""Collection method and target selection (the `select` and `configure` requests). The
selection lives in armo-collectd and is shared by every client, so this view shows the
state's selection until the user edits it, and Apply sends it."""

from python_qt_binding.QtCore import Qt
from python_qt_binding.QtWidgets import (
    QCheckBox,
    QComboBox,
    QDoubleSpinBox,
    QFormLayout,
    QHBoxLayout,
    QLabel,
    QLineEdit,
    QPushButton,
    QTableWidgetItem,
    QVBoxLayout,
    QWidget,
)

from .common import make_table

HEADS = ["pid", "comm", "distro", "methods available", "method"]
EXTRAS = ("bandwidth", "sched", "malloc")
NAMES = ("node", "topic", "exclude")


class SelectionView(QWidget):
    def __init__(self, conn, parent=None):
        super().__init__(parent)
        self.conn = conn
        self.monitor = None
        self.processes = {}
        self.edited = False
        self.all = QCheckBox("All processes")
        self.table = make_table(HEADS)
        self.window_s = QDoubleSpinBox()
        self.window_s.setRange(0.1, 60.0)
        self.window_s.setSingleStep(0.5)
        self.window_s.setSuffix(" s")
        self.extras = {name: QCheckBox(name) for name in EXTRAS}
        self.names = {name: QLineEdit() for name in NAMES}
        self.apply_button = QPushButton("Apply")
        self.revert_button = QPushButton("Revert")
        self.apply_button.clicked.connect(self.apply)
        self.revert_button.clicked.connect(self.revert)
        self.all.toggled.connect(self._edit)
        self.all.toggled.connect(lambda on: self.table.setEnabled(not on))
        self.table.itemChanged.connect(self._edit)
        self.window_s.valueChanged.connect(self._edit)
        for box in self.extras.values():
            box.toggled.connect(self._edit)
        for edit in self.names.values():
            edit.textEdited.connect(self._edit)
        options = QFormLayout()
        options.addRow("Node regex (--node)", self.names["node"])
        options.addRow("Topic regex (--topic)", self.names["topic"])
        options.addRow("Exclude regex (--exclude)", self.names["exclude"])
        options.addRow("Window", self.window_s)
        extras = QHBoxLayout()
        for box in self.extras.values():
            extras.addWidget(box)
        extras.addStretch()
        options.addRow("Extra collection", extras)
        buttons = QHBoxLayout()
        self.status = QLabel()
        buttons.addWidget(self.status, 1)
        buttons.addWidget(self.revert_button)
        buttons.addWidget(self.apply_button)
        layout = QVBoxLayout(self)
        layout.addWidget(self.all)
        layout.addWidget(self.table)
        layout.addLayout(options)
        layout.addLayout(buttons)
        self._loading = False
        self._set_edited(False)

    def _edit(self, *_):
        if not self._loading:
            self._set_edited(True)

    def _set_edited(self, edited):
        self.edited = edited
        self.status.setText("edited, not applied" if edited else "")
        self.revert_button.setEnabled(edited)

    def show_state(self, state):
        self.monitor = state["monitor"]
        if not self.edited:
            self.revert()

    def show_processes(self, processes):
        """Rows follow the window's processes plus selected pids that are not in it."""
        known = {p["pid"]: p for p in processes}
        if set(known) != set(self.processes):
            self.processes = known
            if not self.edited:
                self.revert()

    def revert(self):
        m = self.monitor
        self._loading = True
        selected = set(m["selection"]["pids"])
        pids = sorted(set(self.processes) | selected)
        self.table.setRowCount(len(pids))
        for row, pid in enumerate(pids):
            p = self.processes.get(pid)
            check = QTableWidgetItem(str(pid))
            check.setFlags(Qt.ItemIsEnabled | Qt.ItemIsSelectable | Qt.ItemIsUserCheckable)
            check.setCheckState(Qt.Checked if pid in selected else Qt.Unchecked)
            check.setData(Qt.UserRole, pid)
            self.table.setItem(row, 0, check)
            cells = ["-", "-", "-"]
            if p is not None:
                cells = [p["comm"], p["ros"]["distro"] if p["ros"] else "-"]
                cells.append(", ".join(p["methods_available"]))
            for col, text in enumerate(cells, 1):
                self.table.setItem(row, col, QTableWidgetItem(text))
            method = QComboBox()
            method.addItem("auto", None)
            for name in p["methods_available"] if p is not None else ("uprobe", "preload", "lttng"):
                method.addItem(name, name)
            current = m["method_by_pid"].get(str(pid))
            method.setCurrentIndex(max(0, method.findData(current)))
            method.currentIndexChanged.connect(self._edit)
            self.table.setCellWidget(row, 4, method)
        self.all.setChecked(m["selection"]["all"])
        for name, edit in self.names.items():
            edit.setText(m["selection"].get(name, ""))
        self.window_s.setValue(m["window_s"])
        for name, box in self.extras.items():
            box.setChecked(m[name])
        self._loading = False
        self._set_edited(False)

    def apply(self):
        pids, method_by_pid = [], {}
        names = {k: e.text() for k, e in self.names.items() if e.text()}
        # The names replace the table: the pids they matched come back checked in the state.
        if not self.all.isChecked() and not names:
            for row in range(self.table.rowCount()):
                item = self.table.item(row, 0)
                if item.checkState() == Qt.Checked:
                    pid = item.data(Qt.UserRole)
                    pids.append(pid)
                    method = self.table.cellWidget(row, 4).currentData()
                    if method is not None:
                        method_by_pid[str(pid)] = method
        self.conn.send(
            "select",
            all=self.all.isChecked() and not names,
            pids=pids,
            method_by_pid=method_by_pid,
            **names,
        )
        changes = {}
        if self.window_s.value() != self.monitor["window_s"]:
            changes["window_s"] = self.window_s.value()
        for name, box in self.extras.items():
            if box.isChecked() != self.monitor[name]:
                changes[name] = box.isChecked()
        if changes:
            self.conn.send("configure", **changes)
        self._set_edited(False)
