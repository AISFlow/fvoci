#!/usr/bin/env bash
# Starts the containers of the tests where `cargo xtask test` does not start them (it
# makes PostgreSQL 18 and S3 only when no address is given), and writes their addresses
# to the job's environment, where xtask and the other tests take them.
#
#   test-services.sh postgres [s3]
#   test-services.sh postgres-17 | postgres-19
#
# It makes what xtask/src/containers.rs makes (`prepare_postgres`,
# `prepare_object_store`): the same images, names, ports, credentials and role. Change
# one with the other. FVOCI_TEST_PG_CONTAINER and FVOCI_TEST_PG_PORT name another
# PostgreSQL container and port, as they do for xtask. `postgres-17` and `postgres-19`
# make the same PostgreSQL from the image of that major, where xtask makes 18 only: the
# other majors the server supports (docs/configuration.md), for the store's tests on
# them (ci.yml's `rust-postgres-versions`). Each has a name and a port of its own, so
# that no run of xtask takes it for its own. The addresses of the PostgreSQL started
# last are the ones the job's later steps see. The credentials are the tests' own, of a
# database that lives for one job on the runner's loopback interface.
set -euo pipefail

POSTGRES=fvoci-test-postgres
POSTGRES_PORT=15434
POSTGRES_IMAGE='postgres:18@sha256:64c8b0d31e4ab6ea665e8426ea406a94bf52de3c05b3a4a8bf713704eefa9328'
POSTGRES_17_IMAGE='postgres:17@sha256:97fb53376be3e4fb1a8a61421daa69dcc3bf2fa39bda3259f5245f9a455af1a9'
POSTGRES_19_IMAGE='postgres:19@sha256:4ea94de574c17f0372722806c949757050fbb1cd7648410fe45d6bba275cc00e'
S3=fvoci-test-s3
S3_PORT=18333
S3_BUCKET=fvoci-test
S3_IMAGE='chrislusf/seaweedfs:4.55@sha256:7ca46aeae2f223b37e5d91593c17ab37738eec6df6ab69b48a0487f7643342b9'

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

# PostgreSQL of the major $1 from the image $2, named $3 on the port $4 unless the
# environment names another.
postgres() {
  local major=$1 image=$2
  local name=${FVOCI_TEST_PG_CONTAINER:-$3} port=${FVOCI_TEST_PG_PORT:-$4} version
  if ! [[ $port =~ ^[1-9][0-9]{0,4}$ ]] || ((port > 65535)); then
    echo "FVOCI_TEST_PG_PORT is no port: $port" >&2
    return 2
  fi
  docker run --detach --name "$name" \
    --env POSTGRES_USER=postgres --env POSTGRES_PASSWORD=fvoci-test --env POSTGRES_DB=fvoci \
    --shm-size 1073741824 --publish "127.0.0.1:$port:5432" \
    "$image" -c max_connections=300
  # Over TCP: while the image makes a new database it answers on the socket only.
  wait_for "$name" docker exec "$name" \
    pg_isready --host 127.0.0.1 --username postgres --dbname fvoci
  # The server the image runs, which the tests take for the major asked for.
  version=$(docker exec "$name" psql --username postgres --dbname fvoci \
    --tuples-only --no-align --command 'SHOW server_version_num')
  if ! [[ $version =~ ^[0-9]+$ ]] || ((version / 10000 != major)); then
    echo "$name runs PostgreSQL $version, not $major" >&2
    return 1
  fi
  echo "$name: PostgreSQL $version on port $port"
  # The role the tests connect as: no superuser, held by row-level security, and able
  # to make the schemas that are its test databases.
  docker exec "$name" psql --username postgres --dbname fvoci \
    --set ON_ERROR_STOP=1 --quiet --command "
      CREATE ROLE fvoci LOGIN NOSUPERUSER NOBYPASSRLS NOCREATEDB NOCREATEROLE
        PASSWORD 'fvoci-test';
      GRANT CREATE ON DATABASE fvoci TO fvoci;"
  local at="localhost:$port/fvoci?sslmode=disable"
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
    postgres) postgres 18 "$POSTGRES_IMAGE" "$POSTGRES" "$POSTGRES_PORT" ;;
    postgres-17) postgres 17 "$POSTGRES_17_IMAGE" "$POSTGRES-17" 15417 ;;
    postgres-19) postgres 19 "$POSTGRES_19_IMAGE" "$POSTGRES-19" 15419 ;;
    s3) s3 ;;
    *)
      echo "unknown service: $service" >&2
      exit 2
      ;;
  esac
done
