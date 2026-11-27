#!/bin/sh
# Brings the installation of docker/compose.yaml to the version its files and .env now
# name (the new docker/ in place, FVOCI_IMAGE set): checks them before anything changes,
# pulls the images, asks the new image whether it can open the database as it is
# (`fvoci-server --migrate --check`), backs up (backup.sh), stops fvoci, starts
# everything (`up`, whose migrate brings the schema up), and checks that fvoci is ready
# and runs the commit expected. Says how long fvoci was down, and, when it fails after
# the stop, what is left and how to go back. compose.yaml's header says more.
#
#   docker/update.sh [-p project] [-f compose.yaml]... [--env-file .env]
#                    [--profile name]... [--dir folder] [--keep-days 14]
#                    [--no-pull] [--commit sha]
#
# --no-pull: the images are on this machine already (built here, or loaded).
# --commit: the commit fvoci must say it is; else the image's label says it.
set -eu
script=update
here=$(CDPATH='' cd -- "$(dirname -- "$0")" && pwd)
# shellcheck source=docker/ops.sh
. "$here/ops.sh"

pull=yes
expected=""
while [ $# -gt 0 ]; do
    shared_option "$@"
    if [ "$shift_by" -gt 0 ]; then
        shift "$shift_by"
        continue
    fi
    case $1 in
    --no-pull) pull=no ;;
    --commit)
        expected=${2:?--commit needs a value}
        case $expected in
        [0-9a-f][0-9a-f][0-9a-f][0-9a-f][0-9a-f][0-9a-f][0-9a-f]*) ;;
        *) fail "--commit takes a commit of 7 or more hexadecimal digits" ;;
        esac
        shift
        ;;
    *) fail "unknown option $1" ;;
    esac
    shift
done
check_shared_options

# The new files and .env, before anything changes: compose reads the variable of an
# environment secret only when it starts the service, so `config` passes a .env that
# lacks one, and `up` would stop half-way.
needed=$(printf '%s\n' "$installation" | awk '
    /^[^ ]/ { secrets = ($1 == "secrets:") }
    secrets && index($0, "    environment: ") == 1 { print $2 }')
# The variables compose sees (.env and the shell's), names only: the values are
# printed nowhere.
defined=$(compose config --environment) ||
    fail "compose cannot list the variables it sees (config --environment needs a newer compose). Nothing was changed"
defined=$(printf '%s\n' "$defined" | sed 's/=.*//')
lacking=""
for name in $needed; do
    printf '%s\n' "$defined" | grep -qx -- "$name" || lacking="$lacking $name"
done
unset defined
[ -z "$lacking" ] ||
    fail "the .env ($env_file) lacks$lacking: add the line (empty when unused; .env.example says what each is). Nothing was changed"

running fvoci || fail "fvoci does not run: start it first (docker compose up -d), or restore. Nothing was changed"
# A service that runs and that compose does not see has its profile off here: `up`
# would leave it on its old image.
seen=$(checked_config --services)
runs=$(docker ps --filter "label=com.docker.compose.project=$project" \
    --format '{{.Label "com.docker.compose.service"}}')
for service in $runs; do
    printf '%s\n' "$seen" | grep -qx -- "$service" ||
        fail "$service runs, but compose does not see it here: its profile is off (give --profile, or COMPOSE_PROFILES in the .env), or the new files no longer have it (docker compose rm --stop it first). Nothing was changed"
done
# The name check_image_names took for fvoci's.
image=$fvoci_image
[ -n "$image" ] || fail "compose names no image for fvoci. Nothing was changed"
old_container=$(compose ps --quiet fvoci)
old_image=$(docker inspect --format '{{.Image}}' "$old_container")
schema_before=$(schema_version)
rows_before=$(row_counts)

# The images first: a slow pull does not stand between the backup and the stop.
if [ "$pull" = yes ]; then
    compose pull ||
        fail "the images were not pulled (--no-pull if they are on this machine): fvoci still runs, nothing was changed"
fi
# Every image `up` will start must be here before fvoci stops.
images=$(checked_config --images)
for wanted in $images; do
    docker image inspect "$wanted" >/dev/null 2>&1 ||
        fail "the image $wanted is not on this machine: fvoci still runs, nothing was changed"
done
# The new server reads the database while the old one serves it, and changes nothing: a
# database it would refuse is found before anything stops.
checked=$(compose run --rm --no-deps -T --pull never migrate --migrate --check 2>&1) ||
    fail "the new image would not open the database (fvoci still runs, nothing was changed): $(printf '%s\n' "$checked" | tail -n 3)"
say "$(printf '%s\n' "$checked" | sed -n 's/.*\(the migration would [^"]*\).*/\1/p' | tail -n 1)"
unset checked

# The image that runs now keeps a unique name of its own: the pull may have moved
# its tag, and a second update must not overwrite the tag of the image before the first.
tag_stamp=$(date -u +%Y%m%dT%H%M%SZ)
kept_tag="fvoci-server:before-update-$project-$tag_stamp"
suffix=1
while docker image inspect "$kept_tag" >/dev/null 2>&1; do
    suffix=$((suffix + 1))
    kept_tag="fvoci-server:before-update-$project-$tag_stamp-$suffix"
done
docker tag "$old_image" "$kept_tag"

# The dump holds what was written before it began.
backed_up=$(date -u +%Y-%m-%dT%H:%M:%SZ)
backup_said=$("$here/backup.sh" -p "$project" --env-file "$env_file" --dir "$backup_dir" \
    --keep-days "$keep_days") ||
    fail "the backup failed: fvoci still runs, nothing was changed"
printf '%s\n' "$backup_said"
backup=$(printf '%s\n' "$backup_said" | sed -n 's/^backup: made \(.*\) ([0-9]* KiB: .*/\1/p')
[ -n "$backup" ] || fail "backup.sh did not say which backup it made: fvoci still runs, nothing was changed"

if [ -z "$expected" ]; then
    expected=$(docker image inspect --format \
        '{{index .Config.Labels "org.opencontainers.image.revision"}}' "$image")
fi

# From here on fvoci is down until it is ready again.
since=$(date -u +%Y-%m-%dT%H:%M:%SZ)
stopped=$(now_ms)
after_the_stop() {
    status=$?
    [ "$status" -ne 0 ] || return 0
    printf '%s\n' \
        "update: failed after fvoci was stopped ($since), before it was ready. What is left:" \
        "  - the backup $backup, made at $backed_up: what was written between it and the" \
        "    stop is not in it, and its env and compose copies are the new version's" \
        "    (going back needs the docker/ and .env that ran before)" \
        "  - the image that ran before: $old_image, tagged $kept_tag" \
        "  - the schema: $(schema_version) (it was $schema_before)" \
        "  docker compose logs migrate fvoci says why. There are no migrations down: if the" \
        "  schema moved, going back is restoring the backup into new volumes with the image" \
        "  that ran before (FVOCI_IMAGE=$kept_tag; docker compose down; remove the volumes" \
        "  postgres-data and fvoci-data; docker/restore.sh <backup>). If it did not move," \
        "  FVOCI_IMAGE=$kept_tag and docker compose up -d bring the old version back." >&2
    exit "$status"
}
trap after_the_stop EXIT
trap 'exit 129' HUP
trap 'exit 130' INT
trap 'exit 143' TERM

say "stopping fvoci, then starting $image"
compose stop fvoci
compose up --detach --no-build --wait --wait-timeout 300
ready=$(fvoci_request http GET /api/v1/ready)
case $ready in
200*) ;;
*) fail "fvoci is up but not ready ($(first_line "$ready"))" ;;
esac
down=$(($(now_ms) - stopped))
# fvoci serves people again: what goes wrong now is not undone by a restore, which
# would lose what they write.
trap - EXIT
check_trusted_edge

ping=$(fvoci_request http GET /api/v1/ping) || ping=""
commit=$(printf '%s' "$ping" | sed -n 's/.*"commit":"\([^"]*\)".*/\1/p')
logs=$(compose logs --no-color --since "$since" migrate fvoci 2>&1) || logs=""
errors=$(printf '%s\n' "$logs" | grep -cw ERROR) || errors=0
warnings=$(printf '%s\n' "$logs" | grep -cw WARN) || warnings=0
unset logs
say "fvoci was down $(seconds "$down") and is ready"
say "schema $schema_before -> $(schema_version)"
say "rows before: $rows_before; now: $(row_counts)"
say "since the stop fvoci and migrate logged $errors ERROR and $warnings WARN lines"
say "the image that ran before is tagged $kept_tag"
if [ -z "$expected" ]; then
    say "commit unknown: the image was built without its commit (ping says \"${commit}\")"
else
    # A commit given short, or without the -dirty of a build from a changed checkout. An
    # image that knows its commit says it in ping: none said is no match.
    case $commit in
    "$expected"*) say "fvoci runs the commit $commit" ;;
    *) fail "fvoci is ready and serving, but ping says the commit \"$commit\", not $expected: FVOCI_IMAGE names another build, or ping did not answer. Going back to the image that ran before ($kept_tag) is safe while the schema did not move; a restore of the backup would lose what was written since $since" ;;
    esac
fi
