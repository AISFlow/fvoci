# Isolated Turso test preparation

This manual path is **source preparation only**. Every runtime phase currently
fails with `NOT_IMPLEMENTED` before build, Turso network access, or credential
consumption. No connection, CRUD, migration, persistence, restore, or UI test has
passed. The shared CI workflow registry and actual primary driver remain
root-owned integration work. There is no secret-consuming job yet.

Use only a designated isolated test database. In **AISFlow/fvoci → Settings →
Environments → New environment**, manually create `fvoci-turso-test`; select
deployment branches/tags with the single **Branch `main`** rule. Configure any
required reviewer yourself. Do not use a wildcard or tag rule. Environment
secrets become available only to jobs referencing that Environment after its
protection rules pass. A workflow referencing an absent Environment can create
an unprotected one, so the eventual driver must check existence and the exact
main-only policy before that job starts. See [GitHub's Environment guide](https://docs.github.com/en/actions/how-tos/deploy/configure-and-manage-deployments/manage-environments).

Register these two **Environment secrets** yourself, without posting values to
chat, an issue, a log, or an artifact:

- `FVOCI_TEST_TURSO_DATABASE_URL`: the isolated database's primary `libsql://`
  or `https://` URL; no userinfo, query, fragment, custom port, replica, sync,
  local fallback, or production endpoint. The preparation guard restricts hosts
  to `*.turso.io`; another host requires explicit reviewed admission.
- `FVOCI_TEST_TURSO_AUTH_TOKEN`: one short-expiry **read/write token scoped to
  that database alone**, supporting the authorized isolated schema/data CRUD
  and DDL. Do not select `--read-only`, which would require replacement for
  those later tests. The initial connection phase still performs no destructive
  data/schema operation. Never use an organization/account/admin/platform API
  or billing token; do not delete or recreate the service database. See
  [Turso database tokens](https://docs.turso.tech/cli/db/tokens/create).

Set these nonsecret **Environment variables** and confirm the same host/ID in
the dispatch inputs:

- `FVOCI_TEST_TURSO_EXPECTED_HOST`: exact lowercase isolated DB hostname.
- `FVOCI_TEST_TURSO_DATABASE_ID`: exact isolated DB identifier.
- `FVOCI_TEST_TURSO_ALLOW_DESTRUCTIVE`: keep `false` initially. A later fixed
  mutating phase needs this value `true` **and** the dispatch boolean
  `destructive=true`; connection requires `destructive=false`.

Registration may proceed while the driver and registry work remain pending.
Secret-name presence can be confirmed through Settings without displaying
values; registration does not make the missing consumer ready. The current
empty Environment inventory proves no registration or readiness. Prepared
source is not a dispatchable default-branch workflow:
`workflow_dispatch` requires the workflow on the default branch. Only reviewed
trusted-main adoption can enable this path; never run an unreviewed PR/fork or
arbitrary ref with secrets. See [GitHub manual dispatch](https://docs.github.com/en/actions/reference/workflows-and-actions/events-that-trigger-workflows#workflow_dispatch).

## Required real driver before credentials are consumed

Use the product's pinned `libsql = 0.9.30` remote/TLS SDK and actual
`RemoteDatabase::connect` / `Backend::LibsqlRemote` transaction path. First prove
connectivity, literal readback, and `PRAGMA foreign_keys=1` on the **same stream**
with awaited rollback/closure. `Backend::ping` alone is insufficient for that
oracle. There is no existing real Turso primary harness; the local transport
fixture is not Turso. The remote migrator also currently refuses admission.

Initial URL/host validation is configuration admission, not proof of a remote
database's identity or every later SDK endpoint. The maintained SDK accepts a
server-provided Hrana `base_url` for subsequent requests; the product connection
does not expose a connector override. This is an execution boundary needing a
specific maintained-SDK/trusted-server assessment before consuming credentials,
not a confirmed vulnerability or an adopted transport redesign.

Before any mutations, verify real target metadata and an isolated ownership
marker; reject foreign/mixed data without initializing it. Use a namespace
bound to repository/run ID/run attempt and clean only the current run's objects,
even after primary failure. Keep primary and cleanup failures separate; unknown
COMMIT or mutation outcome must not trigger a blind retry/reset. The fixed
Environment-wide concurrency group uses `cancel-in-progress: false` to
serialize all runs against the same database. No other execution's cleanup or
service resource deletion is authorized.

Mandatory later phases remain explicit: CRUD/current tenant authority;
version/command receipt retry; concurrency/cancellation and original-stream
rollback/COMMIT reply loss with fresh connection reconciliation; current
migrations; persistence/restart; current restore; UI persist ACK/revisions and
fresh-client readback. Missing phases fail closed rather than passing no-ops.
Only a reviewed dedicated Environment job may bind the two secrets to its
single consuming step after credential-free compilation. Never emit raw SDK
errors, URLs, headers, tokens, environment dumps, or credential-bearing traces.

Pure checks now: `python3 scripts/selected-backend-ci/turso-test-fixtures.py`.
Rust compilation, real SDK/DB operations, remote Actions, and browser/runtime
qualification are **NOT RUN** and require separate allocation and review.
