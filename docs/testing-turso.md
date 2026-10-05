# Isolated Turso connection test

The source consumer is prepared; Rust compilation and actual Turso execution are
**NOT RUN**. Independent review and a separately allocated execution are still required.
Root may publish the reviewed fixed source to the single hardcoded branch
`fvoci/v060-turso-verified-connection`. Its push bootstrap runs **only pure
fixtures/source admission**, with no Environment, credentials, build or probe.
The secret job permits only manual dispatch on main or that exact same-repo
reviewed branch, checking exact github.sha. No free-form checkout input, PR,
fork, pull_request_target or other ref is allowed. GitHub documentation says
the workflow must be on the default branch for manual dispatch; bootstrap/API
eligibility on the reviewed branch is not yet verified. Root must record the
actual API acceptance or rejection, without an arbitrary-ref/merge fallback. [GitHub manual dispatch](https://docs.github.com/en/actions/reference/workflows-and-actions/events-that-trigger-workflows#workflow_dispatch).

Use the user-designated isolated test database. The dedicated Environment is
**AISFlow/fvoci → Settings → Environments → `fvoci-turso-test`**. Register only:

- Environment secret `FVOCI_TEST_TURSO_DATABASE_URL`: that database's primary
  `libsql://` or `https://` URL on `*.turso.io`; no userinfo, query, fragment,
  custom port, replica, sync, local fallback or production target.
- Environment secret `FVOCI_TEST_TURSO_AUTH_TOKEN`: one short-expiry **read/write
  token scoped to that database alone**, supporting the authorized isolated
  schema/data CRUD and DDL. Do not select `--read-only`, which would require
  replacement for later tests. Never use an organization/account/admin/platform
  API or billing credential; do not delete or recreate the service database.
  [Turso database tokens](https://docs.turso.tech/cli/db/tokens/create).

The current registration metadata reports that the Environment and these two
secret names exist, with only `FVOCI_TEST_TURSO_ALLOW_DESTRUCTIVE=false` as an
Environment variable. No values were inspected or verified by this author.
**Expected-host and database-name/ID variables or dispatch inputs are not
required.** The secret URL is the designated target; URL shape validation is
neither server identity proof nor permission to initialize/reset anything.

Current Environment `deployment_branch_policy` is null and is allowed. The
secret job enforces AISFlow/fvoci + workflow_dispatch + main or the single
root-reviewed branch, and exact github.sha with credentials persistence disabled.
Bootstrap pushes cannot enter that job. The admission job
first verifies the preexisting named Environment through an anonymous public
GitHub metadata GET. It does not create or modify an Environment or policy;
404, denied/rate-limited access, malformed metadata or redirects fail before the
Environment job. Public metadata reads need no organization/admin token;
private authenticated equivalents need Actions read access. [GitHub Environment API](https://docs.github.com/en/rest/deployments/environments#get-an-environment).

The dedicated `turso-connection` job prepares the maintained pinned SQLite/Rust
inputs and compiles the current library tests **without credentials**. It
freezes the actual Cargo-emitted test ELF and binds source SHA/digest, emitted
profile/features, binary hash and native preparation receipt. Only its final runtime step maps the two registered secrets to product
`FVOCI_DATABASE_BACKEND=libsql-remote`, `FVOCI_LIBSQL_URL`, and
`FVOCI_LIBSQL_AUTH_TOKEN`; the fixture uses `DatabaseSettings::from_env` and the
existing RemoteDatabase constructor, rather than a test-only configuration path. No raw SDK/test error body, URL, header,
token, environment dump or credential-bearing trace is printed or uploaded.

Default phase `connection` requires `destructive=false` and makes no schema/data
mutation even if ALLOW_DESTRUCTIVE is missing or true. It explicitly executes
`db::turso_test::turso_primary_connection` with `--ignored --exact
--test-threads=1`; the wrapper requires **1 passed, 0 failed, 0 ignored**, never
zero-test success. The new test is ignored in ordinary automatic suites so they
never contact Turso; explicit selection with missing credentials fails.

The fixture uses the pinned libsql0.9.30 remote/TLS product SDK through
`RemoteDatabase::connect` and `Backend::LibsqlRemote::begin_read`. It reads
`PRAGMA foreign_keys=1` and literal integer/text values on the same borrowed
stream, then awaits the original transaction rollback even after read failure
and awaits `Backend::close`. Fixed receipts distinguish primary, rollback,
owner-close and lease-accounting outcomes. Owner drain is not proof of a server
Close ACK: the SDK's Drop Close receipt is not exposed. No abort/timeout/drop is
claimed to perform cooperative cleanup.

The maintained SDK accepts a trusted server's Hrana `base_url` for subsequent
requests. Initial URL validation does not prove follow-up endpoint behavior.
This remains a specific maintained-SDK/trusted-primary assessment boundary, not
a confirmed vulnerability or an adopted transport redesign. No SDK, dependency,
product connection or transport framework was changed.

All later phases remain **NOT IMPLEMENTED** and fail before credential
consumption. They must cover current tenant authority/CRUD, receipt/version
retry, concurrent/cancel/original-stream rollback and COMMIT reply loss with
fresh-connection reconciliation, current migrations, persistence/restart,
restore, UI persist ACK/revisions and fresh-client readback. ALLOW_DESTRUCTIVE
must become explicitly true only for a future approved in-DB mutating phase,
with its dispatch confirmation; it can never reset the connection phase.
Before mutations, verify real target metadata and an isolated owned marker,
reject foreign/mixed data, namespace by run ID/attempt, and clean only that run's
objects. Keep primary and cleanup failures separate; unknown mutation outcome
cannot cause blind retry/reset. Constant per-database concurrency uses
`cancel-in-progress:false`; no service database resource deletion/recreation.

Pure local checks: `python3 scripts/selected-backend-ci/turso-test-fixtures.py`
and `bash scripts/test-ci-selection.sh`. These do not prove Rust compilation,
SDK/network execution or actual Turso PASS.
