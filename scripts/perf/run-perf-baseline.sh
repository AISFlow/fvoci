#!/usr/bin/env bash
# Opt-in user-perceived performance baseline against a SOURCE build (release
# Rust binaries + production web bundle). Not part of required CI.
#
#   CARGO_TARGET_DIR=... FVOCI_PERF_OUT=/abs/evidence/dir \
#     bash scripts/perf/run-perf-baseline.sh [minimal] [scaled]
#
# Optional: FVOCI_PERF_SAMPLES (default 30), FVOCI_PERF_QUIET_MAX_MS (contention
# wait per window, default 300000), FVOCI_PERF_GREP + FVOCI_PERF_TAG to re-run
# selected flows (the setup test must match too) into suffixed result files.
# Flow h (collab room saturation; FVOCI_PERF_SAMPLES × 47 s of drain pacing
# alone) is skipped unless FVOCI_PERF_ROOM_SATURATION=1; run it on its own with
# FVOCI_PERF_GREP="setup dataset|h: " and a FVOCI_PERF_TAG, optionally with
# FVOCI_COLLAB_MAX_ROOMS for another room cap.
#
# Prerequisites (not built here, so build time never mixes with results):
#   cargo build --release --locked --bin fvoci-server --bin fvoci-migrate
#   cargo build --release --locked --bin fvoci-e2e-fixture --features db-tests
#   collab-engine release build (FVOCI_PERF_COLLAB_ENGINE, default
#     $CARGO_TARGET_DIR/collab-engine/release/collab-engine)
#   bun ci && (cd apps/web && bun --bun run build)
# Every run gets its own PostgreSQL 18 + Meilisearch container, database,
# storage and browser contexts; all are removed on exit.
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
: "${CARGO_TARGET_DIR:?CARGO_TARGET_DIR must point at the release build}"
: "${FVOCI_PERF_OUT:?FVOCI_PERF_OUT must name the evidence directory}"
CARGO_TARGET_DIR="$(realpath -m -- "$CARGO_TARGET_DIR")"
FVOCI_PERF_OUT="$(realpath -m -- "$FVOCI_PERF_OUT")"
case "$FVOCI_PERF_OUT" in
  "$ROOT"/*) echo "FVOCI_PERF_OUT must be outside the repository" >&2; exit 1 ;;
esac
RELEASE="$CARGO_TARGET_DIR/release"
COLLAB_ENGINE="${FVOCI_PERF_COLLAB_ENGINE:-$CARGO_TARGET_DIR/collab-engine/release/collab-engine}"

for bin in "$RELEASE/fvoci-server" "$RELEASE/fvoci-migrate" "$RELEASE/fvoci-e2e-fixture" "$COLLAB_ENGINE"; do
  [[ -x "$bin" ]] || { echo "missing release artifact: $bin" >&2; exit 1; }
done
[[ -d "$ROOT/apps/web/dist" ]] || { echo "missing apps/web/dist (bun --bun run build)" >&2; exit 1; }
(cd "$ROOT/apps/web" && bun --bun x --no-install playwright --version) >/dev/null 2>&1 \
  || { echo "missing web dependencies (bun ci)" >&2; exit 1; }

DATASETS=("$@")
((${#DATASETS[@]} > 0)) || DATASETS=(minimal scaled)
for ds in "${DATASETS[@]}"; do
  [[ "$ds" == minimal || "$ds" == scaled ]] || { echo "unknown dataset: $ds" >&2; exit 1; }
done

mkdir -p "$FVOCI_PERF_OUT"
bash "$ROOT/scripts/perf/capture-env.sh" "$RELEASE/fvoci-server" "$COLLAB_ENGINE" \
  >"$FVOCI_PERF_OUT/environment$(printf '%s' "${FVOCI_PERF_TAG:-}" | tr -cd 'A-Za-z0-9-').json"

for ds in "${DATASETS[@]}"; do
  RUN_DIR="$(mktemp -d "${TMPDIR:-/tmp}/fvoci-perf.XXXXXX")"
  chmod 700 "$RUN_DIR"
  trap 'rm -rf "$RUN_DIR"' EXIT
  cp -a "$ROOT/apps/web/dist" "$RUN_DIR/static"
  # helpers.ts resolves the fixture under $CARGO_TARGET_DIR/debug; point it at
  # the release fixture without touching the shared target directory.
  mkdir -p "$RUN_DIR/fixture-target/debug"
  ln -s "$RELEASE/fvoci-e2e-fixture" "$RUN_DIR/fixture-target/debug/fvoci-e2e-fixture"
  echo "=== perf baseline dataset: $ds ===" >&2
  bash "$ROOT/scripts/start-test-postgres.sh" \
    bash "$ROOT/scripts/start-test-meili.sh" \
    env ROOT="$ROOT" RUN_DIR="$RUN_DIR" RELEASE="$RELEASE" FVOCI_COLLAB_ENGINE="$COLLAB_ENGINE" \
      FVOCI_PERF_OUT="$FVOCI_PERF_OUT" FVOCI_PERF_DATASET="$ds" \
    bash "$ROOT/scripts/perf/perf-inner.sh"
  rm -rf "$RUN_DIR"
  trap - EXIT
done
echo "results in $FVOCI_PERF_OUT" >&2
