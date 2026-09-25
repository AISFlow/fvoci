#!/usr/bin/env bash
# Measure where search-index Meili latency goes (enqueue, task execution, polling, ensure).
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
RUN_ID="$(openssl rand -hex 8)"
CREATED_TARGET=0
if [[ -z "${CARGO_TARGET_DIR:-}" ]]; then
  TARGET_DIR="/tmp/fvoci-meili-bench-${RUN_ID}"
  CREATED_TARGET=1
else
  TARGET_DIR="$CARGO_TARGET_DIR"
fi
export CARGO_TARGET_DIR="$TARGET_DIR"

now_ms() {
  date +%s%3N
}

elapsed_ms() {
  local start="$1"
  local end
  end="$(now_ms)"
  echo $((end - start))
}

curl_json() {
  local method="$1"
  local path="$2"
  local body="${3:-}"
  local url="${FVOCI_MEILI_URL}${path}"
  if [[ -n "$body" ]]; then
    curl -sS -X "$method" "$url" \
      -H "Authorization: Bearer ${FVOCI_MEILI_KEY}" \
      -H "Content-Type: application/json" \
      -d "$body"
  else
    curl -sS -X "$method" "$url" \
      -H "Authorization: Bearer ${FVOCI_MEILI_KEY}"
  fi
}

wait_task() {
  local uid="$1"
  local poll_ms="${2:-25}"
  local start
  start="$(now_ms)"
  local polls=0
  while true; do
    polls=$((polls + 1))
    local status
    status="$(curl_json GET "/tasks/${uid}" | jq -r '.status')"
    if [[ "$status" == "succeeded" ]]; then
      echo "$polls $(elapsed_ms "$start")"
      return 0
    fi
    if [[ "$status" == "failed" || "$status" == "canceled" ]]; then
      echo "task ${uid} ${status}" >&2
      return 1
    fi
    sleep "$(awk "BEGIN {print ${poll_ms}/1000}")"
  done
}

measure_raw_upserts() {
  local index="bench_${RUN_ID}"
  echo "== raw Meili documentAdditionOrUpdate (index=${index}) =="
  curl_json POST "/indexes" "{\"uid\":\"${index}\",\"primaryKey\":\"id\"}" | jq -r '.taskUid' >/dev/null
  local settings='{"searchableAttributes":["title"],"filterableAttributes":["kind"]}'
  local settings_uid
  settings_uid="$(curl_json PATCH "/indexes/${index}/settings" "$settings" | jq -r '.taskUid')"
  wait_task "$settings_uid" 25 >/dev/null

  local doc='{"id":"doc1","kind":"document","title":"hello"}'
  local i
  for i in 1 2 3 4 5; do
    local t0 t1 uid polls wait_ms
    t0="$(now_ms)"
    uid="$(curl_json POST "/indexes/${index}/documents" "[{\"id\":\"doc${i}\",\"kind\":\"document\",\"title\":\"hello${i}\"}]" | jq -r '.taskUid')"
    t1="$(elapsed_ms "$t0")"
    read -r polls wait_ms < <(wait_task "$uid" 25)
    echo "  upsert ${i}: enqueue+wait=${t1}ms task_wait=${wait_ms}ms polls=${polls}"
  done
}

measure_enqueue_vs_wait() {
  local index="split_${RUN_ID}"
  echo "== enqueue vs task execution (index=${index}) =="
  curl_json POST "/indexes" "{\"uid\":\"${index}\",\"primaryKey\":\"id\"}" | jq -r '.taskUid' >/dev/null
  local settings_uid
  settings_uid="$(curl_json PATCH "/indexes/${index}/settings" '{"searchableAttributes":["title"]}' | jq -r '.taskUid')"
  wait_task "$settings_uid" 25 >/dev/null

  local t0 t_enqueue uid t_exec polls wait_ms
  t0="$(now_ms)"
  uid="$(curl_json POST "/indexes/${index}/documents" '[{"id":"x","title":"t"}]' | jq -r '.taskUid')"
  t_enqueue="$(elapsed_ms "$t0")"
  read -r polls wait_ms < <(wait_task "$uid" 25)
  t_exec=$((wait_ms))
  echo "  HTTP enqueue: ${t_enqueue}ms"
  echo "  task completion (poll ${polls}×25ms): ${t_exec}ms"
  echo "  implied Meili work: ~$((t_exec - polls * 25))ms (poll overhead ~$((polls * 25))ms)"
}

measure_rust_path() {
  echo "== Rust ensure + upsert + PG hydrate (via integration probe) =="
  scripts/start-test-postgres.sh bash scripts/start-test-meili.sh \
    cargo test --locked --offline --features db-tests --test search_index throughput_probe -- --nocapture 2>&1 \
    | sed -n '/^THROUGHPUT_PROBE /p'
}

cleanup_target() {
  if [[ "$CREATED_TARGET" == 1 ]]; then
    rm -rf "$TARGET_DIR"
  fi
}
trap cleanup_target EXIT

cd "$ROOT"
echo "measure-search-index-throughput run_id=${RUN_ID}"
echo "target_dir=${TARGET_DIR}"
echo

scripts/start-test-meili.sh bash -c "$(declare -f now_ms elapsed_ms curl_json wait_task measure_raw_upserts measure_enqueue_vs_wait); measure_raw_upserts; echo; measure_enqueue_vs_wait"

echo
measure_rust_path
