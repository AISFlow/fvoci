#!/usr/bin/env bash
# Real-browser check of the presigned attachment transfer mode (#149 B) with
# MinIO as a separate storage origin. The normal e2e shards run without MinIO,
# so this group lives in apps/web/e2e-s3 and runs only through this script:
#
#   scripts/run-web-e2e-s3.sh
#
# It nests the pinned MinIO (scripts/start-test-minio.sh) around the normal
# group runner (scripts/run-web-e2e.sh: build, PostgreSQL, Meilisearch, one
# server). The app listens on 127.0.0.1 and browsers reach MinIO as
# `localhost`, so the two are different hosts and origins (the server refuses
# a storage endpoint on its own host). MinIO keeps its default CORS (any
# origin, credentials allowed, ETag exposed); a real bucket needs the rules in
# RUNNING.md. Playwright traces of this group hold signed URLs: treat them as
# sensitive (the MinIO credentials are per run).
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"

if [[ "${1:-}" != "--inner" ]]; then
  exec bash "$ROOT/scripts/start-test-minio.sh" bash "$ROOT/scripts/run-web-e2e-s3.sh" --inner
fi

: "${S3_ENDPOINT:?run through scripts/start-test-minio.sh}"
# The server only probes the bucket; create it with a SigV4-signed request.
curl -fsS -X PUT --aws-sigv4 "aws:amz:${S3_REGION}:s3" \
  --user "${S3_ACCESS_KEY_ID}:${S3_SECRET_ACCESS_KEY}" \
  "${S3_ENDPOINT}/${S3_BUCKET}" >/dev/null

export STORAGE_DRIVER=s3
export S3_PUBLIC_ENDPOINT="${S3_ENDPOINT/127.0.0.1/localhost}"
# Two parts for a file just over 5 MiB (the S3 minimum part size).
export FVOCI_UPLOAD_PART_SIZE_BYTES=5242880
exec bash "$ROOT/scripts/run-web-e2e.sh" --config=e2e-s3/playwright.config.ts
