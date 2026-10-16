#!/usr/bin/env bash
# Isolated test Meilisearch. Run as `start-test-meili.sh <command> [args...]`
# to run one command with FVOCI_MEILI_URL/KEY exported and the container
# removed on exit, or `source` it and own the lifetime in the caller:
# fvoci_test_meili_start, with fvoci_test_meili_stop in the caller's EXIT trap.

# Official CE v1.55.0 digest from .github/workflows/rust.yml
FVOCI_TEST_MEILI_IMAGE="getmeili/meilisearch:v1.55.0@sha256:ff546679970126f8822e15d1b6a06bfbff1f2ce2e43f810cfb5258012f92de5c"

# Set (not exported) only by this shell's fvoci_test_meili_start: stop never
# removes a container name inherited from a parent environment.
_fvoci_test_meili_owned=""

fvoci_test_meili_stop() {
  [[ -n "$_fvoci_test_meili_owned" ]] || return 0
  local container="$_fvoci_test_meili_owned"
  _fvoci_test_meili_owned=""
  # -v is defensive: the pinned image declares no VOLUME today (its data stays
  # in the container layer), but an image that did would leak one per run.
  if [[ -n "${FVOCI_TEST_NETWORK:-}" ]]; then
    if docker inspect "$container" >/dev/null 2>&1; then
      local retired=0
      docker stop --time 10 "$container" >/dev/null || retired=1
      [[ "$(docker inspect --format '{{.State.Running}} {{.State.ExitCode}}' "$container")" == 'false 0' ]] || retired=1
      docker rm -v "$container" >/dev/null || retired=1
      return "$retired"
    fi
  else
    docker rm -f -v "$container" >/dev/null 2>&1 || true
  fi
}

# Starts the container and exports FVOCI_MEILI_URL, FVOCI_MEILI_KEY,
# MEILI_MASTER_KEY and FVOCI_TEST_MEILI_CONTAINER. The container name is
# exported before `docker run`, so the caller's fvoci_test_meili_stop also
# removes a container whose start or readiness failed.
fvoci_test_meili_start() {
  local dependency run_id master_key cid deadline port
  for dependency in docker openssl; do
    command -v "$dependency" >/dev/null 2>&1 || {
      echo "$dependency is required for local test Meilisearch" >&2
      return 1
    }
  done
  run_id="$(openssl rand -hex 16)" || return 1
  master_key="$(openssl rand -hex 16)" || return 1
  export FVOCI_TEST_MEILI_CONTAINER="fvoci-rust-test-meili-${run_id}"
  _fvoci_test_meili_owned="$FVOCI_TEST_MEILI_CONTAINER"

  local -a network_args=()
  local -a remove_args=(--rm)
  if [[ -n "${FVOCI_TEST_NETWORK:-}" ]]; then
    network_args=(--network "$FVOCI_TEST_NETWORK" --network-alias fvoci-meili --cpus=1 --memory=2g --pids-limit=256)
    remove_args=()
  fi
  cid="$(docker run -d "${remove_args[@]}" "${network_args[@]}" \
    --name "$FVOCI_TEST_MEILI_CONTAINER" \
    --label "fvoci.test-run=${run_id}" \
    -e "MEILI_MASTER_KEY=${master_key}" \
    -e MEILI_NO_ANALYTICS=true \
    -e MEILI_ENV=production \
    -p 127.0.0.1:0:7700 \
    "$FVOCI_TEST_MEILI_IMAGE")" || return 1

  deadline=$((SECONDS + 30))
  until docker exec "$cid" wget -q -O /dev/null http://127.0.0.1:7700/health >/dev/null 2>&1; do
    if (( SECONDS >= deadline )); then
      echo "meilisearch did not become ready within 30s" >&2
      docker logs "$cid" >&2 || true
      return 1
    fi
    sleep 1
  done

  port="$(docker port "$cid" 7700 | head -1 | awk -F: '{print $NF}')" || return 1
  if [[ ! "$port" =~ ^[0-9]+$ ]]; then
    echo "meilisearch published no host port for 7700" >&2
    return 1
  fi
  export FVOCI_MEILI_URL="http://127.0.0.1:${port}"
  export FVOCI_MEILI_KEY="$master_key"
  export MEILI_MASTER_KEY="$master_key"
}

if [[ "${BASH_SOURCE[0]}" == "$0" ]]; then
  set -euo pipefail
  if [[ $# -eq 0 ]]; then
    echo "usage: $0 <command> [args...]" >&2
    exit 1
  fi
  trap 'status=$?; fvoci_test_meili_stop || { (( status != 0 )) || status=1; }; exit "$status"' EXIT
  trap 'exit 130' INT
  trap 'exit 143' TERM
  fvoci_test_meili_start
  "$@"
fi
