#!/bin/sh
# One-shot database bootstrap for compose: create the non-superuser app role, migrate,
# then apply grant-app-role.sql through fvoci-migrate. Requires owner DATABASE_URL.
#
# Secrets come from the environment (infra/rust/compose.yml with an env file) or,
# in the standalone install (compose.user.yml), from files written by
# `fvoci-migrate --bootstrap-secrets`: POSTGRES_PASSWORD_FILE (with POSTGRES_USER,
# POSTGRES_DB and FVOCI_DB_HOST) replaces DATABASE_URL, FVOCI_APP_PASSWORD_FILE and
# MEILI_MASTER_KEY_FILE replace their variables. Setting both forms is an error.
set -eu

# from_file VAR: VAR_FILE's contents become VAR.
from_file() {
  eval "file=\${${1}_FILE:-}"
  [ -n "$file" ] || return 0
  eval "set_too=\${${1}+x}"
  if [ -n "$set_too" ]; then
    echo "init-db: $1 and ${1}_FILE are both set; set only one" >&2
    exit 1
  fi
  [ -s "$file" ] || { echo "init-db: ${1}_FILE ($file) is missing or empty" >&2; exit 1; }
  value="$(cat "$file")"
  export "$1=$value"
}

if [ -n "${POSTGRES_PASSWORD_FILE:-}" ]; then
  if [ -n "${DATABASE_URL+x}" ]; then
    echo "init-db: DATABASE_URL and POSTGRES_PASSWORD_FILE are both set; set only one" >&2
    exit 1
  fi
  : "${POSTGRES_USER:?POSTGRES_USER is required with POSTGRES_PASSWORD_FILE}"
  : "${POSTGRES_DB:?POSTGRES_DB is required with POSTGRES_PASSWORD_FILE}"
  : "${FVOCI_DB_HOST:?FVOCI_DB_HOST is required with POSTGRES_PASSWORD_FILE}"
  [ -s "$POSTGRES_PASSWORD_FILE" ] || {
    echo "init-db: POSTGRES_PASSWORD_FILE ($POSTGRES_PASSWORD_FILE) is missing or empty" >&2
    exit 1
  }
  # The bootstrap writes hex passwords, so no URL escaping is needed.
  DATABASE_URL="postgres://${POSTGRES_USER}:$(cat "$POSTGRES_PASSWORD_FILE")@${FVOCI_DB_HOST}/${POSTGRES_DB}"
  export DATABASE_URL
fi
from_file FVOCI_APP_PASSWORD
from_file MEILI_MASTER_KEY

: "${DATABASE_URL:?DATABASE_URL is required}"
: "${FVOCI_APP_ROLE:?FVOCI_APP_ROLE is required}"
: "${FVOCI_APP_PASSWORD:?FVOCI_APP_PASSWORD is required}"

# pct_decode S: libpq's percent-decoding of a URI component (printf is a builtin,
# so the value never reaches any argv).
pct_decode() {
  rest=$1
  while [ -n "$rest" ]; do
    case $rest in
      %[0-9A-Fa-f][0-9A-Fa-f]*)
        hex=${rest#%}
        hex=${hex%"${hex#??}"}
        # shellcheck disable=SC2059 # the format is the octal escape itself
        printf "\\$(printf '%03o' "0x$hex")"
        rest=${rest#%??}
        ;;
      *)
        printf '%s' "${rest%"${rest#?}"}"
        rest=${rest#?}
        ;;
    esac
  done
}

# psql gets the owner URL without its password and reads the password from
# PGPASSWORD; the app password is read with \getenv. Neither is on psql's argv,
# which any host user could read from /proc while init runs.
scheme=${DATABASE_URL%%://*}
authority=${DATABASE_URL#*://}
userinfo=${authority%%@*}
OWNER_URL=$DATABASE_URL
owner_password=${PGPASSWORD:-}
case $userinfo in
  "$authority" | */* | *\?*) ;; # no user info before the host
  *:*)
    # The x keeps a trailing decoded newline from being stripped.
    owner_password=$(pct_decode "${userinfo#*:}"; printf x)
    owner_password=${owner_password%x}
    OWNER_URL="${scheme}://${userinfo%%:*}@${authority#*@}"
    ;;
esac

# Role name and password are psql variables quoted by format() (%I / %L),
# never interpolated into SQL text by the shell.
PGPASSWORD=$owner_password psql -X -v ON_ERROR_STOP=1 \
  -v app_role="$FVOCI_APP_ROLE" \
  "$OWNER_URL" <<'SQL'
\getenv app_password FVOCI_APP_PASSWORD
SELECT format('CREATE ROLE %I LOGIN PASSWORD %L NOSUPERUSER NOBYPASSRLS', :'app_role', :'app_password')
WHERE NOT EXISTS (SELECT 1 FROM pg_roles WHERE rolname = :'app_role')
\gexec
SQL

/opt/fvoci/bin/fvoci-migrate
/opt/fvoci/bin/fvoci-migrate --grant-app-role "$FVOCI_APP_ROLE"

if [ -n "${FVOCI_MEILI_URL:-}" ]; then
  : "${MEILI_MASTER_KEY:?MEILI_MASTER_KEY is required when FVOCI_MEILI_URL is set}"
  KEY_FILE="${FVOCI_MEILI_KEY_FILE:-/run/fvoci/meili/api_key}"
  /opt/fvoci/bin/fvoci-migrate --ensure-meili-key "$KEY_FILE"
fi
