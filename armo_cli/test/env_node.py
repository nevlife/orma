import json
import os
import sys

import rclpy

rclpy.init()
node = rclpy.create_node("armo_env_node")
with open("/proc/self/maps") as maps:
    mapped = sorted({line.split()[-1] for line in maps if "libarmo_" in line})
seen = {
    "env": {k: os.environ.get(k) for k in ("LD_PRELOAD", "LTTNG_HOME")},
    "path0": sys.path[1],
    "armo_rclpy": "armo_rclpy" in sys.modules,
    "mapped": mapped,
}
node.destroy_node()
rclpy.shutdown()
with open(sys.argv[1], "w") as out:
    json.dump(seen, out)
