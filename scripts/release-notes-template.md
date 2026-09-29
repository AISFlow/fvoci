# FVOCI @VERSION@ (trial pre-release)

<!-- notes-for: 0.2.0 -->

This is a 0.x trial release. It is not covered by any compatibility promise:
a later 0.y release may change configuration, data layout or behaviour.
Installs are never updated automatically.

- Source commit: `@SHA@`
- Image (multi-arch index): `@IMAGE_REF@`
  - linux/amd64: `@AMD64_DIGEST@`
  - linux/arm64: `@ARM64_DIGEST@`
- Release smoke (pulled the digest above anonymously on native amd64 and arm64
  runners, fresh install from env.example, first admin, core flows, restart,
  rejected settings, preparation failure):
  @RUN_URL@

## Accepted in this release

FVOCI 0.2.0 is a minor release on top of 0.1.1 (`71d252a6`). It adds one
database migration (044) and changes the redirect URI of workspace SSO; the
install files (`compose.yml`, `env.example`, `INSTALL.md`) change only in the
version, source commit and image digest they name, and there is no new `.env`
value. It is still a 0.x
trial with no compatibility promise. The fixes are listed under "Changes since
0.1.1". The features below were accepted on `main` with their tests, CI and an
independent review (feature table in `docs/rewrite.md` at `@SHA@`):

- **Accounts:** first-admin setup, sign-in and sessions, profile, password
  reset, email and password change, account deletion and export, magic links,
  personal API tokens, TOTP two-factor sign-in, OIDC sign-in and workspace SSO.
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
- **Attachments:** uploads with quotas; viewers for PDF, DOCX, XLSX, PPTX and
  HWP/HWPX; editing an HWP/HWPX attachment and saving an authorized copy; text
  extraction for search.
- **Search:** workspace and global search with Meilisearch, including
  comments and attachment text; results are checked against current
  permissions.
- **Notifications:** in-app notifications, mail and digests, webhooks, the
  GitHub app integration, and browser push notifications (Web Push).
- **Sharing and organizing:** public share links and pages, favorites, recent
  items, tags, collections and saved views, and calendar (ICS) feeds.
- **Administration:** consent and legal documents, audit log, license and
  quotas, admin settings and user management, branding.
- **Operations:** API documentation at `/api/docs` (signed-in users), `/health`,
  `/ready` and `/metrics` probes with a container healthcheck,
  `fvoci-migrate --secrets-audit` / `--secrets-rotate` for the encryption
  keyring, and backup and restore scripts (`scripts/backup.sh`,
  `scripts/restore.sh`) that verify keys, stored files and sealed secrets
  before the server starts.

## Changes since 0.1.1

### Security

- **Workspace SSO has its own callback.** Every workspace SSO provider shared
  the instance `generic` callback, so an authorization response from one
  identity provider could complete a sign-in started with another (IdP
  mix-up). Each workspace now uses
  `/api/v1/auth/sso/{workspace_id}/callback`; a response on another
  workspace's or an instance provider's callback ends with
  `oidc_state_mismatch` before any request to the provider. The new URI must
  be registered at the provider (see "Upgrading from 0.1.1").
- **Invitation sign-in with a provider** could be started by a cross-site GET
  (login CSRF). It is now a same-origin
  `POST /api/v1/auth/oidc/{provider}/start`, refused with 403
  `origin_mismatch` from another origin or without an `Origin` header.
- **Invitation accept limits.** Accepting an invitation for an existing
  account checks its password against the sign-in limits, and anonymous
  accepts are limited to 60 per 5 minutes per client address.
- **A two-factor code is accepted once.** A code matching two adjacent time
  steps recorded the older one, so the same code could be used again.
  Verifying or enabling two-factor no longer rejects a valid code while
  `fvoci-migrate --secrets-rotate` runs.
- **Changing an email address ends the old mailbox's links** (migration
  044): password reset, sign-in and email-change links mailed before the
  change, and pending two-factor challenges, stop working. Sessions stay
  signed in.
- **Wiki share links.** Creating a public link to a wiki page now needs View
  on every page below it; a guest could publish child pages they could not
  read. The refusal is 404. Links created before this release are not
  re-checked.
- **No SSO on personal workspaces.** Saving an SSO configuration on a
  personal workspace answers 409 `personal_workspace_is_immutable`, and one
  saved before no longer starts or completes any sign-in.
- **Rate limits.** A client creating many new keys could fill the limiter's
  table and reset every other limit (sign-in, two-factor re-authentication,
  setup, share). Each key now keeps its own window, and a full table evicts
  the least-used key of the kind (namespace) that holds the most keys, so a
  flood ends up evicting its own keys. Limit values and windows are
  unchanged.
- **Revoked sessions during writes.** An instance-admin change (users,
  admins, erase and its cancellation, legal documents, settings, branding)
  that was waiting when its session was revoked was still applied; it now
  answers 404 and writes nothing. Creating a manual revision is likewise
  refused with 404 when, while it waited, the session was revoked, the
  token's owner suspended or the member removed.
- **Server process is non-dumpable.** The helpers the server starts
  (collaboration, document extraction, preview, Office and Markdown
  conversion, all uid 1000) and a uid-1000 `docker compose exec` session can
  no longer read the server's environment (the keyrings,
  `DATABASE_APP_URL`), memory or open descriptors, or attach to it. The
  server refuses to start if the kernel does not allow this, and it writes no
  core dump at all, whatever `fs.suid_dumpable` is set to.
- **`scripts/restore.sh`** passes the app database password through the
  environment instead of the `docker` and `psql` command lines.

### Sign-in in the browser

- In 0.1.x, linking a sign-in provider in account settings was refused (403:
  the form sent `Origin: null`), and in Chromium the page's content security
  policy also blocked it and workspace SSO sign-in by slug on the login page.
  Both now start by script and reach the provider.
  `POST /api/v1/auth/oidc/{provider}/link` answers 200
  `{"authorizationUrl": …}` instead of a 303.
- The workspace SSO settings show the exact redirect URI to register (from
  the server, with a copy button), only for team workspaces.

### Mail and outbox delivery

- **Mail is sent per recipient.** A permanent refusal of one mailbox is final
  for that recipient only; the others are still sent. A retry skips
  recipients the mail server already accepted (remembered in memory for the
  last 64 mail events). Before, one failed recipient stopped the rest of that
  mail, a retry sent it again to recipients already served, and a failure
  could be charged to another event delivered in the same batch.
- **Digests reach every user.** The daily digest served the same first 100
  users every day. It now pages through all due users. A run stops early on
  shutdown, after 15 minutes, after 5 sends fail with no answer from the
  mail server or a temporary refusal, or after 20 permanent refusals that do
  not name the recipient (a sent digest or a refusal of one mailbox restarts
  both counts). Users a run did not reach stay due for the next day's run.
- **Shutdown and slow batches.** Mail, search-index and GitHub (when the
  GitHub app is configured) delivery stop between batches at shutdown or when
  their lease runs out, instead of delivering everything pending first, and
  record what a slow batch delivered before moving on, so it is not lost or
  sent twice.
- **`fvoci-migrate --recover-outbox`** (run by `restore.sh`) keeps the
  processed marks its replay still needs.

### Data

- **Account export** fails instead of silently leaving out an attachment
  whose storage check failed.
- **Markdown ZIP imports** left pending by a cancelled or crashed request are
  marked failed by the daily sweep after 24 hours.
- A database error while checking comment permissions answers 500 instead of
  404.

### Reliability and performance

- **Reads no longer lock projects.** Task, project, comment, document and
  revision reads take no project row lock and no transaction id, so a
  View-only user's reads cannot delay saves and writes, or hold back live
  updates and outbox delivery for every workspace.
- **Pooled database connections are checked at reuse.** A request cancelled
  while its transaction was starting could return its connection with the
  transaction still open (a defect in sqlx 0.8.6). The pool now closes such a
  connection instead of reusing it.
- **Resuming a large upload** on local storage no longer re-reads every stored
  part (1 GiB: about 24 s of CPU before, 12 ms after, in the author's local
  measurement).
- Backlink lookups stop after 15 s.

### Collaboration and live updates

- **Live-update streams recover.** A stream the browser gave up on reopens
  with a jittered backoff (1 s, doubling to 30 s). Access checks run in one
  transaction per poll, a poll no longer rescans events it already passed,
  and collection boards and panels recover from an expired cursor.
- **Losing access closes the workspace stream** when the user is removed
  from the workspace, their role changes, or the workspace is moved to
  trash.
- **Collaboration helpers.** Opening a document or task body starts one
  helper instead of two, and a helper that failed to start no longer leaves
  that room unable to recover. When the helpers are at capacity, a newly
  opened body is refused as busy (close code 1013, as at the room limit)
  instead of unavailable (1011): it shows "not loaded yet" with that reason
  and retries with backoff. A helper no longer outlives the server: it is
  killed when the thread that started it exits.

### Operations

- To inspect the server process (`/proc/1/environ`, `gdb -p`, `strace -p`,
  `lsof`), use `docker compose exec --privileged fvoci …` (root with
  `CAP_SYS_PTRACE`; the image ships none of these tools, so install them in
  the container first or attach from the host). A uid-1000 session
  (`docker compose exec -u 1000:1000`, which the 0.1.1 `RUNNING.md`
  suggested) can no longer read it; root without `CAP_SYS_PTRACE` could not
  before either. `perf -p` also needs a container created with
  `CAP_PERFMON` or `CAP_SYS_ADMIN`.
- No new or changed `.env` value.

## Upgrading from 0.1.1

Upgrading directly from 0.1.0? Read the 0.1.1 changes too
(https://github.com/@REPOSITORY@/releases/tag/v0.1.1); 0.1.1 added no
migration, so the steps below apply with 0.1.0 in place of 0.1.1.

1. Back up first (see "Data, keys and upgrades" below). Migration 044 runs
   when the new `fvoci` container starts. There is no downgrade: 0.1.1 does
   not start on the migrated database, so going back to 0.1.1 means
   restoring the backup taken before the upgrade.
2. Download the 0.2.0 `compose.yml` and `SHA256SUMS` into an empty directory
   and run `sha256sum --ignore-missing -c SHA256SUMS` there (the install
   directory still holds the 0.1.1 `INSTALL.md`, which no longer matches),
   then copy `compose.yml` over the old one, keep `.env`, and run
   `docker compose up -d --wait --wait-timeout 900` in the install
   directory; Compose stops the old `fvoci` container before the new one
   migrates. `docker compose exec fvoci /opt/fvoci/bin/fvoci-server --version`
   then shows `@VERSION@` and the source commit. Reload open browser tabs.
3. **Workspace SSO:** register each workspace's new redirect URI,
   `<FVOCI_PUBLIC_ORIGIN>/api/v1/auth/sso/<workspace id>/callback` (shown in
   the workspace's SSO settings), at its provider before users sign in; until
   then the provider refuses the sign-in. Remove the old
   `/api/v1/auth/oidc/generic/callback` URI there once no instance `generic`
   provider shares that client. A sign-in started before the upgrade fails
   once with `oidc_state_mismatch`. Instance providers (`OIDC_<KEY>_*`) keep
   their URIs. See "Redirect URIs to register at the provider" in
   `RUNNING.md` at `@SHA@`.
4. **SSO saved on a personal workspace** stops working: users who signed in
   through it must use another method. The settings page no longer shows the
   section there; remove the row with
   `DELETE /api/v1/workspaces/{id}/oidc`.
5. **After an email change**, password reset, sign-in and email-change links
   and two-factor challenges must be started again. Sessions are kept.
6. **API clients:** `POST /api/v1/auth/oidc/{provider}/link` answers 200
   `{"authorizationUrl": …}` instead of a 303. Invitation sign-in with a
   provider is `POST /api/v1/auth/oidc/{provider}/start` (urlencoded
   `invitation`, `consents`); `GET …/start` with `invitation` or `consents`
   answers 400.
7. **Behind a reverse proxy**, every client shares the proxy's address for
   rate limits, so bulk onboarding can reach the 60 invitation accepts per 5
   minutes.

## Not verified or optional

These are shipped but off by default, or were only checked against local
stand-ins. Treat them as untested with a real provider:

- **Mail (SMTP):** tested against a local test relay only, including scripted
  per-recipient refusals. Without SMTP, invitation links are shown in the
  app instead of mailed.
- **OIDC sign-in and workspace SSO:** tested with local test providers, not
  a real external identity provider. The browser starts changed in this
  release (invitation, account linking, workspace SSO by slug) were checked
  in Chromium 153 only, in one uncommitted run against a test provider on
  another origin; other browsers were not tried, and there is no automated
  browser test for them.
- **GitHub app:** tested against a local fake of the GitHub API.
- **AI actions and semantic search:** optional, and need an
  OpenAI-compatible embeddings endpoint that you provide. No real provider
  was used.
- **S3 storage:** checked against a local S3-compatible store only, including
  the documented upgrade and rollback steps. No cloud provider was used.
- **Two-factor sign-in:** codes and QR enrolment are tested, but no real
  authenticator app has scanned the QR code.
- **Web Push:** one real delivery was observed, with Chrome for Testing on
  Linux through Google's push service. Other browsers and push services were
  not tried.
- **Korean input (IME):** checked with a real Linux (IBus) input method in
  Chromium only. Windows, macOS and mobile input methods were not tried.
<!-- COORDINATOR: collab-capacity -->
- **Collaboration capacity:** the room-limit behaviour (new in 0.1.1) was
  measured on a source build of an earlier revision of that change on one
  host, not on this image or with many real users. The helper changes in
  this release were not measured under Docker's default AppArmor profile.
<!-- COORDINATOR: upgrade-test -->
- **`/metrics` and `--outbox-reset`** are not part of the release smoke, and
  there is no automated upgrade test from the published 0.1.1 files to
  0.2.0 (migration 044 runs in the database test suites, not in an image
  upgrade test).

## Known limitations

- **No compatibility promise.** 0.x releases may change settings, data
  layout or behaviour between versions. Nothing updates an install on its own.
- **Secret boundary is within one container.** The `fvoci` container prepares
  the database and search as root, then runs the server as uid 1000. The
  server cannot read the secret files, the database owner password or the
  Meilisearch master key. Root
  in the container (`docker compose exec fvoci …`) can read them, and anyone
  who can run Docker commands on the host can read `.env` and the secrets.
  `docker inspect` shows the secret file paths, not their values. The server
  makes itself non-dumpable and refuses to start if the kernel does not
  allow it; the helpers still share uid 1000 file access with it (stored
  files and the scoped search key).
- **Local HTTP by default.** The app is published on `127.0.0.1` over plain
  HTTP. For other users or a domain, put a TLS reverse proxy in front and
  set an `https://` `FVOCI_PUBLIC_ORIGIN` (see `RUNNING.md`).
- **Rate limits are per server process and per direct client address.**
  Forwarded headers are ignored, so behind a reverse proxy all clients share
  one address (for example 30 sign-ins per 5 minutes). Each IPv6 address
  counts separately, and there is no per-account sign-in limit.
- **Workspace SSO login CSRF.** A manager of a team workspace with SSO can
  make a link that signs a visitor in through an identity provider that
  manager controls (sign-in starts are still GET). Team workspaces are
  created only by instance administrators.
- **Invitations accepted through a provider.** A new account opened this
  way gets the invited address and is linked to whatever identity the
  provider returns; the provider's email does not have to match or be
  verified (same as the original product; Naver never sends
  `email_verified` and Kakao often omits it). A decision is pending.
- **Share links outlive their creator's access** until someone revokes them
  (same as the original product; a decision is pending).
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
  comments, `export`) instead of guessing. Keep the values unquoted, as `env.example` writes them.
  With PostgreSQL `log_statement` set to `ddl` or `all` (off by default),
  the app role password that `restore.sh` sets reaches the PostgreSQL log.
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
docker compose up -d --wait
```

`compose.yml` pins the image by digest, so the tag cannot be moved under an
existing install. Compose refuses to start while a value in `.env` is empty,
and the `fvoci` container checks the values before it prepares the database
and starts the server. Open the published address and create the first
administrator on the setup page. Keep `.env` private and with your backups.

## Start, stop, restart

```sh
docker compose stop            # stop, keep everything
docker compose up -d --wait    # start again
docker compose restart fvoci   # restart the application only
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
