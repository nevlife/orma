"""The orma_diagnostics node: a client of orma-collectd that publishes /diagnostics each window.
It leaves the selection to the other clients, as orma-collectd holds one for all of them."""

import os
import sys

import orma_client
import rclpy
from ament_index_python.packages import get_package_share_directory
from diagnostic_msgs.msg import DiagnosticArray, DiagnosticStatus, KeyValue

from orma_diagnostics import rules

CLIENT = "orma_diagnostics"


def array(node, statuses):
    msg = DiagnosticArray()
    msg.header.stamp = node.get_clock().now().to_msg()
    for s in statuses:
        values = [KeyValue(key=k, value=v) for k, v in s["values"].items()]
        msg.status.append(
            DiagnosticStatus(
                level=bytes([s["level"]]),
                name=s["name"],
                message=s["message"],
                hardware_id=CLIENT,
                values=values,
            )
        )
    return msg


def main():
    rclpy.init()
    node = rclpy.create_node(CLIENT)
    default = os.path.join(get_package_share_directory(CLIENT), "config", "rules.yaml")
    path = node.declare_parameter("rules", default).value
    with open(path) as f:
        entries = rules.load(f.read())
    publisher = node.create_publisher(DiagnosticArray, "/diagnostics", 10)
    try:
        with orma_client.connect(CLIENT) as c:
            c.request("subscribe", streams=["window"])
            while True:
                m = c.next_message()
                # rclpy's own SIGTERM handler shuts the context down; recv carries on until here.
                if not rclpy.ok():
                    return 0
                if m["type"] == "window":
                    publisher.publish(array(node, rules.evaluate(entries, m)))
    except KeyboardInterrupt:
        return 0
    except (orma_client.OrmaError, OSError) as e:
        node.get_logger().error(str(e))
        return 1
    finally:
        node.destroy_node()
        rclpy.try_shutdown()


if __name__ == "__main__":
    sys.exit(main())
