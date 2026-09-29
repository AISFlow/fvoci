# Running the Rust slice

This slice initializes a new PostgreSQL database. Importing an existing TypeScript FVOCI installation is not supported.
The Rust server has forward schema migrations (see "Migrate and grant before server" and, for the Compose
install, "Upgrade"; the developer smoke in "Upgrade validation" exercises one image pair per run with local
storage or, with `--storage s3`, the documented S3 procedure against a local run-owned bucket, one injected init
failure and old-image rollback; ordinary PR and main CI does not run it, only an optional manual
`workflow_dispatch`). Downgrading a migrated database is not supported.

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
| `METRICS_ALLOW_IPS` | `/metrics` allowlist (source contract): comma-separated IPv4 addresses or CIDRs with a required `/1`–`/32` prefix; IPv4-mapped IPv6 peers compare as IPv4, IPv6 entries are refused. Unset or empty denies every peer (404). Only the direct socket peer counts; `X-Forwarded-For` is ignored. List the scraper's direct address; a reverse proxy must not forward `/metrics` (or must restrict it itself), because listing the proxy's address makes `/metrics` public to everyone the proxy forwards. One malformed entry refuses startup. `/health` and `/ready` are not affected. |
| `FVOCI_PUBLIC_ORIGIN` | Expected browser `Origin` for mutating routes (default `http://localhost:5173`). Trailing slashes are normalized. An explicit port `0` follows the actual bound port. |
| `FVOCI_COOKIE_SECURE` | `true`/`1` to set `Secure` on session cookies; defaults from `FVOCI_PUBLIC_ORIGIN` scheme. |
| `FVOCI_LICENSE_KEY` | Optional secret FVOCI2 enterprise entitlement. Absent, malformed, untrusted, or expired tokens do not block startup: audit, branding, and workspace SSO remain disabled; seats default to 10 and storage/upload limits to unlimited. The server verifies offline using only the public keys compiled into `src/license-trust.json`, which is currently empty, matching the fixed source. No issued token can activate enterprise features until issuer public keys are supplied in a reviewed release build; there is no environment trust-key override. Rotate the token by restarting the server; its validity window is rechecked during use. Keep the token out of logs and backups shared outside the operator boundary. Instance OIDC remains available without an enterprise license. |
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
| `FVOCI_MEILI_KEY_FILE` | Path to a file containing the API key (preferred in compose). Takes precedence over `FVOCI_MEILI_KEY`. The file must be a regular file of at most 4 KiB and is opened without following a symlink: a path that is a symlink (for example a Kubernetes Secret or projected volume entry, which points into `..data/`) is refused at startup. |
| `FVOCI_MEILI_INDEX` | Index uid (default `fvoci`). Tests may set a per-run uid. |
| `SMTP_HOST`, `SMTP_PORT`, `SMTP_FROM` | Outgoing mail (invitations, password reset). All three or none; unset disables mail and invitation links are shown instead. No AUTH (same as the source). STARTTLS is used whenever the relay offers it, with certificate verification against public roots, so an internal relay needs a publicly trusted certificate or must not offer STARTTLS. |
| `ENCRYPTION_KEYS` / `ENCRYPTION_ACTIVE_KEY_ID` | Optional keyring (same JSON-hex format as the pepper) that seals workspace webhook signing secrets and the Web Push VAPID private key at rest (AES-256-GCM, `enc:v2:<kid>:…`, bound to the webhook row). Both or neither. Unset: webhook creation answers `503 integration_unavailable` and pending deliveries fail closed. Rotate by adding a key and switching the active id; keep old keys while any secret sealed with them exists (`fvoci-migrate --secrets-rotate` re-seals them under the active key; `--secrets-audit` and `--verify-secrets` list the key ids in use). Back it up with the database; restore checks it (see Backup and restore). |
| `FVOCI_WEBHOOK_ALLOW_TARGETS` | Comma list of host names / IP addresses that webhook URLs may use despite the outbound rules (default empty). A listed URL host skips the port (80/443) and host-name rules; a listed IP is accepted as a literal or resolved private address. Meant for local receivers (tests, e2e); leave empty in production. `0.0.0.0` / `::` are refused. |
| `GITHUB_APP_ID`, `GITHUB_APP_PRIVATE_KEY`, `GITHUB_WEBHOOK_SECRET` | Optional GitHub App (all three or none; the PEM may use literal `\n`). Enables `/github/install`, `/api/v1/github/callback`, the signed `/api/v1/github/webhook` endpoint and the `github` outbox consumer that closes/reopens linked issues. The install `state` is single use and bound to the admin session that started it (the callback needs that session cookie); the callback confirms the installation with `GET /app/installations/{id}` and never replaces an existing link to another installation (uninstall first). While the app is not configured the `github` cursor still advances, so enabling it later does not replay older status changes. |
| `GITHUB_STATE_SECRET` | Server-only key (at least 32 bytes) for the install `state` MAC. If unset it is derived (HKDF-SHA256) from the active `ENCRYPTION_KEYS` key; with neither, a configured GitHub App fails at boot. The webhook secret is not used because GitHub App managers also hold it. |
| `GITHUB_API_URL` | GitHub REST base (default `https://api.github.com`). Must be `https`; plain `http` only for a loopback host (tests point it at a local fake). |
| `FVOCI_AI_ENABLED`, `FVOCI_AI_SECRET` | Document AI actions (summarize / generate-tasks / suggest-links) when `FVOCI_AI_ENABLED=1` and the secret is set (source gate). They are local text heuristics over the document markdown (no external model; the Markdown comes from the Rust `--internal-markdown` child, not the Node helper). Otherwise members get `503 ai_unavailable`. |
| `FVOCI_AI_EMBEDDINGS_BASE_URL`, `FVOCI_AI_EMBEDDINGS_MODEL`, `FVOCI_AI_EMBEDDINGS_DIM` | Semantic search (source contract). With `FVOCI_AI_ENABLED=1` and a base URL, the server calls an OpenAI-compatible `POST {base}/embeddings` (model default `text-embedding-3-small`, `FVOCI_AI_SECRET` as the optional bearer, never logged). The extract job embeds extracted attachment text chunks (stored in `attachment_text.embedding`, copied to Meili `_vectors.attachments`), so vectors need `FVOCI_EXTRACTOR_BIN`; older chunks and failed calls are backfilled with backoff. Workspace search with `mode=hybrid` (the web command palette) RRF-merges Meili lexical hits with the nearest chunks; global search and any embedder failure answer lexically. `FVOCI_AI_EMBEDDINGS_DIM` must be `1536` (startup refuses anything else). A bad base URL refuses startup. |
| `FVOCI_AI_EMBEDDINGS_ALLOW_PRIVATE` | `1` lets the embeddings URL resolve to a private or loopback address (a local model server) and only then use plain `http`; public hosts must use `https`. Link-local/metadata addresses are always refused, redirects are never followed and each call is pinned to the checked address. Default `0` (FVOCI hardening; the source has no such rule). |

Remote PostgreSQL with TLS: use `sslmode=require` (or stricter) in both URLs. The crate uses SQLx `runtime-tokio-rustls`.

## Migrate and grant before server

This section and the next two are for running the binaries yourself (a source
checkout, or your own orchestration). The user Compose install
(`compose.user.yml`, "Container install") does the same steps on every start of
its `fvoci` container, before the server starts, and needs none of the commands
below; the developer Compose stack runs them in its `init` service.

Run migrations and app-role grants **before** starting or upgrading `fvoci-server`. Stop old
instances first: old binaries refuse a newer `fvoci.schema_migrations` version and cannot restart
after migrate. Upgrade order is stop old → `fvoci-migrate` → `fvoci-migrate --grant-app-role` →
start new; mixed-version rolling restart is not supported. The server connects only through
`DATABASE_APP_URL`, requires the applied version to equal the compiled set, and exits nonzero
with an operator message if the schema is missing, behind, newer than this binary, or unreadable.
A newer database needs a matching or newer `fvoci-server`; do not run migrate from the old
binary. The gate does not detect stale grants after a later migration; re-run `--grant-app-role`
after every upgrade that applies new migrations. `fvoci-server` itself does not run migrations and
ignores `DATABASE_URL` / `FVOCI_MIGRATION_URL` if set; in the user install the image entrypoint
(`fvoci-migrate --start`) migrates and grants before it starts the server.

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

Behind a reverse proxy or CDN (for example Cloudflare), every attachment part
is its own HTTP request of at most `FVOCI_UPLOAD_PART_SIZE_BYTES` (default
32 MiB) plus headers; the file size itself does not reach the proxy as one
request. Keep the part size below the request-body cap the deployment actually
enforces, including a cap an operator lowered on the zone or proxy: do not
assume a universal 100 MB. Cloudflare's current plan table lists 100 MB for
Free/Pro, 200 MB for Business and up to 5 GB for Enterprise, adjustable per zone
([413](https://developers.cloudflare.com/support/troubleshooting/http-status-codes/4xx-client-error/error-413/)).
The server has no knob for, and does not detect, a proxy cap. A part refused
with 413 fails the upload permanently in the web client (the part size is
server-chosen, so a retry cannot shrink it); the proxy may also reset the
connection instead, which the client treats as transport failure and gives up
after its bounded part retries and one resume. Cloudflare's current defaults are
a 125 s Proxy Read Timeout and a 30 s Proxy Write Timeout
([524](https://developers.cloudflare.com/support/troubleshooting/http-status-codes/cloudflare-5xx-errors/error-524/)).
A part or complete answered 524/502/504 (or with a lost connection) is retried a
bounded number of times; complete is idempotent, the client first checks
whether the attachment is already stored, and a complete abandoned by the proxy
publishes the attachment only if the origin still finishes that complete (a proxy
may not cancel the origin request) or a later complete finishes it; otherwise
nothing is published before the upload expires.
Complete assembles the whole file before answering, so a very large file on
slow storage can exceed the read timeout on every bounded retry and fail
without data loss. Not verified against a real Cloudflare zone: request
buffering, per-part upload duration against the write/read timeouts on slow
uplinks, and assembly time for multi-GiB files. The regression
`proxy_capped_parts_round_trip_exact_bytes_through_413_and_524` uses a local
capped proxy stand-in only.

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

### Probes

Outside `/api/v1`, not in `apps/web/openapi.json`, and never behind the session,
consent or bearer checks (source `INFRA_PATHS`):

| URL | Result |
| --- | --- |
| `GET /health` | Liveness: always `200 {"ok":true}`. |
| `GET /ready` | `200 {"ok":true}`, or `503 {"ok":false,"checks":{"pg":false,...}}`. Checks the app-role PostgreSQL pool (`SELECT 1`) and, when collaboration is enabled, that the hub is not shutting down (`collab`). Each check is bounded by 2 s. There is no Redis to check. |
| `GET /metrics` | Prometheus scrape in OpenMetrics text (`application/openmetrics-text; version=1.0.0`), only for peers inside `METRICS_ALLOW_IPS`; other peers and other methods get the generic `404 not_found` problem. Metrics and scrape setup: "Prometheus scrape" below. |

`fvoci-server healthcheck` requests `GET /ready` from the address in `FVOCI_BIND`
(a wildcard bind is probed on loopback) and exits 0 on a 2xx answer within 4 s,
otherwise 1. It reads no other configuration or secrets. The source modes
`worker`, `compact` and `thumbnail` exit 1 with a message because those roles
run inside the server here. The Compose server healthcheck runs
`/opt/fvoci/bin/fvoci-server healthcheck` every 2 s with a 5 s timeout (the
source Compose used its binary's `healthcheck` with the same timeout), so the
container is healthy only once `/ready` reports PostgreSQL (and collab, when
enabled) ready.

### Prometheus scrape

`/metrics` is the only monitoring surface: the server adds no exporter, and
neither Compose file starts Prometheus, Grafana or a collector. Point an
existing Prometheus at it.

**Access.** Every response is OpenMetrics 1.0.0 text whatever the `Accept`
header (Prometheus 2.x/3.x parse it by the response `Content-Type`). Only a
direct TCP peer inside `METRICS_ALLOW_IPS` (Environment table) gets it; an
unset or empty list, any other peer and any method other than GET/HEAD get the
same `404 not_found` problem. `X-Forwarded-For`, `X-Real-IP` and `Forwarded`
are never read, so a proxy cannot vouch for a client. The user install passes
`METRICS_ALLOW_IPS` from the `fvoci` service environment through the preparation
to the server process unchanged; `.env` alone does not reach the container, so
set it with the override below.

**Compose override.** `infra/rust/compose.metrics.yml` (optional, next to
`compose.yml`) adds `METRICS_ALLOW_IPS` from `.env` and joins `fvoci` to an
internal network with no outside route and no published port. In `.env`:

```sh
FVOCI_METRICS_SUBNET=172.31.250.0/29   # a free private range on this host
METRICS_ALLOW_IPS=172.31.250.6/32      # the Prometheus address in it
```

`docker compose -f compose.yml -f compose.metrics.yml up -d`, then attach the
existing Prometheus container to network `fvoci_metrics` with that address
(`docker network connect --ip 172.31.250.6 fvoci_metrics <prometheus>`, or
`networks: {fvoci_metrics: {ipv4_address: 172.31.250.6}}` with the network
declared `external` in its own Compose file). Use the highest address: `fvoci`
takes a low dynamic one and the host holds the first. List that single
address, not the subnet, or every host process can reach `/metrics` from the
bridge address. Scraping through the
published `127.0.0.1` port instead arrives from the default network's gateway,
so allowing that address lets every local process read `/metrics`.

The override targets the `fvoci` service of the user install (`compose.user.yml`
rendered as `compose.yml`); it is not a release asset and does not apply to the
developer `infra/rust/compose.yml`, where `docker compose config` fails closed.

**Scrape config** (Prometheus 2.49 or newer for `scrape_protocols`):

```yaml
scrape_configs:
  - job_name: fvoci
    scrape_interval: 30s
    scrape_timeout: 10s
    metrics_path: /metrics
    scrape_protocols: [OpenMetricsText1.0.0, PrometheusText0.0.4]
    static_configs:
      - targets: ["fvoci:8080"]
```

**Metrics.** No label holds a workspace, user, document, room, token, URL or
concrete path. Database-derived values are refreshed at most every 15 s by one
bounded (2 s) query; everything else is read on each scrape.

| Name | Type | Meaning | On failure |
| --- | --- | --- | --- |
| `fvoci_http_request_duration_seconds{method,route,status}` | histogram | Request duration; `route` is the router template or `unmatched`, `method` one of the standard verbs or `OTHER` | In-process, cannot fail |
| `fvoci_outbox_lag_seconds` | gauge | Age of the oldest event some outbox consumer cursor has not passed, including events committed behind a long-running or idle-in-transaction session (source `lagSeconds()`) | `NaN` before the first successful refresh and after a failed or timed-out one |
| `fvoci_outbox_xmin_stall_seconds` | gauge | Age of the oldest transaction holding an xid anywhere in the PostgreSQL cluster, prepared transactions included; outbox delivery waits for it (replaces the source counter `fvoci_outbox_xmin_stall_total`; alert on a threshold) | As above |
| `fvoci_db_metrics_last_success_timestamp_seconds` | gauge | Unix time of the last successful outbox refresh | `0` until the first; kept on failure |
| `fvoci_db_metrics_refresh_failures_total` | counter | Outbox refreshes that failed or timed out | — |
| `fvoci_db_pool_connections{state="idle"\|"active"}`, `fvoci_db_pool_max_connections` | gauge | This server's application-role connection pool and its limit. Not total PostgreSQL connections: collab room locks (one per live room, detached from the pool), the preparation, other servers and tools are outside it; use `pg_stat_activity` for totals | In-process, cannot fail |
| `fvoci_task_stream_subscribers` | gauge | Open project task SSE streams | In-process |
| `fvoci_process_resident_memory_bytes` | gauge | Observed RSS (`VmRSS`) of the server process, helpers excluded | `NaN` when `/proc/self/status` is unreadable |
| `fvoci_collab_helper_resident_memory_bytes` | gauge | Observed RSS summed over live collaboration helper processes, the same sum collab admission reads | A helper exiting mid-read is skipped; a helper whose `/proc/<pid>/status` cannot be read contributes 0, so the sum can under-report |
| `fvoci_collab_helper_memory_budget_bytes` | gauge | Configured helper budget `FVOCI_COLLAB_MEMORY_BUDGET` (not an observation) | `NaN` when collaboration is off |

Collab admission refuses a room start when helper RSS plus the start's own
estimate (`max(16 MiB, factor × persisted bytes)`) would exceed the budget;
that per-start estimate, room occupancy and refusals by reason are not
exported yet.

**PromQL examples.**

```promql
# request rate and 5xx ratio
sum(rate(fvoci_http_request_duration_seconds_count[5m]))
sum(rate(fvoci_http_request_duration_seconds_count{status=~"5.."}[5m]))
  / sum(rate(fvoci_http_request_duration_seconds_count[5m]))
# p95 latency per route
histogram_quantile(0.95, sum by (le, route) (rate(fvoci_http_request_duration_seconds_bucket[5m])))
# outbox stuck (NaN compares false, so pair it with the staleness rule)
fvoci_outbox_lag_seconds > 300
time() - fvoci_db_metrics_last_success_timestamp_seconds > 120
increase(fvoci_db_metrics_refresh_failures_total[10m]) > 0
fvoci_outbox_xmin_stall_seconds > 600
# app pool saturation and helper memory against the budget
fvoci_db_pool_connections{state="active"} / fvoci_db_pool_max_connections > 0.9
fvoci_collab_helper_resident_memory_bytes / fvoci_collab_helper_memory_budget_bytes > 0.8
```

### Response security headers

Every response carries the source's global security headers (`http-security.ts`,
nosecone defaults): a `Content-Security-Policy` (`default-src 'self'`, same-origin
`connect-src` for the API and `/collab` WebSocket, `frame-ancestors 'self'`,
`object-src 'none'`, inline shell blocks only by build-time hash, embed frames for
YouTube/Vimeo/Figma), `Referrer-Policy: no-referrer`, `X-Content-Type-Options:
nosniff`, `X-Frame-Options: SAMEORIGIN`, COOP/CORP `same-origin`,
`Origin-Agent-Cluster`, `X-DNS-Prefetch-Control: off`, `X-Download-Options`,
`X-Permitted-Cross-Domain-Policies: none`, `X-XSS-Protection: 0` and a
`Permissions-Policy` that denies unused device APIs. With an `https://`
`FVOCI_PUBLIC_ORIGIN` it adds `Strict-Transport-Security: max-age=31536000;
includeSubDomains` and `upgrade-insecure-requests`. Routes that set a stricter
policy keep it (share pages and fragments, attachment and branding downloads use
`sandbox` or nonce policies).

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

The HTTP server exposes `GET /collab` (426 without WebSocket upgrade) and
upgrades to Hocuspocus 4.6.0 only when `FVOCI_COLLAB_ENGINE` points at a built
`collab-engine` helper binary. The container image and Compose stack set it by
default (see "Container install"); a server run outside them needs the variable.
Without it the route returns 503 `collab_unavailable`.

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
| `FVOCI_COLLAB_MAX_ROOMS` | 30 (clamp 1–512) | Hub room slots. When full, a new room first reclaims the least recently active room that has no members, no join in flight and no HTTP body operation, and whose last activity is older than `max(COLLAB_RPC_TIMEOUT_MS, 3 s)` (5 s by default, at most the idle timer); it waits for that room to close. With no such room the join is refused (WebSocket close 1013, the editor retries with bounded backoff). Default fits stock PostgreSQL `max_connections=100`; the 64-room capacity probe sets `64` and needs a higher Postgres limit. |
| `FVOCI_COLLAB_MAX_CHILDREN` | primary + validator headroom | Bounds the validator helper pool only. Primary cap is `max_rooms + 4` for offline revision capture headroom. |
| `FVOCI_COLLAB_MEMORY_BUDGET` | 2 GiB | Aggregate admission: sum live helper VmRSS plus `max(16 MiB, 14× persisted bytes)` per room start |
| `FVOCI_COLLAB_MAX_CONNECTIONS` | 32 | Per-room WebSocket members |
| `FVOCI_COLLAB_IDLE_MS` | 30000 | Idle room eviction |
| `FVOCI_COLLAB_REVOKE_POLL_MS` | 5000 | ACL revoke poll |

Automatic revision history (session snapshots on last collab disconnect; scheduled
snapshots and automatic retention in the hourly maintenance sweep):

| Variable | Default | Notes |
| --- | --- | --- |
| `REVISION_SESSION_SNAPSHOT` | `1` | When enabled, the room appends a `session` revision (`created_by` null) after the last real collab WebSocket leaves, using durable committed collab state only. Set `0` to disable. |
| `REVISION_KEEP` | 200 | Per target, keeps the newest `session`/`scheduled` rows; `manual` rows are never deleted. |
| `REVISION_SNAPSHOT_INTERVAL_HOURS` | 24 | Scheduled stale-target snapshots (`0` disables). Runs in the maintenance loop (`FVOCI_REVISION_SWEEP_INTERVAL_SECS`, default 1h). |
| `FVOCI_REVISION_SWEEP_INTERVAL_SECS` | 3600 | Cadence of scheduled revision snapshots + automatic retention batches. |

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
uses the durable CRDT state and operation receipts. If a room actor panics, its
peers are closed with 1011, the helper and room guard are released, and the next
join starts one successor from the durable state.

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

## Markdown conversion child

Markdown -> Tiptap (body `PUT` with `contentMd`, markdown-zip/office/Notion imports), Tiptap ->
Markdown (member `GET …/body?format=md`, AI actions) and legal Markdown -> HTML run in Rust, in a
hidden mode of the server binary: `fvoci-server --internal-markdown`. There is nothing to install
or configure; the server re-executes itself (`current_exe`). The public share page renders its
Markdown/HTML in-process without the parser (its Markdown keeps the first pass of the `$`
self-check). Tiptap -> Yjs seeding (body `PUT`, duplicate, imports) runs in the `collab-engine`
child (`FVOCI_COLLAB_ENGINE`, op `seed_from_tiptap`, same limits as the room child). Every
document export (`GET …/documents/{id}/md|docx|pdf|pptx`, wiki and project) and the public share
PDF run in the same child as the Markdown conversions
(`--op tiptap-to-md-export|tiptap-to-docx|tiptap-to-pdf|tiptap-to-pptx`), see "DOCX export",
"PDF export" and "PPTX and Markdown export" below. The server no longer runs the Node document
convert helper; the final image contains no Node/Bun/Deno or bundled JavaScript engine.
Node is used only to build the web assets and run development oracles/tests.
`scripts/document-convert` remains only
as the development oracle for the fixture regeneration scripts (`scripts/regen-*-oracle.sh`).
The installed document smoke uses a host-side Python standard-library client;
it checks the installed Rust API before and after restart, including parsed
OOXML text. Python and its test client are not copied into the product image.
Development/CI Python fixtures and independent readers remain supported.

The child is chosen before any runtime, config or credential is loaded, gets a cleared
environment, RLIMIT_AS 2 GiB and RLIMIT_CPU 30 s, reads one input from stdin (4 MiB cap; the
callers already cap Markdown bodies at 1 MiB) and writes at most 32 MiB. The parent kills it
after 30 s wall time or when the request goes away, dies-with-parent is set on Linux, and at most
two conversions run per server process (the Node helper's concurrency). This is a CPU/memory/
crash boundary, not a filesystem or network sandbox.

Outcomes: a body nested past what can be stored (Tiptap JSON deeper than 126 levels, e.g. 61
nested block quotes), or one that exhausts the CPU/memory/time budget, is `400 invalid_input`;
input or output past the byte caps is `413`; a spawn/IO failure is `500`.

The parser is markdown-rs 1.0.0 (a port of the JS micromark parser the editor uses), vendored
in `vendor/markdown` with one fix for quadratic edit bookkeeping (`vendor/markdown/PATCHES.md`).
Measured on 1 MiB bodies (release build, parse only): each of the 51 oracle corpus files repeated
to 1 MiB takes 0.3–1.1 s and at most 0.62 GB RSS; 1 MiB of 3-line tables or of blank lines about
1.1–1.2 s and about 1 GB RSS. The 30 s watchdog is about 25x the slowest ordinary body. Still super-linear, as in the JS parser:
deeply nested emphasis (60 KB of nested `*a ` 12.9 s), 50,000 nested `>` (15.5 s) and a
1,000-level indented list (11 s); such bodies stop at the 30 s watchdog as `invalid_input`.

## DOCX export

`GET …/documents/{id}/docx` (wiki and project routes) is written in Rust by the Markdown child
above (`--op tiptap-to-docx`, same rlimits, 30 s watchdog and two-per-process concurrency) with
docx-rs 0.4.22 (MIT, `image` feature off). The
Tiptap body is walked into one export model (`src/documents/export_model.rs`, meant for the PDF
and PPTX writers too) and written as Word paragraphs, headings, numbering, tables and runs
(`src/documents/docx.rs`). Fonts are named only (code in Consolas); attachment bytes are never
read (attachments export as their names, as before). Only absolute `http`/`https`/`mailto` links
become hyperlinks. The package is deflated after docx-rs writes it.

Contract as before: `application/vnd.openxmlformats-officedocument.wordprocessingml.document`,
`Content-Disposition` with an RFC 5987 `filename*`, `private, no-store`, `nosniff`; a stored body
over 1 MiB or a file over 20,000,000 bytes is `413`, a body that is not a Tiptap doc `400`, and a
child killed by the watchdog or a resource limit, or a writer failure, `500` (as the Node
export's timeout and serializer errors). Exports share the two-per-process conversion slots with
Markdown imports and body conversions and wait for a free one.

Differences a user can notice against the Node export (full list with fixtures in
`compat/fixtures/export-docx/README.md`): task items show a ☑/☐ glyph instead of a clickable Word
checkbox, empty list items are kept as empty items, file attachments are their name without the
in-app `attachment:` link, relative in-app links (`/docs/1`) are plain text, and underline and
highlight are kept.

Measured (release, 1 MiB stored bodies, one child each): mixed corpus 0.15 s / 83 MB RSS / 85 KB
file; Korean text 0.03 s / 26 MB / 19 KB; 67 tables of 20×8 cells 0.16 s / 97 MB / 42 KB; tiny marked
runs with links 0.11 s / 71 MB / 45 KB (the Node helper: 1.3–3.9 s, 0.37–0.73 GB RSS).

## PDF export

`GET …/documents/{id}/pdf` (wiki and project routes) and the public `GET /api/v1/share/{token}/pdf`
are written in Rust by the same child (`--op tiptap-to-pdf`, same rlimits, 30 s watchdog) from the
same export model: krilla 0.8.2 writes the PDF and subsets the fonts, rustybuzz 0.20.1 shapes the
text, unicode-linebreak 0.1.5 gives the line-break opportunities, and `src/documents/pdf.rs` does
the flow (A4, 35/65/35 pt paddings, 12 pt text at line height 1.5, the TS heading sizes and block
margins, pages broken between lines and table rows). The fonts are the files the Node export embeds
(`packages/editor/src/fonts`: Noto Sans KR, Noto Sans Mono CJK KR, Noto Emoji; SIL OFL 1.1),
read at run time by the export child; each character uses the
first of them that has a glyph, pictographs Noto Emoji first. Only the glyphs used are embedded.
Attachment bytes are never read and nothing is fetched. The font files are not compiled into `fvoci-server`; the export child reads them from `FVOCI_EXPORT_FONT_DIR` (the image sets `/opt/fvoci/share/fonts`; development and tests fall back to `packages/editor/src/fonts` in the source tree). A missing font file fails the PDF op (500), never a partial PDF.

Contract as before: `application/pdf`, `Content-Disposition` with an RFC 5987 `filename*`,
`private, no-store`, `nosniff` (share: also `CSP: sandbox` and `Referrer-Policy: no-referrer`);
authorization and share scope checks run before the child; a stored body over 1 MiB or a file over
20,000,000 bytes is `413`, a body that is not a Tiptap doc `400`, a killed child or writer failure
`500`. Member exports wait for one of the two per-process conversion slots; public share PDFs use
their own pool of one and never wait (`503 share_pdf_busy`, `Retry-After: 5`). The same body gives
the same bytes on both routes.

Differences a user can notice against the Node export: regular (400) and bold (700) weights of
Noto Sans KR instead of its Thin default instance for all text; marks are drawn (bold, slanted
italic, underline, strike, highlight, code font); `http`/`https`/`mailto` links are clickable;
task items have a checkbox, code blocks a grey background, callouts a coloured bar and background,
details their summary line, embeds a `[[doc:ref]]` placeholder; flags, skin tones and CJK
Extension B text render instead of garbage; long URLs wrap without an inserted hyphen. Characters
none of the three fonts has (for example mathematical alphanumerics) still show as boxes. Text is
left-to-right only.

Measured (release, 1 MiB stored bodies, one child each): mixed corpus 0.11 s / 54 MB RSS / 149
pages / 0.55 MB; Korean text 0.12 s / 35 MB / 181 pages / 0.17 MB; 55 tables of 20×8 cells
0.09 s / 50 MB / 53 pages; tiny marked runs with 4,520 links 0.09 s / 48 MB / 1.0 MB; emoji
0.10 s / 30 MB; every Hangul syllable and 20,000 ideographs (the largest font subsets) 1.10 s /
95 MB / 204 pages / 8.7 MB. The 30 s watchdog is about 27x the slowest. The Node helper took
4–273 s and 0.7–3.4 GB RSS on the same bodies.

## PPTX and Markdown export

`GET …/documents/{id}/pptx` (wiki and project routes) is written in Rust by the same child
(`--op tiptap-to-pptx`, same rlimits, 30 s watchdog, shared conversion slots) from the same export
model, with the zip writer the DOCX export already links and hand-written PresentationML
(`src/documents/pptx.rs`; no further crates). The layout is the Node export's: a 16:9 deck
(10 × 5.625 in), text boxes stacked from the top of a slide 0.5 in from the left and 9 in wide, the
title as the first heading (28 pt; later headings on a slide 16 pt), paragraphs 14 pt, one box per
top-level list item, real tables (12 pt), code/math/Mermaid source in the mono font (12 pt), a
top-level horizontal rule opens a new slide, and box heights from the same line estimate. Fonts are
named only (Noto Sans KR, Noto Sans Mono CJK KR); nothing is embedded and attachment bytes are never
read. The package is a minimal PresentationML deck (one master, one blank layout, a theme) that the
product's own OOXML importer reads back.

`GET …/documents/{id}/md` is `# title` (visible title, ASCII punctuation backslash-escaped) and a
blank line, then the body's Markdown (`--op tiptap-to-md-export`; the same Tiptap -> Markdown
conversion as `GET …/body?format=md`), `text/markdown; charset=utf-8`. It is byte-equal to the Node
export on the whole oracle corpus.

Contract as before for both: OOXML/Markdown content type, `Content-Disposition` with an RFC 5987
`filename*`, `private, no-store`, `nosniff`; a stored body over 1 MiB or a file over 20,000,000
bytes is `413`, a body that is not a Tiptap doc `400`, a killed child or writer failure `500`.

Differences a user can notice in the PPTX against the Node export (full list with fixtures in
`compat/fixtures/export-pptx/README.md`): text that does not fit the rest of a slide continues on
the next slide instead of being shrunk, and tables too long for a slide continue on the next with
the header row repeated (the Node export drew them past the slide edge); nested list items are
indented levels and ordered items are numbered 1, 2, 3 (the Node export flattened nesting and
numbered every item 1); task items have a ☑/☐ glyph; marks (bold, italic, underline, strike,
highlight, code font) are kept and `http`/`https`/`mailto` links are clickable; quotes and callouts
have a coloured bar (callouts a background); details show their summary; embeds a `[[doc:ref]]`
placeholder; ragged table rows are padded to a full grid.

Measured (release, 1 MiB stored bodies, one child each), PPTX / Markdown: mixed corpus 0.09 s /
47 MB / 362 slides / 0.68 MB and 0.05 s / 37 MB; Korean text 0.05 s / 21 MB / 503 slides and
0.01 s / 15 MB; 67 tables of 20×8 cells 0.12 s / 47 MB / 83 slides and 0.05 s / 34 MB; tiny marked
runs with links 0.06 s / 42 MB and 0.06 s / 36 MB. The Node helper took 1.0–1.2 s and 0.28–0.42 GB
RSS on the same bodies.

## Container install

There are two Compose files under `infra/rust/`:

- **User install:** `compose.user.yml` with `compose.user.env.example` (a release
  ships them as `compose.yml` and `env.example`). Services `fvoci`, `postgres`,
  `meilisearch`; the `fvoci` container prepares the database and search on every
  start and then runs the server. This is the install for anyone running FVOCI;
  see "Install (compose.yml and .env)", "Release images (0.x)" and "Backup and
  restore".
- **Developer stack:** `compose.yml` with `.env.example` (or
  `fvoci-migrate --init-env`), built from the checkout, with a separate one-shot
  `init` service before `server` and the optional `compose.s3.yml` overlay. It
  is what `scripts/install-smoke.sh`, `backup-restore-smoke.sh` and
  `upgrade-smoke.sh` exercise; see "Developer stack (compose.yml with init)".

The install artifact is a multi-stage Docker image used by both. It builds release `fvoci-server`, `fvoci-migrate`, the production
`collab-engine` helper (`--features worker`), the production `document-extract`
helper (same rhwp pin as `scripts/prepare-extract-helper.sh` / `rust.yml`, without
`test-hang`), and the `apps/web` production bundle (same steps as
`scripts/prepare-web-e2e.sh` + `npm run build`). Runtime images pin base digests,
run as uid/gid `1000` (`fvoci`) by default (the user install below starts the
container as root and runs the server as `1000`), and set:

| Variable | Installed path / note |
| --- | --- |
| `FVOCI_STATIC_DIR` | `/opt/fvoci/static` |
| `FVOCI_COLLAB_ENGINE` | `/opt/fvoci/bin/collab-engine` |
| `FVOCI_EXTRACTOR_BIN` | `/opt/fvoci/bin/document-extract` |
| `FVOCI_STORAGE_DIR` | `/data/storage` (Compose volume, owned by `fvoci`) |

Unset any helper env to disable that feature (API-only). The published image ships
all three helpers and enables them via the defaults above.

### Install (compose.yml and .env)

The user install is `infra/rust/compose.user.yml` with
`infra/rust/compose.user.env.example`; a release ships them as `compose.yml`
(image pinned by digest) and `env.example`, with a short `INSTALL.md`. In an
empty folder:

```sh
cp env.example .env      # fill in each empty value with the command shown above it
docker compose up -d --wait
```

Open `FVOCI_PUBLIC_ORIGIN` (<http://localhost:8080>) and create the first
administrator (no administrator is created automatically). Services: `fvoci`,
`postgres`, `meilisearch`; there is no separate init service.

`.env` holds nine values: `FVOCI_PUBLIC_ORIGIN` and `FVOCI_PUBLISH_PORT`
(filled in: change both together; the server never derives the origin from
`Host` or `Forwarded`), `PASSWORD_PEPPER_ACTIVE_KEY_ID` and
`ENCRYPTION_ACTIVE_KEY_ID` (filled in: `install`), and five values to generate:
`POSTGRES_PASSWORD`, `FVOCI_APP_PASSWORD`, `MEILI_MASTER_KEY`
(`openssl rand -hex 32` each) and the keyrings `PASSWORD_PEPPER_KEYS` and
`ENCRYPTION_KEYS` (`{"install":"<openssl rand -hex 32>"}`, the same format as
`fvoci-migrate --init-env`). Compose requires each value, so an unfilled `.env`
stops before any container is created. Keep `.env` private (`chmod 600 .env`)
and back it up apart from the database backups; PostgreSQL keeps the owner and
app passwords from the first start, and the pepper and encryption keys open
existing accounts and sealed secrets.

Compose passes the values as container environment, each service only those
it names (no `env_file`): `fvoci` all of them except `FVOCI_PUBLISH_PORT`
(the published port), `postgres` only `POSTGRES_PASSWORD`, `meilisearch` only
`MEILI_MASTER_KEY`. A container keeps the environment it was created with:
after editing `.env`, run `docker compose up -d`, which recreates the
containers whose values changed (`docker compose restart` keeps the old
values). That applies a changed setting; it does not change a password or key
already in use. PostgreSQL keeps both passwords from its first start (a
different value is refused, below), and a keyring changes by adding a key and
switching its active id, keeping the old key while anything still uses it
(`--secrets-audit`, `--secrets-rotate` in "Operator commands").

The image entrypoint is `fvoci-migrate --start`. The `fvoci` service starts it
as root (`user: "0:0"`). Given the owner password (`POSTGRES_PASSWORD`), it
runs, on every start of `fvoci`:

1. **Settings check.** Every required value is set, not empty and not an
   example placeholder (`<…>`, `change-me`, …); passwords and the master key
   are at least 16 characters and the app password differs from the owner's;
   the keyrings parse with their active ids; the origin is valid. Errors name
   the variable, never the value, and exit 2.
2. **Readiness.** It waits for PostgreSQL (as the owner) and Meilisearch
   (`/health`) until `FVOCI_PREPARE_TIMEOUT_SECS` (default 120) and exits 1
   after it; SIGTERM/SIGINT end the wait at once (exit 143/130). A wrong owner
   password is reported as such, without waiting.
3. **Preparation**, under a PostgreSQL advisory lock (concurrent starts run one
   after another). If migrations are pending while sessions of the app role are
   open (another server is still running), it refuses and points to "Upgrade".
   Otherwise it creates the `NOBYPASSRLS` app role if missing, migrates (the
   same locked, transactional path as `fvoci-migrate`), applies the grants,
   checks that `FVOCI_APP_PASSWORD` opens the app role, and ensures the scoped
   search key in `/run/fvoci/meili/api_key` (the `meili_key` volume). Root
   first makes that directory `root:root` `0755` (through the open directory;
   one owned by any other user than uid 1000, or writable by others, is
   refused), then writes the key to a new, unpredictably named file, sets it to
   `root:1000` `0640` on the open descriptor and renames it into place, so the
   server can read the key but not replace, redirect or change it; a symlink
   left in the directory is replaced, never followed. Key files are read
   without following a symlink.
4. **Server.** It closes every preparation connection and `exec`s
   `fvoci-server` in the same process (pid 1, so signals, graceful shutdown and
   child reaping are the server's, as before), as uid/gid `1000` with no
   supplementary groups and so no capabilities. Its environment is the
   container's without `POSTGRES_PASSWORD`, `DATABASE_URL`,
   `FVOCI_MIGRATION_URL`, `MEILI_MASTER_KEY`, `FVOCI_MEILI_MASTER_KEY` and
   `FVOCI_APP_PASSWORD`, plus `DATABASE_APP_URL` (the app role; it contains
   the app password, which the server needs) and `HOME=/nonexistent`: it keeps
   the keyrings and never holds the owner password or the master key. This
   removes them from the server process only; the container configuration
   still holds them (below). The service has `no-new-privileges`, and the
   image has no setuid or setgid file. Descriptors the preparation opened are
   close-on-exec.

If any step fails the server does not start; the container restarts and tries
again (`docker compose logs fvoci` names the problem).

**The boundary is the uid, inside one container.** The server and everything it
starts run as uid 1000; the preparation is root's. Every `docker exec` and
healthcheck process starts from the container configuration, so it holds every
value Compose passes to `fvoci`, the owner password and master key included;
they run as root (the service's user). So a compromised server cannot read
those processes' environment, the preparation's memory or environment (another
uid, and root's processes are not traceable by it), or redirect root's search
key write (above). The exception is a session you start as uid 1000
(`docker compose exec -u 1000:1000 fvoci …`): it holds every configured value
in an environment the server's uid can read while it runs.
`scripts/standalone-install-smoke.sh` checks each of these on a running
install. What the server does hold: the app role password (in
`DATABASE_APP_URL`), the pepper and encryption keyrings, and the scoped search
key; that is what it needs to run.

The server also makes itself non-dumpable at startup (`PR_SET_DUMPABLE` 0) and
refuses to start if the kernel does not allow it. The kernel then owns the
files under its `/proc/<pid>` by root, so the helpers it starts (collaboration, document
extraction, preview, Office and Markdown conversion, all uid 1000) and a uid-1000
`docker compose exec` session (which starts with the configured values itself)
can read neither its environment (the keyrings, `DATABASE_APP_URL`) nor its
memory or open descriptors, and cannot attach to it. For the same reason the server writes no core dump at all, whatever
`fs.suid_dumpable` is set to (that setting only applies after a credential
change, which the server never makes), and `gdb -p`, `strace -p` and `lsof` on
the server no longer work from a uid-1000 session. Run them as root with
`CAP_SYS_PTRACE` (`docker compose exec --privileged fvoci …`); the image ships
none of these tools, so install them in the container first or attach from the
host. `perf -p` additionally needs a container created with `CAP_PERFMON` or
`CAP_SYS_ADMIN`, since `exec --privileged` does not change the container's
seccomp profile. The helpers do still share uid
1000 file access with the server: the attachment store (`/data/storage`,
every workspace's files) and the scoped search key
(`/run/fvoci/meili/api_key`, readable by group 1000). The helpers themselves
stay dumpable, so where the host allows same-uid ptrace one helper can attach
to another. What is **not** separated:

- It is one container, not two: root in it (`docker compose exec fvoci …`,
  which defaults to root, and the healthcheck) starts with every configured
  value. Under Docker's default capabilities (no `CAP_SYS_PTRACE`) that root cannot
  read the server's `/proc/1/environ` either, and neither can uid 1000 since
  the server is non-dumpable; inspect the server with
  `docker compose exec --privileged fvoci …` (root with `CAP_SYS_PTRACE`). A
  kernel or container escape from uid 1000 is outside this boundary.
- Anyone who can run Docker commands on the host can read every value:
  `docker inspect`, `docker compose config` and `docker compose exec` show
  them, as `.env` itself does. Do not paste their raw output into logs,
  issues or reviews.
- `postgres` and `meilisearch` hold their own value (the owner password, the
  master key) in their container configuration and process environment;
  neither gets the app's passwords or keyrings.

The owner never reaches the network beyond the Compose network: PostgreSQL and
Meilisearch publish no port.

Without the owner password the entrypoint only execs `fvoci-server` (the
developer stack, `infra/rust/compose.yml`, prepares in its separate `init`
service instead); started as root, it still runs the server as uid 1000.

Other defaults come from the image and the Rust loader: helper paths, static
and storage directories, bind address, shutdown deadline (30 s), collaboration
memory budget (2 GiB), extraction poll interval (30 s), secure cookies for an
`https` origin, and the database names `fvoci`, `fvoci_owner`, `fvoci_app`
(PostgreSQL service `postgres:5432`, Meilisearch `http://meilisearch:7700`).
The file keeps one capacity setting: `FVOCI_COLLAB_MAX_ROOMS: "64"` with
PostgreSQL `max_connections=150`, the verified pair (64 room fences + app pool
64 + reserve 10 = 138). The image default is 30 rooms, which fits a stock
PostgreSQL; change both together.

#### Optional settings

Put optional settings in a `compose.override.yml` next to `compose.yml`;
`docker compose` merges it automatically. List only what you use, then
`docker compose up -d`:

```yaml
services:
  fvoci:
    environment:
      SMTP_HOST: smtp.example.com
      SMTP_PORT: "587"
      SMTP_FROM: fvoci@example.com
```

| Topic | Variables (details in this file) |
| --- | --- |
| Domain, HTTPS, proxy | `FVOCI_PUBLIC_ORIGIN=https://…` in `.env` (also turns on secure cookies) and the published address; see "Developer stack (compose.yml with init)" below for the proxy rules, which apply to both stacks |
| S3 storage | `STORAGE_DRIVER=s3`, `S3_*` ("S3 storage backup") |
| Mail | `SMTP_HOST`, `SMTP_PORT`, `SMTP_FROM` |
| OIDC sign-in | providers are set up in the app, sealed with `ENCRYPTION_KEYS`; `OIDC_ALLOW_INSECURE=1` only for a local http provider |
| GitHub integration | `GITHUB_APP_ID`, `GITHUB_APP_PRIVATE_KEY`, `GITHUB_WEBHOOK_SECRET`, `GITHUB_STATE_SECRET` |
| AI | `FVOCI_AI_ENABLED`, `FVOCI_AI_SECRET`, `FVOCI_AI_EMBEDDINGS_*` |
| Prometheus scrape | `METRICS_ALLOW_IPS` via `compose.metrics.yml` ("Prometheus scrape"); no monitoring service is added |
| Tuning | `FVOCI_COLLAB_*`, `FVOCI_EXTRACT_POLL_SECS`, `FVOCI_SHUTDOWN_DEADLINE_MS`, `FVOCI_UPLOAD_*`, `FVOCI_PREPARE_TIMEOUT_SECS`, `RUST_LOG` |

Unset variables keep the product default; an empty value is a value, so do not
add empty entries. `docker compose down` and `up -d` keep data; `down -v`
deletes the database, files and search index. Back up with `scripts/backup.sh`
(see "Backup and restore").

### Developer stack (compose.yml with init)

`infra/rust/compose.yml` is the developer and source-build stack, not the user
install: the image is built from the checkout, the settings are the longer
`infra/rust/.env.example` (owner and app role names, `FVOCI_IMAGE`,
`FVOCI_COOKIE_SECURE`, `FVOCI_PUBLISH_ADDR`, the optional integrations) passed as
environment, and preparation runs in a separate one-shot `init` service. The
steps below, "Verification" and "Upgrade" are for this stack; a user install
upgrades as in "Release images (0.x)".

1. Generate `infra/rust/.env` with `fvoci-migrate --init-env` (above), or copy
   `infra/rust/.env.example` to `infra/rust/.env` and replace placeholders.
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

Compose always passes `FVOCI_COOKIE_SECURE` (default `false`), so the server's
https-scheme default does not apply: set `true` in `.env` yourself when you
switch an existing file to https (`--init-env` does it for new files). Keep the
published address reachable only by the proxy. The same proxy rules
apply to the user install, where `FVOCI_PUBLIC_ORIGIN=https://…` in `.env`
also turns on secure cookies. The proxy must pass the
browser's `Origin` header unchanged (mutating routes and `/collab` compare it
with `FVOCI_PUBLIC_ORIGIN`) and forward the WebSocket upgrade for `/collab`.
An https origin also sends HSTS with `includeSubDomains`, so serve every
subdomain over https first. After editing `.env`, re-run `up -d --wait server`
with the install's same Compose files, project name and env file (as in "Upgrade"),
then `--doctor` with those same flags (see "Operator commands"): its
`public_origin` check fails for plain http off loopback or https without
secure cookies. Proxy body-size and timeout limits for uploads are under
"Start server".

### Verification

`scripts/install-smoke.sh` builds the image, starts an isolated Compose project
(unique name, ephemeral published port, run-owned volumes), exercises setup/login,
wiki collab body projection, HWPX upload + extraction, `/collab` availability,
a graceful `docker compose stop server` (stopped container must report exit code 0),
a recreated server container on the same volumes, and post-recreate reads.
CI runs the same script on `ubuntu-24.04` and `ubuntu-24.04-arm` via
`.github/workflows/install.yml` (no secrets, no image publish). This is the
developer stack. The user install is exercised by
`scripts/standalone-install-smoke.sh` (a local, manual run: fresh `.env`,
first admin, each service's environment, the uid boundary, restart, recreate,
backup and restore) and, for
a published release, by `scripts/release-smoke.sh` in `release.yml`
(`docs/RELEASING.md`).

### Upgrade

This moves a developer-stack install (`infra/rust/compose.yml` with `init`) to
a newer build of this Rust server on the same volumes. A user install from a
release upgrades by replacing `compose.yml`; see "Release images (0.x)". Each migration commits on its own, so a failed or interrupted migrate
can leave the database between versions, and an older image then refuses to
start against it.

Use the existing install's Compose project name (`fvoci-rust-install` for the
developer stack example, or the name passed to `restore.sh --project`), env file and
all `-f` files throughout. For S3, omitting `infra/rust/compose.s3.yml` silently
selects local storage; a storage doctor probe cannot detect that wrong choice.

1. From the new checkout, build the new image under a new tag and keep the old
   image and checkout:
   `docker build -f infra/rust/Dockerfile -t fvoci-rust-install:<new-tag> .`
   Do not rebuild the tag `.env` still names: `scripts/backup.sh` refuses when
   the running server's image differs from the one `FVOCI_IMAGE` resolves to.
2. From the old checkout, with its `.env` unchanged, back up and leave the server
   stopped (old servers must not run during migrate):
   `scripts/backup.sh --project <name> --env-file infra/rust/.env --output <new-dir> --leave-stopped`.
   Keep a protected copy of the env file with the backup (file mode 0600,
   directory 0700); the archive omits the pepper, encryption keys and passwords.
   With S3 the script refuses. First stop the server using the existing flags:
   `docker compose -f infra/rust/compose.yml -f infra/rust/compose.s3.yml --project-name <name> --env-file infra/rust/.env stop -t 45 server`,
   then take the quiesced dump and protect bucket objects as in "S3 storage backup".
3. Compare the new checkout's `.env.example` and release instructions with the
   existing env file; preserve keys, role/database names, storage and project
   identity. Set `FVOCI_IMAGE` to the new tag. From the new checkout run:
   `docker compose -f infra/rust/compose.yml --project-name <name> --env-file <existing-env> up -d --wait server`.
   For S3, include `-f infra/rust/compose.s3.yml` after the base file, as for
   the stop command. Retain any other overlays used by this install.
   `init` runs `fvoci-migrate`, `--grant-app-role` and `--ensure-meili-key`
   from the new image; `server` starts only if init succeeds.
4. Run `--doctor` using the same files, project name and env file (see "Operator
   commands"), then confirm login and an existing document and attachment.

If init fails, leave the server stopped. Run `logs init` with the same Compose
flags, fix the cause and repeat step 3: already applied migrations are skipped
and the grant commits all or nothing. If init fails with `outbox consumer seed
repair: newest event xid ... is not settled`, a transaction on the same
PostgreSQL cluster (any database, or a prepared transaction) is older than the
newest event. Let it end or roll it back (`pg_stat_activity`,
`pg_prepared_xacts`), then repeat step 3. Migration 041 adds the outbox cursors
an earlier upgrade left missing, so notifications and mail do not replay past
events. Migration 043 builds the index `events_workspace_relay_idx` on
`fvoci.events (workspace_id, xact, seq)` inside the migrate transaction, so it
cannot use `CONCURRENTLY`. While it builds, it holds a SHARE lock on
`fvoci.events`: reads continue, but every write that records an event waits.
The server is stopped during migrate, so this only affects other clients of the
same database. The build is one scan and sort of the table, so its time grows with the
number of rows in `fvoci.events`; check `SELECT count(*) FROM fvoci.events` and
plan the maintenance window accordingly. To go back
to the old build, stop the upgraded server first; do not start the old image on the migrated database.
Restore the pre-upgrade backup into a new project with the old image (local
storage: "Backup and restore"; S3: "S3 storage backup", item 3). A rollback
loses writes made after that backup; preserve the failed install for diagnosis,
but with S3 keep it stopped and never start it again with the same `S3_*`
settings, since its sweeps would delete objects the restored install uses.
Ordinary PR and main CI does not run this image-to-image upgrade;
`install-smoke.sh` recreates the server on the same image. An optional manual
run is described under "Upgrade validation".

#### Upgrade validation

`scripts/upgrade-smoke.sh --old <sha> --new <sha>` runs these steps with local
storage in isolated Compose projects. Both SHAs must be on the first-parent
history of `origin/main` (`--main-ref`), and old must be an ancestor of new. New
must add at least two migrations, including its newest one. Each image is built
from a `git archive` of its SHA, not the working tree. It is labelled with
`org.opencontainers.image.revision` and the Dockerfile and recipe hashes. The
recipe differs from that SHA's Dockerfile only by `ENV CARGO_BUILD_JOBS`
(`--build-jobs`, default 2). A tag with matching labels is reused, and the
images are kept. The script refuses equal image IDs and stops before a build if
the Docker root has less than `--min-free-gib` free. `--plan-only` runs only the
source, migration and recipe checks, plus that disk gate for each image that
would need a build (a reused image is not built, so no disk claim is made for
it); it builds, starts and tears down nothing. A reused tag is trusted on its
revision and recipe labels, which anyone with Docker access can set.

The smoke seeds the old image with a login, a collab wiki body, an HWPX
attachment (sha256 and extraction), a comment, and a TOTP secret sealed with
`ENCRYPTION_KEYS`. It runs the old checkout's `backup.sh --leave-stopped`, then
pre-creates the first table of the newest migration so the new image's `init`
fails. The server must stay stopped: no running container and no HTTP answer.
Only that migration may be missing. After the table is dropped, one rerun of
step 3 must succeed. The server then runs the new image, with no old-image
container left in the project. Doctor passes, the seeded data and extraction are
intact, and `--verify-secrets` opens the secret. The same probe with a different
k1 must fail and report the MFA secret `invalid`, not a missing key or a
database error. Next the smoke stops the upgraded server and runs the old
checkout's `restore.sh` into a fresh project on the old image. The seeded data
must be back and the write made after the upgrade must be gone. The old image
never runs on the migrated database.

`--storage s3` runs the same flow with `infra/rust/compose.s3.yml`. It follows
"Upgrade" step 2 for S3 and "S3 storage backup", not `backup.sh`/`restore.sh`.
It uses the project's own pinned silo and a run-owned bucket and credentials.
It enables bucket versioning before seeding and stores two HWPX attachments.

1. **Backup:** the old `backup.sh` must refuse the S3 install. It must write no
   output and leave the server running. The smoke then runs the documented
   `stop -t 45 server`, checks that no other client sessions remain, and takes
   the same `pg_dump` as `backup.sh`. It records each object's checkpoint
   version ID.
2. **Upgrade:** the upgrade and its checks are unchanged. In addition, the new
   image's `--verify-storage` must report both objects.
3. **Damage:** the upgrade project's `postgres` and `meilisearch` are stopped,
   so only its silo runs. Then one object is deleted (a delete marker) and the
   other is overwritten with other bytes.
4. **Restore:** the dump is restored into a fresh database on the old image with
   the old `restore.sh` steps minus the volume archive (app role, `pg_restore`,
   `init`, `--recover-outbox`, `--rebuild-search`). That project's server joins
   the upgrade project's local Docker network and points at the same bucket.
5. **Checks before start:** `--verify-storage` must fail at HeadBucket for a
   wrong bucket. For the damaged bucket it must exit non-zero with exactly the
   deleted attachment `missing` and the overwritten one in `sizeMismatch`. No
   server container may exist and nothing may answer HTTP.
6. **Version restore:** the smoke removes the delete marker and copies the
   checkpoint version back. `--verify-storage` and `--verify-secrets` must then
   pass before the server starts. Both attachments must download with their
   original sha256.

On success the trap runs `down -v` for each project with the compose file of
the source tree that started it, then checks that no container, volume or
network with that project label remains. Only then does it delete the work dir
and its 0600 env files. A failed `down` or a leftover fails the run and keeps
the work dir. On any other failure after a project started, it keeps the
projects and the work dir for diagnosis and prints the cleanup commands. The
evidence dir holds logs with the generated secrets redacted. It is kept on
success and on failure, including build failures. It defaults to a new 0700
directory under `TMPDIR`, and `--evidence-dir` overrides it. Generated secrets
reach the redactor through its environment, not argv. The wrong key of the
negative control does appear in a `docker compose run -e` argument, and the
fixed test login appears in curl arguments. Use a single-user host.

A run proves only what it ran: one old/new pair, the host architecture, the
selected storage, and one injected failure (a pre-created table of the newest
migration, not an interrupted migrate or a crash). It does not compare search
indexes or doctor output with the old install. The S3 mode proves that restore
works from versions of the same local silo bucket. It does not cover
replication, a second region, a cloud provider's versioning or backup service,
lifecycle rules, or the presigned direct mode, which is not implemented.
`--verify-storage` compares attachment sizes only, so a same-size overwrite is
not detected before start. Ordinary PR and main CI does not run this smoke. A manual dispatch of the
Container install workflow with `run_upgrade_smoke_arm=true`
(`gh workflow run install.yml --ref <branch> -f run_upgrade_smoke_arm=true`) runs it once on native
`ubuntu-24.04-arm` with local storage, for the fixed pair in the `upgrade-smoke-arm64` job and the
tested commit as `--main-ref`. The job being registered is not a result. Record the pair, image IDs,
architecture and logs of a run with the change it supports; this guide does not.

### Release images (0.x)

Trial releases are published by `.github/workflows/release.yml` as
`ghcr.io/aisflow/fvoci:0.y.z` (linux/amd64 and linux/arm64) with a GitHub
pre-release holding `compose.yml` pinned to the image digest, `env.example`,
`INSTALL.md`, `release.json` and `SHA256SUMS`; maintainer steps are in
`docs/RELEASING.md`. Nothing updates an install on its own. To move a release
install to a newer 0.y.z, back it up, check the new release's `SHA256SUMS`,
replace `compose.yml` in the same directory (same Compose project name, so the
same volumes; keep `.env`) and run `docker compose up -d --wait --wait-timeout 900`
(a long migration such as 043 can outlast the healthcheck's two minutes; if
`--wait` still gives up, the preparation keeps going: follow
`docker compose logs -f fvoci` until `prepared; starting the server`). Compose
recreates `fvoci`, so the old server has stopped before the new container
migrates. From a release whose `compose.yml` passed the passwords and keyrings
as Compose secret files (0.1.x), the same steps apply: the new file reads the
same `.env`, Compose recreates all three containers on the same volumes, and
those files existed only inside the old containers. Stopping it during a migration is safe (that migration rolls back),
but the next start waits until PostgreSQL has ended the interrupted statement; the preparation refuses to migrate while any other server still has
app-role sessions open, and a failure leaves the server stopped as described
above. 0.x releases make no compatibility promise between minor versions and
there is no downgrade: going back means restoring the pre-upgrade backup.
`docker compose down -v` deletes the data; the keys stay in `.env`.
`fvoci-server --version` (for example
`docker compose exec fvoci /opt/fvoci/bin/fvoci-server --version`) prints the
version and source commit.

## Backup and restore

This is the logical backup for both Compose stacks above (PostgreSQL +
attachment storage). It is not a stopped-stack copy
of every volume, and it is not PITR.

### Install from compose.yml and .env

`scripts/backup.sh` and `scripts/restore.sh` take the user install like the
developer stack: its `.env` is the env file, and the app service is found as
the one publishing port 8080. Run them from a checkout of the same release:

```sh
scripts/backup.sh --project fvoci --env-file /path/to/.env --compose-file /path/to/compose.yml --output /backups/fvoci-1
scripts/restore.sh --project fvoci-restored --env-file /path/to/.env --compose-file /path/to/compose.yml --input /backups/fvoci-1
```

The keys stay in `.env` and are not copied into the backup; keep a copy of
`.env` with it. Without an init service, restore runs the preparation with
`fvoci-migrate --prepare` and the owner commands in the `fvoci` service. As
with every restore the target is a new project name; run it with
`docker compose -p fvoci-restored …` (or change `name:`).

`restore.sh` takes the keyrings from `docker compose config` (what the server
will get) and reads the app role and its password from the env file the way
Compose does for these forms: `KEY=value`, `KEY='value'` and `KEY="value"`
(the whole value in one pair of quotes, with no `\`, `$` or inner quote of the
same kind inside double quotes). It refuses anything whose Compose meaning
could differ from the text (escapes, `$` interpolation, an inline `#` comment,
spaces, `export`, a key set twice) instead of guessing. The shipped
`env.example` values are unquoted. `bash scripts/test-restore-env.sh` checks
these cases and, when `docker compose` is available, compares the accepted
ones with Compose's own parse.

The remaining examples in this section use the developer stack
(`infra/rust/compose.yml`, project `fvoci-rust-install`); for the user install
add `--compose-file` as above.

Run `scripts/backup.sh` and `scripts/restore.sh` on the operator's Linux host
with Bash, Docker Compose, jq, GNU coreutils and tar. The scripts check their
principal host tools before changing the stack. The selected installed product
image runs `fvoci-migrate --backup-manifest` and the offline
`--restore-preflight` with only the key environment, a read-only backup mount
for restore, and no network. PostgreSQL dump/restore, storage archiving and
the Rust `--verify-storage`/`--verify-secrets` probes run in the specified
Compose containers. Python is not required for the operational scripts;
independent Python compatibility fixtures remain available for testing.

**Included:** a custom-format `pg_dump` of schemas `public` (RLS helper
functions) and `fvoci`, taken as the PostgreSQL owner role through the
`postgres` service, plus a `tar` of the `storage` volume. **Omitted:** Meilisearch (`searchdata`), the scoped API key
volume, Compose env files, pepper keys, `ENCRYPTION_KEYS`, and database passwords. The search
index is derived. Restore runs `fvoci-migrate --ensure-meili-key` (new scoped
key, index settings), then `fvoci-migrate --recover-outbox` (rebases outbox
cursors to the new cluster's xids before any server starts) and
`fvoci-migrate --rebuild-search` (reindexes from PostgreSQL, including
attachment text chunks). Keep `PASSWORD_PEPPER_KEYS` / `PASSWORD_PEPPER_ACTIVE_KEY_ID` the same as
the original or existing passwords will not verify. `POSTGRES_USER`,
`POSTGRES_DB`, and `FVOCI_APP_ROLE` names must match; cluster passwords and
`MEILI_MASTER_KEY` may be new. `scripts/restore.sh` compares the keyring fingerprint recorded in the backup manifest and refuses to restore with a different keyring.

`ENCRYPTION_KEYS` seals TOTP secrets, workspace SSO client secrets, webhook
signing secrets and the Web Push VAPID private key in the dump. The manifest's `encryptionKeys` entry records, per
key id, `HMAC-SHA256(key, label || id)` (and a whole-keyring SHA-256 like the
pepper's), never the keys. Before any volume is
created, restore requires every backed-up key id with the same key; extra keys
and another active id (a rotation done since the backup) are accepted, a
missing or changed key id is refused. A backup made without `ENCRYPTION_KEYS`
records `configured: false`; an older manifest without the entry skips this
comparison. Either way the decrypt probe below decides.

**Ordering:** `scripts/backup.sh` stops the server (the only writer) and checks
that no other client sessions remain, then dumps PostgreSQL, then archives
storage. Stored attachment keys in the dump must exist as
`objects/<key>/payload` in the tar, so restored files cover every database
reference. Archives are created with directory mode `0700` and file mode
`0600`. Published image previews (`variants.preview.key`) are never
regenerated, so they must be in the tar too. The dump contains whatever the
database already stored (including password hashes and sealed secrets); the
archive does not add the env file, the keyrings or the Meili master key.

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
application role, restores the dump, restores storage, then runs the
preparation (`fvoci-migrate`, `--grant-app-role`, `--ensure-meili-key`; all
idempotent on this path): the developer stack's one-shot `init` job, or
`fvoci-migrate --prepare` in the user install's `fvoci` service, rebases the outbox, rebuilds search, and runs
`fvoci-migrate --verify-storage` with the server's own environment: every
`stored` attachment and its published preview in the restored database must
exist in the configured storage with their recorded sizes (a missing preview
fails: the product does not regenerate a published one), and every branding
asset (logo/favicon) the restored instance settings reference must exist with
its recorded SHA-256. It then runs `fvoci-migrate --verify-secrets`, also with
the server's environment (`DATABASE_APP_URL`, `ENCRYPTION_KEYS`; the app role
reads the ciphertext columns in the system context, no owner URL): every
sealed value in `user_mfa`, `workspace_oidc` and `webhooks`, and the VAPID
private key (`vapid`, context `vapid:1`), must open with its row's context. It prints counts, failing row ids and the key ids in use, never
secret values, and exits nonzero on any failure. OIDC flow states are not
opened (single-use, expired ten minutes after issue, fail closed per sign-in).
Either failure stops the restore before the server starts.
Then it starts the server. Confirm login with the original password, document
body, attachment bytes, extraction text, and tasks.

Links created before migration 035 can have `issuer IS NULL`. With migration
038 these links remain unchanged and sign-in is refused: a new token cannot
establish their historical issuer. An already authenticated account holder can
unlink and reconnect through the normal account flow. An account holder with
no other trusted sign-in method needs verified account recovery; do not infer
ownership from the new token's email or subject. Review affected links with
`SELECT id, provider, user_id FROM fvoci.identity_links WHERE issuer IS NULL`.

Upgrading to migration 036 (Microsoft tenant issuer): Microsoft
`common`/`organizations`/`consumers` sign-ins now record the tenant issuer the
id_token was verified against (the discovery template with the token's `tid`,
which must be a GUID), so a link is pinned to one tenant and another tenant's
same `sub` is refused. Earlier links with a NULL issuer or a literal template
(`https://login.microsoftonline.com/{tenantid}/v2.0`) fail closed with
`oidc_not_linked` and remain unchanged. The account holder can use an existing
authenticated session to unlink and reconnect, which stores the verified issuer.
Review template links with `SELECT id, user_id FROM fvoci.identity_links WHERE provider =
'microsoft' AND issuer LIKE '%{tenantid}%'`. A JWKS key that names an
`issuer` (Microsoft's common key set does) only verifies id_tokens from that
issuer or, for a `{tenantid}` template, from a tenant of it.

OIDC providers are read with the `openidconnect` crate (4.0.1) over the
server's own guarded fetch (https only, public addresses, no redirects or
proxy, 10 s and 256 KiB per request; `OIDC_ALLOW_INSECURE=1` admits plain
http to loopback only). A provider must publish the discovery fields OpenID
Discovery requires (`issuer`, `authorization_endpoint`, `token_endpoint`,
`jwks_uri`, `response_types_supported`, `subject_types_supported`,
`id_token_signing_alg_values_supported`). id_tokens must be RS256 (keys with
a 2048 to 4096 bit modulus; shorter keys are dropped from the key set) or ES256, carry a `kid` when the key set has more than one
eligible key, name only this client in `aud`, and send `email_verified` as a
JSON boolean; anything else fails the sign-in with `oidc_provider_error`.

Redirect URIs to register at the provider: an instance provider
(`OIDC_<KEY>_*`) uses `<FVOCI_PUBLIC_ORIGIN>/api/v1/auth/oidc/<key>/callback`
(`google`, `microsoft`, `kakao`, `naver`, `generic`). A workspace SSO provider
(enterprise `workspaceSso`) uses its own
`<FVOCI_PUBLIC_ORIGIN>/api/v1/auth/sso/<workspace id>/callback`, where the
workspace id is the `id` from `GET /api/v1/me/workspaces`. A callback only
completes a sign-in or link started for the same workspace (the instance path
only instance flows), so a response from one provider cannot finish another
provider's flow; anything else ends with `oidc_state_mismatch` before a token
request is made. **Upgrading:** a workspace SSO configuration made before this
release was registered with the instance `/api/v1/auth/oidc/generic/callback`
URI. Add the workspace URI at that provider (the provider refuses an
unregistered redirect URI) and remove the old one once no instance `generic`
provider shares that client. Sign-ins started before the upgrade fail once
with `oidc_state_mismatch`. Instance providers keep their URIs.

`scripts/backup-restore-smoke.sh` builds the install image, seeds an isolated
source project (setup/login, wiki collab body, HWPX upload and extraction,
project/task, a document comment, an MFA secret sealed with `ENCRYPTION_KEYS`),
backs it up, checks a restore with a different pepper and one with a different
key under the backed-up `ENCRYPTION_KEYS` id are refused before any volume
exists, restores into a second project with a rotated superset keyring (the
secret opens), and checks those artifacts
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
   dump is quiesced). Record the UTC time, in whole seconds, at which the dump
   finished; restore needs it. Objects deleted by workspace purge after the dump
   are recoverable only from bucket versions.
3. **Restore:** `scripts/restore.sh` needs a volume archive and a manifest, so
   run its database steps by hand into a **fresh** Compose project. Use the
   image that took the dump (`FVOCI_IMAGE` in the env file set to that tag), and
   run the commands below from the checkout that built it so `$C` uses its
   Compose files. After a failed upgrade this is the old image and checkout;
   never start it on the migrated database.

   Only one install may use a bucket. Its background sweeps (abandoned-upload
   cleanup, trashed-document and workspace purge) delete bucket objects based on
   its own database, so two installs whose databases diverged delete objects
   the other still references, silently and after `--verify-storage` has
   passed. Before starting the restored server, stop every other install that
   uses these `S3_*` settings (the source, or a failed or migrated upgrade), and
   never start that install again with them. A restore drill must use an
   independent replica or copy of the bucket, never the live one, and must not
   hold production integration credentials: `--ack-external-replay` below
   re-sends external events. You need these inputs:
   - the dump;
   - `<dump-utc>`: the UTC time recorded when the dump finished, truncated to
     the second (`date -u +%Y-%m-%dT%H:%M:%SZ`). There is no manifest to
     recover it from. If it was not recorded, stop instead of guessing;
   - an env file with the original `POSTGRES_USER`, `POSTGRES_DB` and
     `FVOCI_APP_ROLE` names;
   - the same `PASSWORD_PEPPER_KEYS` / `PASSWORD_PEPPER_ACTIVE_KEY_ID`;
   - `ENCRYPTION_KEYS` with every original key id unchanged (a superset is
     fine). Nothing compares a fingerprint here; only `--verify-secrets` below
     checks the keyring;
   - `S3_*` pointing at the bucket (or the replica), used by no other install.

   Then, with `C="docker compose -f infra/rust/compose.yml -f infra/rust/compose.s3.yml --project-name <new-project> --env-file <env>"`:

   1. Run `$C up -d --wait postgres meilisearch`. Then, as `scripts/restore.sh`
      does:
      - confirm the database is empty (no user relations outside
        `pg_catalog`/`information_schema`); stop if it is not;
      - create `FVOCI_APP_ROLE` (`LOGIN`, `NOSUPERUSER`, `NOBYPASSRLS`, password
        `FVOCI_APP_PASSWORD`);
      - copy the dump into the postgres container;
      - run `pg_restore --exit-on-error --single-transaction --no-owner` with a
        `--use-list` that drops the `SCHEMA - public` entry.
   2. Run `$C run --rm init`. This does migrate, `--grant-app-role` and
      `--ensure-meili-key`. Only the `init` service receives the owner
      `DATABASE_URL`. (These commands name the developer stack's `init` and
      `server` services. With the user compose, run
      `$C run --rm --no-deps --entrypoint /opt/fvoci/bin/fvoci-migrate fvoci --prepare`
      here and use `fvoci` in place of both `init` and `server` below.)
   3. Rebase the outbox and rebuild search with the same bounds `restore.sh`
      derives from its manifest. Set `<snapshot>` = `<dump-utc>` + 1 s, and
      `<since>` = `<snapshot>` − 29 days (the widest window
      `--recover-outbox` accepts):

      ```sh
      $C run --rm --no-deps --entrypoint /opt/fvoci/bin/fvoci-migrate init \
        --recover-outbox --since <since> --snapshot-at <snapshot> \
        --apply --reason "restore into <new-project>" --ack-external-replay
      $C run --rm --no-deps --entrypoint /opt/fvoci/bin/fvoci-migrate init --rebuild-search
      ```

   4. Before any server starts, run the storage check and then the secrets
      check. Both use the server's environment (app role only, no owner URL):

      ```sh
      $C run --rm --no-deps --entrypoint /opt/fvoci/bin/fvoci-migrate server --verify-storage
      $C run --rm --no-deps --entrypoint /opt/fvoci/bin/fvoci-migrate server --verify-secrets
      ```

   5. Start the server with `$C up -d --wait server`, and only after both
      checks pass.

   `--verify-storage` prints `{"checked":N,"missing":[...],"sizeMismatch":[...],"previewChecked":P,"previewMissing":[...],"previewSizeMismatch":[...],"brandingChecked":M,"brandingMissing":[...],"brandingMismatch":[...]}`
   and exits non-zero when any stored attachment or published preview is
   missing or has a different size, when a branding asset referenced by the
   instance settings (`logo`/`favicon`, uploaded in the admin console) is
   missing or does not match its recorded SHA-256, or when the bucket cannot be read (credentials,
   wrong bucket, network). Restore the listed objects from bucket versions
   before starting the server. Branding assets are stored like attachments
   (same driver, key from the setting), so the local volume archive and the S3
   bucket protection above cover them too.

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

## Operator commands (`fvoci-migrate`)

The source's `fvoci <command>` CLI maps onto `fvoci-migrate`, the one-shot
operator binary shipped in the image. It is the user install's entrypoint
(`--start`: prepare, then exec the server), the developer stack's `init` job,
and what backup and restore run (the server binary stays single-purpose):

| Source | Rust | Environment |
| --- | --- | --- |
| `fvoci init` | `fvoci-migrate --init-env --public-origin <url> --out <path> [--yes]` | none |
| `fvoci doctor` | `fvoci-migrate --doctor` | the server's |
| `fvoci bootstrap` (migrate) | `fvoci-migrate`, then `--grant-app-role <role>` | owner `DATABASE_URL` |
| `fvoci search-rebuild [workspaceId]` | `fvoci-migrate --rebuild-search [workspace-id]` | owner `DATABASE_URL`, Meili |
| `fvoci outbox-recover` | `fvoci-migrate --recover-outbox ...` | owner `DATABASE_URL` |
| `fvoci outbox-reset [--override-reason=...]` | `fvoci-migrate --outbox-reset [--consumer <name>]... [--apply --reason <text> [--override-reason <text>] [--ack-external-replay]]` | owner `DATABASE_URL` (see below) |
| `fvoci backup <collect\|restore\|...>` | `scripts/backup.sh`, `scripts/restore.sh` (below) | Compose project |
| — (restore check) | `fvoci-migrate --verify-storage` | the server's |
| `fvoci secrets rotate-vapid` | `fvoci-migrate --rotate-vapid` | the server's (`DATABASE_APP_URL`, `ENCRYPTION_KEYS`) |
| `fvoci secrets audit` | `fvoci-migrate --secrets-audit` | the server's (`DATABASE_APP_URL`, `ENCRYPTION_KEYS`, `PASSWORD_PEPPER_KEYS`) |
| `fvoci secrets rotate` | `fvoci-migrate --secrets-rotate` | the server's (`DATABASE_APP_URL`, `ENCRYPTION_KEYS`) |

`fvoci healthcheck` is `fvoci-server healthcheck` (see "Probes"; it probes the
server, not a `fvoci-migrate` mode).

Not ported: `reindex` (extract re-enqueue) and the split worker roles (`worker`,
`compact`, `thumbnail`, `collab`) with their `healthcheck <role>` heartbeat
checks; the Rust server runs those jobs in-process.

**`--secrets-audit` / `--secrets-rotate`** (source `fvoci secrets audit|rotate`)
run as the app role in the system context, like the server; they refuse a
superuser, `BYPASSRLS` or schema-owner URL and a schema that is not current.
Both walk the webhook signing secrets, workspace SSO client secrets, TOTP
secrets and the VAPID private key, 100 rows per transaction, with each value's
row-bound AAD. Output is one JSON line of key ids and counts; secret values,
password hashes and key material are never printed.

- `--secrets-audit` prints `secrets` (`<class>:<key id>` → count, `invalid` for
  a malformed value), `passwords` (pepper key id → count, `unknown` for a hash
  in no known format), `problems`, `activeKeyId`, `notActive` (values that
  open but are not under the active key), `missingKeyIds` and
  `missingPasswordKeyIds`. It exits 1 when `problems` is nonzero: a value that
  does not open (missing key id, wrong key, corrupted or moved value) or a
  password hash whose pepper key is missing or whose format is invalid.
- `--secrets-rotate` re-seals every value not under `ENCRYPTION_ACTIVE_KEY_ID`
  and prints `{"changed":n,"unchanged":m}`. It first opens every value and
  refuses before writing anything if one does not open (the source re-seals
  earlier batches and then stops). Each write is a compare-and-set on the value
  it read (the VAPID key through `app_replace_vapid_private`), so a concurrent
  change fails the command with a conflict instead of being overwritten;
  committed batches are valid, and re-running finishes the rest. A second run
  reports `changed: 0`. Password hashes are re-peppered at sign-in, not here.

Key rotation: add the new key to `ENCRYPTION_KEYS`, switch
`ENCRYPTION_ACTIVE_KEY_ID`, restart the server, run `--secrets-rotate`, then
`--secrets-audit`; drop the old key only once `secrets` no longer names it.

```sh
docker compose -f infra/rust/compose.yml --env-file infra/rust/.env \
  run --rm --no-deps --entrypoint /opt/fvoci/bin/fvoci-migrate server --secrets-audit
```

**`--outbox-reset`** (source `fvoci outbox-reset`) puts outbox consumer
cursors back at the point their `processed_events` marks show, without
replaying everything or deleting anything. The source had one relay cursor and
moved it to just before the first event of the last 29 days that the
`notifications` consumer had not marked; the Rust server keeps one cursor per
consumer, so the same rule runs per consumer against that consumer's own marks
(no mark in the window: the newest event of the window; no events in the
window: unchanged). Use it when a cursor was hand-edited, lost or moved past
events that were never delivered on the same cluster. After a restore, or when
a consumer reports an outbox xid epoch mismatch, use `--recover-outbox`
instead; `--outbox-reset` refuses an epoch mismatch.

- Without `--apply` it only diagnoses: a read-only transaction that is safe
  while the server runs. It prints one JSON line: `mode`, `windowDays` (29),
  per consumer `before`, `target`, `direction` (`forward`, `backward`,
  `unchanged`), `leaseActive`, `externalEffects`, `redelivered` (unmarked
  events a backward move hands to the consumer again), `deadLettered` (unmarked
  events in the same range with a dead-letter failure row: the dispatcher passes
  them without delivery while that row stays, so they are not in `redelivered`),
  `externalReplay` (see below) and `skip` (unmarked events older than the
  window that a forward move passes: `skippedCount`, the `(xact, seq)` lexical
  `min`/`max`, `oldestCreatedAt`, up to 100 `sample` ids and verbs), and
  `excluded` with the reason for each consumer left out.
- The default set is every consumer of this build that marks each event it
  passes: `notifications`, `mail`, `push`, `webhooks`, and `search-index` when
  `FVOCI_MEILI_URL` is configured in the environment. `github` is left out
  because it does not mark events while the GitHub app is unconfigured (a reset
  would rewind it and replay up to 29 days of status changes once configured).
  `--consumer <name>` (repeatable) selects exactly the named cursors, `github`
  included.
- Consumers with external effects (`mail`, `push`, `webhooks`, `github`, and
  any name this build does not know) are not rewound past their replay floor:
  just before the first event they marked, and never behind their current cursor
  when they marked nothing. Migrations 027/040/041 seed such consumers at the
  tail on upgrade, so they hold no marks for older events; the per-consumer rule
  alone would move them back to the start of the window and send up to 29 days
  of pre-upgrade events to devices and external URLs again. When the rule asks
  for more than the floor, the consumer reports `externalReplay` with the
  `floor`, the rule's `target`, its `redelivered`/`deadLettered` counts and
  `acknowledged`. By default the move stops at the floor (events after the floor
  that are unmarked are still redelivered). `--apply --ack-external-replay`
  (the same flag as `--recover-outbox`) moves them to the rule's target and
  accepts that at-least-once external replay; the top-level `ackExternalReplay`
  records it. `notifications` writes only this database and `search-index`
  only re-indexes Meilisearch, which is idempotent, so both follow the rule
  without a floor.
- `--apply --reason <text>` moves the cursors in one transaction. It refuses
  unless the `DATABASE_URL` role is a superuser or has the privileges of
  `pg_read_all_stats` (membership through a `NOINHERIT` role or an
  `INHERIT FALSE` grant does not count; a plain schema owner cannot see other
  roles' sessions in `pg_stat_activity`, so the next check would pass
  blindly); while any other
  session is connected to the database (stop the server and every other client
  first); while a selected consumer holds a live lease; when a forward target is
  at or above the cluster snapshot xmin (a transaction in any database that may
  still commit an earlier event is running, including prepared transactions:
  retry after it ends); and when a move would skip unmarked events older than
  the window unless `--override-reason <text>` acknowledges them. Both reasons
  are echoed in the JSON report. Events, marks and failure rows are never
  deleted; a second run reports every consumer `unchanged`.
- `fvoci-migrate` installs no log subscriber, so the JSON line on stdout is the
  only record of an apply: keep it with the ticket.
- Unlike the source, which ran as the app role, this runs as the owner
  `DATABASE_URL` like `--recover-outbox`: the app role has no access to the
  consumer cursor tables by design (`scripts/grant-app-role.sql`), and this
  operator path does not widen it. Consumers that moved backward redeliver only
  events without their mark; delivery stays at-least-once for external effects.

```sh
docker compose -f infra/rust/compose.yml --env-file infra/rust/.env \
  run --rm --no-deps --entrypoint /opt/fvoci/bin/fvoci-migrate init --outbox-reset
docker compose -f infra/rust/compose.yml --env-file infra/rust/.env stop server
docker compose -f infra/rust/compose.yml --env-file infra/rust/.env \
  run --rm --no-deps --entrypoint /opt/fvoci/bin/fvoci-migrate init \
  --outbox-reset --apply --reason "cursor ahead of marks, ticket 123"
```

**`--init-env`** writes the Compose env file from `infra/rust/.env.example` with
fresh secrets: `POSTGRES_PASSWORD`, `FVOCI_APP_PASSWORD`, `MEILI_MASTER_KEY`
(64 hex), `PASSWORD_PEPPER_KEYS` and `ENCRYPTION_KEYS` (`{"install":"<64 hex>"}`),
`FVOCI_PUBLIC_ORIGIN`, and `FVOCI_COOKIE_SECURE=true` for an https origin (a
loopback `http://` origin also sets `FVOCI_PUBLISH_PORT` to its port). The file
is created mode 0600 and renamed into place; an existing file is kept unless
`--yes`. Only the path is printed. It replaces step 1 of "Developer stack (compose.yml with init)":

```sh
cargo run --release --bin fvoci-migrate -- --init-env \
  --public-origin https://fvoci.example.com --out infra/rust/.env
```

Back up the generated file with the database backups: the pepper and
encryption keys cannot be regenerated.

**`--doctor`** checks the server's environment without starting it and prints
`{"ok":true|false,"checks":[{"name","ok","detail"?}]}`; the exit code is 1 when
any check fails. Each setting is checked on its own so every problem is named:
`env` (the server's full config parse), `password_pepper_keys` and
`encryption_keys` (published development keys fail), `public_origin` (plain
http off loopback, or https without secure cookies, fail), `identity`,
`integrations`, `database` (connect with `DATABASE_APP_URL`), `app_role`
(no superuser/`BYPASSRLS`, not the schema owner), `schema_version` (migrated to
this build), `pg_connection_budget` (collab rooms + pool + reserve ≤
`max_connections`), `storage` (local directory or S3 bucket probe),
`meilisearch` (the scoped key reads its index), `smtp` (connect/EHLO/STARTTLS
when offered; no mail sent),
`collab_engine` (spawn and ping; a set path that is not a file fails because
the server would silently disable collaboration), `extractor`, and
`document_convert` (run a small Markdown→Tiptap/HTML conversion and MD, DOCX,
PDF, PPTX exports through the sibling `fvoci-server` binary). A missing,
nonexecutable, or wrong server binary or unavailable PDF font files fails this
check. It confirms basic converter readiness and output structure; independent
reader and renderer tests cover export quality. Optional
features that are unset report `disabled (...)`. Nothing is created, migrated or
sent, and database URLs in details are masked.

```sh
docker compose -f infra/rust/compose.yml --env-file infra/rust/.env \
  run --rm --no-deps --entrypoint /opt/fvoci/bin/fvoci-migrate server --doctor
```

### Web Push (VAPID)

Browser push needs `ENCRYPTION_KEYS` and a secure browser origin: service
workers and `PushManager` only run on `https://` origins (or `localhost`), so
production uses `FVOCI_PUBLIC_ORIGIN=https://…`. The same origin is the VAPID
`sub` claim push services use to contact the operator. The static root must
serve the unhashed `/sw.js` (the web build copies `apps/web/public/sw.js`).

- **Bootstrap.** On start the server generates the instance P-256 keypair if
  none exists and stores it in `instance_config` (public key plain, private key
  sealed with `ENCRYPTION_KEYS` under context `vapid:1`). Replicas starting
  together keep the first stored pair. Without `ENCRYPTION_KEYS` (or on any
  error) the server still starts, logs `push.skipped` / `vapid_keys_missing`,
  and `GET /api/v1/instance` answers `webPushPublicKey: null`, so the toggle in
  workspace notification settings shows "not available". Notifications created
  while keys are missing are not pushed later.
- **Delivery.** The `push` outbox consumer queues one `push_deliveries` row per
  recipient device (no endpoint, key or content) in the same transaction as
  its processed mark. An in-process sender claims up to 8 rows (30 s lease)
  and, in one short transaction, re-checks each row: the event's current
  recipients (membership, resource access, in-app preference), a user who is
  not deleted or suspended, and the subscription with the session that
  registered it still live. It commits, then posts outside any transaction
  (5 s timeout each) and acknowledges. Every device gets one attempt; 404/410
  remove the endpoint, other failures are logged with the endpoint origin
  only. A crash or failed acknowledgement repeats only that batch after the
  lease, after the same check (at least once per device).
- **Sessions and logout.** A subscription is bound to the session that last
  registered it; the web client re-binds it once per new session. Expired or
  revoked sessions (logout, password reset, revoke-all, suspension) no longer
  authorize sends. Logging out also deletes this browser's rows for that user
  in the logout transaction (the ending session's rows plus the endpoint the
  browser reports), never other accounts' rows or the user's other devices,
  and the browser then unsubscribes (best effort). A logout committed before
  a row's final check prevents that send; a send already past the check
  completes, and messages a push service already accepted can still be shown.
  Another account signing in on the same browser profile without a logout
  sees the toggle off; that account's subscription keeps delivering until its
  session ends or the new user enables push, which replaces the browser
  subscription.
- **Rotation.** `fvoci-migrate --rotate-vapid` (server environment, app role)
  stores a new keypair, deletes every browser subscription (push services
  reject old-key subscriptions with 401/403, which the sender does not clean
  up), and records `instance.vapid_rotated` `{revokedSubscriptions}` as an
  event and audit row, all in one transaction. It prints only
  `{"publicKey","revokedSubscriptions"}`. `/instance` shows the new key without
  a restart (up to its 60 s HTTP cache); browsers resubscribe when users open
  notification settings. This is not `ENCRYPTION_KEYS` rotation: adding a new
  active key id keeps the sealed VAPID key readable while the old id stays in
  the keyring.
- **Backup and restore.** The sealed private key is in the database dump;
  `--verify-secrets` opens it (reported as `vapid`, nil id). A restore without
  the original key id cannot open it: push stays off (logged per event) until
  the original `ENCRYPTION_KEYS` is restored, or `--rotate-vapid` issues a new
  pair, which revokes all subscriptions.

```sh
docker compose -f infra/rust/compose.yml --env-file infra/rust/.env \
  run --rm --no-deps --entrypoint /opt/fvoci/bin/fvoci-migrate server --rotate-vapid
```

## Product MCP server (`fvoci-mcp`)

`fvoci-mcp` is the source `apps/mcp` server as a Rust binary: an MCP server over
stdio (newline-delimited JSON-RPC 2.0) whose tools call the FVOCI HTTP API with a
personal API token. Tool names, descriptions and input schemas are the source's
(`src/bin/fvoci-mcp/tools.json`, generated from the source server at the pinned
SHA): `list_tasks`, `get_task`, `list_task_activity`, `create_task`, `patch_task`,
`add_comment`, `resolve_comment`, `list_labels`, `create_label`, `patch_label`,
`list_task_dependencies`, `add_task_dependency`, `remove_task_dependency`,
`search`, `get_document_body`, `put_document_body`, `patch_document_block`,
`get_calendar`, `ai_summarize_document`, `ai_generate_tasks`, `ai_suggest_links`.
There are no resources or prompts (as in the source).

```sh
cargo build --release --bin fvoci-mcp
FVOCI_URL=https://fvoci.example.com FVOCI_TOKEN=<personal API token> target/release/fvoci-mcp
```

MCP client configuration (for example):

```json
{ "mcpServers": { "fvoci": { "command": "/path/to/fvoci-mcp",
  "env": { "FVOCI_URL": "https://fvoci.example.com", "FVOCI_TOKEN": "<token>" } } } }
```

- Create the token in workspace settings (API tokens). The server enforces its
  scopes and workspace: `tasks.read`/`tasks.write` for task, label, dependency
  and task-comment tools, `documents.read`/`documents.write` for document body
  and document-comment tools. A call outside the token's scope or workspace
  returns the server's refusal as a tool error (`{"status":404,...}`), a revoked
  token `{"status":401,...}`.
- `FVOCI_URL` must be `https://`, or `http://` to `localhost`/`127.0.0.1`/`[::1]`.
  The token is read only from `FVOCI_TOKEN`, never printed, and never sent
  across a redirect (redirects are not followed). Messages over 16 MiB on stdin,
  API responses over 8 MiB and requests over 60 s are refused.
- Arguments are validated against the schema before any request (tool error
  `MCP error -32602: Input validation error: ...`).
- Not provided: the source's `--http <port>` streamable HTTP transport (the
  binary exits 1 on any argument). Server routes still missing for some tools
  return their HTTP error: PAT access to `search`, `get_document_body` with
  `format=md`, `put_document_body`, `patch_document_block`, and project-document
  bodies.

Tests: `tests/mcp_integration.rs` starts a real `fvoci-server` process (fresh
database, app role, port 0), mints tokens over HTTP and drives the binary over
stdio (`cargo test --features db-tests --test mcp_integration`).
