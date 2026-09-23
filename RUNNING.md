# Running the Rust slice

This slice initializes a new PostgreSQL database. Upgrading or importing an existing FVOCI installation is not supported yet.

## Toolchain

Install Rust 1.98.1 (see `rust-toolchain.toml`) or point `CARGO_HOME`, `RUSTUP_HOME`, and `PATH` at your toolchain.

## Environment

| Variable | Purpose |
| --- | --- |
| `DATABASE_URL` | Migration owner connection (superuser or schema owner). Used only by `fvoci-migrate` and startup migration — **never** for request handling. |
| `DATABASE_APP_URL` | Application DML role (non-superuser, no `BYPASSRLS`). Required; must differ from `DATABASE_URL` in URL and role name. |
| `PASSWORD_PEPPER_KEYS` | JSON map of pepper key id → 64-char hex. |
| `PASSWORD_PEPPER_ACTIVE_KEY_ID` | Active pepper id. |
| `FVOCI_BIND` | Listen address (default `127.0.0.1:0`). |
| `FVOCI_PUBLIC_ORIGIN` | Expected browser `Origin` for mutating routes (default `http://localhost:5173`). Trailing slashes are normalized. |
| `FVOCI_COOKIE_SECURE` | `true`/`1` to set `Secure` on session cookies; defaults from `FVOCI_PUBLIC_ORIGIN` scheme. |
| `FVOCI_BRANDING_NAME` | Setup status branding (default `FVOCI`). |

Remote PostgreSQL with TLS: use `sslmode=require` (or stricter) in both URLs. The crate uses SQLx `runtime-tokio-rustls`.

## Provision app role (after migrate)

Create the dedicated app LOGIN role first, then run migrations, then apply grants:

```sh
export DATABASE_URL='postgres://owner@host:5432/fvoci?sslmode=require'
psql "$DATABASE_URL" -v ON_ERROR_STOP=1 -c "CREATE ROLE fvoci_app_prod LOGIN NOSUPERUSER NOBYPASSRLS"
psql "$DATABASE_URL" -c '\password fvoci_app_prod'
cargo run --bin fvoci-migrate
psql "$DATABASE_URL" -v app_role=fvoci_app_prod -f scripts/grant-app-role.sql
export DATABASE_APP_URL='postgres://fvoci_app_prod:***@host:5432/fvoci?sslmode=require'
```

Never grant the app role before the role exists. Keep database credentials and pepper keys in your secret configuration, outside Git. Retain the same pepper keyring across restarts; replacing it prevents verification of existing passwords.

## Start server

```sh
cargo run --bin fvoci-server
```

Migrations run once at startup via the owner URL; the server connects only through `DATABASE_APP_URL`. The app pool is closed explicitly on shutdown and startup failures.

Rate limits use the direct socket peer. Forwarded headers are ignored; behind a reverse proxy, clients share the proxy's IP bucket. Trusted-proxy configuration and distributed limits are not implemented yet.

## Tests

Pure unit tests:

```sh
cargo test --lib
```

DB integration tests (isolated DB + unique app role per run):

```sh
export TEST_DATABASE_URL='postgres://admin@host:5432/postgres?sslmode=require'
cargo test --features db-tests --test db_integration
```

Optional local PostgreSQL via Docker (loopback only, random password). The helper starts an ephemeral container, waits for readiness, runs the given command with `TEST_DATABASE_URL` set for that command only, then removes the container:

```sh
scripts/start-test-postgres.sh
# or
scripts/start-test-postgres.sh cargo test --features db-tests --test db_integration
```

Integration tests always create and drop their own UUID database and app role; they never reuse or drop an externally supplied database.

## HTTP entry points

| Method | URL | Result |
| --- | --- | --- |
| GET / POST | `/api/v1/setup` | Setup status / first administrator and workspace, session cookie |
| POST | `/api/v1/auth/login` | Password login, `fvoci_session` cookie |
| GET / PATCH | `/api/v1/auth/me` | Current session user / authenticated profile change |
| POST | `/api/v1/auth/logout` | Revoke current session and clear cookie |

PATCH requires `givenName`; `familyName` omitted preserves the value, null or an empty string clears it. Other optional fields are `locale` (`ko`), `timezone`, `weekStartsOn` (0/1), and `textScale` (16/18/20). Unknown fields are rejected. Use the bound address printed at startup; default port 0 is selected by the listening socket.

## Initial workspace operations

Authenticated sessions can list `GET /api/v1/me/workspaces` and read metadata with
`GET /api/v1/workspaces/{id}`. The list contains the current membership role.
An instance administrator can `POST /api/v1/workspaces`; membership authorization
still applies to private workspace access. Authorized owners/admins can rename
with `PATCH /api/v1/workspaces/{id}`. Role updates/removal use
`PATCH`/`DELETE /api/v1/workspaces/{id}/members/{userId}` with the source role caps,
self-change restrictions and owner invariant. `POST /api/v1/me/personal-workspace`
is idempotent; personal workspace metadata/members are immutable.

Migration 003 adds the membership self-selection policy and personal-workspace
constraints. **Re-run `scripts/grant-app-role.sql` after applying migration 003**
to restrict the new helper function's EXECUTE grant to the app role. The tested
upgrade is from this Rust slice's 001/002 schema, not from a TypeScript installation.

Workspace mutations write the body, event and audit in one transaction. Name and
personal workspace events are deliberate additions to the source contract.
RLS isolates tenants; current membership and session authorization are additionally
enforced by product operations. This is not a claim that arbitrary SQL executed
with the app credentials is restricted to an authenticated end user's authority.

Document/task count fields currently return zero because those domains are not
implemented. Counts, quotas, member listing, invitations, exports, deletion and
collaborative editing remain unsupported in this slice.

## Web UI (React)

Generate the OpenAPI contract and TypeScript client from Rust DTOs:

```sh
scripts/generate-api.sh
```

Development (Vite proxy to a running `fvoci-server` API):

```sh
cd apps/web
npm install
API_PROXY_TARGET=http://127.0.0.1:8080 npm run dev
```

Production-style serving from the Rust binary (built assets required):

```sh
cd apps/web && npm install && npm run build
export FVOCI_STATIC_DIR="$PWD/apps/web/dist"
export FVOCI_PUBLIC_ORIGIN=http://127.0.0.1:8080
cargo run --bin fvoci-server
```

Browser end-to-end tests (isolated PostgreSQL, real app role, Playwright Chromium).
Preparation installs dependencies and builds artifacts; the gate assumes preparation
completed and refuses stale binaries:

```sh
scripts/prepare-web-e2e.sh
scripts/run-web-e2e.sh
```

The UI reuses source auth/workspace/settings styling for setup, login, workspace
list, rename, and logout. Magic link, OIDC/MFA/consent, member list, invites,
import/export, and deletion surfaces are shown as unavailable rather than faked.
