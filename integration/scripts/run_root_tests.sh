#!/usr/bin/env bash
# orma_integration's tests that need root (sudo -n) for ormad and docker, one after the other
# (design 17). Run it from a shell that sourced the workspace. The Humble image is built from
# test/Dockerfile.humble when it is missing; the tests build HUMBLE_WS in it when it is empty.
set -u
test_dir="$(cd "$(dirname "$0")/../test" && pwd)"
image="${IMAGE:-orma-humble}"
ws="${HUMBLE_WS:-/tmp/orma-humble-ws}"
if ! sudo -n docker image inspect "$image" >/dev/null 2>&1; then
  sudo -n docker build -t "$image" -f "$test_dir/Dockerfile.humble" "$test_dir"
fi
failed=()
run() {
  echo "== $*"
  python3 -u "$test_dir/$1" "${@:2}" || failed+=("$*")
}
run agreement.py
run intervals.py
run lifetime.py
run node_cpu.py
run selfcheck.py
run humble.py --image "$image" --ws "$ws"
run node_cpu.py --humble "$image" "$ws" --method uprobe --method preload
run selfcheck.py --humble "$image" "$ws"
echo "failed: ${#failed[@]}"
for f in "${failed[@]}"; do
  echo "  $f"
done
[ "${#failed[@]}" -eq 0 ]
