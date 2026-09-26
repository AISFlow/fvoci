#!/usr/bin/env bash
# Fail closed unless the native admission integration test passed exactly once.
set -euo pipefail

if [[ $# -ne 1 ]]; then
  echo "usage: $0 <captured-test-log>" >&2
  exit 2
fi

log="$1"
test_name='collab_primary_huge_varint_memory_rejected_1008'
ok_pattern="^test ${test_name} \\.\\.\\. ok\$"

if grep -qE "^test ${test_name} \\.\\.\\. FAILED" "$log"; then
  echo "native admission test failed in captured output" >&2
  exit 1
fi

if grep -qE "^test ${test_name} \\.\\.\\. ignored" "$log"; then
  echo "native admission test was ignored in captured output" >&2
  exit 1
fi

count="$(grep -cE "$ok_pattern" "$log" || true)"
if [[ "$count" -ne 1 ]]; then
  echo "expected exactly one passing run of ${test_name}, found ${count}" >&2
  exit 1
fi
