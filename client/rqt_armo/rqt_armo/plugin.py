import sys

from rqt_gui.main import Main
from rqt_gui_py.plugin import Plugin

from .widget import ArmoWidget


class Armo(Plugin):
    def __init__(self, context):
        super().__init__(context)
        self.setObjectName("Armo")
        self._widget = ArmoWidget()
        if context.serial_number() > 1:
            self._widget.setWindowTitle(f"armo ({context.serial_number()})")
        context.add_widget(self._widget)

    def shutdown_plugin(self):
        self._widget.shutdown()


def main():
    sys.exit(Main().main(sys.argv, standalone="rqt_armo.plugin.Armo"))
