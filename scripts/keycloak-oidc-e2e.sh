#!/usr/bin/env bash
# Opt-in local check (not CI): the official Keycloak image (start-dev, one
# imported test realm, published on 127.0.0.1) as the instance OIDC provider
# (OIDC_GENERIC_*) of the release FVOCI server built from this checkout,
# driven through the web UI by Playwright Chromium on this host.
#
# It checks sign-in, account linking, invitation acceptance, sign-out and the
# failure boundaries in apps/web/e2e-keycloak/oidc-keycloak-flow.spec.ts. It
# does not check external providers, HTTPS or a reverse proxy, or the
# container deployment path.
#
# --workspace-sso also imports two workspace realms and runs the ignored
# Rust test keycloak_workspace_sso_with_a_test_entitlement (in-process app
# with a test license; not the release server, whose builds trust no license
# key) against them.
#
# Usage: scripts/keycloak-oidc-e2e.sh [--skip-build] [--workspace-sso]
#   FVOCI_KC_E2E_EVIDENCE_DIR=<dir>  also keep redacted evidence there
# Needs docker (compose), openssl, git, cargo, bun, setsid (util-linux),
# realpath (coreutils) and scripts/prepare-web-e2e.sh; the first run pulls the
# Keycloak image. The web build, Playwright and the helper
# tools/keycloak/kc-e2e.ts run under Bun, as in the web e2e harness; node,
# npm and python are not used.
# Exits non-zero when a group fails or when its compose project could not be
# removed completely.
set -euo pipefail

# The per-run secrets reach child processes as environment assignments; a
# shell trace would print them.
if [[ $- == *x* ]]; then
  echo "refusing to run with xtrace on: it would print the per-run secrets" >&2
  exit 2
fi
unset BASH_XTRACEFD BASH_ENV ENV

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
KC_DIR="$ROOT/scripts/keycloak"
COMPOSE_FILE="$KC_DIR/compose.yml"
HELPER="$ROOT/tools/keycloak/kc-e2e.ts"
CARGO_TARGET_DIR="${CARGO_TARGET_DIR:-$ROOT/target}"
CARGO_TARGET_DIR="$(realpath -m -- "$CARGO_TARGET_DIR")"
export ROOT CARGO_TARGET_DIR
REALM="fvoci-e2e"
CLIENT_ID="fvoci-e2e"
LABEL="Keycloak E2E"
READY_LIMIT_S=240
# One server per group: the OIDC rate limit (30 per IP per 5 minutes, in
# memory) cannot take every flow on one server.
MODES=(flows failures wrong-secret)

# The workspace SSO test binary is built without debug info or incremental
# state (about 1.7 GiB of target/debug instead of 5).
SSO_TEST=(env CARGO_PROFILE_DEV_DEBUG=0 CARGO_INCREMENTAL=0 cargo test --locked --offline
  --features db-tests --test identity_integration)
SKIP_BUILD=0
WORKSPACE_SSO=0
for arg in "$@"; do
  case "$arg" in
    --skip-build) SKIP_BUILD=1 ;;
    --workspace-sso) WORKSPACE_SSO=1 ;;
    *)
      echo "usage: $0 [--skip-build] [--workspace-sso]" >&2
      exit 2
      ;;
  esac
done

for dependency in docker openssl git cargo bun setsid realpath; do
  command -v "$dependency" >/dev/null 2>&1 || {
    echo "$dependency is required" >&2
    exit 1
  }
done
# The workspace's locked Playwright, run by Bun; never fetched on demand.
if ! (cd "$ROOT/apps/web" && bun --bun x --no-install playwright --version) >/dev/null 2>&1; then
  echo "missing web dependencies or Playwright; run scripts/prepare-web-e2e.sh" >&2
  exit 1
fi
# For versions.json: the package.json of @playwright/test and the
# browsers.json of the playwright-core it runs, as Bun resolves them from
# apps/web (bunfig.toml hoists them to the root node_modules).
PLAYWRIGHT_FILES="$(cd "$ROOT/apps/web" && bun --bun -e '
const path = require("node:path");
const test = require.resolve("@playwright/test/package.json");
const playwright = require.resolve("playwright/package.json", { paths: [path.dirname(test)] });
const core = require.resolve("playwright-core/package.json", { paths: [path.dirname(playwright)] });
console.log(JSON.stringify({ test, browsers: path.join(path.dirname(core), "browsers.json") }));
')"
# The helper reads per-run secrets from its environment: it loads no .env
# file and no bunfig.toml from the caller's directory, only the repository's.
KC=(bun --no-env-file --config="$ROOT/bunfig.toml" "$HELPER")
kc() {
  "${KC[@]}" "$@"
}
# The redaction every log and evidence file goes through, on its known cases.
kc selftest >&2
# The groups run with TMPDIR=$TMPDIR/fvoci-kc-e2e.XXXXXX/tmp (removed on
# exit); Chromium keeps sockets there and aborts on a long path.
TMP_BASE="${TMPDIR:-/tmp}"
if (( ${#TMP_BASE} > 36 )); then
  echo "TMPDIR is too long for Chromium's socket paths under the run directory; use a short TMPDIR" >&2
  exit 1
fi

SOURCE_SHA="$(git -C "$ROOT" rev-parse HEAD)"
if [[ "$SKIP_BUILD" == 0 ]]; then
  echo "=== build: release fvoci-server/fvoci-migrate and web assets at ${SOURCE_SHA} ===" >&2
  (cd "$ROOT" && FVOCI_BUILD_SHA="$SOURCE_SHA" cargo build --locked --offline --release \
    --bin fvoci-server --bin fvoci-migrate)
  (cd "$ROOT/apps/web" && bun --bun run build)
  if [[ "$WORKSPACE_SSO" == 1 ]]; then
    (cd "$ROOT" && "${SSO_TEST[@]}" --no-run)
  fi
fi
for bin in fvoci-server fvoci-migrate; do
  [[ -x "$CARGO_TARGET_DIR/release/$bin" ]] || {
    echo "missing $CARGO_TARGET_DIR/release/$bin" >&2
    exit 1
  }
done
BUILT="$("$CARGO_TARGET_DIR/release/fvoci-server" --version)"
if [[ "$BUILT" != *"(${SOURCE_SHA})"* ]]; then
  echo "target/release/fvoci-server is not the build of HEAD ${SOURCE_SHA} (${BUILT}); run without --skip-build" >&2
  exit 1
fi
if [[ -n "$(git -C "$ROOT" status --porcelain --untracked-files=no)" ]]; then
  echo "warning: uncommitted changes; the binaries report ${SOURCE_SHA} and --skip-build reuses apps/web/dist as is" >&2
fi

RUN_ID="$(openssl rand -hex 6)"
PROJECT="fvoci-kc-e2e-${RUN_ID}"
WORK="$(mktemp -d "${TMPDIR:-/tmp}/fvoci-kc-e2e.XXXXXX")"
chmod 700 "$WORK"
# The groups' own TMPDIR: whatever the harness, Playwright or Chromium leave
# there (also after an interrupt) goes with $WORK.
GROUP_TMP="$WORK/tmp"
mkdir -p "$WORK/out" "$WORK/realm" "$GROUP_TMP"
# Listed by the container's keycloak user; $WORK above stays 0700.
chmod 755 "$WORK/realm"
CONFIG="$WORK/kc-e2e.json"
SSO_CONFIG="$WORK/kc-sso.json"
REALM_DIR="$WORK/realm"
EVIDENCE="${FVOCI_KC_E2E_EVIDENCE_DIR:-}"
if [[ -n "$EVIDENCE" ]]; then
  mkdir -p "$EVIDENCE"
fi

# Per-run secrets. They live in this shell (not exported), the Keycloak
# container, the rendered realm copy and the mode-600 spec config under $WORK,
# which is removed on exit. The server receives only the client secret.
# Children get them as environment assignments, never as arguments (argv is
# readable by every local user).
ADMIN_PASSWORD="$(openssl rand -hex 24)"
CLIENT_SECRET="$(openssl rand -hex 32)"
WRONG_SECRET="$(openssl rand -hex 32)"
declare -A USER_PASSWORD=()
for user in ALICE BOB CAROL MALLORY ERIN TINA SSO_A SSO_B; do
  USER_PASSWORD[$user]="$(openssl rand -hex 16)"
done
SSO_A_SECRET="$(openssl rand -hex 32)"
SSO_B_SECRET="$(openssl rand -hex 32)"
# Passwords of the FVOCI accounts the spec creates.
FVOCI_OWNER_PASSWORD="$(openssl rand -hex 16)"
FVOCI_MEMBER_PASSWORD="$(openssl rand -hex 16)"

with_secrets() {
  KC_BOOTSTRAP_ADMIN_PASSWORD="$ADMIN_PASSWORD" KC_E2E_CLIENT_SECRET="$CLIENT_SECRET" \
    KC_E2E_WRONG_SECRET="$WRONG_SECRET" \
    KC_E2E_PASSWORD_ALICE="${USER_PASSWORD[ALICE]}" KC_E2E_PASSWORD_BOB="${USER_PASSWORD[BOB]}" \
    KC_E2E_PASSWORD_CAROL="${USER_PASSWORD[CAROL]}" KC_E2E_PASSWORD_MALLORY="${USER_PASSWORD[MALLORY]}" \
    KC_E2E_PASSWORD_ERIN="${USER_PASSWORD[ERIN]}" KC_E2E_PASSWORD_TINA="${USER_PASSWORD[TINA]}" \
    KC_E2E_SSO_A_CLIENT_SECRET="$SSO_A_SECRET" KC_E2E_SSO_A_PASSWORD="${USER_PASSWORD[SSO_A]}" \
    KC_E2E_SSO_B_CLIENT_SECRET="$SSO_B_SECRET" KC_E2E_SSO_B_PASSWORD="${USER_PASSWORD[SSO_B]}" \
    KC_E2E_FVOCI_OWNER_PASSWORD="$FVOCI_OWNER_PASSWORD" KC_E2E_FVOCI_MEMBER_PASSWORD="$FVOCI_MEMBER_PASSWORD" "$@"
}

compose() {
  KC_BOOTSTRAP_ADMIN_PASSWORD="$ADMIN_PASSWORD" FVOCI_KC_REALM_DIR="$REALM_DIR" \
    docker compose -p "$PROJECT" -f "$COMPOSE_FILE" "$@"
}

compose_detached() {
  KC_BOOTSTRAP_ADMIN_PASSWORD="$ADMIN_PASSWORD" FVOCI_KC_REALM_DIR="$REALM_DIR" \
    setsid -w docker compose -p "$PROJECT" -f "$COMPOSE_FILE" "$@"
}

redact() {
  kc redact "$CONFIG"
}

# Readers of a group's output. They ignore INT, TERM and HUP (Bun keeps an
# inherited ignore), so after Ctrl-C they read on until the group's own
# cleanup (retained artifacts, run directory) has finished writing.
redact_stream() {
  (trap '' INT TERM HUP; exec "${KC[@]}" redact "$CONFIG")
}
log_stream() {
  (trap '' INT TERM HUP; exec tee -a "$@")
}

# A group that ends by HUP, INT or TERM stops the run.
stop_if_interrupted() {
  if (( $1 == 129 || $1 == 130 || $1 == 143 )); then
    echo "group $2 was interrupted (exit $1); stopping" >&2
    exit "$1"
  fi
}

keep() {
  # keep <name>: stdin, redacted, into the evidence directory (if any).
  if [[ -n "$EVIDENCE" ]]; then
    redact >"$EVIDENCE/$1"
  else
    cat >/dev/null
  fi
}

# A failing group leaves a copy of its Playwright output (error-context.md
# with the page's URLs and text) and server log in
# $GROUP_TMP/fvoci-collab-e2e-fail.* (web-e2e-run-group.sh), named in the
# group's GITHUB_OUTPUT file. Keep a redacted copy as evidence, then remove the
# raw one (the rest of $GROUP_TMP goes with $WORK).
collect_failure_artifacts() {
  local out retained real base mode dest file
  base="$(realpath -e -- "$GROUP_TMP")"
  for out in "$WORK"/group-*.out; do
    [[ -f "$out" ]] || continue
    mode="$(basename "$out" .out)"
    mode="${mode#group-}"
    while IFS= read -r retained; do
      real="$(realpath -e -- "$retained" 2>/dev/null)" || continue
      if [[ "$(dirname "$real")" != "$base" || "$(basename "$real")" != fvoci-collab-e2e-fail.* ]]; then
        echo "warning: not removing unexpected failure-artifacts path ${retained}" >&2
        continue
      fi
      if [[ -n "$EVIDENCE" ]]; then
        dest="$EVIDENCE/failure-${mode}"
        mkdir -p "$dest"
        while IFS= read -r -d '' file; do
          redact <"$file" >"$dest/$(realpath --relative-to="$real" "$file" | tr '/' '_')"
        done < <(find "$real" -type f \( -name '*.md' -o -name '*.log' -o -name '*.txt' \) -print0)
        echo "failure artifacts of group ${mode}: redacted copy in ${dest}" >&2
      fi
      rm -rf -- "$real"
      echo "failure artifacts of group ${mode}: raw copy ${real} removed" >&2
    done < <(sed -n 's/^failure-artifacts=//p' "$out")
  done
}

# Docker commands of the cleanup run in their own session: a further Ctrl-C
# to the terminal's process group does not reach them (the docker CLI
# installs its own SIGINT handler even when the signal is ignored).
detached() {
  setsid -w "$@"
}

# Runs as the EXIT trap, which captures the status and ignores INT, TERM,
# HUP and PIPE before calling it: a further signal, or a stderr reader that
# is gone (`... 2>&1 | tee log` after Ctrl-C), must not cut it short. The run
# directory holds the rendered realms and the spec config (per-run
# secrets). The helpers below inherit the ignore.
cleanup() {
  local status=$EXIT_STATUS
  set +e
  collect_failure_artifacts
  if [[ -n "$EVIDENCE" && -f "$CONFIG" ]]; then
    compose_detached logs --no-color keycloak 2>/dev/null | keep keycloak-container.log
  fi
  compose_detached down -v --remove-orphans >/dev/null 2>&1
  local left
  left="$(
    detached docker ps -aq --filter "label=com.docker.compose.project=${PROJECT}"
    detached docker volume ls -q --filter "label=com.docker.compose.project=${PROJECT}"
    detached docker network ls -q --filter "label=com.docker.compose.project=${PROJECT}"
  )"
  if [[ -n "$left" ]]; then
    echo "warning: compose project ${PROJECT} left resources behind" >&2
    status=1
  else
    echo "cleanup: compose project ${PROJECT} removed (containers, volumes, network)" >&2
  fi
  rm -rf "$WORK"
  exit "$status"
}
trap 'EXIT_STATUS=$?; trap "" INT TERM HUP PIPE; cleanup' EXIT
# Each ignores further signals before exiting. PIPE: stderr's reader is gone.
trap 'trap "" INT TERM HUP PIPE; exit 130' INT
trap 'trap "" INT TERM HUP PIPE; exit 143' TERM
trap 'trap "" INT TERM HUP PIPE; exit 129' HUP
trap 'trap "" INT TERM HUP PIPE; exit 141' PIPE

echo "=== keycloak: compose project ${PROJECT} ===" >&2
with_secrets kc render "$KC_DIR/realm.template.json" "$REALM_DIR/fvoci-e2e-realm.json"
if [[ "$WORKSPACE_SSO" == 1 ]]; then
  with_secrets kc render-sso "$KC_DIR/workspace-sso-realm.template.json" "$REALM_DIR"
fi
KC_IMAGE="$(compose config --images)"
compose up -d --quiet-pull >&2
HOST_PORT="$(compose port keycloak 8080)"
if [[ "$HOST_PORT" != 127.0.0.1:* ]]; then
  echo "keycloak is not published on 127.0.0.1: ${HOST_PORT}" >&2
  exit 1
fi
KEYCLOAK_ORIGIN="http://${HOST_PORT}"
ISSUER="${KEYCLOAK_ORIGIN}/realms/${REALM}"
with_secrets kc config "$ISSUER" "$CONFIG"
READY_ISSUERS=("$ISSUER")
if [[ "$WORKSPACE_SSO" == 1 ]]; then
  with_secrets kc sso-config "$KEYCLOAK_ORIGIN" "$SSO_CONFIG"
  READY_ISSUERS+=("${KEYCLOAK_ORIGIN}/realms/fvoci-e2e-ws-a" "${KEYCLOAK_ORIGIN}/realms/fvoci-e2e-ws-b")
fi

# Ready means: each realm's discovery answers 200 with exactly its issuer
# and its JWKS has a signing key (polled from this host, bounded).
started=$SECONDS
last_reason=""
until last_reason="$(kc ready "${READY_ISSUERS[@]}" 2>&1)"; do
  if [[ -z "$(compose ps --status running --quiet keycloak)" ]]; then
    echo "keycloak exited before it was ready" >&2
    compose logs --no-color --tail 40 keycloak 2>&1 | redact >&2
    exit 1
  fi
  if (( SECONDS - started >= READY_LIMIT_S )); then
    echo "keycloak not ready after ${READY_LIMIT_S}s: ${last_reason}" >&2
    exit 1
  fi
  sleep 1
done
echo "keycloak ready after $((SECONDS - started))s: ${READY_ISSUERS[*]}" >&2

VERIFY_ARGS=("$CONFIG")
if [[ "$WORKSPACE_SSO" == 1 ]]; then
  VERIFY_ARGS+=("$SSO_CONFIG")
fi
kc verify "${VERIFY_ARGS[@]}" >"$WORK/keycloak-setup.json"
keep keycloak-setup.json <"$WORK/keycloak-setup.json"

KC_REPO_DIGESTS="$(docker image inspect --format '{{json .RepoDigests}}' "$KC_IMAGE")"
kc versions --evidence="$EVIDENCE" --playwright-files="$PLAYWRIGHT_FILES" --source-sha="$SOURCE_SHA" \
  --root="$ROOT" --target-dir="$CARGO_TARGET_DIR" --keycloak-image="$KC_IMAGE" \
  --repo-digests="$KC_REPO_DIGESTS" --compose-project="$PROJECT" --published="$HOST_PORT" \
  --issuer="$ISSUER"

declare -A GROUP_STATUS=()
for mode in "${MODES[@]}"; do
  secret="$CLIENT_SECRET"
  if [[ "$mode" == wrong-secret ]]; then
    secret="$WRONG_SECRET"
  fi
  echo "=== group ${mode}: release server, OIDC_GENERIC_ISSUER=${ISSUER} ===" >&2
  status=0
  TMPDIR="$GROUP_TMP" FVOCI_E2E_PROFILE=release GITHUB_OUTPUT="$WORK/group-${mode}.out" \
    PLAYWRIGHT_NO_COPY_PROMPT=1 \
    OIDC_GENERIC_ISSUER="$ISSUER" OIDC_GENERIC_CLIENT_ID="$CLIENT_ID" \
    OIDC_GENERIC_CLIENT_SECRET="$secret" OIDC_GENERIC_LABEL="$LABEL" OIDC_ALLOW_INSECURE=1 \
    FVOCI_KC_E2E_CONFIG="$CONFIG" FVOCI_KC_E2E_MODE="$mode" FVOCI_KC_E2E_OUT="$WORK/out" \
    bash "$ROOT/scripts/web-e2e-run-group.sh" e2e-keycloak/oidc-keycloak-flow.spec.ts \
      --config=e2e-keycloak/keycloak.config.ts </dev/null 2>&1 |
    redact_stream | log_stream "$WORK/run.log" || status=$?
  GROUP_STATUS[$mode]=$status
  stop_if_interrupted "$status" "$mode"
done

if [[ "$WORKSPACE_SSO" == 1 ]]; then
  echo "=== workspace SSO: ignored Rust test, test entitlement environment ===" >&2
  status=0
  (cd "$ROOT" && TMPDIR="$GROUP_TMP" FVOCI_KC_SSO_E2E_CONFIG="$SSO_CONFIG" \
    bash "$ROOT/scripts/start-test-postgres.sh" \
    "${SSO_TEST[@]}" keycloak_workspace_sso_with_a_test_entitlement -- --ignored --nocapture) \
    </dev/null 2>&1 |
    redact_stream | log_stream "$WORK/run.log" "$WORK/out/workspace-sso.log" || status=$?
  GROUP_STATUS[workspace-sso]=$status
  stop_if_interrupted "$status" workspace-sso
  MODES+=(workspace-sso)
fi

EVENT_REALMS=("$REALM")
if [[ "$WORKSPACE_SSO" == 1 ]]; then
  EVENT_REALMS+=(fvoci-e2e-ws-a fvoci-e2e-ws-b)
fi
kc events "$CONFIG" "${EVENT_REALMS[@]}" | keep keycloak-events.json
if [[ -n "$EVIDENCE" ]]; then
  redact <"$WORK/run.log" >"$EVIDENCE/run.log"
  for file in "$WORK"/out/*; do
    if [[ -f "$file" ]]; then
      redact <"$file" >"$EVIDENCE/$(basename "$file")"
    fi
  done
fi

echo "=== summary (Keycloak ${KC_IMAGE}, issuer ${ISSUER}) ===" >&2
failed=0
for mode in "${MODES[@]}"; do
  summary="$(kc summary "$WORK/out/playwright-${mode}.json" "$WORK/out/${mode}.log")"
  echo "group ${mode}: exit ${GROUP_STATUS[$mode]} (${summary})" >&2
  if [[ "${GROUP_STATUS[$mode]}" != 0 ]]; then
    failed=1
  fi
done
exit "$failed"
