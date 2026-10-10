"""Qt default icons for status, and the table helpers every view uses."""

from armo_cli.cli import lat, ns, num, size  # noqa: F401  (re-exported for the views)
from python_qt_binding.QtCore import Qt
from python_qt_binding.QtWidgets import (
    QAbstractItemView,
    QApplication,
    QHeaderView,
    QStyle,
    QTableWidget,
    QTableWidgetItem,
)

KEY = Qt.UserRole
# The order of armo_cli.cli.lat: tail first.
LAT_HEADS = ["p99", "p99.9", "max", "p50"]
STATUS_ICONS = {
    "ok": QStyle.SP_DialogApplyButton,
    "info": QStyle.SP_MessageBoxInformation,
    "warn": QStyle.SP_MessageBoxWarning,
    "error": QStyle.SP_MessageBoxCritical,
    "run": QStyle.SP_MediaPlay,
}


def icon(status):
    return QApplication.style().standardIcon(STATUS_ICONS[status])


def lat_heads(prefix=""):
    return [f"{prefix}{h}" for h in LAT_HEADS]


def warn_if(value, text=None):
    """A cell with the warning icon when `value` is a positive count."""
    shown = "-" if value is None else (text if text is not None else str(value))
    return (shown, "warn") if value else shown


def make_table(headers):
    table = QTableWidget(0, len(headers))
    table.setHorizontalHeaderLabels(headers)
    table.setEditTriggers(QAbstractItemView.NoEditTriggers)
    table.setSelectionBehavior(QAbstractItemView.SelectRows)
    table.setSelectionMode(QAbstractItemView.SingleSelection)
    table.verticalHeader().setVisible(False)
    table.horizontalHeader().setSectionResizeMode(QHeaderView.ResizeToContents)
    table.horizontalHeader().setStretchLastSection(True)
    return table


def set_rows(table, rows):
    """`rows` are (key, cells); a cell is text or (text, status). The key goes on every item."""
    table.setRowCount(len(rows))
    for r, (key, cells) in enumerate(rows):
        for c, cell in enumerate(cells):
            text, status = cell if isinstance(cell, tuple) else (cell, None)
            item = QTableWidgetItem(str(text))
            item.setData(KEY, key)
            if status is not None:
                item.setIcon(icon(status))
            table.setItem(r, c, item)


def row_key(table, row):
    item = table.item(row, 0)
    return None if item is None else item.data(KEY)


def selected_key(table):
    rows = table.selectionModel().selectedRows()
    return row_key(table, rows[0].row()) if rows else None
