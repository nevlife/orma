import os
import sys

from orma_cli.launch import OrmaTrace
from launch import LaunchDescription
from launch.actions import ExecuteProcess, Shutdown


def generate_launch_description():
    node = os.path.join(os.path.dirname(os.path.abspath(__file__)), "env_node.py")
    return LaunchDescription(
        [
            OrmaTrace(
                method=os.environ["ORMA_TEST_METHOD"],
                record=os.environ["ORMA_TEST_RECORD"] == "1",
                stop_on_shutdown=os.environ["ORMA_TEST_STOP"] == "1",
                kind="windows",
            ),
            ExecuteProcess(
                cmd=[sys.executable, node, os.environ["ORMA_TEST_OUT"]], on_exit=Shutdown()
            ),
        ]
    )
