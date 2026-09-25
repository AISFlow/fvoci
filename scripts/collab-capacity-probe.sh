#!/usr/bin/env bash
# Heavy collab capacity probe — not part of default CI. Requires Linux, PostgreSQL, release helper.
set -euo pipefail
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
cd "$ROOT"

export COLLAB_PROBE_ROOMS="${COLLAB_PROBE_ROOMS:-200}"
export COLLAB_PROBE_PEERS="${COLLAB_PROBE_PEERS:-2}"
export COLLAB_PROBE_DURATION_SECS="${COLLAB_PROBE_DURATION_SECS:-180}"
export FVOCI_COLLAB_MAX_ROOMS="${FVOCI_COLLAB_MAX_ROOMS:-200}"

echo "Building release collab-engine helper..."
CARGO_TARGET_DIR="$ROOT/crates/collab-engine/target" \
  cargo build --locked --release \
  --manifest-path crates/collab-engine/Cargo.toml \
  --features worker --bin collab-engine

export FVOCI_COLLAB_ENGINE="$ROOT/crates/collab-engine/target/release/collab-engine"

echo "Probe: rooms=$COLLAB_PROBE_ROOMS peers=$COLLAB_PROBE_PEERS duration=${COLLAB_PROBE_DURATION_SECS}s"

"$ROOT/scripts/start-test-postgres.sh" bash -c "
  set -euo pipefail
  cd '$ROOT'
  export FVOCI_COLLAB_ENGINE='$FVOCI_COLLAB_ENGINE'
  export COLLAB_PROBE_ROOMS='$COLLAB_PROBE_ROOMS'
  export COLLAB_PROBE_PEERS='$COLLAB_PROBE_PEERS'
  export COLLAB_PROBE_DURATION_SECS='$COLLAB_PROBE_DURATION_SECS'
  export FVOCI_COLLAB_MAX_ROOMS='$FVOCI_COLLAB_MAX_ROOMS'
  cargo test --release --features db-tests --test collab_capacity_probe -- --nocapture
"
