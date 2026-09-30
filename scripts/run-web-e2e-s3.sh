#!/usr/bin/env bash
# Real-browser check of the presigned attachment transfer mode (#149 B) with
# MinIO as a separate storage origin. The normal e2e shards run without MinIO,
# so this group lives in apps/web/e2e-s3 and runs only through this script:
#
#   scripts/run-web-e2e-s3.sh               # presigned and proxy transfers
#   scripts/run-web-e2e-s3.sh --narrow-cors # storage CORS for another origin
#
# It nests the pinned MinIO (scripts/start-test-minio.sh) around the normal
# group runner (scripts/run-web-e2e.sh: build, PostgreSQL, Meilisearch, one
# server). The app listens on 127.0.0.1 and browsers reach MinIO as
# `localhost`, so the two are different hosts and origins (the server refuses
# a storage endpoint on its own host). By default MinIO keeps its default CORS
# (any origin, credentials allowed, ETag exposed; the check strips the
# credentials header itself to show the viewers do not need it); a real bucket
# needs the rules in RUNNING.md. With --narrow-cors MinIO answers CORS only for
# an unrelated origin, and the check expects the presigned upload to fail
# without an API fallback. Playwright traces of this group hold signed URLs:
# treat them as sensitive (the MinIO credentials are per run).
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
# Any origin other than the app's; never contacted.
NARROW_CORS_ORIGIN="https://cors-elsewhere.invalid"

if [[ "${1:-}" != "--inner" ]]; then
  case "${1:-}" in
    "") SPEC=e2e-s3/attachment-transfer.spec.ts ;;
    --narrow-cors)
      SPEC=e2e-s3/attachment-transfer-cors.spec.ts
      export FVOCI_TEST_MINIO_CORS_ALLOW_ORIGIN="$NARROW_CORS_ORIGIN"
      export FVOCI_E2E_S3_CORS_ALLOW_ORIGIN="$NARROW_CORS_ORIGIN"
      ;;
    *)
      echo "usage: $0 [--narrow-cors]" >&2
      exit 2
      ;;
  esac
  exec bash "$ROOT/scripts/start-test-minio.sh" bash "$ROOT/scripts/run-web-e2e-s3.sh" --inner "$SPEC"
fi
SPEC="${2:?internal: spec}"

: "${S3_ENDPOINT:?run through scripts/start-test-minio.sh}"
# The server only probes the bucket; create it with a SigV4-signed request.
curl -fsS -X PUT --aws-sigv4 "aws:amz:${S3_REGION}:s3" \
  --user "${S3_ACCESS_KEY_ID}:${S3_SECRET_ACCESS_KEY}" \
  "${S3_ENDPOINT}/${S3_BUCKET}" >/dev/null

export STORAGE_DRIVER=s3
export S3_PUBLIC_ENDPOINT="${S3_ENDPOINT/127.0.0.1/localhost}"
# Two parts for a file just over 5 MiB (the S3 minimum part size).
export FVOCI_UPLOAD_PART_SIZE_BYTES=5242880
exec bash "$ROOT/scripts/run-web-e2e.sh" --config=e2e-s3/playwright.config.ts "$SPEC"
