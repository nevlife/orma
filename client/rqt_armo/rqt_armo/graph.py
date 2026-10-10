"""The message flow graph: Graphviz `dot` layout through qt_dotgraph like
rqt_graph, and a walk that picks one chain node by node with the cost of each hop."""

from python_qt_binding.QtGui import QPainter, QPalette
from python_qt_binding.QtWidgets import (
    QComboBox,
    QGraphicsScene,
    QGraphicsView,
    QHBoxLayout,
    QLabel,
    QPushButton,
    QSplitter,
    QVBoxLayout,
    QWidget,
)
from python_qt_binding.QtCore import Qt
from qt_dotgraph.dot_to_qt import DotToQtGenerator
from qt_dotgraph.pydotfactory import PydotFactory

from .common import lat, lat_heads, make_table, ns, num, set_rows

HOP_HEADS = ["step", "hop", "where"] + lat_heads() + ["mean", "share"]
WORK = {"sync": "callback (sync)", "stamp": "dwell (stamp)"}


def hops(w, path):
    """The rows of a walked path: per step the wire hop into the next node, and between two
    hops the work inside the node. A link that is not measured, ambiguous or blind is a gap:
    the row has no time and the chain is partial (roscope's rule: never invent a link)."""
    subs = {(s["topic"], s["node"]): s for s in w["subscriptions"]}
    names = {n["id"]: n["fqn"] for n in w["nodes"]}
    topics = {t["id"]: t["name"] for t in w["topics"]}
    rows = []
    for step in range(1, len(path)):
        topic, node = path[step]
        if step > 1:
            prev_topic, prev_node = path[step - 1]
            flows = [
                f
                for f in w["flows"]
                if (f["node"], f["input"], f["output"]) == (prev_node, prev_topic, topic)
            ]
            where = names.get(prev_node, prev_node)
            if not flows:
                rows.append((step, "no link", where, None))
            elif len(flows) > 1 or flows[0]["ambiguous"]:
                rows.append((step, "ambiguous", where, None))
            elif flows[0]["kind"] == "blind":
                rows.append((step, "blind", where, None))
            else:
                rows.append((step, WORK[flows[0]["kind"]], where, flows[0]["dwell"]))
        sub = subs.get((topic, node))
        where = f"{topics.get(topic, topic)} -> {names.get(node, node)}"
        rows.append((step, "transport", where, sub and sub["hop"]))
    return rows


class GraphView(QWidget):
    def __init__(self, parent=None):
        super().__init__(parent)
        self.window_msg = None
        self.path = []
        self.topology = None
        self.scene = QGraphicsScene(self)
        self.view = QGraphicsView(self.scene)
        self.view.setDragMode(QGraphicsView.ScrollHandDrag)
        self.view.setRenderHints(QPainter.Antialiasing | QPainter.TextAntialiasing)
        self.dot_to_qt = DotToQtGenerator()
        self.factory = PydotFactory()
        fit = QPushButton("Fit")
        fit.clicked.connect(self.fit)
        self.start = QComboBox()
        self.next = QComboBox()
        self.add_button = QPushButton("Add hop")
        self.back_button = QPushButton("Back")
        self.clear_button = QPushButton("Clear")
        self.start.activated.connect(self._start)
        self.add_button.clicked.connect(self._add)
        self.back_button.clicked.connect(self._back)
        self.clear_button.clicked.connect(self._clear)
        controls = QHBoxLayout()
        for label, widget in (("Walk from", self.start), ("next", self.next)):
            controls.addWidget(QLabel(label))
            controls.addWidget(widget, 1)
        for widget in (self.add_button, self.back_button, self.clear_button, fit):
            controls.addWidget(widget)
        self.hops = make_table(HOP_HEADS)
        self.e2e = QLabel()
        bottom = QWidget()
        bottom_layout = QVBoxLayout(bottom)
        bottom_layout.setContentsMargins(0, 0, 0, 0)
        bottom_layout.addLayout(controls)
        bottom_layout.addWidget(self.hops)
        bottom_layout.addWidget(self.e2e)
        split = QSplitter(Qt.Vertical)
        split.addWidget(self.view)
        split.addWidget(bottom)
        split.setStretchFactor(0, 3)
        layout = QVBoxLayout(self)
        layout.setContentsMargins(0, 0, 0, 0)
        layout.addWidget(split)
        for button in (self.add_button, self.back_button, self.clear_button):
            button.setEnabled(False)

    def show_window(self, w):
        self.window_msg = w
        names = self.names()
        topics = {t["id"] for t in w["topics"]}
        # A vanished node or topic cuts the walk back to its longest prefix that still exists.
        valid = []
        for topic, node in self.path:
            if node not in names or (topic is not None and topic not in topics):
                break
            valid.append((topic, node))
        self.path = valid
        self._draw()
        self._fill_walk()

    def names(self):
        w = self.window_msg
        out = {n["id"]: n["fqn"] for n in w["nodes"]}
        for t in w["topics"]:
            for n in t["publishers"] + t["subscribers"]:
                out.setdefault(n, n)
        return out

    def candidates(self):
        """(topic, node) pairs one hop on from the end of the path."""
        last = self.path[-1][1]
        return [
            (t["id"], n)
            for t in self.window_msg["topics"]
            if last in t["publishers"]
            for n in t["subscribers"]
            if n != last
        ]

    def _draw(self):
        w = self.window_msg
        highlight = self.palette().color(QPalette.Highlight).name()
        on_path = {n for _, n in self.path} | {t for t, _ in self.path if t}
        f = self.factory
        graph = f.get_graph(rankdir="LR", simplify=False)
        for node, name in self.names().items():
            color = highlight if node in on_path else None
            # pydot leaves an id such as n:1:2:ab unquoted, and dot reads the colons as ports.
            f.add_node_to_graph(graph, node, f'"{name}"', shape="ellipse", color=color)
        subs = {(s["topic"], s["node"]): s for s in w["subscriptions"]}
        walked = set(self.path[1:])
        walked |= {(a[1], b[0]) for a, b in zip(self.path, self.path[1:])}
        for t in w["topics"]:
            label = t["name"] if t["transport"] == "ros" else f"{t['name']} ({t['transport']})"
            color = highlight if t["id"] in on_path else None
            f.add_node_to_graph(graph, t["id"], f'"{label}"', shape="box", color=color)
            for n in t["publishers"]:
                width = 3 if (n, t["id"]) in walked else 1
                label = f"{num(t['pub_hz'])} Hz"
                f.add_edge_to_graph(graph, n, t["id"], label=label, penwidth=width)
            for n in t["subscribers"]:
                hop = (subs.get((t["id"], n)) or {}).get("hop")
                width = 3 if (t["id"], n) in walked else 1
                label = f"p99 {lat(hop)[0]}" if hop else ""
                f.add_edge_to_graph(graph, t["id"], n, label=label, penwidth=width)
        self.scene.clear()
        self.dot_to_qt.dotcode_to_qt_items(f.create_dot(graph), highlight_level=3, scene=self.scene)
        self.scene.setSceneRect(self.scene.itemsBoundingRect())
        topology = (tuple(self.names()), tuple(t["id"] for t in w["topics"]))
        if topology != self.topology:
            self.topology = topology
            self.fit()

    def fit(self):
        self.view.fitInView(self.scene.itemsBoundingRect(), Qt.KeepAspectRatio)

    def _fill_walk(self):
        names = self.names()
        current = self.start.currentData()
        self.start.clear()
        for node, name in sorted(names.items(), key=lambda x: x[1]):
            self.start.addItem(name, node)
        if current in names:
            self.start.setCurrentIndex(self.start.findData(current))
        self.next.clear()
        topics = {t["id"]: t["name"] for t in self.window_msg["topics"]}
        for topic, node in self.candidates() if self.path else []:
            self.next.addItem(f"{topics[topic]} -> {names[node]}", (topic, node))
        self.add_button.setEnabled(self.next.count() > 0)
        self.back_button.setEnabled(len(self.path) > 1)
        self.clear_button.setEnabled(bool(self.path))
        self._fill_hops()

    def _fill_hops(self):
        rows = hops(self.window_msg, self.path)
        measured = [r[3]["mean_ns"] for r in rows if r[3]]
        total = sum(measured)
        biggest = max(measured, default=None)
        cells = []
        for step, kind, where, value in rows:
            mean = value["mean_ns"] if value else None
            share = f"{100 * mean / total:.0f}%" if mean is not None and total else "-"
            if mean is not None and mean == biggest:
                share += " (largest)"
            status = None if value else "warn"
            cells.append(
                (
                    step,
                    [step, (kind, status) if status else kind, where]
                    + lat(value)
                    + [ns(mean), share],
                )
            )
        set_rows(self.hops, cells)
        gaps = [f"step {r[0]} {r[1]}" for r in rows if not r[3]]
        if not rows:
            self.e2e.setText("Pick a start node, then add hops.")
        elif gaps:
            self.e2e.setText(f"e2e partial: {ns(total)} measured, gaps at {', '.join(gaps)}")
        else:
            self.e2e.setText(f"e2e (sum of hop means): {ns(total)}")

    def _start(self):
        self.path = [(None, self.start.currentData())]
        self._redraw()

    def _add(self):
        self.path.append(self.next.currentData())
        self._redraw()

    def _back(self):
        self.path.pop()
        self._redraw()

    def _clear(self):
        self.path = []
        self._redraw()

    def _redraw(self):
        self._draw()
        self._fill_walk()
