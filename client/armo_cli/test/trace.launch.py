import os
import sys

from armo_cli.launch import ArmoTrace
from launch import LaunchDescription
from launch.actions import ExecuteProcess, Shutdown


def generate_launch_description():
    node = os.path.join(os.path.dirname(os.path.abspath(__file__)), "env_node.py")
    return LaunchDescription(
        [
            ArmoTrace(
                method=os.environ["ARMO_TEST_METHOD"],
                record=os.environ["ARMO_TEST_RECORD"] == "1",
                stop_on_shutdown=os.environ["ARMO_TEST_STOP"] == "1",
                kind="windows",
            ),
            ExecuteProcess(
                cmd=[sys.executable, node, os.environ["ARMO_TEST_OUT"]], on_exit=Shutdown()
            ),
        ]
    )
