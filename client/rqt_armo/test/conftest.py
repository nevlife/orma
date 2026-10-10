import json
import os
import shutil
import subprocess
import sys
import tempfile
import time
from pathlib import Path

import pytest

os.environ.setdefault("QT_QPA_PLATFORM", "offscreen")

HERE = Path(__file__).resolve().parent
PACKAGE = HERE.parent
ARMO = PACKAGE.parents[1]
GOLDEN = ARMO / "common" / "armo_proto" / "golden"
FAKE_SERVER = ARMO / "target/release/armo-fake-server"

sys.path.insert(0, str(PACKAGE))

from python_qt_binding.QtWidgets import QApplication  # noqa: E402


def golden(kind, name):
    return json.loads((GOLDEN / kind / f"{name}.json").read_text())


def without_id(message):
    return {k: v for k, v in message.items() if k != "id"}


def wait_for(predicate, timeout=10.0):
    """Runs the Qt event loop until `predicate` holds."""
    deadline = time.monotonic() + timeout
    while not predicate():
        assert time.monotonic() < deadline, "timed out"
        QApplication.processEvents()
        time.sleep(0.01)


class Fake:
    def __init__(self, socket_path, log):
        self.socket_path = socket_path
        self.log = log
        with open(log, "a") as out:
            self.process = subprocess.Popen([FAKE_SERVER, str(socket_path)], stdout=out)
        deadline = time.monotonic() + 10
        while not socket_path.exists():
            assert time.monotonic() < deadline, "fake server did not bind"
            time.sleep(0.01)

    def requests(self):
        return [json.loads(line) for line in self.log.read_text().splitlines()]

    def ops(self):
        return [r["op"] for r in self.requests()]

    def stop(self):
        self.process.kill()
        self.process.wait()
        self.socket_path.unlink(missing_ok=True)


@pytest.fixture(scope="session")
def app():
    return QApplication.instance() or QApplication([])


@pytest.fixture
def runtime(monkeypatch):
    # mkdtemp's short /tmp path: unix socket paths stop at 108 bytes.
    path = Path(tempfile.mkdtemp(prefix="armo-"))
    (path / "armo").mkdir()
    monkeypatch.setenv("XDG_RUNTIME_DIR", str(path))
    yield path
    shutil.rmtree(path)


@pytest.fixture
def fake(runtime):
    server = Fake(runtime / "armo" / "collectd.sock", runtime / "fake.log")
    yield server
    server.stop()


@pytest.fixture
def gui(app, fake):
    from rqt_armo.widget import ArmoWidget

    widget = ArmoWidget()
    widget.resize(1280, 800)
    widget.show()
    wait_for(lambda: widget.window_msg is not None)
    yield widget
    widget.shutdown()
    widget.deleteLater()
