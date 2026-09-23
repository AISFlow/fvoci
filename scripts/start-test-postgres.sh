#!/usr/bin/env bash
set -euo pipefail

IMAGE="postgres:18.3@sha256:7e32e9833a6fb1c92c32552794cb6ed569d51b445a54907d35fc112ef39684db"
NAME="fvoci-rust-test-pg-${USER:-dev}"

if docker ps --format '{{.Names}}' | grep -qx "$NAME"; then
  cid=$(docker ps -qf "name=^${NAME}$")
else
  cid=$(docker run -d --rm --name "$NAME" -e POSTGRES_PASSWORD=spike -p 0:5432 "$IMAGE")
fi

port=$(docker port "$cid" 5432 | head -1 | awk -F: '{print $NF}')
export FVOCI_TEST_DATABASE_URL="postgres://postgres:spike@127.0.0.1:${port}/postgres"
echo "FVOCI_TEST_DATABASE_URL=$FVOCI_TEST_DATABASE_URL"
