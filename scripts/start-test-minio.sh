#!/usr/bin/env bash
set -euo pipefail

for dependency in docker openssl curl; do
  command -v "$dependency" >/dev/null 2>&1 || {
    echo "$dependency is required for local test MinIO" >&2
    exit 1
  }
done

if [[ $# -eq 0 ]]; then
  echo "usage: $0 <command> [args...]" >&2
  exit 1
fi
CMD=("$@")

# Pinned silo (MinIO) image from source packages/storage/test/s3.test.ts at
# SHA 393795261322b916e588043cf94feca999175843; keep in sync with
# src/attachments/s3.rs MINIO_TEST_IMAGE.
IMAGE="pgsty/silo:RELEASE.2026-08-06T00-00-00Z@sha256:29a498b24669cae1fed11c1a2fb2b3d73c68829a0a9c0b14e71b386671d38fac"
RUN_ID="$(openssl rand -hex 16)"
CONTAINER="fvoci-rust-test-minio-${RUN_ID}"
ACCESS_KEY="fvoci$(openssl rand -hex 8)"
SECRET_KEY="$(openssl rand -hex 24)"
# Credentials reach the container through a mode-600 file, never argv.
ENV_FILE="$(mktemp "${TMPDIR:-/tmp}/fvoci-minio-env.XXXXXX")"
chmod 600 "$ENV_FILE"
printf 'MINIO_ROOT_USER=%s\nMINIO_ROOT_PASSWORD=%s\n' "$ACCESS_KEY" "$SECRET_KEY" >"$ENV_FILE"

cleanup() {
  docker rm -f "$CONTAINER" >/dev/null 2>&1 || true
  rm -f "$ENV_FILE"
}
trap cleanup EXIT
trap 'exit 130' INT
trap 'exit 143' TERM

cid="$(docker run -d --rm \
  --name "$CONTAINER" \
  --label "fvoci.test-run=${RUN_ID}" \
  --env-file "$ENV_FILE" \
  -p 127.0.0.1:0:9000 \
  "$IMAGE" \
  server /data)"

port="$(docker port "$cid" 9000 | head -1 | awk -F: '{print $NF}')"
endpoint="http://127.0.0.1:${port}"

deadline=$((SECONDS + 30))
until curl -fsS -o /dev/null "${endpoint}/minio/health/ready"; do
  if (( SECONDS >= deadline )); then
    echo "minio/silo did not become ready within 30s" >&2
    docker logs "$cid" 2>&1 | tail -20 >&2 || true
    exit 1
  fi
  sleep 1
done

# The bucket is created by the tests (S3Storage::ensure_bucket), which also
# exercises the signed CreateBucket path.
export S3_ENDPOINT="$endpoint"
export S3_REGION="${S3_REGION:-us-east-1}"
export S3_BUCKET="fvoci-test-${RUN_ID:0:12}"
export S3_ACCESS_KEY_ID="$ACCESS_KEY"
export S3_SECRET_ACCESS_KEY="$SECRET_KEY"
export S3_FORCE_PATH_STYLE="${S3_FORCE_PATH_STYLE:-1}"
export FVOCI_TEST_MINIO_CONTAINER="$CONTAINER"

# Keep this shell alive so EXIT cleans up after both successful and failed commands.
"${CMD[@]}"
