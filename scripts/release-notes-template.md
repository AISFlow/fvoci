# FVOCI @VERSION@ (trial pre-release)

<!-- notes-for: 0.3.0 -->

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

FVOCI 0.3.0 is a minor release on top of 0.2.0 (`d560ac8f`). It changes the
install files: `compose.yml` now passes the `.env` values to each service as
container environment instead of Compose secret files, and the new image
refuses to start with a `compose.yml` of an earlier release (see "Upgrading
from 0.2.0"). It also adds an optional way to move attachment bytes directly
between the browser and S3 storage, off by default and chosen by an
environment variable or an admin setting. There is no database migration
(the newest is still 044, from 0.2.0) and no new or changed `.env` value.
Workspace SSO needs an enterprise license (`workspaceSso`), and a published
build trusts no license key (`src/license-trust.json` is empty), so a
published install cannot turn it on; the workspace SSO items below apply
only to a build that accepts such a license, except the change to the SSO
start endpoint's refusals, which applies to every build. It is still a 0.x
trial with no compatibility promise. The changes are listed under "Changes
since 0.2.0". The features below were accepted on `main` with their tests,
CI and an independent review (feature table in `docs/rewrite.md` at
`@SHA@`):

- **Accounts:** first-admin setup, sign-in and sessions, profile, password
  reset, email and password change, account deletion and export, magic links,
  personal API tokens, TOTP two-factor sign-in, OIDC sign-in, and workspace
  SSO (licensed builds only, see above).
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

## Changes since 0.2.0

### Install files

- **`.env` values are container environment.** `compose.yml` no longer turns
  the passwords and keyrings into Compose secret files (`/run/secrets`,
  `<VAR>_FILE` settings). Each service gets only the `.env` values it uses,
  as environment: `fvoci` all of them except the published port, `postgres`
  only `POSTGRES_PASSWORD`, `meilisearch` only `MEILI_MASTER_KEY`. `.env`
  keeps the same nine variables and values; nothing is regenerated.
- **What this exposes.** Anyone who can run Docker commands on the host can
  read every value with `docker inspect`, `docker compose config` or
  `docker compose exec`, as they could already read `.env`; in 0.2.0
  `docker inspect` showed only the secret file paths. A root
  `docker compose exec fvoci …` session and the healthcheck start with every
  value the `fvoci` service gets, the owner password and master key included.
- **What stays the same.** The server still runs as uid 1000 without the
  database owner password or the Meilisearch master key in its environment
  (it holds the app password only inside `DATABASE_APP_URL`), and uid 1000
  cannot read a root process's environment. The exception is a session you
  start as uid 1000 (`docker compose exec -u 1000:1000`): it holds every
  value, and the server's uid can read it while it runs. The start step still
  checks the settings and prepares the database and search as root, then runs
  the server as uid 1000; variable names, the project name and the volumes
  are unchanged.
- **Retired `<VAR>_FILE` settings are refused.** The image entrypoint
  (`fvoci-migrate --start`) exits 2 when any of `POSTGRES_PASSWORD_FILE`,
  `FVOCI_APP_PASSWORD_FILE`, `MEILI_MASTER_KEY_FILE`,
  `PASSWORD_PEPPER_KEYS_FILE` or `ENCRYPTION_KEYS_FILE` is set, naming each
  one. An old `compose.yml` with only its image line changed therefore does
  not start.
- **Backup and restore.** `scripts/backup.sh` and `scripts/restore.sh` read
  the keys from the container configuration or `docker compose config`, and
  refuse an install whose `compose.yml` still uses secret files (0.1.x,
  0.2.0), pointing to the scripts of the release it runs.
- **Changing a setting.** A container keeps the environment it was created
  with: after editing `.env`, run `docker compose up -d`, which recreates the
  containers whose values changed; `docker compose restart` keeps the old
  values. Editing `.env` does not change a password or key already in use;
  rotation keeps its documented procedure (`RUNNING.md`).
- `INSTALL.md` now says to `chmod 600 .env` and to keep its copy apart from
  the database backups.

### Attachments

- **Two transfer modes.** `proxy`, the default and the only mode before,
  streams part uploads and original downloads through the API. `presigned`
  (S3 storage with `S3_PUBLIC_ENDPOINT` only) lets the browser, after the
  same authorization, PUT each part straight to the bucket and follow a `302`
  from `GET …/download` to a short-lived signed GET. Choose it with
  `FVOCI_ATTACHMENT_TRANSFER_MODE`, which wins over the admin setting
  `attachmentTransfer.mode` (Instance settings → 첨부 전송 방식), which wins
  over `proxy`. Local storage and API-token requests always use the proxy;
  `HEAD`, previews, share-link downloads, `--verify-storage` and restore stay
  on the API. Each upload session keeps the mode it was created with.
- **Refusals, not fallbacks.** Startup is refused for an invalid mode,
  `presigned` without S3 or `S3_PUBLIC_ENDPOINT`, a plain `http` endpoint
  under an `https` `FVOCI_PUBLIC_ORIGIN`, or an endpoint on the app's host.
  Saving `presigned` where storage cannot presign answers 400
  `attachment_transfer_unavailable`; a stored `presigned` that can no longer
  apply falls back to `proxy` for new transfers, with a startup warning and a
  notice on the admin page. A failed presigned transfer is never retried
  through the API.
- **Setup:** the public bucket host, the bucket CORS rule (the exact
  `FVOCI_PUBLIC_ORIGIN`, exposing `ETag`), a lifecycle rule for incomplete
  multipart uploads, and the URL lifetimes
  (`FVOCI_ATTACHMENT_PRESIGN_PART_TTL_SECS`, default 900 s;
  `FVOCI_ATTACHMENT_PRESIGN_DOWNLOAD_TTL_SECS`, default 60 s) are described
  under "Attachment transfer modes" in `RUNNING.md` at `@SHA@`.
- **Issued URLs cannot be revoked.** A signed download URL works until it
  expires, even after the permission is revoked, the mode is switched back or
  the attachment is deleted (until the object is reclaimed). A part URL can
  stage bytes into its upload, though not publish them, until it expires or
  the multipart upload is completed or aborted. Rotating the S3 access key
  invalidates all of them at once. The server does not log signed URLs;
  browser traces and HAR files of presigned transfers contain them.

### Tasks: Gantt

- **New page.** The project Gantt is a new page built with Vue 3 and Nuxt
  UI; moving between it and any other page is a full page load. The React
  Gantt is removed. The page saves through the existing task `PATCH` with
  `expectedDates` as the layout returned them, so permissions, dependency
  checks and conflicts stay on the server.
- **What a change writes.** Moving a bar writes only the date fields the task
  has. The start handle writes only the start date and the end handle only
  the due side. A moved `dueAt` keeps its time of day.
- **Keyboard and errors.** Arrow keys move a bar by a day and Shift+Arrow
  moves its end. A 409 conflict shows a message and refetches; a 400
  dependency contradiction shows a message and snaps the bar back.
- Today and the default month use the user's time zone.
- The React login page now honours `returnTo`: after signing in it opens the
  same-site page that sent you there, such as the Gantt.
- **Limitations:** a `dueAt` moved across a daylight-saving change can land
  one day off the drawn bar; in overlap mode the rail rows do not line up
  with the lanes; only Chromium was tested.

### Security and defect fixes

- **Linking a sign-in provider needs an `Origin` header.**
  `POST /api/v1/auth/oidc/{provider}/link` without an `Origin` header, or
  with one that cannot be read, is refused with 403 `origin_mismatch` and
  issues no state, as the invitation start already was. The web app always
  sends it.
- **Workspace SSO start refusals return to the login page** (every build).
  `GET /api/v1/auth/sso` answered a refusal with a raw JSON error page. Every
  refusal is now a 302 to `/login?error=<code>`. A published build, which
  cannot turn workspace SSO on, refuses every request: it now answers a 302
  to `/login?error=provider_not_configured` where it answered 404
  problem+json, and over the per-address limit a 302 to
  `/login?error=rate_limit_exceeded` where it answered 429 with
  `Retry-After`. Refusals no longer carry `Retry-After` and no longer count
  as 429 or 5xx in the HTTP metrics. Only the login page's workspace SSO
  form is limited to licensed builds.
- **Date-times with non-ASCII digits.** A Unicode digit in a date-time (for
  example `effectiveAt` in `POST /api/v1/admin/legal`) made the parser panic
  and dropped the connection, and one in the fraction was accepted. Only
  ASCII digits are accepted now; such a request gets 400 `invalid_input`.
- **An unsendable address no longer stalls mail.** An event whose only
  recipient's address passed the app's email check but not the mail
  library's mailbox parser (for example `a..b@example.com`) failed about five
  times and dead-lettered, holding back every later mail event for about 15 s
  meanwhile. Such a recipient is now skipped and logged as
  `mail.recipient_rejected` with `code=invalid_recipient` (without the
  address), and the event completes. An event all of whose recipients the
  mail server refuses still dead-letters.
- **Collaboration memory admission.** Rooms starting at the same time could
  all be admitted past `FVOCI_COLLAB_MEMORY_BUDGET`, because each read the
  same helper memory use. Each start's estimate is now checked and recorded
  under one lock and held until the room's first successful load.
- **Dead collaboration rooms are reclaimed on HTTP use.** A revision restore,
  body write or live read over HTTP on a room whose actor had died answered
  503 until the idle sweep ran; it now reclaims the room and starts a new
  one, as opening the document already did.
- **HWP extraction deadline.** Time spent waiting for the single HWP helper
  slot was taken out of the helper's own parse time. The helper now gets its
  full timeout from admission; the wait is still bounded by the same timeout,
  so one HWP extraction can take up to about twice the timeout.
- **Document helpers first in an out-of-memory kill.** The HWP helper and the
  preview, Office and Markdown conversion children now set
  `oom_score_adj=1000` on themselves, as collaboration helpers have since
  0.2.0, so a cgroup out-of-memory kill prefers them to the server.
- **Restart-required list.** If a server process's first settings request
  was a change, the admin settings' list of changes that need a restart
  (`restartRequired`) stayed empty for the life of that process. The start
  snapshot is now also taken at the first change.
- **Task reschedule conflicts.**
  `PATCH /api/v1/workspaces/{workspace_id}/tasks/{task_id}` compared
  `expectedDates.dueAt` to the microsecond, while the Gantt layout,
  collections and browsers work in milliseconds, so rescheduling a task
  whose stored `dueAt` had sub-millisecond digits answered 409
  `document_version_mismatch`. `dueAt` is now compared to the millisecond
  (the stored value truncated); start and due dates are still compared
  exactly.

### Performance

Each figure is the author's local measurement from the change's pull
request, not a measurement of the published image.

- **Search permissions in one statement.** Building a user's project
  permissions for search, dashboards and collections ran two statements per
  project; it is now one statement whatever the number of projects, and the
  guest collection list reads it once instead of once per wiki collection.
  Statements per request with 3 and 30 projects, before → after (one test
  run each, wall time not measured): member dashboard 32 and 86 → 26 and 26;
  guest dashboard 26 and 80 → 20 and 20; member collection list 15 and 69 →
  9 and 9; member collection query 26 and 80 → 20 and 20. Guest collection
  list with 1 and 5 wiki collections: 13 and 25 → 11 and 15.
- **Collaborative edits read less.** Saving each client update no longer
  reads the stored snapshot and body to lock the document. Bytes read from
  PostgreSQL per update: 1,006 → 852 (small document), 486,983 → 852 (about
  475 KiB body), 4,681,285 → 852 (4 MiB snapshot and that body); statements
  20 → 18 (debug build, PostgreSQL 18.3, 300 updates per document; byte and
  statement counts are exact).
- **HWP/HWPX text extraction** no longer waits out a 250 ms poll after the
  helper has finished: p50 251.1 → 11.1 ms and p95 251.2–251.3 → 11.3 ms per
  extract call (release build, one HWP fixture, 3 × 50 calls; a
  microbenchmark of the call, not of the server).

### For developers

- The web workspace is installed, built and tested with Bun 1.4.2 and one
  `bun.lock`, including the image's web build stage; the runtime image still
  contains no JavaScript runtime. An existing checkout must delete its npm
  `node_modules` before `bun ci` (`RUNNING.md`, "Web UI").
- The web app now has a Vue 3 + Nuxt UI app beside the React one, sharing one
  `index.html`: `src/boot.ts` starts Vue for the project Gantt and React for
  every other path (`RUNNING.md`, "Web UI (React and Vue)"). Type checking
  runs `tsc` and `vue-tsc` under Bun, which needs a Bun `patchedDependencies`
  patch of `@volar/typescript` 2.4.28 (the open upstream fix
  volarjs/volar.js#310) until a Volar release contains it; the image's web
  build stage copies `patches/` for it.
- An opt-in check of instance OIDC against a local Keycloak was added
  (`scripts/keycloak-oidc-e2e.sh`, run with Bun).

## Upgrading from 0.2.0

Upgrading directly from 0.1.x? Read the 0.2.0 notes too
(https://github.com/@REPOSITORY@/releases/tag/v0.2.0): the steps below then
also apply migration 044, after which 0.1.x no longer starts on the database.

1. Back up first, with `scripts/backup.sh` from the source at `v0.2.0` while
   0.2.0 runs (the 0.3.0 script refuses a container started from the 0.2.0
   `compose.yml`), and keep the copy of `.env` apart from it (see "Data, keys
   and upgrades" below). With S3 storage `backup.sh` refuses; follow
   "S3 storage backup" in `RUNNING.md` instead. From 0.1.x, use that
   release's scripts.
2. Download the 0.3.0 `compose.yml` and `SHA256SUMS` into an empty directory
   and run `sha256sum --ignore-missing -c SHA256SUMS` there. Copy
   `compose.yml` over the old one and keep `.env` as it is: the variables and
   values are the same, and nothing is regenerated. Run
   `docker compose up -d --wait --wait-timeout 900` in the install directory.
   Compose recreates all three containers on the same volumes; there is no
   database migration.
   `docker compose exec fvoci /opt/fvoci/bin/fvoci-server --version` then
   shows `@VERSION@` and the source commit. Reload open browser tabs.
3. **Replace the whole file.** A 0.2.0 `compose.yml` with only its image line
   changed does not start: the `fvoci` container exits 2 naming each retired
   `<VAR>_FILE` setting, and keeps restarting. `docker compose up -d` without
   `--wait` does not report this; check with `--wait` or
   `docker compose logs fvoci`.
4. **Values are now visible to Docker users.** `docker inspect`,
   `docker compose config` and `docker compose exec` show the `.env` values;
   do not paste their raw output into logs, issues or reviews. The 0.2.0
   secret files existed only inside the old containers.
5. **S3 storage** (settings in `compose.override.yml`): transfers stay on the
   API unless you choose `presigned`. The new optional variables are
   `FVOCI_ATTACHMENT_TRANSFER_MODE`, `FVOCI_ATTACHMENT_PRESIGN_PART_TTL_SECS`
   and `FVOCI_ATTACHMENT_PRESIGN_DOWNLOAD_TTL_SECS`; list only those you set
   (leave the mode out to choose it on the admin page; an empty TTL refuses
   startup). `S3_PUBLIC_ENDPOINT` was checked but not used before: a value
   left set now makes `presigned` available, adds its origin to the content
   security policy (`connect-src`, `img-src`), and refuses startup when it is
   plain `http` under an `https` `FVOCI_PUBLIC_ORIGIN` or uses the app's
   host. Correct or remove it before upgrading. The developer stack's S3
   overlay (`infra/rust/compose.s3.yml`) passes the three variables from
   `.env`.
6. **API clients:** linking a provider without an `Origin` header gets 403
   `origin_mismatch`; `GET /api/v1/auth/sso` answers every refusal, in every
   build, with a 302 to `/login?error=<code>` instead of a 404 or 429
   problem+json (a published build refuses every request, with
   `provider_not_configured`, or `rate_limit_exceeded` over the limit);
   `GET …/projects/{project_id}/task-layout` also returns `canEdit`, `links`,
   `linkTotal` and `calendar` (nothing removed); with `presigned` on, browser
   uploads and original downloads use signed storage URLs (`GET …/download`
   answers 302), while API-token requests keep the API paths.
7. **Going back to 0.2.0** is not supported. The documented way back is
   restoring the pre-upgrade backup with the 0.2.0 `restore.sh` and
   `compose.yml`, which loses the writes made since. 0.3.0 adds no
   migration, and in the upgrade check the 0.2.0 image, started with its
   `compose.yml` and the same `.env` on the upgraded database (local
   storage, no presigned upload sessions), passed its schema check and served
   the same data. Doing so is unsafe while upload sessions opened in
   `presigned` mode are unfinished: 0.2.0 does not know their mode.

## Not verified or optional

These are shipped but off by default, or were only checked against local
stand-ins. Treat them as untested with a real provider:

- **Mail (SMTP):** tested against a local test relay only, including scripted
  per-recipient refusals. Without SMTP, invitation links are shown in the
  app instead of mailed.
- **OIDC sign-in and workspace SSO:** instance OIDC sign-in, account
  linking, invitation acceptance and sign-out were checked against a real
  Keycloak 26.7.4 on the same host, in headless Chromium 153, with a server
  built from source that contains the other 0.3.0 product changes (the
  opt-in `scripts/keycloak-oidc-e2e.sh`, last run at `8ddd1486`; not in
  CI). Workspace SSO was checked against Keycloak only in a Rust test under
  a test entitlement. Not tried:
  any external identity provider (Google, Microsoft, Naver, Kakao, or one on
  another site), HTTPS, a reverse proxy or Secure cookies, the container
  install, Keycloak production mode and key rotation, and browsers other
  than Chromium. Other tests use local test providers.
- **GitHub app:** tested against a local fake of the GitHub API.
- **AI actions and semantic search:** optional, and need an
  OpenAI-compatible embeddings endpoint that you provide. No real provider
  was used.
- **S3 storage:** checked against a local S3-compatible store only; an image
  upgrade and versioned rollback were checked in an earlier release (#190).
  The 0.2.0 → 0.3.0 upgrade was not tried on S3. No cloud provider was used.
- **Presigned attachment transfer:** Rust integration tests against the same
  local store, and a manual Chromium check across origins (upload, download,
  range, image viewer, switching back, and a narrowed CORS rule that must
  fail) that is not part of CI. Not run: real AWS S3 (CORS on the redirected
  download, enforcement of the signed length, virtual-host style addressing,
  the `response-*` overrides, lifecycle rules), other S3-compatible services,
  a CDN or reverse proxy in front of the bucket, Firefox and Safari, and
  clock skew.
- **Two-factor sign-in:** codes and QR enrolment are tested, but no real
  authenticator app has scanned the QR code.
- **Web Push:** one real delivery was observed, with Chrome for Testing on
  Linux through Google's push service. Other browsers and push services were
  not tried.
- **Korean input (IME):** checked with a real Linux (IBus) input method in
  Chromium only. Windows, macOS and mobile input methods were not tried.
- **Collaboration capacity:** the room-limit behaviour was measured for 0.1.1
  (see the 0.2.0 notes). The collaboration changes of 0.2.0 and 0.3.0
  (helpers, memory admission, saving updates) were not measured that way,
  nor under Docker's default AppArmor profile, nor with many real users.
- **Upgrade from 0.2.0:** checked by hand once, on amd64 with local storage
  and no presigned upload sessions, from an install made with the published
  0.2.0 release files and backed up with the v0.2.0 `backup.sh`. The new
  image was built from the release-preparation commit `04f61092`, not the
  published image; that commit predates the Gantt change (#255), which
  changes only the web build (web code and the Dockerfile line that copies
  `patches/`). An old `compose.yml` with only its image line changed exited
  2 naming the five retired `<VAR>_FILE` settings and changed nothing (the
  database dump was byte-identical). The documented upgrade applied no
  migration (still 044) and kept all seeded data with 0 differences
  (sign-in, a document and its body, comments, attachment bytes, task dates
  and a sealed two-factor secret); each service got the documented `.env`
  names, and the server's environment had none of the preparation-only
  values such as the owner password or the master key. The 0.2.0 image then
  started again on the upgraded database with the same data (step 7), and
  the 0.2.0 backup restored with the 0.3.0 `restore.sh` into a new project.
  Not tried: arm64, S3 storage, presigned upload sessions, and an automated
  upgrade test.
- **`/metrics` and `--outbox-reset`** are not part of the release smoke.

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
