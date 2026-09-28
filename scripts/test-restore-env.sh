#!/usr/bin/env bash
# The $ in single quotes below is literal env-file text on purpose.
# shellcheck disable=SC2016
# Checks how scripts/restore.sh reads keyring and password values from the
# operator env file: the accepted forms must give the value docker compose
# interpolates, and every other form must be refused. When `docker compose`
# is available each accepted case is also compared with Compose's own parse.
# Also checks that restore.sh keeps the app password off every argv.
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

indent() { while IFS= read -r line; do printf '     %s\n' "$line"; done; }

# The app password must stay off every argv: the host docker/compose process
# and psql inside the container are both readable from /proc by any local
# user. Static part: nothing in restore.sh hands the value to -e/-v, and only
# create_app_role reads $APP_PASSWORD.
leaks="$(grep -nE -- '-[ev][[:space:]]+["'\'']?app_password=' "$ROOT/scripts/restore.sh" || true)"
if [[ -n "$leaks" ]]; then
  echo "FAIL app password: passed as a -e/-v value"
  indent <<<"$leaks"
  failures=$((failures + 1))
else
  echo "ok   app password: no -e/-v value"
fi
readers="$(grep -nF -e '$APP_PASSWORD' -e '${APP_PASSWORD' "$ROOT/scripts/restore.sh" | grep -vE '^[0-9]+:[[:space:]]*app_password="\$APP_PASSWORD" "\$\{COMPOSE\[@\]\}" exec' || true)"
if [[ -n "$readers" ]]; then
  echo "FAIL app password: \$APP_PASSWORD used outside create_app_role's exec"
  indent <<<"$readers"
  failures=$((failures + 1))
else
  echo "ok   app password: read only by create_app_role's exec"
fi

# Dynamic part: run create_app_role against a stub compose that records its
# argv, its environment and the SQL on stdin.
sed -n '/^# --- app role/,/^# --- end app role ---$/p' "$ROOT/scripts/restore.sh" >"$WORK/role.sh"
if ! grep -q '^create_app_role() {' "$WORK/role.sh"; then
  echo "FAIL app password: create_app_role not found in restore.sh"
  failures=$((failures + 1))
else
  cat >"$WORK/compose-stub" <<'STUB'
#!/usr/bin/env bash
# Records one call; refuses anything but `exec`, so nothing else can run here.
[[ "${1-}" == exec ]] || { echo "stub compose: unexpected call: $1" >&2; exit 97; }
printf '%s\0' "$@" >"$STUB_DIR/argv"
if [[ -n "${app_password+x}" ]]; then printf '%s' "$app_password" >"$STUB_DIR/env"; fi
cat >"$STUB_DIR/stdin"
STUB
  chmod 700 "$WORK/compose-stub"
  sentinel="wp4-sentinel-'q\"d\\b\$x"
  status=0
  (
    # shellcheck source=/dev/null
    source "$WORK/role.sh"
    # shellcheck disable=SC2034 # read by create_app_role
    COMPOSE=("$WORK/compose-stub") APP_ROLE=fvoci_app APP_PASSWORD="$sentinel"
    export STUB_DIR="$WORK"
    create_app_role
  ) || status=$?
  role_fail() {
    echo "FAIL app password: $1"
    failures=$((failures + 1))
  }
  if ((status != 0)) || [[ ! -f "$WORK/argv" ]]; then
    role_fail "create_app_role did not run the stub (status $status)"
  else
    mapfile -d '' -t argv <"$WORK/argv"
    bare=0 script=""
    for ((i = 0; i < ${#argv[@]}; i++)); do
      [[ "${argv[i]}" == -e && "${argv[i + 1]-}" == app_password ]] && bare=1
      [[ "${argv[i]}" == -c ]] && script="${argv[i + 1]-}"
    done
    if grep -qF -- "$sentinel" "$WORK/argv"; then
      role_fail "the value is on the compose argv"
    elif ((!bare)); then
      role_fail "compose does not get -e app_password by name"
    elif [[ "$(cat "$WORK/env" 2>/dev/null)" != "$sentinel" ]]; then
      role_fail "compose's environment does not hold app_password"
    elif [[ -z "$script" || "$script" == *app_password* ]]; then
      role_fail "the container shell puts app_password on psql's argv"
    elif ! grep -qxF '\getenv app_password app_password' "$WORK/stdin"; then
      role_fail "psql does not read app_password with \\getenv"
    elif grep -qF -- "$sentinel" "$WORK/stdin"; then
      role_fail "the value is in the SQL text"
    else
      echo "ok   app password: environment only, read with \\getenv"
    fi
  fi
fi

if ((failures)); then
  echo "$failures case(s) failed" >&2
  exit 1
fi
echo "restore.sh env-file parsing and app password handling: ok"
