# Isolated Turso primary connection and migration consumers

This document describes the current source boundaries and isolated test consumers.
The acceptance record for Rust compilation and actual Turso execution is the
[current checkpoint in PR #347](https://github.com/AISFlow/fvoci/pull/347)
(`<!-- fvoci-current-checkpoint -->`) and [#331](https://github.com/AISFlow/fvoci/issues/331).
Local checks and historical executions do not establish acceptance for a new head;
independent review and separately allocated remote execution remain required.
The integrator (통합) may publish the reviewed fixed source to the single hardcoded branch
`fvoci/v060-turso-verified-connection` under the same review and approval
rules as the PR branch in AGENTS.md. Its push bootstrap runs **only pure
fixtures/source admission**, with no Environment, credentials, build or probe.
The secret connection job permits only manual dispatch on main or that exact
same-repo reviewed branch, checking exact github.sha. Manual dispatch of
`ui-baseline` or `ui-ack` also admits the #347 branch
`fvoci/v060-product-integration-20261005`. No free-form checkout input, PR,
fork, pull_request_target or other ref is allowed. GitHub documents the default-
branch requirement for manual dispatch. The evidence keeper (증거기록) records
each exact checkout and API acceptance/rejection in the acceptance record,
without an arbitrary-ref/merge fallback.
[GitHub manual dispatch](https://docs.github.com/en/actions/reference/workflows-and-actions/events-that-trigger-workflows#workflow_dispatch).

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

Environment `fvoci-turso-test` deployment branches are limited to `main`,
`fvoci/v060-turso-verified-connection`, and
`fvoci/v060-product-integration-20261005`. The secret job enforces
AISFlow/fvoci + workflow_dispatch + main or the single reviewed branch, and
exact github.sha with credentials persistence disabled. `ui-baseline` and
`ui-ack` may also be dispatched on `fvoci/v060-product-integration-20261005`.
Bootstrap pushes cannot enter that job. The admission job
first verifies the preexisting named Environment through an anonymous public
GitHub metadata GET. It does not create or modify an Environment or policy;
404, denied/rate-limited access, malformed metadata or redirects fail before the
Environment job. Public metadata reads need no organization/admin token;
private authenticated equivalents need Actions read access. [GitHub Environment API](https://docs.github.com/en/rest/deployments/environments#get-an-environment).

The dedicated `turso-connection` job (historical job ID retained) prepares the maintained pinned SQLite/Rust
inputs and compiles the current library tests **without credentials**. It
freezes the actual Cargo-emitted test ELF and binds source SHA/digest, emitted
profile/features, binary hash and native preparation receipt. Its final runtime step maps the two registered secrets to
`FVOCI_DATABASE_BACKEND=libsql-remote`, `FVOCI_TEST_TURSO_DATABASE_URL`, and
`FVOCI_TEST_TURSO_AUTH_TOKEN`. Reset reads only that test pair. It does not
read `FVOCI_LIBSQL_URL` or `FVOCI_LIBSQL_AUTH_TOKEN`; if either product name is
present, or the test pair is missing, reset refuses before connect. The
`turso-ui` job's final step still maps the same secrets to `FVOCI_LIBSQL_URL`
and `FVOCI_LIBSQL_AUTH_TOKEN`, because that step starts the product server
through `DatabaseSettings::from_env`. The product server does not read
`FVOCI_TEST_TURSO_*`. The consume wrapper reads that test pair from the step.
It forwards the values to connection, migration, and inventory children as
`FVOCI_LIBSQL_*`, because those tests use `DatabaseSettings::from_env`. The
reset child receives only the test pair.
Reset allows one host, compared as the raw authority host with no case
folding and no trailing-dot, port, or userinfo stripping:
`fvoci-fvoci.aws-ap-northeast-1.turso.io`. The verification database is named
`fvoci`, so a database-name prefix cannot distinguish it from production.
Case variants, a trailing dot, a port, userinfo, a subdomain,
`fvoci-prod…turso.io`, and any other region are refused before connect. The
allowlist is that literal, not an Environment variable or dispatch input.
For PREFIX11, before any DROP, reset sums `count(*)` over the user-data tables:
every reset `DROP TABLE` target except the ledger `schema_migrations`, the
infrastructure singletons `instance_settings_meta`, `event_sequence`,
`collab_fence_counter`, and `instance_config`, and step-12
`maintenance_job_claims` (not part of PREFIX11). A non-zero sum refuses and
drops nothing. For current12, reset instead requires exactly the migration
consumer's retained structural fixture, checked on the same reserved writer:

- The complete ledger and catalog match the compiled current12 lineage and
  per-step hashes. Every table in that verified catalog is accounted for by
  the literal drop plan; a newly compiled table without a reset plan refuses.
- Exactly one workspace has slug `turso-migration`, name
  `Turso populated011 literal`, settings `{}`, kind `team`, no deletion,
  document number zero and auto-join domains `[]`. Its ID has UUIDv7 version
  and RFC4122 variant bits. The embedded millisecond time is no later than the
  first ledger receipt: the consumer chooses its UUID before migrations.
  `created_at` equals `updated_at` and lies between the step11 and step12
  receipts. There is no lower bound on UUID time; the remaining literal fields,
  catalog, empty tables and seeds carry the structural identification.
- Every other user-data table is empty. Infrastructure is exactly settings
  revision zero, event sequence zero, fence counter17 and null VAPID keys,
  each under singleton ID1. Claim keys are exactly1–9, owner/expiry null,
  generation7 for key8 and zero for every other key.

Any other current12 data or a failed fixture query refuses with
`RESET_FIXTURE_REFUSED`, before any DROP. This is structural fixture matching,
not proof of which process originally wrote matching data. UI ACK data,
another workspace and any extra user row are retained by refusal. Unsupported
or altered schemas still return `RESET_SCHEMA_REFUSED`. All admission and DDL
share the original transaction. Only a confirmed commit permits the fresh
connection's complete blank-catalog check; an uncertain commit stops without
an alternate observer or retry.

Manual phase `reset` keeps both destructive gates and the exact selected test
`db::turso_test::turso_primary_disposable_prefix11_reset`; its historical name
also covers the admitted current12 fixture. No raw SDK/test error body, URL,
header, token, environment dump or credential-bearing trace is printed or uploaded.

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
Environment variable `FVOCI_TEST_TURSO_ALLOW_DESTRUCTIVE=true`. The lead (리드)
owns that flag lifecycle and the allocated isolated remote execution. Setting the
flag and any destructive remote DB operation require explicit approval from the
user; secrets are set or changed only by the user. The current registered flag
remains false. Bootstrap pushes cannot select migration.
The final consuming wrapper chooses exactly
`db::turso_test::turso_primary_current12_install_resume`, exports the exact four
cfg-test helper selection/phase/destructive flags, and requires one passed test
plus the strict prefix/FK-rollback/current/restart/close/lease receipt. It uses
the same source/ELF/native-input frozen qualification and credential filtering
as connection; fake fixture receipt tests are not remote runtime evidence.

Normal remote preparation in `src/prepare.rs` and the `libsql-remote` branch
of `fvoci-migrate` call `run_remote_migrations`: they install an exact blank
database, resume an exact prefix of the compiled lineage, and leave current12
unchanged. Other catalogs or ledgers refuse without automatic reset. Normal
server startup calls `connect_remote_app`, which requires the exact current
schema and performs no migration or repair. These product paths are distinct
from the ignored manual migration consumer, which calls the maintained compiled
SQLite registry and per-step migration owner through cfg(test, db-tests) helpers.
For that consumer, exact blank catalog admission occurs before DDL;
unexpected/foreign/populated/current
targets refuse, without automatic reset, service deletion or schema overwrite.
The secret URL remains the designated target, not an invented host/DB-ID proof.
The lead may separately prepare/reset the disposable test DB only with the
user's explicit approval for that reset and after concrete target checks; this
consumer never performs that reset.

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
passed in their isolated SDK check. That is SDK-only evidence, not actual Turso
migration or a compile result for a new wrapper composition. Provenance and full
MIT notice are in `vendor/libsql-0.9.30/`; see the pinned
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
identified test workspace and current schema remain retained for the lead's inspection.

Local reset controls use the pinned official sqld binary selected through
`FVOCI_TEST_SQLD`; the existing fixture verifies its digest and owns each
temporary database and daemon. With the maintained native SQLite inputs
prepared, run the automatic controls from the repository root:

```sh
cargo fmt --check
cargo test --locked --offline --features db-tests --lib db::turso_test
```

To explicitly select only the loopback reset controls, set the local fixture
binary and disk-backed `TMPDIR`, then run:

```sh
FVOCI_TEST_TURSO_MIGRATION_SELECTED=1 \
FVOCI_TEST_TURSO_PHASE=migration \
FVOCI_TEST_TURSO_DESTRUCTIVE=true \
FVOCI_TEST_TURSO_ALLOW_DESTRUCTIVE=true \
cargo test --locked --offline --features db-tests --lib \
  db::turso_test::reset_policy_tests::reset_ -- --ignored --test-threads=1
```

These controls need no Turso credentials and do not select the remote reset
consumer. They cover PREFIX11 preservation/reset, current12 fixture reset,
fixture counterexamples with unchanged persisted rows/catalog, and a lost
commit reply without a fresh blank observer. Loopback sqld is separate from
the pinned local SQLite engine and actual Turso acceptance.

Other phases (`crud`, `transactions`, `persistence`) remain
**NOT IMPLEMENTED** and refuse before credential consumption. `ui-ack` is an
allowed dispatch input, with `ui-baseline`, on main, the reviewed branch, or
`fvoci/v060-product-integration-20261005`. Current tenant
CRUD/authorization, request/version replay, real concurrent/cancel/uncertain
finish, remote setup/startup behavior, backup/restore, real UI persist ACK and
fresh-client history require their own product acceptance in the checkpoint.
Local SQLite or loopback SDK results cannot be labeled actual Turso PASS. Constant per-database
concurrency remains `cancel-in-progress:false`; no service resource reset,
provider/account/secret/policy/permission change is made by the worker.

The separate `restore` phase is admitted only on exact main source, with both
destructive gates and the just-observed baseline and target digests required
by `ui-ack`. It uses the same credential-free build seal, private keyring,
normal migration/server lifecycle, native observer and source preservation
audit. Its six browser cases cover UI revision preview/confirm and command
replay with matching save ACK, source restart, authenticated current native
archive export, malformed input rejection and one-job restore replay into an
independent SQLite installation, destination restart, and archive comparison.
All six must actually pass once; missing, skipped, retried or zero-case reports
refuse success. Existing `ui-ack` retains its ON/restart/OFF counts and reports
restore and precision as NOTRUN.

This transfer exercises the current single-author private-project archive:
content IDs, references, native history and revision provenance, attachment
bytes, numeric and timestamp values travel through authenticated HTTP. The
source actor and workspace map explicitly to the destination owner and personal
workspace. Activity actor fields become null under the existing inert provenance
contract. Capture time and its inventory association binding change on re-export;
all other inventory evidence and native member bytes remain exact. Each archive
manifest is checked against actual member sizes and SHA256 digests, and a rebound
binding with a valid ZIP manifest must be rejected by real product preflight.
Authentication, sessions, credentials and operational database state
are outside that content archive. Unsupported archive models remain product
refusals; SQLite file copying is not this backup contract. Authorized fresh
clients read the restored data and anonymous requests are denied. A supplemental
readonly destination count witness checks publication and one-job replay;
its Bun SQLite version is recorded separately from the product's sealed native
SQLite, and counts alone do not establish full-row preservation.

Local `tools/turso/ui.ts --local-restore` requires the maintained controller
allocation for its immutable source, stages, native inputs and owned output.
It consumes the shared pinned official sqld/TLS fixture as source and a distinct
owned SQLite destination. Local evidence is separate from hosted acceptance.
The controller serializes final hosted execution on a green exact main SHA,
owns the destructive flag lifecycle, and retains one hosted primary. The worker
does not access hosted credentials or dispatch that execution.

Pure local checks: `bun tools/turso/fixtures.ts` (the admission fixtures, from
any directory after `bun install --frozen-lockfile --ignore-scripts` at the
repository root: it runs the pinned guard
`tools/turso/guard*.test.ts`, UI consumer `tools/turso/ui*.test.ts` and
`turso-test.yml` literal `tools/ci/verify/registry.test.ts` suites with only
`PATH` and `TMPDIR`, and refuses if a pinned suite is missing) and, from the
repository root, `bun tools/ci/verify-workflows.ts` and `bun test
./tools/ci/planner/ ./tools/ci/gate/ ./tools/ci/verify/ ./tools/ci/argv.test.ts
./tools/ci/workflows.test.ts`. These do not prove Rust compilation,
SDK/network execution or actual Turso PASS. The UI consumer is
`tools/turso/ui.ts` (`--record-before`, `--freeze`, and the member `--actor`
the browser fixture runs); the guard's `--consume` calls its exported
`consume`.
