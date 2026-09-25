# Running the Rust slice

This slice initializes a new PostgreSQL database. Upgrading or importing an existing FVOCI installation is not supported yet.

## Toolchain

Install Rust 1.98.1 (see `rust-toolchain.toml`) or point `CARGO_HOME`, `RUSTUP_HOME`, and `PATH` at your toolchain.

## Environment

| Variable | Purpose |
| --- | --- |
| `DATABASE_URL` | Migration owner connection (superuser or schema owner). Used only by `fvoci-migrate` — **never** by `fvoci-server`. |
| `DATABASE_APP_URL` | Application DML role (non-superuser, no `BYPASSRLS`). Required by the server; must use a dedicated app role, not the migration owner. |
| `PASSWORD_PEPPER_KEYS` | JSON map of pepper key id → 64-char hex. |
| `PASSWORD_PEPPER_ACTIVE_KEY_ID` | Active pepper id. |
| `FVOCI_BIND` | Listen address (default `127.0.0.1:0`). |
| `FVOCI_PUBLIC_ORIGIN` | Expected browser `Origin` for mutating routes (default `http://localhost:5173`). Trailing slashes are normalized. An explicit port `0` follows the actual bound port. |
| `FVOCI_COOKIE_SECURE` | `true`/`1` to set `Secure` on session cookies; defaults from `FVOCI_PUBLIC_ORIGIN` scheme. |
| `FVOCI_STATIC_DIR` | Optional built frontend directory containing index.html; validated at startup. |
| `FVOCI_STORAGE_DIR` | Required persistent local attachment directory when `STORAGE_DRIVER=local` (the default). Writable by the server. Reuse the same directory across restarts and preserve it with the database. |
| `STORAGE_LOCAL_PATH` | Source-compatible storage path alias, used only when `FVOCI_STORAGE_DIR` is absent. |
| `STORAGE_DRIVER` | `local` (default) or `s3`. |
| `S3_ENDPOINT` | S3-compatible API origin. Required when `STORAGE_DRIVER=s3`. HTTP(S) only; no credentials, query, or fragment. |
| `S3_REGION` | Bucket region. Required when `STORAGE_DRIVER=s3`. |
| `S3_BUCKET` | Bucket name. Required when `STORAGE_DRIVER=s3`. The server probes with HeadBucket at startup and does not create the bucket. |
| `S3_ACCESS_KEY_ID` / `S3_SECRET_ACCESS_KEY` | Credentials. Required when `STORAGE_DRIVER=s3`. Never logged. |
| `S3_FORCE_PATH_STYLE` | Path-style URLs unless set to `0` (default matches the source: on). |
| `S3_PUBLIC_ENDPOINT` | Optional; validated (HTTP(S), no credentials/query/fragment) but **currently unused**. Only the proxied mode is implemented: parts and downloads go through the API, so no bucket CORS or public S3 endpoint is needed. **Not done** (source parity, tracked separately): presigned direct part PUT, 302 presigned download signed against `S3_PUBLIC_ENDPOINT`, the matching CSP `connect-src` and bucket CORS. |
| `UPLOAD_INCOMPLETE_TTL_HOURS` | Abandoned `uploading`/`assembling` rows older than this are removed by the maintenance scheduler's upload-cleanup job: every open multipart upload for the key is aborted, any object deleted, then the row removed (default 24). A row whose storage cleanup fails is kept for the next run. Must be a positive integer. |
| `FVOCI_UPLOAD_GC_INTERVAL_SECS` | Cadence of that upload-cleanup job (default 600). Each run takes its own cluster-wide advisory claim, examines at most 200 rows, and stops early on shutdown; leftovers wait for the next run. Runs walk the stale rows in global `(created_at, id)` order, each resuming after the previous batch and wrapping at the end, so rows that are skipped or fail every time cannot starve the rest. |
| `FVOCI_UPLOAD_MAX_CONCURRENT_PARTS` | Part PUTs in flight per server process (default 64, positive). Each holds an inbound connection and, with S3, an outbound one while the client streams its body. When no slot is free, a part PUT is refused before its body is read with `503` problem `upload_capacity_exceeded` and `Retry-After: 2`; the web client waits out `Retry-After` (up to 2 minutes per part) without using its transport retries. On every driver a part body must arrive within 95 s plus its length at 64 KiB/s (35 s more than the S3 driver's own deadline), or its slot is released: with the local driver the PUT fails with `400`; with S3 the driver's own deadline fires first and the PUT fails with a logged `500`, which the web client retries. |
| `FVOCI_UPLOAD_MAX_CONCURRENT_PARTS_PER_USER` | Share of those slots one user may hold at once (default 6, twice the web client's part parallelism; clamped to the process limit), so one user cannot refuse uploads for everyone else. |
| `FVOCI_UPLOAD_PART_SIZE_BYTES` | Multipart part size; defaults to 32 MiB. Must be positive and no larger than the maximum file size. With `STORAGE_DRIVER=s3` it must be between 5 MiB (S3 minimum part size) and 1 GiB. S3 parts are streamed to `UploadPart` with the request's declared length and are not buffered in server memory: a part PUT must declare `Content-Length` (browsers do for a `Blob` body), a length above the part's maximum is refused with 413 before the body is read, and a body shorter or longer than its declared length fails with 400 and is never stored. |
| `FVOCI_UPLOAD_MAX_FILE_SIZE_BYTES` | Upload size ceiling; defaults to 5120 MiB. This is independent of the native extractor's 20 MiB input ceiling. |
| `FVOCI_UPLOAD_CREATE_RATE_PER_5MIN` | Upload creation rate limit; defaults to 120. Must be positive. |
| `FVOCI_BRANDING_NAME` | Setup status branding (default `FVOCI`). |
| `FVOCI_MEILI_URL` | Meilisearch HTTP origin. Unset disables search (later routes return a problem). |
| `FVOCI_MEILI_KEY` | API key used when `FVOCI_MEILI_KEY_FILE` is unset. Required (with the file form) if the URL is set. Never logged. |
| `FVOCI_MEILI_KEY_FILE` | Path to a file containing the API key (preferred in compose). Takes precedence over `FVOCI_MEILI_KEY`. |
| `FVOCI_MEILI_INDEX` | Index uid (default `fvoci`). Tests may set a per-run uid. |
| `SMTP_HOST`, `SMTP_PORT`, `SMTP_FROM` | Outgoing mail (invitations, password reset). All three or none; unset disables mail and invitation links are shown instead. No AUTH (same as the source). STARTTLS is used whenever the relay offers it, with certificate verification against public roots, so an internal relay needs a publicly trusted certificate or must not offer STARTTLS. |
| `ENCRYPTION_KEYS` / `ENCRYPTION_ACTIVE_KEY_ID` | Optional keyring (same JSON-hex format as the pepper) that seals workspace webhook signing secrets at rest (AES-256-GCM, `enc:v2:<kid>:…`, bound to the webhook row). Both or neither. Unset: webhook creation answers `503 integration_unavailable` and pending deliveries fail closed. Rotate by adding a key and switching the active id; keep old keys while any secret sealed with them exists. Back it up with the database. |
| `FVOCI_WEBHOOK_ALLOW_TARGETS` | Comma list of host names / IP addresses that webhook URLs may use despite the outbound rules (default empty). A listed URL host skips the port (80/443) and host-name rules; a listed IP is accepted as a literal or resolved private address. Meant for local receivers (tests, e2e); leave empty in production. |
| `GITHUB_APP_ID`, `GITHUB_APP_PRIVATE_KEY`, `GITHUB_WEBHOOK_SECRET` | Optional GitHub App (all three or none; the PEM may use literal `\n`). Enables `/github/install`, `/api/v1/github/callback`, the signed `/api/v1/github/webhook` endpoint and the `github` outbox consumer that closes/reopens linked issues. |
| `GITHUB_API_URL` | GitHub REST base (default `https://api.github.com`). Tests point it at a local fake. |
| `FVOCI_AI_ENABLED`, `FVOCI_AI_SECRET` | Document AI actions (summarize / generate-tasks / suggest-links) when `FVOCI_AI_ENABLED=1` and the secret is set (source gate). They are local text heuristics over the document markdown (no external model) and also need `FVOCI_DOCUMENT_CONVERT_BIN`. Otherwise members get `503 ai_unavailable`. |

Remote PostgreSQL with TLS: use `sslmode=require` (or stricter) in both URLs. The crate uses SQLx `runtime-tokio-rustls`.

## Migrate and grant before server

Run migrations and app-role grants **before** starting or upgrading `fvoci-server`. Stop old
instances first: old binaries refuse a newer `fvoci.schema_migrations` version and cannot restart
after migrate. Upgrade order is stop old → `fvoci-migrate` → `fvoci-migrate --grant-app-role` →
start new; mixed-version rolling restart is not supported. The server connects only through
`DATABASE_APP_URL`, requires the applied version to equal the compiled set, and exits nonzero
with an operator message if the schema is missing, behind, newer than this binary, or unreadable.
A newer database needs a matching or newer `fvoci-server`; do not run migrate from the old
binary. The gate does not detect stale grants after a later migration; re-run `--grant-app-role`
after every upgrade that applies new migrations. The server does not run migrations and ignores
`DATABASE_URL` / `FVOCI_MIGRATION_URL` if set.

## Create role, migrate, then grant

Create the dedicated app LOGIN role first, then run migrations, then apply grants:

```sh
export DATABASE_URL='postgres://owner@host:5432/fvoci?sslmode=require'
psql "$DATABASE_URL" -v ON_ERROR_STOP=1 -c "CREATE ROLE fvoci_app_prod LOGIN NOSUPERUSER NOBYPASSRLS"
psql "$DATABASE_URL" -c '\password fvoci_app_prod'
cargo run --bin fvoci-migrate
cargo run --bin fvoci-migrate -- --grant-app-role fvoci_app_prod
export DATABASE_APP_URL='postgres://fvoci_app_prod:***@host:5432/fvoci?sslmode=require'
```

`--grant-app-role` applies `scripts/grant-app-role.sql` as a single transaction
and exits nonzero on any error, leaving the previous privileges unchanged; do not
start the server after a failed grant. It refuses a missing, superuser or
BYPASSRLS role and any role that owns, or inherits ownership of, fvoci objects.
Re-run it after every upgrade that applies new migrations: new SECURITY DEFINER
functions are not executable by PUBLIC, so requests that need them fail until
the grant is re-run. If you must use psql instead, run
`psql -X -v ON_ERROR_STOP=1 --single-transaction -v app_role=<role> -f scripts/grant-app-role.sql`;
without those flags psql commits each statement and can leave a partial grant.
Never grant the app role before the role exists. Keep database credentials and pepper keys in your secret configuration, outside Git. Retain the same pepper keyring across restarts; replacing it prevents verification of existing passwords.

## Start server

After `fvoci-migrate` and `fvoci-migrate --grant-app-role` succeed:

```sh
export DATABASE_APP_URL='postgres://fvoci_app_prod:***@host:5432/fvoci?sslmode=require'
export FVOCI_STORAGE_DIR='/path/to/persistent/fvoci-storage'
cargo run --bin fvoci-server
```

S3-compatible storage (MinIO/silo or AWS). Credentials come only from the environment; `FVOCI_STORAGE_DIR` is not required:

```sh
export STORAGE_DRIVER=s3
export S3_ENDPOINT='http://127.0.0.1:9000'
export S3_REGION=us-east-1
export S3_BUCKET=fvoci
export S3_ACCESS_KEY_ID='...'
export S3_SECRET_ACCESS_KEY='...'
export S3_FORCE_PATH_STYLE=1
cargo run --bin fvoci-server
```

The only database URL the server process requires is `DATABASE_APP_URL`.

The current durability implementation requires the server account to read/search every ancestor of the storage directory up to `/`, as well as write within it, because those directory entries are synchronized. Validate permissions for the actual service account before deployment.

Local attachment storage must be on persistent storage. A new empty directory
does not restore the files referenced by an existing database.

With `STORAGE_DRIVER=s3`, the S3 client sends signed requests only to
`S3_ENDPOINT`: it follows no redirects and ignores `HTTP(S)_PROXY`, and uses
TCP keepalive. A streamed part PUT has a deadline of 60 s plus its length at
64 KiB/s (32 MiB part: 572 s), which bounds both an endpoint that stops reading
and a client that trickles its body, and waits at most 30 s for response
headers once the body has been sent. A client that disconnects mid-body gets a
400 and nothing is stored. A bodiless `404` from DeleteObject counts as
already deleted only if HeadBucket succeeds. Ranged
downloads require a `206` whose `Content-Range` matches the request. A part PUT
whose session is revoked while its body streams is answered 4xx after the bytes
already reached the multipart upload (as with the source's presigned PUTs):
S3 may list that part, but only the same uploader with a live session can
complete the upload, so it is never published by the revoked session. The
local driver stages the part and discards it instead. Uploads retain
their original bytes separately from derived extraction results; native
extraction job integration and search indexing are not yet accepted.

Invalid upload-limit values fail startup instead of silently selecting defaults.
After applying migration 006 to an existing Rust slice database, re-run
`fvoci-migrate --grant-app-role` for the same application role before serving requests.
This does not provide an importer for the original TypeScript installation.

The app pool is closed explicitly on shutdown and before exiting on startup gate failures.

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

S3 driver + abandoned-upload GC against a pinned MinIO-compatible silo (loopback, random keys, port 0). Nest PostgreSQL when the suite needs the app database:

```sh
scripts/start-test-minio.sh scripts/start-test-postgres.sh cargo test --locked --offline --no-fail-fast --features db-tests --test attachment_s3_integration
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
constraints. **Re-run `fvoci-migrate --grant-app-role` after applying migration 003**
to restrict the new helper function's EXECUTE grant to the app role. The tested
upgrade is from this Rust slice's 001/002 schema, not from a TypeScript installation.

Workspace mutations write the body, event and audit in one transaction. Name and
personal workspace events are deliberate additions to the source contract.
RLS isolates tenants; current membership and session authorization are additionally
enforced by product operations. This is not a claim that arbitrary SQL executed
with the app credentials is restricted to an authenticated end user's authority.

Document/task count fields currently return zero because those domains are not
implemented. Counts, quotas, member listing, invitations, exports, and deletion
remain unsupported in this slice.

## Collaboration (`/collab`)

Collaboration is **opt-in**. The HTTP server exposes `GET /collab` (426 without
WebSocket upgrade) and upgrades to Hocuspocus 4.6.0 only when
`FVOCI_COLLAB_ENGINE` points at a built `collab-engine` helper binary. Without
that variable the route returns 503 `collab_unavailable`.

Build the helper (separate crate graph; parent depends on `collab-engine` with
`default-features = false` and talks to the child through framed JSON only):

```sh
cd crates/collab-engine
cargo build --bin collab-engine --features worker
export FVOCI_COLLAB_ENGINE="$PWD/target/debug/collab-engine"
```

Optional tuning:

| Variable | Default | Notes |
| --- | --- | --- |
| `FVOCI_COLLAB_MAX_ROOMS` | 30 (clamp 1–512) | Hub room slots; immediate refusal when full. Default fits stock PostgreSQL `max_connections=100`; the 64-room capacity probe sets `64` and needs a higher Postgres limit. |
| `FVOCI_COLLAB_MAX_CHILDREN` | primary + validator headroom | Bounds the validator helper pool only. Primary cap is `max_rooms + 4` for offline revision capture headroom. |
| `FVOCI_COLLAB_MEMORY_BUDGET` | 2 GiB | Aggregate admission: sum live helper VmRSS plus `max(16 MiB, 14× persisted bytes)` per room start |
| `FVOCI_COLLAB_MAX_CONNECTIONS` | 32 | Per-room WebSocket members |
| `FVOCI_COLLAB_IDLE_MS` | 30000 | Idle room eviction |
| `FVOCI_COLLAB_REVOKE_POLL_MS` | 5000 | ACL revoke poll |

Capacity refusals close WebSocket clients with **1013** “try again later” (retryable).
Per-child limits stay unchanged (AS 1 GiB, observed RSS kill 512 MiB, 8 s wall, 256-op recycle).
Helpers try to set `oom_score_adj=1000` so cgroup OOM prefers a helper over `fvoci-server`; where the container profile denies it (e.g. AppArmor docker-default), the helper still starts without it.
The server raises soft `RLIMIT_NOFILE` to the hard limit at startup.

Heavy load probe (not in default CI; Linux + PostgreSQL via `scripts/start-test-postgres.sh`):

```sh
# Merge bar: 64 rooms × 2 distinct-user peers, 180s (~1 edit/s/room), release helper
./scripts/collab-capacity-probe.sh

# Shorter local smoke (still records PROBE_SUMMARY lines; duration floor is 180s):
COLLAB_PROBE_ROOMS=5 ./scripts/collab-capacity-probe.sh
```

Environment: `COLLAB_PROBE_ROOMS`, `COLLAB_PROBE_PEERS`, `COLLAB_PROBE_DURATION_SECS` (minimum 180),
`COLLAB_PROBE_OPEN_CONCURRENCY`, `FVOCI_COLLAB_MAX_ROOMS`, `FVOCI_COLLAB_ENGINE`,
`RUST_LOG` (default `collab.stage=info` for per-stage breakdown),
`FVOCI_TEST_PG_MAX_CONNECTIONS` (probe script only; default 150 — each live room holds one PG
connection via `RoomGuard`, so docker Postgres must exceed room count plus app pool and reserve).
`scripts/start-test-postgres.sh` defaults to `max_connections=150` when unset; ordinary DB tests
that do not set `FVOCI_TEST_PG_MAX_CONNECTIONS` inherit that value.

PostgreSQL coupling at server startup: with collab enabled, `FVOCI_COLLAB_MAX_ROOMS` must fit
`max_connections` together with the app pool (`max(16, max_rooms)`) and a 10-connection reserve;
the server refuses to start when the sum exceeds `SHOW max_connections`. The default 30 rooms need
70 connections (30 + 30 + 10). The verified 64-room probe needs 138 (`64 + 64 + 10`); compose sets
Postgres `max_connections=150`.

The probe checks achieved rate ≥ 0.95 edits/s/room, zero writer loss, zero **1011** closes under
load, room-capacity **1013**, slot reuse after idle eviction, exact hostile 5/5 isolation with
victim recovery, and prints `PROBE_SUMMARY` plus `collab.stage` tracing lines for validate /
auth_tx / append_tx / apply / broadcast timings. Full logs are saved under
`${FVOCI_EVIDENCE_DIR:-target/collab-probe-logs}/collab-capacity-probe-<timestamp>.log`
(set `FVOCI_EVIDENCE_DIR` for a custom directory).

`FVOCI_SHUTDOWN_DEADLINE_MS` sets the whole server shutdown deadline (default
30000, positive milliseconds). SIGTERM/Ctrl+C stops collaboration admission
before HTTP draining; independent rooms drain concurrently. Normal shutdown
joins room helpers and releases their database guards before closing the pool.
An observed shutdown failure or deadline expiry exits nonzero. Expiry is not a
successful flush or proof that an in-flight transaction rolled back; recovery
uses the durable CRDT state and operation receipts. Actor panic/rejoin handling
is still under acceptance review, so collaboration remains opt-in.

Product tests require `TEST_DATABASE_URL`, the helper path above, and run as:

```sh
export TEST_DATABASE_URL='postgres://admin@host:5432/postgres?sslmode=require'
export FVOCI_COLLAB_ENGINE=/path/to/collab-engine
cargo test --features db-tests --test collab_product
```

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
(cd apps/web && npm ci && npm run build)
export FVOCI_STATIC_DIR="$PWD/apps/web/dist"
export FVOCI_PUBLIC_ORIGIN=http://127.0.0.1:8080
export FVOCI_BIND=127.0.0.1:8080
cargo run --bin fvoci-server
```

Browser end-to-end tests (isolated PostgreSQL, real app role, Playwright Chromium).
Preparation installs dependencies and builds artifacts; the gate assumes preparation
completed and rebuilds current contracts, assets and binaries offline:

```sh
scripts/prepare-web-e2e.sh
scripts/run-web-e2e.sh
```

The UI reuses source auth/workspace/settings styling for setup, login, workspace
list, rename, and logout. Magic link, OIDC/MFA/consent, member list, invites,
import/export, and deletion surfaces are shown as unavailable rather than faked.


문서 추출 클라이언트는 `crates/document-extract-client`에서 parser 의존성 없이
빌드·검사한다 (`cargo test --locked --offline --all-targets`). 실제 추출 실행은
별도 `document-extract` native helper가 필요하며 클라이언트만으로 지원 완료가 아니다.
협업 Live actor의 비정상 완료를 복구했더라도 해당 hub의 수명 동안 실패 기록이
유지되어 이후 정상 종료 요청의 프로세스 exit가 non-zero가 될 수 있다.

## Native attachment text extraction

After applying migration 007, rerun `fvoci-migrate --grant-app-role` with the existing
app-role procedure. The no-argument claim function has a fixed search path and
PUBLIC execution revoked. Its migration owner needs table-owner access; the
runtime role remains non-superuser without BYPASSRLS.

Build the production helper separately from the server:

```sh
(cd crates/document-extract && bash fetch-rhwp.sh && cargo fetch --locked)
cargo fetch --locked
cargo build --manifest-path crates/document-extract/Cargo.toml --locked --offline --bin document-extract
cargo build --locked --offline --bin fvoci-server --bin fvoci-migrate
export FVOCI_EXTRACTOR_BIN="$PWD/crates/document-extract/target/debug/document-extract"
```

Use the default helper build for deployment; `test-hang`, `extract-native-tests`
and `extract-job-driver` are test-only. The helper must be installed alongside
the server at the configured executable path. No Node or browser process is used
for server-side extraction.

## Container install

The install artifact is a multi-stage Docker image plus a small Compose stack under
`infra/rust/`. It builds release `fvoci-server`, `fvoci-migrate`, the production
`collab-engine` helper (`--features worker`), the production `document-extract`
helper (same rhwp pin as `scripts/prepare-extract-helper.sh` / `rust.yml`, without
`test-hang`), and the `apps/web` production bundle (same steps as
`scripts/prepare-web-e2e.sh` + `npm run build`). Runtime images pin base digests,
run as uid/gid `1000` (`fvoci`), and set:

| Variable | Installed path / note |
| --- | --- |
| `FVOCI_STATIC_DIR` | `/opt/fvoci/static` |
| `FVOCI_COLLAB_ENGINE` | `/opt/fvoci/bin/collab-engine` |
| `FVOCI_EXTRACTOR_BIN` | `/opt/fvoci/bin/document-extract` |
| `FVOCI_STORAGE_DIR` | `/data/storage` (Compose volume, owned by `fvoci`) |

Unset any helper env to disable that feature (API-only). The published image ships
all three helpers and enables them via the defaults above.

### Bootstrap

1. Copy `infra/rust/.env.example` to `infra/rust/.env` and replace placeholders.
   Keep `POSTGRES_*` as the migration owner credentials. Create the dedicated app
   role only through the init path below — never grant superuser or `BYPASSRLS` to
   the app role.
2. Build locally (no registry push required):

```sh
docker build -f infra/rust/Dockerfile -t fvoci-rust-install:local .
```

3. Start PostgreSQL, the one-shot init job, then the server:

```sh
docker compose -f infra/rust/compose.yml --env-file infra/rust/.env up -d --wait server
```

Optional S3-compatible storage (pinned silo, not published on the host). Set `S3_ACCESS_KEY_ID` / `S3_SECRET_ACCESS_KEY` in `.env` first:

```sh
docker compose -f infra/rust/compose.yml -f infra/rust/compose.s3.yml --env-file infra/rust/.env up -d --wait server
```

The `init` service runs `fvoci-migrate`, creates the non-superuser
`FVOCI_APP_ROLE` if missing, then `fvoci-migrate --grant-app-role <role>` with the
owner `DATABASE_URL`. When `FVOCI_MEILI_URL` is set it also runs
`fvoci-migrate --ensure-meili-key` with the Meilisearch **master** key, writes a
scoped API key (index `fvoci` only) to `/run/fvoci/meili/api_key` (mode 0600,
uid 1000), and ensures index settings. The stack fails if init exits nonzero;
`server` starts only after init succeeds. The server receives only
`DATABASE_APP_URL` and `FVOCI_MEILI_URL` + `FVOCI_MEILI_KEY_FILE`; it never
receives the owner database URL or `MEILI_MASTER_KEY`. Preserve the `storage`,
`pgdata`, `searchdata`, and `meili_key` volumes across restarts.

Search is disabled when `FVOCI_MEILI_URL` is unset. If the URL is set without
`FVOCI_MEILI_KEY` or `FVOCI_MEILI_KEY_FILE`, the server refuses to start. Keys
are never logged.

The server is published on `FVOCI_PUBLISH_ADDR:FVOCI_PUBLISH_PORT` (default
`127.0.0.1`, loopback only). `FVOCI_PUBLIC_ORIGIN` must be the exact origin browsers
use. Beyond local evaluation, terminate TLS in a reverse proxy, set
`FVOCI_PUBLIC_ORIGIN=https://…` and `FVOCI_COOKIE_SECURE=true`.

### Verification

`scripts/install-smoke.sh` builds the image, starts an isolated Compose project
(unique name, ephemeral published port, run-owned volumes), exercises setup/login,
wiki collab body projection, HWPX upload + extraction, `/collab` availability,
a graceful `docker compose stop server` (stopped container must report exit code 0),
a recreated server container on the same volumes, and post-recreate reads.
CI runs the same script on `ubuntu-24.04` and `ubuntu-24.04-arm` via
`.github/workflows/install.yml` (no secrets, no image publish).

## Backup and restore

This is the logical backup for the Compose install above (the source advanced
install path: PostgreSQL + attachment storage). It is not a stopped-stack copy
of every volume, and it is not PITR.

**Included:** a custom-format `pg_dump` of schemas `public` (RLS helper
functions) and `fvoci`, taken as the PostgreSQL owner role through the
`postgres` service, plus a `tar` of the `storage` volume. **Omitted:** Meilisearch (`searchdata`), the scoped API key
volume, Compose env files, pepper keys, and database passwords. The search
index is derived. Restore runs `fvoci-migrate --ensure-meili-key` (new scoped
key, index settings), then `fvoci-migrate --recover-outbox` (rebases outbox
cursors to the new cluster's xids before any server starts) and
`fvoci-migrate --rebuild-search` (reindexes from PostgreSQL, including
attachment text chunks). Keep `PASSWORD_PEPPER_KEYS` / `PASSWORD_PEPPER_ACTIVE_KEY_ID` the same as
the original or existing passwords will not verify. `POSTGRES_USER`,
`POSTGRES_DB`, and `FVOCI_APP_ROLE` names must match; cluster passwords and
`MEILI_MASTER_KEY` may be new. `scripts/restore.sh` compares the keyring fingerprint recorded in the backup manifest and refuses to restore with a different keyring.

**Ordering:** `scripts/backup.sh` stops the server (the only writer) and checks
that no other client sessions remain, then dumps PostgreSQL, then archives
storage. Stored attachment keys in the dump must exist as
`objects/<key>/payload` in the tar, so restored files cover every database
reference. Archives are created with directory mode `0700` and file mode
`0600`. The dump contains whatever the database already stored (including
password hashes); the archive does not add the env file or Meili master key.

Backup a running project (restarts the server afterwards unless
`--leave-stopped`):

```sh
scripts/backup.sh \
  --project fvoci-rust-install \
  --env-file infra/rust/.env \
  --output /srv/fvoci-backups/fvoci-2026-09-25
```

Restore only into a **new** Compose project whose install volumes do not exist.
Do not restore onto the source project. On failure the target is left for
diagnosis; delete only that project with `docker compose -p <name> down -v`.

```sh
scripts/restore.sh \
  --project fvoci-restore-check \
  --env-file /srv/fvoci-restore/.env \
  --input /srv/fvoci-backups/fvoci-2026-09-25
```

Restore starts postgres and Meilisearch on empty volumes, creates the
application role, restores the dump, restores storage, then runs the one-shot
`init` job (`fvoci-migrate`, `--grant-app-role`, `--ensure-meili-key`; all
idempotent on this path), rebases the outbox, rebuilds search, and runs
`fvoci-migrate --verify-storage` with the server's own environment: every
`stored` attachment in the restored database must exist in the configured
storage with its recorded size, or the restore stops before the server starts.
Then it starts the server. Confirm login with the original password, document
body, attachment bytes, extraction text, and tasks.

`scripts/backup-restore-smoke.sh` builds the install image, seeds an isolated
source project (setup/login, wiki collab body, HWPX upload and extraction,
project/task, a document comment), backs it up, deletes that stack
and its volumes, restores into a second project, and checks those artifacts
plus uid `1000` and that the restored server receives only `DATABASE_APP_URL`.
Trap cleanup removes only those two projects. CI runs it as a separate job on
`ubuntu-24.04` and `ubuntu-24.04-arm` in `.github/workflows/install.yml` (no
secrets, no image publish).

### S3 storage backup

`scripts/backup.sh` and `scripts/restore.sh` archive and restore the **local**
storage volume. They do not copy bucket objects, and a volume archive of an S3
install would contain none, so `scripts/backup.sh` refuses a server running
with `STORAGE_DRIVER=s3`. The supported model for S3 is:

1. **Objects:** the operator protects the bucket itself: enable bucket
   versioning (so a deleted or overwritten object can be recovered) and, for
   site loss, replication to a second bucket/region, or the provider's backup
   service. Attachment objects are immutable once `stored`; keys are
   server-generated UUIDs.
2. **Database:** a `pg_dump` of schemas `public` and `fvoci` taken the same way
   as `scripts/backup.sh` does (custom format, owner role, server stopped so the
   dump is quiesced). Objects deleted by workspace purge after the dump are
   recoverable only from bucket versions.
3. **Restore:** restore the dump, point the server at the bucket (or the
   replica), and before starting the server run the storage check with the
   server's environment:

   ```sh
   docker compose -f infra/rust/compose.yml -f infra/rust/compose.s3.yml \
     --project-name <project> --env-file <env> \
     run --rm --no-deps --entrypoint /opt/fvoci/bin/fvoci-migrate server --verify-storage
   ```

   It prints `{"checked":N,"missing":[...],"sizeMismatch":[...]}` and exits
   non-zero when any stored attachment is missing or has a different size, or
   when the bucket cannot be read (credentials, wrong bucket, network). Restore
   the listed objects from bucket versions before starting the server.

A scripted S3-aware backup/restore (dump-only archives, bucket snapshot
orchestration) is not implemented.

When `FVOCI_EXTRACTOR_BIN` is absent, extraction is explicitly disabled and stored
HWP/HWPX attachments remain pending. An invalid configured path fails startup.
`FVOCI_EXTRACT_POLL_SECS` defaults to 30 and must be positive. One job is in flight
per server, with a 1-second interval after work; upload completion does not wake
the poller immediately. Upload success confirms the original file and its DB
metadata, independently of later text extraction success. Original downloads
remain byte-preserving if extraction fails.

The durable queue uses a 300-second token lease and at most 2 crash/I/O attempts.
Missing source files, read failures or failed result commits leave the lease for
expiry; a second exhausted attempt becomes `worker_failure`. Cooperative shutdown
cancels and joins the helper before closing the DB pool and releases its attempt.
A stale token cannot overwrite a newer claim. Extraction only publishes while
the workspace and parent document remain live; deletion and finish share the
workspace→document→attachment lock order. Future deletion paths must preserve it.

The native boundary limits input to 20 MiB, output to 500k characters, helper time
to 120 seconds and memory to the native component's configured limits. Oversized
originals can remain valid attachments while their extraction ends with
`resource_limit`. Results distinguish `ok`, `empty`, `partial`, `unsupported`,
`corrupt`, `resource_limit` and `worker_failure`; bounded warnings and the pinned
rhwp revision accompany extracted text. Results are stored for subsequent product
consumers. Search indexing and search permission-revocation propagation are not
connected by this slice. Other attachment parents and thumbnails remain out
of this slice's acceptance. S3-compatible storage and abandoned-upload cleanup
are implemented behind `STORAGE_DRIVER=s3` and the local driver; see "S3
storage backup" for what backup/restore covers with S3.

The Native documents CI runs actual PostgreSQL product tests on x64 and ARM64.
Its ordinary helper checks authenticated HWP/HWPX input, then a separately built
test helper exercises cancellation and process recovery. Local preparation and
offline test commands are in `scripts/prepare-extract-helper.sh` and
`scripts/run-extract-tests.sh`; the latter must fail if required DB/helper inputs
are absent. These integration commands are being wired with the pending native
job submission and are not yet a released support claim.
