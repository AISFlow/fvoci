#!/usr/bin/env bash
# Run one isolated web e2e group (fresh DB/app/server/storage per invocation).
set -euo pipefail

: "${ROOT:?ROOT is required}"
: "${CARGO_TARGET_DIR:?CARGO_TARGET_DIR is required}"

GROUP_LABEL="default-suite"
if [[ "${FVOCI_E2E_PENDING:-}" == "1" ]] && (($# < 1)); then
  GROUP_LABEL="collaboration-pending"
fi

if (($# >= 1)); then
  GROUP_LABEL="$(basename "${1%.spec.ts}")"
  if (($# > 1)); then
    GROUP_LABEL="${GROUP_LABEL}+$(basename "${2%.spec.ts}")"
  fi
fi

RUN_DIR="$(mktemp -d "${TMPDIR:-/tmp}/fvoci-web-e2e.XXXXXX")"
SERVER_LOG="$RUN_DIR/server.log"
PEPPER='{"test":"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"}'

redact_server_log() {
  sed -E \
    -e 's#postgres(ql)?://[^[:space:]]+#postgres://redacted#g' \
    -e 's#(DATABASE_URL|DATABASE_APP_URL|FVOCI_E2E_ADMIN_DATABASE_URL|TEST_DATABASE_URL)=[^[:space:]]+#\1=redacted#g' \
    "$1"
}

# Digest of one retained trace.zip (see scripts/web-e2e-trace-summary.py).
summarize_trace() {
  python3 "$ROOT/scripts/web-e2e-trace-summary.py" "$1"
}

retain_failure_artifacts() {
  local retain_dir log dest trace
  retain_dir="$(mktemp -d "${TMPDIR:-/tmp}/fvoci-collab-e2e-fail.XXXXXX")"
  chmod 700 "$retain_dir"
  if [[ -d "$RUN_DIR/playwright-output" ]] && [[ -n "$(ls -A "$RUN_DIR/playwright-output" 2>/dev/null || true)" ]]; then
    cp -a "$RUN_DIR/playwright-output" "$retain_dir/playwright-output"
  fi
  # The group's own server (ordinary runs); pending runs start servers per spec.
  if [[ -f "$SERVER_LOG" ]]; then
    redact_server_log "$SERVER_LOG" >"$retain_dir/server.log"
  fi
  mkdir -p "$retain_dir/owned-server"
  while IFS= read -r -d '' log; do
    dest="$retain_dir/owned-server/$(basename "$(dirname "$log")").log"
    redact_server_log "$log" >"$dest"
  done < <(find "$RUN_DIR" -mindepth 2 -name server.log -type f -print0 2>/dev/null || true)
  while IFS= read -r -d '' trace; do
    dest="$(dirname "$trace")/browser-summary.txt"
    # Python's own error text is not redacted, so it never reaches the file.
    if ! summarize_trace "$trace" >"$dest" 2>/dev/null; then
      echo "trace summary failed; reproduce the group locally to inspect its trace.zip" >"$dest"
      echo "could not summarize $(basename "$(dirname "$trace")")/trace.zip" >&2
    fi
  done < <(find "$retain_dir" -name trace.zip -type f -print0 2>/dev/null || true)
  echo "retained failure artifacts for group ${GROUP_LABEL} in $retain_dir" >&2
  if [[ -n "${GITHUB_OUTPUT:-}" ]]; then
    printf 'failure-artifacts=%s\n' "$retain_dir" >>"$GITHUB_OUTPUT"
    printf 'failure-group=%s\n' "$GROUP_LABEL" >>"$GITHUB_OUTPUT"
  fi
}

cleanup() {
  local status=$?
  if (( status != 0 )); then
    retain_failure_artifacts || true
  fi
  rm -rf "$RUN_DIR"
}
trap cleanup EXIT

if [[ ! -d "$ROOT/apps/web/dist" ]]; then
  echo "missing apps/web/dist; build web assets before running groups" >&2
  exit 1
fi

cp -a "$ROOT/apps/web/dist" "$RUN_DIR/static"

echo "=== web e2e group: ${GROUP_LABEL} ===" >&2
bash "$ROOT/scripts/start-test-postgres.sh" \
  bash "$ROOT/scripts/start-test-meili.sh" \
  env RUN_DIR="$RUN_DIR" SERVER_LOG="$SERVER_LOG" PEPPER="$PEPPER" ROOT="$ROOT" \
    CARGO_TARGET_DIR="$CARGO_TARGET_DIR" FVOCI_STATIC_DIR="$RUN_DIR/static" \
  bash "$ROOT/scripts/web-e2e-inner.sh" "$@"
