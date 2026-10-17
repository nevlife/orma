import os

from ament_index_python.packages import get_package_share_directory
from launch import LaunchDescription
from launch.actions import DeclareLaunchArgument
from launch.substitutions import LaunchConfiguration
from launch_ros.actions import Node


def generate_launch_description():
    rules = os.path.join(get_package_share_directory("armo_diagnostics"), "config", "rules.yaml")
    return LaunchDescription(
        [
            DeclareLaunchArgument("rules", default_value=rules),
            # The node ends when armo-collectd goes away; it comes back with the next one.
            Node(
                package="armo_diagnostics",
                executable="armo_diagnostics",
                parameters=[{"rules": LaunchConfiguration("rules")}],
                respawn=True,
                respawn_delay=2.0,
            ),
        ]
    )
