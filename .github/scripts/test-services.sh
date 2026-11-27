#!/usr/bin/env bash
# Starts the containers of the tests for a job that does not run `cargo xtask test`,
# which starts them itself, and writes their addresses to the job's environment.
#
#   test-services.sh postgres [s3]
#
# It makes what xtask/src/containers.rs makes (`prepare_postgres`,
# `prepare_object_store`): the same images, names, ports, credentials and role. Change
# one with the other. The credentials are the tests' own, of a database that lives for
# one job on the runner's loopback interface.
set -euo pipefail

POSTGRES=fvoci-test-postgres
POSTGRES_PORT=15434
POSTGRES_IMAGE='postgres:18@sha256:eb63372c1e05c755f55fa30cbd07aecc443676fc6c0fa90b9002d01537841e1a'
S3=fvoci-test-s3
S3_PORT=18333
S3_BUCKET=fvoci-test
S3_IMAGE='chrislusf/seaweedfs:latest@sha256:4b42c212543a0acd4c0e675215ecdcaf62a186ef98def76c7e12701871222d15'

# Tries a command every half second until it succeeds, for 90 seconds by the clock.
wait_for() {
  local what=$1
  shift
  SECONDS=0
  while ((SECONDS < 90)); do
    if "$@" >/dev/null 2>&1; then return 0; fi
    sleep 0.5
  done
  echo "$what was not ready after 90 seconds" >&2
  return 1
}

postgres() {
  docker run --detach --name "$POSTGRES" \
    --env POSTGRES_USER=postgres --env POSTGRES_PASSWORD=fvoci-test --env POSTGRES_DB=fvoci \
    --shm-size 1073741824 --publish "127.0.0.1:$POSTGRES_PORT:5432" \
    "$POSTGRES_IMAGE" -c max_connections=300
  # Over TCP: while the image makes a new database it answers on the socket only.
  wait_for "$POSTGRES" docker exec "$POSTGRES" \
    pg_isready --host 127.0.0.1 --username postgres --dbname fvoci
  # The role the tests connect as: no superuser, held by row-level security, and able
  # to make the schemas that are its test databases.
  docker exec "$POSTGRES" psql --username postgres --dbname fvoci \
    --set ON_ERROR_STOP=1 --quiet --command "
      CREATE ROLE fvoci LOGIN NOSUPERUSER NOBYPASSRLS NOCREATEDB NOCREATEROLE
        PASSWORD 'fvoci-test';
      GRANT CREATE ON DATABASE fvoci TO fvoci;"
  local at="localhost:$POSTGRES_PORT/fvoci?sslmode=disable"
  {
    echo "FVOCI_STORE_TEST_CONN_STRING=postgres://fvoci:fvoci-test@$at"
    echo "FVOCI_STORE_TEST_SUPERUSER_CONN_STRING=postgres://postgres:fvoci-test@$at"
  } >>"$GITHUB_ENV"
}

# 200: the bucket was made. 409: it was there already.
bucket_answers() {
  local status
  status=$(curl --silent --output /dev/null --write-out '%{http_code}' --max-time 2 \
    --request PUT "http://localhost:$S3_PORT/$S3_BUCKET")
  [[ $status == 200 || $status == 409 ]]
}

s3() {
  docker run --detach --name "$S3" --publish "127.0.0.1:$S3_PORT:8333" \
    "$S3_IMAGE" server -s3 -s3.port=8333
  wait_for "the bucket of $S3" bucket_answers
  {
    echo "FVOCI_S3_TEST_ENDPOINT=localhost:$S3_PORT"
    echo "FVOCI_S3_TEST_BUCKET=$S3_BUCKET"
  } >>"$GITHUB_ENV"
}

for service in "$@"; do
  case $service in
    postgres) postgres ;;
    s3) s3 ;;
    *)
      echo "unknown service: $service" >&2
      exit 2
      ;;
  esac
done
