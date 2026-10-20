"""orma_client on a reader QThread, with each server message delivered as a Qt signal."""

import socket
import threading

import orma_client
from python_qt_binding.QtCore import QThread, Signal

CLIENT = "rqt_orma"


class Connection(QThread):
    connected = Signal(dict)
    message = Signal(dict)
    lost = Signal(str)

    def __init__(self, parent=None):
        super().__init__(parent)
        self.client = None
        self._replies = {}
        self._closing = False
        self._start_daemon = False
        self._lock = threading.Lock()
        self.message.connect(self._route)

    def open(self, start_daemon):
        """Connects on the reader thread, starting orma-collectd if `start_daemon`."""
        self._closing = False
        self._start_daemon = start_daemon
        self.start()

    def run(self):
        try:
            client = orma_client.connect(CLIENT, start=self._start_daemon)
        except (orma_client.OrmaError, OSError) as e:
            self.lost.emit(str(e))
            return
        with self._lock:
            if self._closing:
                client.close()
                return
            self.client = client
        self.connected.emit(client.server)
        try:
            while True:
                self.message.emit(client.next_message())
        except (orma_client.OrmaError, OSError) as e:
            if not self._closing:
                self.lost.emit(str(e))

    def send(self, op, on_reply=None, **fields):
        id = self.client.send(op, **fields)
        if on_reply is not None:
            self._replies[id] = on_reply
        return id

    def _route(self, message):
        """Replies go to their request's callback; errors stay with the widget's event line."""
        callback = self._replies.pop(message.get("id"), None)
        if callback is not None and message["type"] == "reply" and not self._closing:
            callback(message)

    def drop(self):
        """After `lost`: forget the dead socket without a bye."""
        self.wait()
        if self.client is not None:
            self.client.sock.close()
        self.client = None
        self._replies.clear()

    def close(self):
        """Sends bye and nothing else: recordings belong to orma-collectd."""
        with self._lock:
            self._closing = True
            client, self.client = self.client, None
        if client is not None:
            # Wakes the reader's recv; closing the fd alone does not.
            client.sock.shutdown(socket.SHUT_RD)
            try:
                client.close()
            except OSError:
                # orma-collectd went away first: there is nobody left to say bye to.
                client.sock.close()
        self.wait()
