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
# Needs docker (compose), openssl, python3 and scripts/prepare-web-e2e.sh.
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
KC_DIR="$ROOT/scripts/keycloak"
COMPOSE_FILE="$KC_DIR/compose.yml"
HELPER="$KC_DIR/kc_e2e.py"
CARGO_TARGET_DIR="${CARGO_TARGET_DIR:-$ROOT/target}"
CARGO_TARGET_DIR="$(python3 -c 'import pathlib,sys; print(pathlib.Path(sys.argv[1]).resolve())' "$CARGO_TARGET_DIR")"
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

for dependency in docker openssl python3 git; do
  command -v "$dependency" >/dev/null 2>&1 || {
    echo "$dependency is required" >&2
    exit 1
  }
done
if [[ ! -x "$ROOT/apps/web/node_modules/.bin/playwright" ]]; then
  echo "missing Playwright install; run scripts/prepare-web-e2e.sh" >&2
  exit 1
fi
# Chromium keeps sockets under TMPDIR; a long path makes it abort.
TMP_BASE="${TMPDIR:-/tmp}"
if (( ${#TMP_BASE} > 60 )); then
  echo "TMPDIR is too long for Chromium's socket paths; use a short TMPDIR" >&2
  exit 1
fi

SOURCE_SHA="$(git -C "$ROOT" rev-parse HEAD)"
if [[ "$SKIP_BUILD" == 0 ]]; then
  echo "=== build: release fvoci-server/fvoci-migrate and web assets at ${SOURCE_SHA} ===" >&2
  (cd "$ROOT" && FVOCI_BUILD_SHA="$SOURCE_SHA" cargo build --locked --offline --release \
    --bin fvoci-server --bin fvoci-migrate)
  (cd "$ROOT/apps/web" && npm run build)
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

RUN_ID="$(openssl rand -hex 6)"
PROJECT="fvoci-kc-e2e-${RUN_ID}"
WORK="$(mktemp -d "${TMPDIR:-/tmp}/fvoci-kc-e2e.XXXXXX")"
chmod 700 "$WORK"
mkdir -p "$WORK/out" "$WORK/realm"
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

with_secrets() {
  KC_BOOTSTRAP_ADMIN_PASSWORD="$ADMIN_PASSWORD" KC_E2E_CLIENT_SECRET="$CLIENT_SECRET" \
    KC_E2E_WRONG_SECRET="$WRONG_SECRET" \
    KC_E2E_PASSWORD_ALICE="${USER_PASSWORD[ALICE]}" KC_E2E_PASSWORD_BOB="${USER_PASSWORD[BOB]}" \
    KC_E2E_PASSWORD_CAROL="${USER_PASSWORD[CAROL]}" KC_E2E_PASSWORD_MALLORY="${USER_PASSWORD[MALLORY]}" \
    KC_E2E_PASSWORD_ERIN="${USER_PASSWORD[ERIN]}" KC_E2E_PASSWORD_TINA="${USER_PASSWORD[TINA]}" \
    KC_E2E_SSO_A_CLIENT_SECRET="$SSO_A_SECRET" KC_E2E_SSO_A_PASSWORD="${USER_PASSWORD[SSO_A]}" \
    KC_E2E_SSO_B_CLIENT_SECRET="$SSO_B_SECRET" KC_E2E_SSO_B_PASSWORD="${USER_PASSWORD[SSO_B]}" "$@"
}

compose() {
  KC_BOOTSTRAP_ADMIN_PASSWORD="$ADMIN_PASSWORD" FVOCI_KC_REALM_DIR="$REALM_DIR" \
    docker compose -p "$PROJECT" -f "$COMPOSE_FILE" "$@"
}

redact() {
  python3 "$HELPER" redact "$CONFIG"
}

keep() {
  # keep <name>: stdin, redacted, into the evidence directory (if any).
  if [[ -n "$EVIDENCE" ]]; then
    redact >"$EVIDENCE/$1"
  else
    cat >/dev/null
  fi
}

cleanup() {
  local status=$?
  set +e
  if [[ -n "$EVIDENCE" && -f "$CONFIG" ]]; then
    compose logs --no-color keycloak 2>/dev/null | keep keycloak-container.log
  fi
  compose down -v --remove-orphans >/dev/null 2>&1
  local left
  left="$(
    docker ps -aq --filter "label=com.docker.compose.project=${PROJECT}"
    docker volume ls -q --filter "label=com.docker.compose.project=${PROJECT}"
    docker network ls -q --filter "label=com.docker.compose.project=${PROJECT}"
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
trap cleanup EXIT
trap 'exit 130' INT
trap 'exit 143' TERM

echo "=== keycloak: compose project ${PROJECT} ===" >&2
with_secrets python3 "$HELPER" render "$KC_DIR/realm.template.json" "$REALM_DIR/fvoci-e2e-realm.json"
if [[ "$WORKSPACE_SSO" == 1 ]]; then
  with_secrets python3 "$HELPER" render-sso "$KC_DIR/workspace-sso-realm.template.json" "$REALM_DIR"
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
with_secrets python3 "$HELPER" config "$ISSUER" "$CONFIG"
READY_ISSUERS=("$ISSUER")
if [[ "$WORKSPACE_SSO" == 1 ]]; then
  with_secrets python3 "$HELPER" sso-config "$KEYCLOAK_ORIGIN" "$SSO_CONFIG"
  READY_ISSUERS+=("${KEYCLOAK_ORIGIN}/realms/fvoci-e2e-ws-a" "${KEYCLOAK_ORIGIN}/realms/fvoci-e2e-ws-b")
fi

# Ready means: each realm's discovery answers 200 with exactly its issuer
# and its JWKS has a signing key (polled from this host, bounded).
started=$SECONDS
last_reason=""
until last_reason="$(python3 "$HELPER" ready "${READY_ISSUERS[@]}" 2>&1)"; do
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

python3 "$HELPER" verify "$CONFIG" >"$WORK/keycloak-setup.json"
keep keycloak-setup.json <"$WORK/keycloak-setup.json"
if grep -rqF -f <(printf '%s\n' "$CLIENT_SECRET") "$ROOT/apps/web/dist"; then
  echo "the client secret is in the web bundle" >&2
  exit 1
fi

KC_REPO_DIGESTS="$(docker image inspect --format '{{json .RepoDigests}}' "$KC_IMAGE")"
python3 - "$EVIDENCE" <<PY
import json, os, platform, subprocess, sys
evidence = sys.argv[1]
def run(*cmd):
    try:
        return subprocess.run(cmd, capture_output=True, text=True, timeout=30).stdout.strip()
    except OSError:
        return None
browsers = json.load(open("$ROOT/apps/web/node_modules/playwright-core/browsers.json"))
# Headless runs use Playwright's chromium-headless-shell build.
shell = next(b for b in browsers["browsers"] if b["name"] == "chromium-headless-shell")
info = {
    "sourceSha": "$SOURCE_SHA",
    "sourceTreeClean": run("git", "-C", "$ROOT", "status", "--porcelain") == "",
    "fvociServer": run("$CARGO_TARGET_DIR/release/fvoci-server", "--version"),
    "fvociServerBinary": "$CARGO_TARGET_DIR/release/fvoci-server (cargo build --release from source)",
    "rustc": run("rustc", "-V"),
    "node": run("node", "-v"),
    "playwright": json.load(open("$ROOT/apps/web/node_modules/@playwright/test/package.json"))["version"],
    "browser": {"name": "chromium-headless-shell", "revision": shell["revision"],
                "version": shell["browserVersion"], "headless": True},
    "keycloakImage": "$KC_IMAGE",
    "keycloakRepoDigests": json.loads('$KC_REPO_DIGESTS'),
    "keycloakMode": "start-dev --import-realm (dev-file database inside the container)",
    "docker": run("docker", "version", "--format", "{{.Server.Version}}"),
    "compose": run("docker", "compose", "version", "--short"),
    "os": platform.platform(),
    "composeProject": "$PROJECT",
    "keycloakPublished": "$HOST_PORT",
    "issuer": "$ISSUER",
}
print(json.dumps(info, indent=2))
if evidence:
    with open(os.path.join(evidence, "versions.json"), "w") as out:
        json.dump(info, out, indent=2)
PY

declare -A GROUP_STATUS=()
for mode in "${MODES[@]}"; do
  secret="$CLIENT_SECRET"
  if [[ "$mode" == wrong-secret ]]; then
    secret="$WRONG_SECRET"
  fi
  echo "=== group ${mode}: release server, OIDC_GENERIC_ISSUER=${ISSUER} ===" >&2
  status=0
  FVOCI_E2E_PROFILE=release \
    OIDC_GENERIC_ISSUER="$ISSUER" OIDC_GENERIC_CLIENT_ID="$CLIENT_ID" \
    OIDC_GENERIC_CLIENT_SECRET="$secret" OIDC_GENERIC_LABEL="$LABEL" OIDC_ALLOW_INSECURE=1 \
    FVOCI_KC_E2E_CONFIG="$CONFIG" FVOCI_KC_E2E_MODE="$mode" FVOCI_KC_E2E_OUT="$WORK/out" \
    bash "$ROOT/scripts/web-e2e-run-group.sh" e2e-keycloak/oidc-keycloak-flow.spec.ts \
      --config=e2e-keycloak/keycloak.config.ts </dev/null 2>&1 |
    redact | tee -a "$WORK/run.log" || status=$?
  GROUP_STATUS[$mode]=$status
done

if [[ "$WORKSPACE_SSO" == 1 ]]; then
  echo "=== workspace SSO: ignored Rust test, test entitlement environment ===" >&2
  status=0
  (cd "$ROOT" && FVOCI_KC_SSO_E2E_CONFIG="$SSO_CONFIG" bash "$ROOT/scripts/start-test-postgres.sh" \
    "${SSO_TEST[@]}" keycloak_workspace_sso_with_a_test_entitlement -- --ignored --nocapture) \
    </dev/null 2>&1 |
    redact | tee -a "$WORK/run.log" "$WORK/out/workspace-sso.log" || status=$?
  GROUP_STATUS[workspace-sso]=$status
  MODES+=(workspace-sso)
fi

EVENT_REALMS=("$REALM")
if [[ "$WORKSPACE_SSO" == 1 ]]; then
  EVENT_REALMS+=(fvoci-e2e-ws-a fvoci-e2e-ws-b)
fi
python3 "$HELPER" events "$CONFIG" "${EVENT_REALMS[@]}" | keep keycloak-events.json
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
  summary="$(python3 - "$WORK/out/playwright-${mode}.json" "$WORK/out/${mode}.log" <<'PY'
import json, sys
try:
    report = json.load(open(sys.argv[1]))
except OSError:
    try:
        results = [l.strip() for l in open(sys.argv[2]) if l.startswith("test result:")]
        print(results[-1] if results else "no report")
    except OSError:
        print("no report")
    sys.exit(0)
stats = report.get("stats", {})
print(f"passed {stats.get('expected', 0)}, failed {stats.get('unexpected', 0)}, "
      f"flaky {stats.get('flaky', 0)}, skipped {stats.get('skipped', 0)}")
PY
)"
  echo "group ${mode}: exit ${GROUP_STATUS[$mode]} (${summary})" >&2
  if [[ "${GROUP_STATUS[$mode]}" != 0 ]]; then
    failed=1
  fi
done
exit "$failed"
