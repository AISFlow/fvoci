# Isolated Turso primary connection and migration consumers

The source consumers are prepared; Rust compilation and actual Turso execution are
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

The dedicated `turso-connection` job (historical job ID retained) prepares the maintained pinned SQLite/Rust
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
a confirmed vulnerability or an adopted transport redesign. The same-version
SDK patch below adds typed error inspection; upstream connection, transport and
parser behavior are unchanged.

Manual phase `migration` requires both dispatch `destructive=true` and
Environment variable `FVOCI_TEST_TURSO_ALLOW_DESTRUCTIVE=true`. ROOT alone owns
that flag lifecycle and the allocated isolated remote execution; the current
registered flag remains false. Bootstrap pushes cannot select migration.
The final consuming wrapper chooses exactly
`db::turso_test::turso_primary_current12_install_resume`, exports the exact four
cfg-test helper selection/phase/destructive flags, and requires one passed test
plus the strict prefix/FK-rollback/current/restart/close/lease receipt. It uses
the same source/ELF/native-input frozen qualification and credential filtering
as connection; fake fixture receipt tests are not remote runtime evidence.

This first migration slice is **not normal remote installation/startup support**.
Normal server and `fvoci-migrate` remote refusal remain unchanged. The ignored
manual consumer calls the maintained compiled SQLite registry and per-step
migration owner through cfg(test, db-tests) Fable-owned helpers. Initial exact
blank catalog admission occurs before DDL; unexpected/foreign/populated/current
targets refuse, without automatic reset, service deletion or schema overwrite.
The secret URL remains the designated target, not an invented host/DB-ID proof.
ROOT may separately prepare/reset the user-authorized disposable test DB after
concrete target checks; this consumer never performs that reset.

The consumer applies genuine steps01–11, commits a uniquely identified test
workspace and fence counter17, and verifies the complete-current gate refuses
that exact partial lineage. It executes actual compiled step12 DDL in the same
real reserved writer as an invalid task-fence INSERT, awaits original rollback,
and checks original prefix receipts/catalog/data remain unchanged. A remote
failure must be a specifically classified genuine FK rejection, not arbitrary
HTTP/transport failure. The retained libsql0.9.30 source has a narrowly scoped
FVOCI `Error::hrana_error_code()` accessor: it borrows the structured code from
upstream Hrana stream/cursor-step errors, preserving the original error and
message. The classifier accepts exact typed `SQLITE_CONSTRAINT_FOREIGNKEY` or
public numeric787; generic19/text mentions, transport and arbitrary/nested boxed
errors cannot satisfy the FK oracle. Original manifests/dependencies/features,
transport and parser files are unchanged. Upstream accessor-absence compile
control fails with E0599; the patched remote,tls library's five accessor controls
passed at ROOT353d. That is SDK-only evidence, not actual Turso migration or a
compile result for this wrapper composition. Provenance and full MIT notice are
in `vendor/libsql-0.9.30/`; see the pinned
[SDK error variants](https://github.com/tursodatabase/libsql/blob/0653c5788d77ef16a97c56ff3e9fdc11717a72d9/libsql/src/errors.rs)
and [module visibility](https://github.com/tursodatabase/libsql/blob/0653c5788d77ef16a97c56ff3e9fdc11717a72d9/libsql/src/lib.rs).

After original owner close, a fresh primary owner resumes current12, checks all
nine claim seeds and preserved data, and performs rollback-only gap/digest/
extra-object negative catalog checks with the maintained full comparator. It
advances key8 generation7 by acknowledged commit, closes/reconnects, reruns the
same maintained initializer, and compares complete raw receipt timestamps,
lineage/hash/schema, original data/counter and every seeded key/generation.
Only confirmed commits advance; remote uncertain commit/rollback stops without
fresh-observer reconciliation or blind retry. Successful close is product owner
drain/zero active leases, not an unexposed server Close ACK. The uniquely
identified test workspace and current schema remain retained for ROOT inspection.

Other phases (`crud`, `transactions`, `persistence`, `restore`, `ui-ack`) remain
**NOT IMPLEMENTED** and refuse before credential consumption. Current tenant
CRUD/authorization, request/version replay, real concurrent/cancel/uncertain
finish, normal remote setup, backup/restore, real UI persist ACK and fresh-client
history remain required separate product acceptance. Local SQLite or loopback
SDK results cannot be labeled actual Turso PASS. Constant per-database
concurrency remains `cancel-in-progress:false`; no service resource reset,
provider/account/secret/policy/permission change is made by the worker.

Pure local checks: `python3 scripts/selected-backend-ci/turso-test-fixtures.py`
and `bash scripts/test-ci-selection.sh`. These do not prove Rust compilation,
SDK/network execution or actual Turso PASS.
