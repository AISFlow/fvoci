# FVOCI @VERSION@ (trial pre-release)

<!-- notes-for: 0.1.1 -->

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

FVOCI 0.1.1 is a patch release on top of 0.1.0 (`57497e2f`): the same
database schema, configuration and install files, with the fixes and
additions listed under "Changes since 0.1.0". The features below were
accepted on `main` with their tests, CI and an independent review (feature
table in `docs/rewrite.md` at `@SHA@`):

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

## Changes since 0.1.0

- **Collaboration at the room limit.** When every collaboration room is
  taken, opening another document or task body now reclaims the least
  recently used room that nobody is in (and that has been empty for a few
  seconds) instead of waiting for the 30 s idle timer. If every room is in
  use, the body shows "not loaded yet" with the reason, instead of an empty
  body, and the browser retries with a bounded backoff (0.5 to 10 s) on a
  single connection instead of reconnecting rapidly. Typing elsewhere on the
  page (title, comments) is no longer reset by those retries, and edits made
  just before leaving a document are sent before its connection closes.
- **Attachment viewers and page loading.** Attachment viewers show their
  first page sooner, and the admin, Gantt, attachment and legal pages load
  on demand.
- **Metrics (`/metrics`).** `fvoci_outbox_lag_seconds` and
  `fvoci_outbox_xmin_stall_seconds` now read `NaN` before the first
  successful refresh and after a failed one (0.1.0 kept `0` or the last
  value), so threshold alerts no longer fire on stale values; alert on the
  new `fvoci_db_metrics_refresh_failures_total` or
  `fvoci_db_metrics_last_success_timestamp_seconds` instead. Also new:
  `fvoci_process_resident_memory_bytes`,
  `fvoci_collab_helper_resident_memory_bytes` and
  `fvoci_collab_helper_memory_budget_bytes`. An optional
  `compose.metrics.yml` override for a Prometheus scrape is in
  `infra/rust/` at tag `v@VERSION@`; it is not one of the release files.
- **Operator command `fvoci-migrate --outbox-reset`.** Diagnoses (default,
  read-only) or moves (`--apply --reason …`) the cursors of the outbox
  consumers (notifications, mail, push, webhooks, search index). `--apply`
  needs a PostgreSQL superuser (or `pg_read_all_stats`) and refuses while any
  other database session is connected, so the server must be stopped. It is
  documented for the development stack in `RUNNING.md`; there is no
  release-install procedure for it yet.

## Upgrading from 0.1.0

No database migration and no new `.env` value between 0.1.0 and 0.1.1:
back up, download the new `compose.yml` and `SHA256SUMS` and check them,
replace `compose.yml`, keep `.env`, and run
`docker compose up -d --wait --wait-timeout 900`. `fvoci-server --version`
then shows `@VERSION@` and the source commit. Reload open browser tabs
after the upgrade (the pages are loaded in new chunks). Going back to 0.1.0
means restoring the backup taken before the upgrade.

## Not verified or optional

These are shipped but off by default, or were only checked against local
stand-ins. Treat them as untested with a real provider:

- **Mail (SMTP):** tested against a local test relay only. Without SMTP,
  invitation links are shown in the app instead of mailed.
- **OIDC sign-in and workspace SSO:** tested with local test providers, not
  a real external identity provider.
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
- **Collaboration room-limit behaviour** was measured on a source build of an
  earlier revision of the change on one host; it was not measured on this
  published image or with many real users.
- **`/metrics` and `--outbox-reset`** are not part of the release smoke, and
  there is no automated upgrade test from the published 0.1.0 files to
  0.1.1 (the upgrade tests build both images from source).
- **Blank first page (under investigation).** In CI, the web app's first
  load occasionally rendered nothing (2 of about 8 test runs on fast
  runners since the page-loading change); it was not reproduced locally in
  about 1,800 attempts. If a page stays blank, reload it.

## Known limitations

- **No compatibility promise.** 0.x releases may change settings, data
  layout or behaviour between versions. Nothing updates an install on its own.
- **Secret boundary is within one container.** The `fvoci` container prepares
  the database and search as root, then runs the server as uid 1000. The
  server cannot read the secret files, the database owner password or the
  Meilisearch master key. Root
  in the container (`docker compose exec fvoci …`) can read them, and anyone
  who can run Docker commands on the host can read `.env` and the secrets.
  `docker inspect` shows the secret file paths, not their values.
- **Local HTTP by default.** The app is published on `127.0.0.1` over plain
  HTTP. For other users or a domain, put a TLS reverse proxy in front and
  set an `https://` `FVOCI_PUBLIC_ORIGIN` (see `RUNNING.md`).
- **Task updates from other browsers take up to about 0.75 s** to appear:
  live task changes are polled every 750 ms. Collaborative document text is
  pushed and not affected.
- **Collaboration room limit.** At most 64 documents or task bodies can be
  open for editing at the same time in this compose file (30 is the image
  default). While every room has a user in it, a newly opened body shows
  "not loaded yet" and retries until a room frees up; a room emptied a few
  seconds ago is kept for a returning user first. Reclaiming only applies to
  the room count, not to the helpers' memory budget.
- **Search key file must not be a symlink.** A `FVOCI_MEILI_KEY_FILE` path
  that is a symlink is refused at startup, for example a Kubernetes Secret
  volume entry.
- **Quoting in `.env` for restore.** `scripts/restore.sh` accepts values
  written unquoted or wholly in single or double quotes, as Compose reads
  them (inside single quotes `$` and backslashes are literal). It refuses
  other forms (escapes outside quotes, `$` outside single quotes, inline
  comments, `export`) instead of guessing. Keep the values unquoted, as `env.example` writes them.
- **Upgrades only as documented.** Stop and back up first, then replace
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
- To upgrade, stop the stack, replace `compose.yml` with the one from the new
  release (check its `SHA256SUMS`), keep `.env`, and run
  `docker compose up -d --wait --wait-timeout 900` (a long migration can
  outlast the default wait; `docker compose logs -f fvoci` shows progress).
  The `fvoci` container applies database
  migrations before the server starts; if that fails the server does not
  start, and it refuses to migrate while another server is still connected.
- Migrations only move forward. Going back to an older 0.y release means
  restoring the backup taken before the upgrade.
- Keep the same Compose project name (the directory name by default), or the
  new stack starts with empty volumes.
