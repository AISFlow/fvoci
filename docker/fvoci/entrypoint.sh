#!/bin/sh
# The entry of the fvoci image: reads secrets from files into the variables the server
# reads, then becomes the server (its arguments are passed on).
#
# For each NAME below, NAME_FILE may name a file (a Docker secret, /run/secrets/...)
# whose content, without carriage returns and final newlines, becomes NAME. A file
# that is named but missing, empty or blank stops the start, saying which variable
# named it; the value is never printed. A non-empty NAME next to NAME_FILE is refused,
# so that it is clear which one holds.
#
# The settings come from /opt/fvoci/config.json (mount another over it) and the
# FVOCI_* variables; the arguments that would choose another file or port are refused,
# since the health check reads the same file and variables and would ask elsewhere.
#
#   PGPASSWORD                 the password of the PostgreSQL role in
#                              FVOCI_DATABASE_CONNECTION
#                              (the address then carries no password)
#   FVOCI_SECRET_KEY           the key the server seals stored secrets with
#   FVOCI_SEARCH_PASSWORD      the password of the search server's user
#   FVOCI_SEARCH_API_KEY       an Elasticsearch API key, instead of a user
#   FVOCI_METRICS_TOKEN        the bearer token of the metrics listener
#   FVOCI_SMTP_PASSWORD        the password of the mail relay
#   FVOCI_CLOUDFLARE_API_TOKEN, FVOCI_SES_SECRET_ACCESS_KEY  mail providers' secrets
#   AWS_ACCESS_KEY_ID, AWS_SECRET_ACCESS_KEY                 S3 credentials
set -eu

for argument in "$@"; do
    case $argument in
    --config | --config=* | --port | --port=* | --database-kind | --database-kind=* | \
        --database-connection | --database-connection=*)
        echo "fvoci: ${argument%%=*} is not taken here: set FVOCI_PORT, FVOCI_DATABASE_KIND, FVOCI_DATABASE_CONNECTION, or mount a file over /opt/fvoci/config.json" >&2
        exit 1
        ;;
    esac
done

for name in PGPASSWORD FVOCI_SECRET_KEY FVOCI_SEARCH_PASSWORD FVOCI_SEARCH_API_KEY \
    FVOCI_METRICS_TOKEN FVOCI_SMTP_PASSWORD FVOCI_CLOUDFLARE_API_TOKEN \
    FVOCI_SES_SECRET_ACCESS_KEY AWS_ACCESS_KEY_ID AWS_SECRET_ACCESS_KEY; do
    eval "file=\${${name}_FILE:-}"
    [ -n "$file" ] || continue
    eval "given=\${${name}:+set}"
    if [ -n "$given" ]; then
        echo "fvoci: both $name and ${name}_FILE are set; give one" >&2
        exit 1
    fi
    if [ ! -r "$file" ]; then
        echo "fvoci: ${name}_FILE names $file, which cannot be read" >&2
        exit 1
    fi
    value=$(tr -d '\r' <"$file")
    case $value in
    *[![:space:]]*) ;;
    *)
        echo "fvoci: ${name}_FILE names $file, which is empty or blank" >&2
        exit 1
        ;;
    esac
    export "$name=$value"
    unset "${name}_FILE"
done
unset file given value

exec /opt/fvoci/fvoci-server --config /opt/fvoci/config.json "$@"
