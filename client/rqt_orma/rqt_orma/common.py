"""Qt default icons for status, and the table helpers every view uses."""

import re

from orma_cli.cli import lat, ns, num, p99, size  # noqa: F401  (re-exported for the views)
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
SORT = Qt.UserRole + 1
UNITS = {"": 1, "ns": 1, "us": 1e3, "ms": 1e6, "s": 1e9, "B": 1}
UNITS.update({"KiB": 1 << 10, "MiB": 1 << 20, "GiB": 1 << 30})
NUMBER = re.compile(r"(-?\d+(?:\.\d+)?)([A-Za-z]*)")
# The order of orma_cli.cli.lat.
LAT_HEADS = ["p50", "p90", "p99", "max"]
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


def sort_key(text):
    """Numbers in the units of `ns` and `size` sort by value, and `-` (not measured) below them."""
    if text == "-":
        return (0, 0.0)
    m = NUMBER.fullmatch(text)
    if m and m.group(2) in UNITS:
        return (1, float(m.group(1)) * UNITS[m.group(2)])
    return (2, text)


class SortItem(QTableWidgetItem):
    def __lt__(self, other):
        return self.data(SORT) < other.data(SORT)


def make_sortable(table):
    """Unsorted until a header is clicked; the order then holds across `set_rows`."""
    table.horizontalHeader().setSortIndicator(-1, Qt.AscendingOrder)
    table.setSortingEnabled(True)


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
    # ResizeToContents measures the column again on every setItem of a visible table.
    header = table.horizontalHeader()
    header.setSectionResizeMode(QHeaderView.Interactive)
    # Sorting while filling moves rows under the indices being written.
    sortable = table.isSortingEnabled()
    table.setSortingEnabled(False)
    table.setRowCount(len(rows))
    for r, (key, cells) in enumerate(rows):
        for c, cell in enumerate(cells):
            text, status = cell if isinstance(cell, tuple) else (cell, None)
            item = SortItem(str(text)) if sortable else QTableWidgetItem(str(text))
            if sortable:
                item.setData(SORT, sort_key(str(text)))
            item.setData(KEY, key)
            if status is not None:
                item.setIcon(icon(status))
            table.setItem(r, c, item)
    table.setSortingEnabled(sortable)
    header.setSectionResizeMode(QHeaderView.ResizeToContents)


def row_key(table, row):
    item = table.item(row, 0)
    return None if item is None else item.data(KEY)


def selected_key(table):
    rows = table.selectionModel().selectedRows()
    return row_key(table, rows[0].row()) if rows else None
