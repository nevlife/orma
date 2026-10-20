#!/usr/bin/env bash
# Rewrites orma_preload/include/orma_preload/orma_{shm,events}.h from orma_common and the golden
# frames and schema/v1.json from orma_proto. The same tests without ORMA_BLESS fail when stale.
set -eu
ORMA_BLESS=1 exec "$(dirname "$0")/cargo.sh" test -p orma-common --test headers \
  -p orma-proto --test golden
