#!/usr/bin/env python3
"""Run clang-tidy (.clang-tidy) over the C files handed in, and fail on any finding.

clang-tidy needs the compile commands of a build. colcon writes one per package under the
workspace's build/ when CMAKE_EXPORT_COMPILE_COMMANDS is on. No compile commands is a failure, not a pass: a gate that cannot run must not
report clean.

A header has no compile command of its own, so a changed header checks every translation unit.
"""

import json
import os
import subprocess
import sys
import tempfile
from concurrent.futures import ThreadPoolExecutor
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
PACKAGES = ("armo_preload", "armo_lttng", "armo_span")


def compile_commands():
    build = ROOT.parent.parent / "build"
    entries = []
    for pkg in PACKAGES:
        db = build / pkg / "compile_commands.json"
        if db.is_file():
            entries += json.loads(db.read_text())
    return build, entries


def main(argv):
    build, entries = compile_commands()
    if not entries:
        print(
            f"no compile_commands.json under {build}/<package>/: build with "
            "-DCMAKE_EXPORT_COMPILE_COMMANDS=ON, or set ARMO_BUILD_DIR",
            file=sys.stderr,
        )
        return 1
    ours = {
        str(Path(e["file"]).resolve())
        for e in entries
        if str(ROOT) in str(Path(e["file"]).resolve())
    }
    given = [str((ROOT / f).resolve()) if not os.path.isabs(f) else f for f in argv]
    if any(f.endswith(".h") for f in given):
        targets = sorted(ours)
    else:
        targets = sorted(f for f in given if f in ours)
    if not targets:
        return 0

    with tempfile.TemporaryDirectory() as tmp:
        (Path(tmp) / "compile_commands.json").write_text(json.dumps(entries))

        def tidy(f):
            return subprocess.run(
                ["clang-tidy", "-p", tmp, "--quiet", f],
                capture_output=True,
                text=True,
                cwd=ROOT,
            ).stdout

        with ThreadPoolExecutor() as pool:
            outputs = list(pool.map(tidy, targets))
    hits = [
        line
        for out in outputs
        for line in out.splitlines()
        if " warning: " in line or " error: " in line
    ]
    if hits:
        print("\n".join(hits))
    return 1 if hits else 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
