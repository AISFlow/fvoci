#!/usr/bin/env bash
# Heavy collab capacity probe — not part of default CI. Requires Linux, PostgreSQL, release helper.
set -euo pipefail
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
cd "$ROOT"

export COLLAB_PROBE_ROOMS="${COLLAB_PROBE_ROOMS:-64}"
export COLLAB_PROBE_PEERS="${COLLAB_PROBE_PEERS:-2}"
export COLLAB_PROBE_DURATION_SECS="${COLLAB_PROBE_DURATION_SECS:-180}"
export COLLAB_PROBE_OPEN_CONCURRENCY="${COLLAB_PROBE_OPEN_CONCURRENCY:-8}"
export FVOCI_COLLAB_MAX_ROOMS="${FVOCI_COLLAB_MAX_ROOMS:-64}"
# Probe-only: each live room holds one PG connection via RoomGuard; raise docker PG for 64+ rooms.
export FVOCI_TEST_PG_MAX_CONNECTIONS="${FVOCI_TEST_PG_MAX_CONNECTIONS:-150}"
export RUST_LOG="${RUST_LOG:-collab.stage=info}"

LOG_DIR="${FVOCI_EVIDENCE_DIR:-$ROOT/target/collab-probe-logs}"
mkdir -p "$LOG_DIR"
STAMP="$(date -u +%Y%m%dT%H%M%SZ)"
LOG_PATH="${LOG_DIR}/collab-capacity-probe-${STAMP}.log"

echo "Building release collab-engine helper..."
CARGO_TARGET_DIR="$ROOT/crates/collab-engine/target" \
  cargo build --locked --release \
  --manifest-path crates/collab-engine/Cargo.toml \
  --features worker --bin collab-engine

export FVOCI_COLLAB_ENGINE="$ROOT/crates/collab-engine/target/release/collab-engine"

echo "Building release collab_capacity_probe test binary..."
cargo build --locked --release --features db-tests --test collab_capacity_probe

echo "Probe: rooms=$COLLAB_PROBE_ROOMS peers=$COLLAB_PROBE_PEERS duration=${COLLAB_PROBE_DURATION_SECS}s concurrency=$COLLAB_PROBE_OPEN_CONCURRENCY"
echo "Saving full log to $LOG_PATH"

set +e
"$ROOT/scripts/start-test-postgres.sh" bash -c "
  set -euo pipefail
  cd '$ROOT'
  export FVOCI_COLLAB_ENGINE='$FVOCI_COLLAB_ENGINE'
  export COLLAB_PROBE_ROOMS='$COLLAB_PROBE_ROOMS'
  export COLLAB_PROBE_PEERS='$COLLAB_PROBE_PEERS'
  export COLLAB_PROBE_DURATION_SECS='$COLLAB_PROBE_DURATION_SECS'
  export FVOCI_COLLAB_MAX_ROOMS='$FVOCI_COLLAB_MAX_ROOMS'
  export RUST_LOG='$RUST_LOG'
  cargo test --release --features db-tests --test collab_capacity_probe -- --nocapture
" 2>&1 | tee "$LOG_PATH"
PROBE_EXIT=${PIPESTATUS[0]}
set -e

if [[ "$PROBE_EXIT" -ne 0 ]]; then
  echo "collab capacity probe failed (exit $PROBE_EXIT); log: $LOG_PATH" >&2
  exit "$PROBE_EXIT"
fi

echo "collab capacity probe passed; log: $LOG_PATH"
