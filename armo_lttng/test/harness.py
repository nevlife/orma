"""armo-collectd for a test."""

import os
import re
import signal
import subprocess

import armo_client
from session import wait_for


class Collectd:
    """The installed armo-collectd under its own XDG_RUNTIME_DIR and HOME in `tmp`; this process's
    environment points armo_client at it."""

    def __init__(self, tmp):
        self.env = dict(os.environ, XDG_RUNTIME_DIR=str(tmp / "x"), HOME=str(tmp / "h"))
        for d in ("x", "h"):
            (tmp / d).mkdir(exist_ok=True)
        os.environ.update(self.env)
        self.lttng_home = tmp / "x/armo/lttng"
        self.start()

    def start(self):
        self.proc = subprocess.Popen(["armo-collectd"], env=self.env, start_new_session=True)
        wait_for(self.answers, "armo-collectd")

    def answers(self):
        try:
            armo_client._open(armo_client.socket_path()).close()
        except OSError:
            return False
        return True

    def lttng(self, *args):
        """`lttng` against armo-collectd's session daemon; None when there is no daemon."""
        run = subprocess.run(
            ["lttng", "--no-sessiond", *args],
            env=dict(self.env, LTTNG_HOME=str(self.lttng_home)),
            capture_output=True,
            text=True,
        )
        return run.stdout if run.returncode == 0 else None

    def session_up(self):
        """An armo session is recording; an ended recording's may linger inactive."""
        return re.search(r"armo-\S+ \[active\]", self.lttng("list") or "") is not None

    def guard(self):
        out = subprocess.run(
            ["pgrep", "-P", str(self.proc.pid), "-f", "collectd --guard"],
            capture_output=True,
            text=True,
        ).stdout
        return int(out.split()[0])

    def stop(self):
        self.proc.send_signal(signal.SIGTERM)
        self.proc.wait(timeout=20)
