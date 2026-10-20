import sys

from rqt_gui.main import Main
from rqt_gui_py.plugin import Plugin

from .widget import OrmaWidget


class Orma(Plugin):
    def __init__(self, context):
        super().__init__(context)
        self.setObjectName("Orma")
        self._widget = OrmaWidget()
        if context.serial_number() > 1:
            self._widget.setWindowTitle(f"orma ({context.serial_number()})")
        context.add_widget(self._widget)

    def shutdown_plugin(self):
        self._widget.shutdown()


def main():
    sys.exit(Main().main(sys.argv, standalone="rqt_orma.plugin.Orma"))
