#!/usr/bin/env bash
# Rewrites armo_preload/include/armo_preload/armo_{shm,events}.h from armo_common and the golden
# frames and schema/v1.json from armo_proto. The same tests without ARMO_BLESS fail when stale.
set -eu
ARMO_BLESS=1 exec "$(dirname "$0")/cargo.sh" test -p armo-common --test headers \
  -p armo-proto --test golden
