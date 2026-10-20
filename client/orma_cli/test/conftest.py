import json
import os
import shutil
import subprocess
import sys
import tempfile
import time
from pathlib import Path

import pytest

HERE = Path(__file__).resolve().parent
PACKAGE = HERE.parent
ORMA = PACKAGE.parents[1]
GOLDEN = ORMA / "common" / "orma_proto" / "golden"
FAKE_SERVER = ORMA / "target/release/orma-fake-server"

sys.path.insert(0, str(PACKAGE))


def golden(name):
    return json.loads((GOLDEN / "client" / f"{name}.json").read_text())


def wait_for(predicate, timeout=10.0):
    deadline = time.monotonic() + timeout
    while not predicate():
        assert time.monotonic() < deadline, "timed out"
        time.sleep(0.02)


class Fake:
    def __init__(self, socket_path, log):
        self.log = log
        with open(log, "w") as out:
            self.process = subprocess.Popen([FAKE_SERVER, str(socket_path)], stdout=out)
        wait_for(socket_path.exists)

    def requests(self):
        return [json.loads(line) for line in self.log.read_text().splitlines()]

    def stop(self):
        self.process.kill()
        self.process.wait()


@pytest.fixture
def runtime(monkeypatch):
    # mkdtemp's short /tmp path: unix socket paths stop at 108 bytes.
    path = Path(tempfile.mkdtemp(prefix="orma-"))
    (path / "orma").mkdir()
    monkeypatch.setenv("XDG_RUNTIME_DIR", str(path))
    yield path
    shutil.rmtree(path)


@pytest.fixture
def fake(runtime):
    server = Fake(runtime / "orma" / "collectd.sock", runtime / "fake.log")
    yield server
    server.stop()


@pytest.fixture(scope="session")
def prefix(tmp_path_factory):
    """An install prefix holding stand-ins for liborma_preload.so and liborma_lttng.so."""
    root = tmp_path_factory.mktemp("prefix")
    (root / "lib").mkdir()
    index = root / "share" / "ament_index" / "resource_index" / "packages"
    index.mkdir(parents=True)
    for package in ("orma_preload", "orma_lttng"):
        (index / package).touch()
        library = root / "lib" / f"lib{package}.so"
        compile = ["cc", "-shared", "-fPIC", "-o", str(library), "-x", "c", "/dev/null"]
        subprocess.run(compile, check=True)
    return root


@pytest.fixture
def env(runtime, prefix, monkeypatch):
    """The environment for orma subprocesses: the stand-in prefix, this package's source."""
    monkeypatch.setenv("AMENT_PREFIX_PATH", f"{prefix}:{os.environ['AMENT_PREFIX_PATH']}")
    monkeypatch.delenv("LD_PRELOAD", raising=False)
    monkeypatch.delenv("LTTNG_HOME", raising=False)
    monkeypatch.setenv("ROS_DOMAIN_ID", str(100 + os.getpid() % 100))
    python_path = os.environ.get("PYTHONPATH")
    monkeypatch.setenv("PYTHONPATH", f"{PACKAGE}:{python_path}" if python_path else str(PACKAGE))
    return os.environ


def orma(*args):
    """The command line of `orma ARGS` from this package's source."""
    main = "import sys; from orma_cli.cli import main; sys.exit(main())"
    return [sys.executable, "-c", main, *args]
