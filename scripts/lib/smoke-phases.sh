# shellcheck shell=bash
# Phases, first error and owned-resource teardown for the container smokes.
# Sourced (never executed) by scripts/{install,backup-restore,upgrade}-smoke.sh.
#
#   smoke_init NAME            after `set -euo pipefail`: ERR trap (records only)
#   phase NAME                 closes the previous phase group, opens this one
#   fail MESSAGE               prints FAIL: MESSAGE, records it, exits 1
#   smoke_done STATUS          last call in the EXIT trap
#   smoke_report STATUS        first call in the EXIT trap: closes the group and
#                              prints `phase NAME failed (exit N): <first error>`
#   smoke_teardown PROJECT CMD...   `CMD down -v --remove-orphans`, then proves no
#                              container, volume or network of PROJECT remains
#   expect_service_image PROJECT SERVICE ID   one running container on image ID
#   smoke_quote_begin / smoke_quote_end       log text between them cannot act as
#                              GitHub workflow commands
#
# In GitHub Actions phases are `::group::` blocks and a failure is also an
# `::error::` annotation; elsewhere they are `== phase NAME` lines.

SMOKE_NAME=""
SMOKE_PHASE=""
SMOKE_LAST_ERR=""
SMOKE_FAIL_FILE=""
SMOKE_QUOTE_TOKEN=""
# JSON reads and assertions (tools/install-smoke/smoke.ts; JSON arg `-` = stdin).
SMOKE_TS="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)/tools/install-smoke/smoke.ts"
SMOKE_REPORT=""
# When set, fail also appends `FAIL: MESSAGE` to this assertion log.
SMOKE_ASSERT_LOG="${SMOKE_ASSERT_LOG:-}"

smoke_ts() {
  bun "$SMOKE_TS" "$@"
}

# json_field JSON KEY...: value at the path (digit KEY = array index).
json_field() {
  smoke_ts field "$@"
}

smoke_check() {
  smoke_ts check "$@"
}

smoke_actions() {
  [[ "${GITHUB_ACTIONS:-}" == true ]]
}

# smoke_acquire_image ROOT: sets IMAGE_TAG and IMAGE_ID. FVOCI_INSTALL_IMAGE
# (and FVOCI_INSTALL_IMAGE_ID if set) must pass install-image.sh verify; a
# mismatch is refused, never rebuilt. Unset: a local run builds it with
# install-image.sh, CI refuses (the image is built once per arch there).
smoke_acquire_image() {
  local root="$1" out
  if [[ -n "${FVOCI_INSTALL_IMAGE:-}" ]]; then
    out="$(bash "$root/scripts/install-image.sh" verify "$FVOCI_INSTALL_IMAGE" ${FVOCI_INSTALL_IMAGE_ID:+"$FVOCI_INSTALL_IMAGE_ID"})" \
      || fail "install image ${FVOCI_INSTALL_IMAGE} refused (see install-image above)"
  elif smoke_actions; then
    fail "FVOCI_INSTALL_IMAGE is required in CI: the image is built once per arch by scripts/install-image.sh; this smoke does not build it"
  else
    echo "FVOCI_INSTALL_IMAGE unset: building the image for this checkout (scripts/install-image.sh build)"
    out="$(bash "$root/scripts/install-image.sh" build)" || fail "install image build failed"
  fi
  IMAGE_TAG="$(sed -n 's/^FVOCI_INSTALL_IMAGE=//p' <<<"$out")"
  IMAGE_ID="$(sed -n 's/^FVOCI_INSTALL_IMAGE_ID=//p' <<<"$out")"
  [[ -n "$IMAGE_TAG" && -n "$IMAGE_ID" ]] || fail "install-image.sh printed no image reference"
}

# Record the command that failed in the main shell. Never exits: set -e
# carries the status, so `cleanup`'s $? and `|| fail` sites keep their meaning.
smoke_on_err() {
  local status="$1" line="$2" command="$3"
  [[ "$BASHPID" == "$$" ]] || return 0
  SMOKE_LAST_ERR="line ${line}: ${command} (exit ${status})"
}

smoke_init() {
  SMOKE_NAME="$1"
  SMOKE_FAIL_FILE="$(mktemp "${TMPDIR:-/tmp}/${SMOKE_NAME}-fail.XXXXXX")"
  set -E
  trap 'smoke_on_err "$?" "$LINENO" "$BASH_COMMAND"' ERR
}

phase() {
  if [[ -n "$SMOKE_PHASE" ]] && smoke_actions; then
    echo "::endgroup::"
  fi
  SMOKE_PHASE="$1"
  SMOKE_LAST_ERR=""
  : >"$SMOKE_FAIL_FILE"
  if smoke_actions; then
    echo "::group::phase ${SMOKE_PHASE}"
  else
    echo "== phase ${SMOKE_PHASE}"
  fi
}

# Written to a file so a fail inside $(...) still names the first error.
fail() {
  printf 'FAIL: %s\n' "$*" >&2
  [[ -z "$SMOKE_ASSERT_LOG" ]] || printf "FAIL: %s\n" "$*" >>"$SMOKE_ASSERT_LOG"
  [[ -z "$SMOKE_FAIL_FILE" ]] || printf '%s\n' "$*" >"$SMOKE_FAIL_FILE"
  exit 1
}

smoke_first_error() {
  if [[ -s "$SMOKE_FAIL_FILE" ]]; then
    cat "$SMOKE_FAIL_FILE"
  elif [[ -n "$SMOKE_LAST_ERR" ]]; then
    printf '%s\n' "$SMOKE_LAST_ERR"
  else
    echo "see the output above"
  fi
}

smoke_report() {
  local status="$1" message
  SMOKE_REPORT=""
  trap - ERR
  if smoke_actions && [[ -n "$SMOKE_PHASE" ]]; then
    echo "::endgroup::"
  fi
  (( status != 0 )) || return 0
  message="phase ${SMOKE_PHASE:-setup} failed (exit ${status}): $(smoke_first_error)"
  SMOKE_REPORT="$message"
  echo "$message" >&2
  if smoke_actions; then
    # Annotation text is data, not a workflow command.
    message="${message//'%'/%25}"
    message="${message//$'\r'/%0D}"
    echo "::error title=${SMOKE_NAME}::${message//$'\n'/%0A}"
  fi
}

# Last call in the EXIT trap: repeats the failure as the final line (a teardown
# failure after passing checks is reported here) and removes the record file.
smoke_done() {
  local status="$1"
  if (( status != 0 )); then
    if [[ -z "$SMOKE_REPORT" ]]; then
      SMOKE_REPORT="phase cleanup failed (exit ${status}): teardown left resources or failed"
      smoke_actions && echo "::error title=${SMOKE_NAME}::${SMOKE_REPORT}"
    fi
    echo "${SMOKE_NAME}: ${SMOKE_REPORT}" >&2
  fi
  rm -f "$SMOKE_FAIL_FILE"
}

# Opens/closes an Actions group for the trap's own output (logs, cleanup).
smoke_group() {
  if smoke_actions; then
    echo "::group::$1"
  else
    echo "== $1"
  fi
}

smoke_group_end() {
  if smoke_actions; then
    echo "::endgroup::"
  fi
}

smoke_quote_begin() {
  smoke_actions || return 0
  SMOKE_QUOTE_TOKEN="$(openssl rand -hex 16)"
  echo "::stop-commands::${SMOKE_QUOTE_TOKEN}"
}

smoke_quote_end() {
  [[ -n "$SMOKE_QUOTE_TOKEN" ]] || return 0
  echo "::${SMOKE_QUOTE_TOKEN}::"
  SMOKE_QUOTE_TOKEN=""
}

service_ids() {
  docker ps -a -q --filter "label=com.docker.compose.project=$1" --filter "label=com.docker.compose.service=$2"
}

running_ids() {
  docker ps -q --filter "label=com.docker.compose.project=$1" --filter "label=com.docker.compose.service=$2"
}

# One container of the service, running, from the expected image.
expect_service_image() {
  local project="$1" service="$2" image="$3" ids
  ids="$(running_ids "$project" "$service")"
  [[ -n "$ids" && "$(wc -l <<<"$ids")" == 1 ]] || fail "$project/$service: expected one running container, got '${ids}'"
  [[ "$(docker inspect -f '{{.Image}}' "$ids")" == "$image" ]] || fail "$project/$service does not run image $image"
}

# Containers, volumes and networks that still carry the project's compose label.
owned_resources() {
  local filter="label=com.docker.compose.project=$1" containers volumes networks
  containers="$(docker ps -a -q --filter "$filter")" || return 1
  volumes="$(docker volume ls -q --filter "$filter")" || return 1
  networks="$(docker network ls -q --filter "$filter")" || return 1
  printf '%s\n' "$containers" "$volumes" "$networks" | awk 'NF' | paste -sd' ' -
}

smoke_teardown() {
  local project="$1" out left rc=0
  shift
  if ! out="$("$@" down -v --remove-orphans 2>&1)"; then
    printf 'cleanup: down failed for %s:\n%s\n' "$project" "$out" >&2
    rc=1
  fi
  if ! left="$(owned_resources "$project")"; then
    echo "cleanup: could not list the resources of $project" >&2
    return 1
  fi
  if [[ -n "$left" ]]; then
    echo "cleanup: $project still has: $left" >&2
    rc=1
  fi
  return "$rc"
}
