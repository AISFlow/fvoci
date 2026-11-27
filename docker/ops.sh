# shellcheck shell=sh disable=SC2034,SC2154 # set here, read by the scripts that source it; and the reverse.
# Sourced by backup.sh, restore.sh and update.sh (docker/compose.yaml's header says how
# to use them): the options they share, compose on the installation they name, and the
# helpers they share. Nothing here prints a secret.

# Bytes, whatever the locale: sed and awk leave a line that is not of its encoding
# unchanged, which would print it whole.
LC_ALL=C
export LC_ALL

# The installation: compose's project, its files and its .env, as the options say.
project=""
compose_files=""
env_file=""
profiles=""
# Where backups go, and how many days they are kept.
backup_dir=${FVOCI_BACKUP_DIR:-${HOME:-.}/fvoci-backups}
keep_days=14

say() { printf '%s: %s\n' "$script" "$*"; }
fail() {
    printf '%s: %s\n' "$script" "$*" >&2
    exit 1
}

# Reads one of the shared options from "$1" and "$2", and sets `shift_by` to how many
# it took (0 when "$1" is none of them).
shared_option() {
    shift_by=2
    case $1 in
    -p | --project-name) project=${2:?$1 needs a value} ;;
    -f | --file) compose_files="$compose_files${compose_files:+:}${2:?$1 needs a value}" ;;
    --env-file) env_file=${2:?$1 needs a value} ;;
    --profile) profiles="$profiles${profiles:+,}${2:?$1 needs a value}" ;;
    --dir) backup_dir=${2:?$1 needs a value} ;;
    --keep-days) keep_days=${2:?$1 needs a value} ;;
    *) shift_by=0 ;;
    esac
}

# Checks the shared options once they are read, fixes compose's files, and reads them
# with the .env through checked_config before compose is asked anything else.
check_shared_options() {
    # Decimal, as the shell's arithmetic reads it (no leading 0, which is octal there),
    # and of at most five digits.
    case $keep_days in
    0) ;;
    '' | *[!0-9]* | 0* | ??????*) fail "--keep-days takes a number of days, 0 to 99999 (0 keeps every backup)" ;;
    esac
    # compose.yaml beside these scripts unless -f (or COMPOSE_FILE, as compose reads it)
    # names others; .env beside the first.
    [ -n "$compose_files" ] || compose_files=${COMPOSE_FILE:-$here/compose.yaml}
    first_file=${compose_files%%:*}
    [ -n "$env_file" ] || env_file="$(dirname -- "$first_file")/.env"
    [ -r "$env_file" ] || fail "$env_file cannot be read (give --env-file)"
    if [ -n "$project" ]; then
        case $project in
        *[!a-z0-9_-]* | [_-]*) fail "the project name \"$project\" is not one compose takes" ;;
        esac
        COMPOSE_PROJECT_NAME=$project
        export COMPOSE_PROJECT_NAME
    fi
    COMPOSE_FILE=$compose_files
    # --profile, else COMPOSE_PROFILES of the environment or the .env, as compose reads.
    if [ -n "$profiles" ]; then
        COMPOSE_PROFILES=$profiles
        export COMPOSE_PROFILES
    fi
    # compose's own separator of COMPOSE_FILE is ':' here: a file whose name holds one
    # cannot be given.
    COMPOSE_PATH_SEPARATOR=:
    export COMPOSE_FILE COMPOSE_PATH_SEPARATOR
    installation=$(checked_config --format yaml) || exit 1
    check_image_names
    # Without -p, the project as compose names it: COMPOSE_PROJECT_NAME of the
    # environment or the .env, else the files' name.
    if [ -z "$project" ]; then
        project=$(printf '%s\n' "$installation" | sed -n 's/^name: //p' | head -n 1)
        # YAML quotes a name that would read as something else ("123", 'yes').
        case $project in
        \"*\" | \'*\') project=${project#?}; project=${project%?} ;;
        esac
        case $project in
        '' | *[!a-z0-9_-]*) fail "compose names no project this script can use" ;;
        esac
        COMPOSE_PROJECT_NAME=$project
        export COMPOSE_PROJECT_NAME
    fi
}

compose() { docker compose --env-file "$env_file" "$@"; }

# Milliseconds since the epoch (whole seconds where date has no %N).
now_ms() {
    ns=$(date +%s%N)
    case $ns in
    *N) echo $(($(date +%s) * 1000)) ;;
    *) echo $((ns / 1000000)) ;;
    esac
}

# "12.3 s" from a number of milliseconds.
seconds() { printf '%d.%d s' $(($1 / 1000)) $(($1 % 1000 / 100)); }

# Whether the container of a service runs. Fails, saying so, if compose cannot tell.
running() {
    containers=$(compose ps --quiet --status running "$1") || fail "compose cannot list the containers of $project"
    [ -n "$containers" ]
}

# Prints `compose config "$@"`, if compose says nothing besides. What it says besides
# (why it refuses its files or .env, or a warning) is shown as compose_said shows it,
# and stops the script: compose reads the files and .env anew at each call and would
# say it again, unhidden, at every later one (`ps`, `pull`, `up`...). A warning is no
# less a stop: the commonest, a variable not set, is a `$` in a value of the .env,
# which compose reads as the start of a variable (and gives the services a value
# other than the one written).
checked_config() {
    # What compose says besides is kept in memory, never in a file: it may quote a secret.
    refused=no
    complaint=$(compose config "$@" 2>&1 >/dev/null) || refused=yes
    if [ "$refused" = yes ] || [ -n "$complaint" ]; then
        shown=$(printf '%s\n' "$complaint" | compose_said)
        printf '%s\n' "$shown" >&2
        case $shown in
        *"compose: warning: The <hidden> variable is not set. Defaulting to a blank string."*)
            say "a \$ in a value of the .env starts a variable, which compose replaces (with nothing when it is not set): the services get the value without it. To keep what they get, take the \$ and the name after it out of the value; to give them the \$, write \$\$ or put the value in single quotes (a secret key or a password changed so no longer opens what it sealed)" >&2
            ;;
        esac
        unset complaint shown
        said_as_is="COMPOSE_FILE='$COMPOSE_FILE' docker compose --env-file '$env_file' config -q shows all of it as compose writes it, values and all"
        [ "$refused" = no ] ||
            fail "compose refuses its files or .env (above, without what it quotes of them; $said_as_is). Nothing was changed"
        fail "compose warns of its files or .env (above, without what it quotes of them; $said_as_is). Nothing was changed"
    fi
    # The same files and .env, which compose has just read without a word.
    compose config "$@" 2>/dev/null ||
        fail "compose read its files or .env once and not twice: were they changed meanwhile? Nothing was changed"
}

# What compose wrote on stderr, read on stdin, as lines it is known to write with
# everything taken from the files or .env left out: a line number at most. Any other
# line is counted, not shown: what compose quotes of a value pasted wrong (a key's
# lines, a quote never closed, a ${...} whose message is the value) may be a secret.
compose_said() {
    awk '
        function said(text) { if (!(text in shown)) { shown[text] = 1; print "compose: " text } }
        /^[ \t]*$/ { next }
        match($0, /^failed to read [^:]*: line [0-9]+: /) {
            rest = substr($0, RLENGTH + 1)
            line = substr($0, 1, RLENGTH - 2)
            sub(/^.*: /, "", line)
            if (rest ~ /^unterminated quoted value /) { said(line ": unterminated quoted value <hidden>"); next }
            if (rest ~ /^unexpected character /) { said(line ": unexpected character <hidden> in variable name <hidden>"); next }
            if (rest == "key cannot contain a space") { said(line ": key cannot contain a space"); next }
        }
        /^failed to read [^:]*: Invalid template: / { said("Invalid template: <hidden>"); next }
        /^failed to read [^:]*: required variable / { said("required variable <hidden> is missing a value"); next }
        /^invalid project name ".*": must consist only of lowercase alphanumeric characters, hyphens, and underscores as well as start with a letter or number$/ {
            said("invalid project name <hidden>: lowercase letters, digits, - and _, starting with a letter or digit"); next
        }
        /^time="[^"]*" level=warning msg="The \\"[A-Za-z0-9_]*\\" variable is not set\. Defaulting to a blank string\."$/ ||
        /^WARN\[[0-9]*\] The "[A-Za-z0-9_]*" variable is not set\. Defaulting to a blank string\.$/ {
            said("warning: The <hidden> variable is not set. Defaulting to a blank string."); next
        }
        { hidden++ }
        END {
            if (hidden) print "compose: " hidden " line(s) hidden: they may quote a value of the .env"
        }'
}

# compose writes its YAML two spaces a level: a service at two, its settings at four,
# its environment's variables at six. A value that holds line ends is written deeper
# (`KEY: |-` and its lines below), and its lines may read like a setting: only a line
# at its own depth is read. A value longer than the line is folded at a space, the rest
# on the lines below, deeper.

# Fails, without showing it, if an image the files and .env name is not a name: compose
# takes a line such as FVOCI_IMAGE=<pasted> ghcr.io/..., which `pull`, `run` and this
# script's messages would then say. Sets fvoci_image to the service fvoci's. A name
# folded on lines below held a space: it is marked so, whatever its first line reads.
check_image_names() {
    names=$(printf '%s\n' "$installation" | awk '
        held != "" && /^     / { held = held " (folded)"; next }
        held != "" { print held; held = "" }
        /^[^ ]/ { top = $1 }
        /^  [^ ]/ { service = $1; sub(/:$/, "", service) }
        top == "services:" && index($0, "    image: ") == 1 { held = service " " substr($0, 12) }
        END { if (held != "") print held }')
    fvoci_image=""
    while IFS=' ' read -r service name; do
        [ -n "$service" ] || continue
        name=${name#[\"\']}
        name=${name%[\"\']}
        case $name in
        '' | *[!A-Za-z0-9._/:@-]*)
            fail "an image named by the files or .env (FVOCI_IMAGE, FVOCI_SEARCH_IMAGE...) is not a name of letters, digits and ._/:@- (it is not shown: it may hold what was pasted beside it). Nothing was changed"
            ;;
        esac
        [ "$service" != fvoci ] || fvoci_image=$name
    done <<END
$names
END
    unset names service name
}

# A variable of the service fvoci's environment as compose reads its files and .env,
# its quotes taken off, and the lines below it that are deeper joined by a space.
# Empty when it has none.
fvoci_setting() {
    printf '%s\n' "$installation" | awk -v line="      $1:" '
        found && /^       / { sub(/^ */, ""); value = value " " $0; next }
        found { exit }
        /^[^ ]/ { top = $1 }
        /^  [^ ]/ { service = $1 }
        /^    [^ ]/ { setting = $1 }
        top == "services:" && service == "fvoci:" && setting == "environment:" &&
        index($0 " ", line " ") == 1 {
            value = substr($0, length(line) + 1); sub(/^ */, "", value)
            found = 1
        }
        END { if (found) { gsub(/["\047]/, "", value); print value } }'
}

# FVOCI_SECRET_KEY as compose resolves it from "$1" (and the shell's environment, unless
# the caller unset FVOCI_SECRET_KEY), as one JSON-encoded string value so line ends and
# inner NAME= lines inside a multiline value are never split. Prints nothing when it is
# unset or empty; fails if compose refuses the .env.
secret_key_from() {
    json=$(
        unset COMPOSE_FILE COMPOSE_PROJECT_NAME COMPOSE_PROFILES COMPOSE_ENV_FILES
        docker compose --project-name fvoci-key-check --env-file "$1" -f - config --format json 2>/dev/null <<'YAML'
services:
  k:
    image: scratch
    environment:
      FVOCI_SECRET_KEY:
YAML
    ) || return 1
    printf '%s\n' "$json" | awk '
        /"FVOCI_SECRET_KEY":[ \t]*"/ {
            sub(/^.*"FVOCI_SECRET_KEY":[ \t]*"/, "")
            sub(/"[ \t]*,?[ \t]*$/, "")
            if ($0 != "") print
        }'
    unset json
}

# Warns when the project runs cloudflared and the range of its network edge is not
# among FVOCI_TRUSTED_PROXIES: a new network (after `down`) may have another, and every
# person behind the tunnel would then be counted as cloudflared's one address.
check_trusted_edge() {
    services=$(compose config --services) || return 0
    printf '%s\n' "$services" | grep -qx cloudflared || return 0
    ranges=$(docker network inspect "${project}_edge" \
        --format '{{range .IPAM.Config}}{{.Subnet}} {{end}}' 2>/dev/null) || return 0
    trusted=$(fvoci_setting FVOCI_TRUSTED_PROXIES | tr -d ' ')
    for range in $ranges; do
        case ",$trusted," in
        *",$range,"*) ;;
        *) say "warning: the network edge is $range, which FVOCI_TRUSTED_PROXIES lacks: set it to $range in the .env and docker compose up -d" ;;
        esac
    done
}

# Which storage fvoci's files are in (local or s3), as compose gives it fvoci.
file_storage() {
    storage=$(fvoci_setting FVOCI_FILE_STORAGE) || return 1
    if [ "$storage" = s3 ]; then echo s3; else echo local; fi
}

# Runs SQL as PostgreSQL's superuser in the database fvoci and prints the rows, fields
# apart by '|'.
sql() {
    compose exec -T postgres psql --username=postgres --dbname=fvoci --no-psqlrc \
        --no-align --tuples-only --set ON_ERROR_STOP=1 --command "$1"
}

# The newest migration the database has had: "none" before the first, "unknown" when
# PostgreSQL does not answer.
schema_version() {
    sql "SELECT COALESCE((SELECT max(version) FROM _sqlx_migrations WHERE success)::text, 'none')" \
        2>/dev/null || echo "unknown (PostgreSQL did not answer)"
}

# Rows of the users and the blocks (cards, pages, boards), "users N, blocks M".
row_counts() {
    sql "SELECT 'users ' || (SELECT count(*) FROM users) || ', blocks ' || (SELECT count(*) FROM blocks)" \
        2>/dev/null || echo "not counted"
}

# Sends "$2 $3" (a method and a path) inside fvoci's container, to its admin socket when
# "$1" is admin and to its HTTP port (under FVOCI_BASE_PATH) when it is http. Prints the
# status code on a line, then the body. fvoci's image has no curl; perl is in every
# Debian.
fvoci_request() {
    # shellcheck disable=SC2016 # perl's variables, not the shell's.
    compose exec -T fvoci perl -MIO::Socket::UNIX -MIO::Socket::INET -e '
        my ($to, $method, $path) = @ARGV;
        # A comparison of many files takes long; a server that hangs does not hold this.
        $SIG{ALRM} = sub { die "fvoci did not answer ($to $path) within 600 s\n" };
        alarm 600;
        my $socket = $to eq "admin"
            ? IO::Socket::UNIX->new(Peer => $ENV{FVOCI_ADMIN_SOCKET_PATH} || "/var/tmp/fvoci_local.socket")
            : IO::Socket::INET->new(PeerAddr => "127.0.0.1", PeerPort => $ENV{FVOCI_PORT} || 8000, Timeout => 10);
        $socket or die "cannot reach fvoci ($to): $!\n";
        $path = ($ENV{FVOCI_BASE_PATH} || "") . $path if $to eq "http";
        print $socket "$method $path HTTP/1.0\r\nHost: localhost\r\nContent-Length: 0\r\n\r\n";
        local $/;
        my ($head, $body) = split /\r\n\r\n/, <$socket> // "", 2;
        my ($status) = ($head // "") =~ m{^HTTP/\S+ (\d{3})};
        print $status // "000", "\n", $body // "";
    ' "$@"
}

# The value of a number member "$1" in the JSON "$2" (the first of that name).
json_number() { printf '%s' "$2" | sed -n "s/.*\"$1\":\([0-9][0-9]*\).*/\1/p" | head -n 1; }

# The first line of "$1".
first_line() { printf '%s\n' "$1" | head -n 1; }
