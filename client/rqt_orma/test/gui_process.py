"""The GUI in its own process for the kill test: starts a recording, prints its
id once the state shows it, then runs until killed."""

import os
import sys

os.environ.setdefault("QT_QPA_PLATFORM", "offscreen")

from python_qt_binding.QtWidgets import QApplication  # noqa: E402
from rqt_orma.widget import OrmaWidget  # noqa: E402

app = QApplication([])
gui = OrmaWidget()


def started(message):
    ids = [r["id"] for r in message.get("recordings", ())]
    if message["type"] == "state" and "r-fake-1" in ids:
        print("recording r-fake-1", flush=True)


gui.conn.message.connect(started)
gui.conn.connected.connect(lambda _: gui.recordings.start({}))
sys.exit(app.exec_())
