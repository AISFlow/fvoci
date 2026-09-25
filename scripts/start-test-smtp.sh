#!/usr/bin/env bash
# Start a worktree-owned plaintext SMTP sink on 127.0.0.1:0, export SMTP_*,
# then run the given command. Cleanup is trap-owned; do not reuse a global port.
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
RUN_DIR="${FVOCI_SMTP_RUN_DIR:-$(mktemp -d "${TMPDIR:-/tmp}/fvoci-smtp.XXXXXX")}"
mkdir -p "$RUN_DIR"
chmod 700 "$RUN_DIR"
CAPTURE="$RUN_DIR/smtp.jsonl"
PORT_FILE="$RUN_DIR/smtp.port"
: >"$CAPTURE"

python3 "$ROOT/scripts/smtp-sink.py" --capture "$CAPTURE" --port-file "$PORT_FILE" &
SMTP_PID=$!

cleanup() {
  if [[ -n "${SMTP_PID:-}" ]] && kill -0 "$SMTP_PID" 2>/dev/null; then
    kill "$SMTP_PID" 2>/dev/null || true
    wait "$SMTP_PID" 2>/dev/null || true
  fi
  if [[ -z "${FVOCI_SMTP_RUN_DIR:-}" ]]; then
    rm -rf "$RUN_DIR"
  fi
}
trap cleanup EXIT
trap 'exit 130' INT
trap 'exit 143' TERM

deadline=$((SECONDS + 10))
until [[ -s "$PORT_FILE" ]]; do
  if (( SECONDS >= deadline )); then
    echo "smtp sink did not write port file" >&2
    exit 1
  fi
  if ! kill -0 "$SMTP_PID" 2>/dev/null; then
    echo "smtp sink exited before becoming ready" >&2
    exit 1
  fi
  sleep 0.05
done

export SMTP_HOST="127.0.0.1"
export SMTP_PORT
SMTP_PORT="$(cat "$PORT_FILE")"
export SMTP_FROM="${SMTP_FROM:-noreply@example.com}"
export FVOCI_E2E_SMTP_CAPTURE="$CAPTURE"

if [[ $# -gt 0 ]]; then
  "$@"
else
  echo "SMTP_HOST=$SMTP_HOST SMTP_PORT=$SMTP_PORT FVOCI_E2E_SMTP_CAPTURE=$FVOCI_E2E_SMTP_CAPTURE"
  wait "$SMTP_PID"
fi
