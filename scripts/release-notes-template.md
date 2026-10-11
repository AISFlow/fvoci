# FVOCI @VERSION@ (trial pre-release)

<!-- notes-for: 0.5.0 -->

This is a 0.x trial release. It is not covered by any compatibility promise:
a later 0.y release may change configuration, data layout or behaviour.
Installs are never updated automatically.

- Source commit: `@SHA@`
- Image (multi-arch index): `@IMAGE_REF@`
  - linux/amd64: `@AMD64_DIGEST@`
  - linux/arm64: `@ARM64_DIGEST@`
- Release smoke (pulled the digest above anonymously on native amd64 and arm64
  runners, fresh install from env.example, the uid and environment boundary,
  first admin, core flows, restart, rejected settings, preparation failure):
  @RUN_URL@

## Accepted in this release

FVOCI 0.5.0 is a trial pre-release on top of 0.4.0. It includes the
personal/team workflow, editor, timer and archive work integrated in
[#339](https://github.com/@REPOSITORY@/pull/339), and the backend integration
from [#347](https://github.com/@REPOSITORY@/pull/347). The app remains Vue 3
with Nuxt UI and Tiptap, the server is Rust, and collaboration uses Yrs.
This trial does not satisfy the 0.6.0 completion gate in
[docs/rewrite.md](https://github.com/@REPOSITORY@/blob/@SHA@/docs/rewrite.md).
In particular, integrating a backend is separate from verifying all user
flows, recovery and resource costs on that backend.

New installations use a twelve-step schema lineage:
`fvoci-postgres-060` for PostgreSQL and `fvoci-sqlite-060` for local
SQLite and remote libSQL/Turso. A database from 0.4.0's migration 044 is
not upgraded in place; see "Moving from 0.4.0" below. PostgreSQL remains
the documented user Compose installation. Local SQLite and remote
libSQL/Turso paths are integrated, with the verification limits below.

Workspace SSO, audit-log viewing and branding still require an enterprise
license. Published builds trust no license key
(`src/license-trust.json` is empty), so these licensed features cannot be
activated in a published install; audit entries are recorded regardless.

The accepted feature families remain:

- **Accounts:** first-admin setup, sign-in and sessions, profile, password
  reset, email and password change, account deletion and export, magic links,
  personal API tokens, TOTP two-factor sign-in, OIDC sign-in, and workspace
  SSO (licensed builds only, see below).
- **Workspaces:** members, invitations with seat limits, groups and
  permissions, projects, workspace export, trash and purge, a workspace event
  log with an activity section in settings.
- **Wiki documents:** create, move, sort, trash and restore; real-time
  collaborative editing (Yrs); import (including office files and Notion
  exports) and export to Markdown, DOCX, PDF and PPTX; templates; backlinks;
  revisions with restore.
- **Tasks:** lists, board, Gantt and calendar views, workflows, WIP
  limits, recurring tasks, labels and assignees, task bodies, activity and
  comments, live updates between browsers.
- **Attachments:** uploads with quotas, through the API or (optional, S3
  only) directly to storage with short-lived signed URLs; viewers for PDF,
  DOCX, XLSX, PPTX and HWP/HWPX; editing an HWP/HWPX attachment and saving an
  authorized copy; text extraction for search.
- **Search:** workspace and global search with Meilisearch, including
  comments and attachment text; results are checked against current
  permissions.
- **Notifications:** in-app notifications, mail and digests, webhooks, the
  GitHub app integration, and browser push notifications (Web Push).
- **Sharing and organizing:** public share links and pages, favorites, recent
  items, tags, collections and saved views, and calendar (ICS) feeds.
- **Administration:** consent and legal documents, audit log, license and
  quotas, admin settings and user management, branding. Viewing the audit
  log and branding need an enterprise license, which a published build
  cannot accept (the same as workspace SSO); audit entries are recorded
  either way.
- **Operations:** API documentation at `/api/docs` (signed-in users), `/health`,
  `/ready` and `/metrics` probes with a container healthcheck,
  `fvoci-migrate --secrets-audit` / `--secrets-rotate` for the encryption
  keyring, and backup and restore scripts (`scripts/backup.sh`,
  `scripts/restore.sh`) that verify keys, stored files and sealed secrets
  before the server starts.

## Changes since 0.4.0

- **Personal and team work.** Personal input and transfer workflows support
  COPY and same-ID MOVE into team resources, with captured actor/target
  checks and durable-save barriers. Task streams are shared at workspace
  level and refresh authorized caches after missed events or resync.
- **Editor and revisions.** Rich, block, Markdown and preview modes retain
  the editor's document/provider identity. Source Apply preserves supported
  IDs and peer edits; revision comparison and restore are connected to the
  existing native history. Read-only Zotero data and document links are
  included without claiming live external Zotero-account verification.
- **Task time.** The native stopwatch, manual time corrections and history
  use server-owned intervals, single-running ownership, replay and conflict
  checks. Stopping a timer keeps the task status. Calendar and task forms
  preserve dirty drafts when metadata refetch fails.
- **Current data and archives.** Current-format native archive/import is
  integrated, and whole-install backup and restore is a separate mechanism.
  Preserving IDs, history, references, attachments and permissions remains
  the contract of `docs/rewrite.md` sections 3.5 and 3.6; it is not yet
  verified on this release commit. Backend-to-backend archive support and
  rejection must be checked for the actual source and destination; this is
  not a compatibility promise for old development schemas.
- **Backend integration.** Selected-backend services connect PostgreSQL,
  local SQLite and remote libSQL/Turso operations to authorization,
  collaboration, attachment cleanup and maintenance. Fresh installation
  checks the exact ordered lineage and SQL checksums; a foreign, incomplete,
  changed or newer ledger is refused. These changes do not complete the
  remaining three-backend UI, commit/ACK, restart and recovery acceptance.
- **Build and CI.** #344 balances browser shards using observed group costs;
  #369, #370 and #371 update the harness image, ARM PostgreSQL coverage and
  nextest availability; #378 limits cache publication to main and registers
  actionlint runner labels. #382 strengthens XLSX negative controls and
  #383 isolates browser-test email identities and cursor-wait observations.
  #374 and #393 clarify repository verification and approval rules. #385
  updates the locked Jiff dependency, and #346 adds the Renovate
  configuration.
- **Harness runtime.** #397 replaces repository-managed Python dev, test,
  build, CI and release harness dependencies with TypeScript/Bun and Rust
  xtask, including planner/gate policy and release tooling. Development and
  harness execution use pinned Bun 1.4.3. The product image contains no
  JavaScript runtime; browser JavaScript is still part of the web app.

The release workflow separately builds and smoke-tests the exact tagged
image on native amd64 and arm64 before publishing it. A local fixture,
a passing PR or a historical image run does not verify an untested
provider, physical device, backend flow or upgrade path.

## Moving from 0.4.0

This version installs into an empty database. The old migration-044
ledger and retired development lineages are refused without rewriting
their data. Replacing the image against a 0.4.0 database is not a supported
in-place upgrade, and there is no verified published 0.4.0-to-0.5.0 migration
or rollback pair.

1. Keep the old installation and its matching release files and keys.
   Back it up using that release's scripts, with a separate private copy
   of `.env`; for S3, follow its documented storage-backup procedure.
2. Install 0.5.0 as a separate Compose project with empty volumes. Never
   attach the new server to the old database or run two servers against
   one database.
3. The current-format native archive is the data-transfer mechanism,
   subject to source/destination format support and the recovery limits
   below. Verify export, restore and fresh-client readback for your actual
   data before retiring the source; old-schema export compatibility is not
   established by the current-format archive tests.
4. Recovery means returning to the preserved old installation or restoring
   its backup with matching release files and keys. Writes made after that
   backup are not present in it. Do not start the old binary on a database
   initialized with the new lineage.

For earlier installations, read the
[0.4.0 release notes](https://github.com/@REPOSITORY@/releases/tag/v0.4.0)
and the matching version's backup instructions first. Keep `.env` private:
its values are readable by Docker users; never publish raw
`docker inspect` or `docker compose config` output.

## Not verified or optional

The following remain unverified or optional for this trial:

- **Three-backend completion:** the 0.6.0 gate still requires actual
  UI → Rust authorization → backend commit → matching ACK → fresh-client
  readback, restart and separate-install restore across PostgreSQL, local
  SQLite and remote libSQL/Turso. Remaining flows include logout/revocation,
  personal/project/member changes, OFF CAS/conflicts/drafts, lost responses
  and historical ACKs, data precision and equal-work ON/OFF resource costs.
  Earlier accepted PostgreSQL or scoped SQLite results do not establish
  completion on another backend or the release commit.
- **Turso/libSQL remote support:** live verification on the release commit pending.
  TURSO-RESULT: pending — coordinator to record release-commit live verification.
- **Zotero:** read-only fixture and archive evidence does not verify access
  to a real external account with live credentials.
- **Mail and GitHub app:** SMTP was checked with a local test relay,
  including per-recipient refusals; GitHub integration with a local fake
  API. Actual external SMTP and GitHub App operation remain unverified.
- **OIDC and workspace SSO:** existing local Keycloak 26.7.4 and Rust test
  entitlement evidence does not cover external IdPs, production HTTPS and
  proxy configuration, key rotation, the published container or every
  browser. Published license trust remains empty as described above.
- **AI actions and semantic search:** optional and require a user-provided
  OpenAI-compatible embeddings endpoint; no real provider was verified.
- **S3 and presigned attachment transfer (#149):** both transfer policies
  were approved and the implementation has local MinIO/PostgreSQL and
  Chromium cross-origin evidence. Actual AWS S3, other S3-compatible
  providers, CDN/proxy deployments, Firefox/Safari and clock skew remain
  unverified. Signed-length enforcement, redirected-download CORS,
  virtual-host addressing, response overrides and lifecycle rules need
  provider-specific evidence. The external validation item remains open.
- **Two-factor sign-in:** code and QR enrolment tests do not replace a
  real authenticator-app scan, which remains unverified.
- **Web Push:** one real Linux Chrome delivery through Google's push
  service was observed; other browsers and providers remain unverified.
- **IME, touch and browser coverage:** Linux X11 IBus Hangul/Chromium
  witnesses cover specific composition and selection cases. Synthetic/CDP
  tests are separate evidence. Physical keyboards/touch, Windows, macOS,
  mobile, Firefox and WebKit are outside those witnesses.
- **Capacity and isolation:** no new many-user capacity measurement or
  full default-AppArmor performance claim is made for this release.
- **Upgrades and platforms:** earlier local-storage and local S3 image
  upgrade/rollback evidence does not verify the published 0.4.0-to-0.5.0
  pair, ARM S3, automatic upgrades, Docker Desktop, rootless Docker or Podman.
- **Release smoke scope:** `/metrics` and `--outbox-reset` are outside the
  smoke. Only its documented setup-based browser subset runs against the
  release image; the broader browser suite runs separately in Web CI.

## Known limitations

- **No compatibility promise.** 0.x releases may change settings, data
  layout or behaviour between versions. Nothing updates an install on its own.
- **Settings are container environment.** Anyone who can run Docker
  commands on the host can read every `.env` value (`docker inspect`,
  `docker compose config`, `docker compose exec`), and root in the `fvoci`
  container (`docker compose exec fvoci …`, the healthcheck) starts with all
  of them. The server runs as uid 1000 without the owner password or the
  Meilisearch master key and cannot read root's processes, but a session you
  start as uid 1000 (`docker compose exec -u 1000:1000`) holds every value,
  readable by the server's uid while it runs. The server makes itself
  non-dumpable and refuses to start if the kernel does not allow it; the
  helpers still share uid 1000 file access with it (stored files and the
  scoped search key). The helpers themselves stay dumpable, so where the host
  allows same-uid ptrace one helper can attach to another.
- **Helper memory.** Collaboration helpers and document helpers (HWP,
  Office, Markdown, image preview) set `oom_score_adj=1000` so an
  out-of-memory kill prefers them over the server; where the container
  profile denies it (for example AppArmor docker-default), they still start
  without it, and only a collaboration helper's denial is logged (once).
  This was not measured under that profile.
- **Local HTTP by default.** The app is published on `127.0.0.1` over plain
  HTTP. For other users or a domain, put a TLS reverse proxy in front and
  set an `https://` `FVOCI_PUBLIC_ORIGIN` (see `RUNNING.md`).
- **Rate limits are per server process and per direct client address.**
  Forwarded headers are ignored, so behind a reverse proxy all clients share
  one address (for example 30 sign-ins per 5 minutes). Each IPv6 address
  counts separately, and there is no per-account sign-in limit.
- **Workspace SSO login CSRF** (licensed builds only). A manager of a team
  workspace with SSO can make a link that signs a visitor in through an
  identity provider that manager controls (sign-in starts are still GET).
  Team workspaces are created only by instance administrators.
- **Invitations accepted through a provider.** A new account opened this
  way gets the invited address and is linked to whatever identity the
  provider returns; the provider's email does not have to match or be
  verified (same as the original product, and also seen with a real
  Keycloak; Naver never sends `email_verified` and Kakao often omits it). A
  decision is pending.
- **Share links outlive their creator's access** until someone revokes them
  (same as the original product; a decision is pending).
- **Presigned URLs** (`presigned` mode only). An issued download URL works
  until it expires (default 60 s), even after the permission is revoked or
  the attachment is deleted, until the object is reclaimed. A part URL
  (default 15 minutes) can still stage bytes into its upload, though not
  publish them, until it expires or the multipart upload is completed or
  aborted. Only rotating the S3 access key revokes them all. Signed URLs use
  the server's clock, so keep it synchronized. After its retries, a
  storage connection failure shows the generic network error.
- **Digest can stop at the same place every day.** If the same recipients
  fail every day (a lasting temporary refusal such as `452 4.2.2`, or a bare
  `550` from a mail server that sends no enhanced status codes), five such
  temporary failures (or twenty such refusals) with no digest delivered and
  no refusal of one mailbox between them stop the daily run at the same user
  each day, and users after it get no digest while those failures last.
- **Mail can be sent twice to one recipient** when the server restarts
  between retries of one mail event: the list of recipients already accepted
  is kept in memory only.
- **Database connection check.** Until sqlx 0.9, each reuse of a pooled
  connection runs one check query in place of sqlx's ping. In the author's
  local measurement (a bare `SELECT 1` through the pool) it cost about
  10–12% per query run one at a time and 1–24% with the pool saturated;
  requests that run several queries on one connection pay relatively less.
- **Task updates from other browsers take up to about 0.75 s** to appear:
  live task changes are polled every 750 ms. Collaborative document text is
  pushed and not affected.
- **Collaboration room limit.** At most 64 documents or task bodies can be
  open for editing at the same time in this compose file (30 is the image
  default). While every room has a user in it, or the helpers are at
  capacity, a newly opened body shows "not loaded yet" and retries until
  one frees up; a room emptied a few seconds ago is kept for a returning
  user first. Reclaiming only applies to the room count, not to the helpers'
  memory budget.
- **Blank first page.** In CI, Chromium sometimes aborted the app's first
  script requests with `net::ERR_NETWORK_CHANGED` while the host's network
  was changing, and the page stayed blank. The app does not retry a script
  that failed during the first load; if a page stays blank, reload it.
- **Search key file must not be a symlink.** A `FVOCI_MEILI_KEY_FILE` path
  that is a symlink is refused at startup, for example a Kubernetes Secret
  volume entry.
- **Quoting in `.env` for restore.** `scripts/restore.sh` accepts values
  written unquoted or wholly in single or double quotes, as Compose reads
  them (inside single quotes `$` and backslashes are literal). It refuses
  other forms (escapes outside quotes, `$` outside single quotes, inline
  comments, `export`) instead of guessing. Keep the values unquoted, as
  `env.example` writes them. With PostgreSQL `log_statement` set to `ddl`
  or `all` (off by default), the app role password that `restore.sh` sets
  reaches the PostgreSQL log.
- **Installation lineage.** This release needs an empty database; the
  migration-044 schema from 0.4.0 is refused. Rolling upgrades, two servers
  sharing one database and downgrades are unsupported. Preserve the source
  installation and its backup when moving data.
- **OFF editing and recovery acceptance.** Dirty-draft retirement policy
  remains unsettled, and a historical receipt can clear the UI conflict
  state in the recorded client counterexample. Actual HTTP/DB/browser
  requalification remains open; this is not evidence of server overwrite.
  SQLite/libSQL logout and password-change/reset paths also retain static
  PostgreSQL-gate candidates pending actual HTTP/DB verification.
- **ARM64** (linux/arm64) release smoke runs on native runners. No
  long-running ARM64 install has been exercised.

## Install

Requires Docker Engine with the Compose plugin (v2.24+) and `openssl`, on
linux/amd64 or linux/arm64. Only Linux Docker Engine was exercised for this
release; Docker Desktop, rootless Docker and Podman were not tested. From an
empty directory:

```sh
for f in compose.yml env.example INSTALL.md SHA256SUMS; do
  curl -fsSLO https://github.com/@REPOSITORY@/releases/download/v@VERSION@/$f
done
sha256sum --ignore-missing -c SHA256SUMS
cp env.example .env   # fill in each empty value with the command shown above it
chmod 600 .env
docker compose up -d --wait
```

`compose.yml` pins the image by digest, so the tag cannot be moved under an
existing install. Compose refuses to start while a value in `.env` is empty,
and the `fvoci` container checks the values before it prepares the database
and starts the server. Open the published address and create the first
administrator on the setup page. Keep `.env` private and back it up apart
from the database backups; Compose passes its values to the containers as
environment (see "Known limitations").

## Start, stop, restart

```sh
docker compose stop            # stop, keep everything
docker compose up -d --wait    # start again; after editing .env, applies it
docker compose restart fvoci   # restart the application only, same settings
docker compose down            # remove containers, keep volumes (data and keys)
```

`docker compose down -v` deletes every volume: the database, uploaded files,
and the search index. The keys stay in `.env`; without them, encrypted data
from a backup cannot be read. Only use it to throw an install away.

## Data, keys and recovery

- Back up with the matching version's instructions in `RUNNING.md`.
  Keep `.env` separate from the data backup and retain its encryption keys.
- A 0.4.0 installation needs a separate empty 0.5.0 installation, as
  described in "Moving from 0.4.0". The new schema lineage is not an
  in-place upgrade of the old one.
- Current-format native archives and whole-install backups are different
  recovery mechanisms. Verify the format, backend and attachment coverage
  of the mechanism you use; copying only the SQLite main file is not a
  WAL-consistent backup.
- Restore backups with matching release files, database lineage and keys.
  Do not point an older server at the new schema. Keep the old Compose
  project until the destination's data and access checks are complete.
