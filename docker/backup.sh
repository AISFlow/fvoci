#!/bin/sh
# Backs up the installation of docker/compose.yaml: its database (pg_dump's custom
# format), its files when they are kept in the volume (not those in S3: the bucket is
# the operator's to back up), and copies of its .env and compose files, into a folder
# of their own under --dir that only its owner can read. Then removes this project's
# backups older than --keep-days. compose.yaml's header says more.
#
#   docker/backup.sh [-p project] [-f compose.yaml]... [--env-file .env]
#                    [--profile name]... [--dir folder] [--keep-days 14]
set -eu
script=backup
here=$(CDPATH='' cd -- "$(dirname -- "$0")" && pwd)
# shellcheck source=docker/ops.sh
. "$here/ops.sh"

while [ $# -gt 0 ]; do
    shared_option "$@"
    [ "$shift_by" -gt 0 ] || fail "unknown option $1"
    shift "$shift_by"
done
check_shared_options
# find would take a folder named -x or ! for a part of its expression.
case $backup_dir in
/* | ./*) ;;
*) backup_dir=./$backup_dir ;;
esac

started=$(now_ms)
storage=$(file_storage)
running postgres || fail "PostgreSQL of the project $project does not run (docker compose up -d postgres)"

umask 077
if [ -d "$backup_dir" ]; then
    # Someone else's folder keeps its mode; one that others may read is said. -H: the
    # folder a link names, not the link.
    open=$(find -H "$backup_dir" -maxdepth 0 -perm /077)
    [ -z "$open" ] || say "warning: $backup_dir may be read by others than its owner, and a backup holds every secret"
else
    mkdir -p -- "$backup_dir"
    chmod 700 -- "$backup_dir"
fi
stamp=$(date -u +%Y%m%dT%H%M%SZ)
name="$project-$stamp"
# Made under a name of its own and renamed once whole: a folder of the final name is a
# whole backup, and two backups of the same second do not share one.
work=$(mktemp -d "$backup_dir/.$name.XXXXXX")
trap 'rm -rf -- "$work"' EXIT
trap 'exit 129' HUP
trap 'exit 130' INT
trap 'exit 143' TERM

# The database first, then the files: a file uploaded in between is kept without a
# record (a comparison of the files reports it), never a record without its file.
compose exec -T postgres pg_dump --username=postgres --dbname=fvoci --format=custom \
    >"$work/db.dump"
if [ "$storage" = local ]; then
    # GNU tar ends with 1 when a file changed or went while it was read (an upload, a
    # purge): what it wrote is whole, and the database tells what the files must be.
    # shellcheck disable=SC2016 # the container's shell expands it.
    set -- -c 'tar "$@"; status=$?
        [ "$status" -ne 1 ] || echo "backup: a file changed or went while it was read; the archive is whole" >&2
        [ "$status" -le 1 ]' tar --create --file=- --directory=/data \
        --warning=no-file-changed --warning=no-file-removed \
        --exclude=files/.tmp --exclude=files/.fvoci-ready files
    if running fvoci; then
        compose exec -T fvoci sh "$@" >"$work/files.tar"
    else
        # The volume through fvoci's image and user, as it is here: nothing is pulled
        # or built.
        compose run --rm --no-deps -T --pull never --entrypoint sh fvoci "$@" >"$work/files.tar"
    fi
    files_said="its files"
else
    files_said="no files: they are in S3, whose bucket is yours to back up"
fi
# What it was made with; the .env holds every secret, as the folder's mode says.
cp -- "$env_file" "$work/env"
n=0
set -f
old_ifs=$IFS
IFS=:
for file in $COMPOSE_FILE; do
    n=$((n + 1))
    cp -- "$file" "$work/compose-$n-$(basename -- "$file")"
done
IFS=$old_ifs
set +f
commit=unknown
if running fvoci; then
    ping=$(fvoci_request http GET /api/v1/ping) || ping=""
    commit=$(printf '%s' "$ping" | sed -n 's/.*"commit":"\([^"]*\)".*/\1/p')
    commit=${commit:-unknown}
fi
{
    echo "project=$project"
    echo "made=$stamp"
    echo "file_storage=$storage"
    echo "schema=$(schema_version)"
    echo "commit=$commit"
} >"$work/made"
chmod 600 -- "$work"/*
chmod 700 -- "$work"
# -T: never into a folder of that name made meanwhile.
mv -T -- "$work" "$backup_dir/$name" || fail "$backup_dir/$name is there already"
trap - EXIT

size=$(du -sk -- "$backup_dir/$name" | cut -f1)
say "made $backup_dir/$name (${size} KiB: the database, $files_said) in $(seconds $(($(now_ms) - started)))"

if [ "$keep_days" -gt 0 ]; then
    # This project's backups, and what a backup that was killed left, and nothing else.
    stamp_glob="[0-9][0-9][0-9][0-9][0-9][0-9][0-9][0-9]T[0-9][0-9][0-9][0-9][0-9][0-9]Z"
    # -H: into the folder a link names. find hands the names over itself: a name may
    # hold any byte, a line end too.
    # shellcheck disable=SC2016 # the inner shell's.
    find -H "$backup_dir" -mindepth 1 -maxdepth 1 -type d \
        \( -name "$project-$stamp_glob" -o -name ".$project-$stamp_glob.*" \) \
        -mmin +$((keep_days * 1440)) -exec sh -c '
            days=$1
            shift
            status=0
            for gone; do
                if rm -rf -- "$gone"; then
                    printf "%s: removed %s, older than %s days\n" "$0" "$gone" "$days"
                else
                    status=1
                fi
            done
            exit "$status"' "$script" "$keep_days" {} +
fi
