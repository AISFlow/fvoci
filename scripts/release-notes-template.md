# FVOCI @VERSION@ (trial pre-release)

<!-- notes-for: 0.4.0 -->

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

FVOCI 0.4.0 is a trial release on top of 0.3.0 (`6f64febc`). The web app is
now Vue throughout, including the collaborative editor and document viewers;
the former React application and split React/Vue boot path are removed.
The server remains Rust, and collaboration continues to use Yrs. This is
not an engine replacement or a declaration that every external platform has
been verified.

There is no database migration since 0.3.0 (the newest is still 044), and
no change to the user Compose file's environment variables. Workspace SSO,
audit-log viewing and branding still require an enterprise license. Published
builds trust no license key (`src/license-trust.json` is empty), so these
licensed features cannot be activated in a published install; audit entries
are recorded regardless.

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

## Changes since 0.3.0

- **Vue application and editor.** Authentication, workspace administration,
  discovery, wiki, task views and document viewers now use the Vue app.
  Navigation between Gantt and other app pages no longer requires the old
  React/Vue application switch. The Vue editor includes collaboration,
  formatting, inline math, attachments, import/export and read-only controls.
- **Editing and collaboration recovery.** Fixes preserve schema identities,
  marks and live relative caret endpoints during remote updates; guard
  composition against unencodable marks; and prevent retired editor,
  revision and persistence callbacks from affecting a new document or actor.
  A closed outbound collaboration receiver is handled without losing the
  final accepted update. These changes do not imply physical IME coverage
  on every operating system.
- **Permissions and navigation.** Task metadata permissions are independent
  of collaborative-body admission, and REST controls refresh after grants
  and resynchronization. Revoked sessions and transports retire across
  authentication boundaries. Late task/dependency mutation responses and
  conflict recovery remain scoped to their original task. Confirmed
  workspace deletion keeps its intended navigation.
- **Draft preservation.** Dirty task hierarchy and document-header drafts
  survive metadata commits and refreshes. Calendar drafts remain open and
  intact when cached-field refresh fails or metadata loading is retried.
  Empty task details and rescheduling targets recover without borrowing a
  different task's state.
- **Cache and document consistency.** Calendar writes and stream reopening
  refresh task caches; retained trash caches refresh before document
  navigation. Markdown export preserves linked whitespace through emphasis.
- **Verification and development.** Regression coverage now checks actor
  retirement, exact persistence frames and readback bodies, navigation and
  draft-retry flows. CI selects relevant checks for pull requests and runs
  the required gates on the merged main commit. Development uses pinned
  Bun 1.4.2; importer-specific Zod versions remain isolated. The runtime
  image contains no JavaScript runtime.

These notes describe changes already merged into the release source. The
release workflow separately builds and smoke-tests the exact tagged image
on native amd64 and arm64 before publishing it. A local fixture, a passing
PR, or a historical image run is not evidence for an untested provider,
physical device or upgrade path.

## Upgrading from 0.3.0

1. Back up the running installation using the scripts from its release,
   and keep a separate private copy of `.env`. For S3, follow "S3 storage
   backup" in `RUNNING.md`; the local-storage backup script refuses S3.
2. Download this release's `compose.yml` and `SHA256SUMS` into an empty
   directory and verify `sha256sum --ignore-missing -c SHA256SUMS`. Replace
   the installed `compose.yml`, retain `.env` and the existing volumes, then
   run `docker compose up -d --wait --wait-timeout 900`. No new database
   migration or environment value is introduced by this release.
3. Check `docker compose exec fvoci /opt/fvoci/bin/fvoci-server --version`
   for `@VERSION@` and the source commit, then reload open browser tabs.
   This documented procedure is not a claim that the published
   0.3.0-to-0.4.0 image pair has undergone an upgrade/rollback test.
4. Downgrades, rolling upgrades and two servers sharing one database remain
   unsupported. Recovery means restoring the pre-upgrade backup with its
   matching release files and keys; writes made after the backup are lost.

For an older installation, read the
[0.3.0 release notes](https://github.com/@REPOSITORY@/releases/tag/v0.3.0)
first. In particular, 0.1.x/0.2.0 Compose secret-file settings are refused:
replace the whole Compose file rather than editing only its image line.
The `.env` values are container environment and readable by Docker users;
never paste raw `docker inspect` or `docker compose config` output publicly.

## Not verified or optional

The following are existing verification limits, not newly completed work:

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
  upgrade/rollback evidence does not verify the published 0.3.0-to-0.4.0
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
- **Upgrades only as documented.** Back up first, then replace
  `compose.yml` (see below). Rolling upgrades, running two servers against
  one database, and downgrades are not supported.
- **ARM64** (linux/arm64) is verified by the CI release smoke on native
  runners only. No long-running ARM64 install has been exercised.

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

## Data, keys and upgrades

- Back up before any upgrade (see `RUNNING.md` at `@SHA@` for backup and
  restore). Keep the copy of `.env` separate from the database backup.
- To upgrade, back up, replace `compose.yml` with the one from the new
  release (check its `SHA256SUMS` in an empty directory first), keep `.env`,
  and run `docker compose up -d --wait --wait-timeout 900` (a long migration
  can outlast the default wait; `docker compose logs -f fvoci` shows
  progress). Compose recreates `fvoci`, so the old server stops before the
  new container migrates. The `fvoci` container applies database
  migrations before the server starts; if that fails the server does not
  start, and it refuses to migrate while another server is still connected.
- Migrations only move forward. Going back to an older 0.y release means
  restoring the backup taken before the upgrade.
- Keep the same Compose project name (the directory name by default), or the
  new stack starts with empty volumes.
