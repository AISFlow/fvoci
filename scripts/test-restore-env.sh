#!/usr/bin/env bash
# The $ in single quotes below is literal env-file text on purpose.
# shellcheck disable=SC2016
# Checks how scripts/restore.sh reads keyring and password values from the
# operator env file: the accepted forms must give the value docker compose
# interpolates, and every other form must be refused. When `docker compose`
# is available each accepted case is also compared with Compose's own parse.
#
#   bash scripts/test-restore-env.sh
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
WORK="$(mktemp -d)"
trap 'rm -rf "$WORK"' EXIT

# Load only the env-file parser from restore.sh (the rest of it runs a restore).
sed -n '/^# --- env-file values/,/^# --- end env-file values ---$/p' "$ROOT/scripts/restore.sh" >"$WORK/parser.sh"
grep -q '^env_file_value() {' "$WORK/parser.sh" || { echo "env_file_value not found in restore.sh" >&2; exit 1; }
# shellcheck source=/dev/null
source "$WORK/parser.sh"

COMPOSE=0
if docker compose version >/dev/null 2>&1; then
  COMPOSE=1
  printf 'services:\n  probe:\n    image: scratch\n    environment:\n      V: ${KEY}\n' >"$WORK/compose.yml"
fi

failures=0
json='{"install":"0123abcd"}'

accept() { # name, env line, expected value
  local got status=0
  printf '%s\n' "$2" >"$WORK/env"
  got="$(env_file_value KEY "$WORK/env" 2>/dev/null)" || status=$?
  if ((status != 0)) || [[ "$got" != "$3" ]]; then
    echo "FAIL accept $1: status $status, got [$got], want [$3]"
    failures=$((failures + 1))
    return
  fi
  if ((COMPOSE)); then
    local composed
    composed="$(cd "$WORK" && env -i PATH="$PATH" HOME="$WORK" docker compose -f compose.yml --env-file env config --format json |
      python3 -c 'import json,sys; print(json.load(sys.stdin)["services"]["probe"]["environment"]["V"])')"
    if [[ "$composed" != "$3" ]]; then
      echo "FAIL compose $1: compose gives [$composed], restore.sh [$got]"
      failures=$((failures + 1))
      return
    fi
  fi
  echo "ok   accept $1"
}

refuse() { # name, env file content
  local status=0
  printf '%s\n' "$2" >"$WORK/env"
  env_file_value KEY "$WORK/env" >/dev/null 2>&1 || status=$?
  if ((status != 2)); then
    echo "FAIL refuse $1: status $status, want 2"
    failures=$((failures + 1))
    return
  fi
  echo "ok   refuse $1"
}

accept "unquoted json" "KEY=$json" "$json"
accept "single-quoted json" "KEY='$json'" "$json"
accept "double-quoted hex" 'KEY="0123abcd"' "0123abcd"
accept "unquoted base64" "KEY=ab+/cd==" "ab+/cd=="
accept "crlf line" $'KEY=abc\r' "abc"
accept "other keys around" $'OTHER=1\nKEY=abc\nKEYS=2' "abc"

refuse "inner double quotes" "KEY=\"$json\""
refuse "double-quoted with backslash" 'KEY="a\nb"'
refuse "double-quoted with dollar" 'KEY="a${B}"'
refuse "unquoted with dollar" 'KEY=a$B'
refuse "unquoted inline comment" 'KEY=abc # note'
refuse "unquoted with space" 'KEY=a b'
refuse "unquoted with hash" 'KEY=a#b'
refuse "single quote not closed" "KEY='abc"
refuse "text after single quotes" "KEY='abc' # note"
refuse "inner single quote" "KEY='a'b'"
refuse "export prefix" 'export KEY=abc'
refuse "space around =" 'KEY = abc'
refuse "leading space" ' KEY=abc'
refuse "repeated key" $'KEY=abc\nKEY=def'

status=0
printf 'OTHER=1\n' >"$WORK/env"
env_file_value KEY "$WORK/env" >/dev/null 2>&1 || status=$?
if ((status == 1)); then echo "ok   missing key reports 1"; else echo "FAIL missing key: status $status"; failures=$((failures + 1)); fi

((COMPOSE)) || echo "note: docker compose not available; accepted values were not compared with Compose"
if ((failures)); then
  echo "$failures case(s) failed" >&2
  exit 1
fi
echo "restore.sh env-file parsing: ok"
