# Releasing 0.y.z trial builds

`.github/workflows/release.yml` turns a `v0.y.z` tag on a green `main` commit
into a multi-arch image index pushed by digest, a smoke run against that digest
on both architectures, then the tags `ghcr.io/aisflow/fvoci:0.y.z` (and `:0.y`)
and a GitHub pre-release with a digest-pinned `compose.yml`. There is no 1.0.0+ release, no automatic
update of installs, and no credential other than the job-scoped `GITHUB_TOKEN`.

## Version

The root `Cargo.toml` `[package] version` is the only source. The OpenAPI
`info.version` is `env!("CARGO_PKG_VERSION")`, `fvoci-server --version` prints
it with the build commit, and the image carries it as an OCI label.
`apps/web` has no version of its own. `scripts/release-check-version.sh --tag vX`
fails unless the tag, `Cargo.toml`, `Cargo.lock`, the committed
`apps/web/openapi.json` and the image tag agree and the version is `0.y.z`.

## Steps

1. Release-prep PR to `main`: set the version in `Cargo.toml`, refresh the lock
   entry (`cargo update -p fvoci-server --offline`), regenerate the API contract
   (`scripts/generate-api.sh`), and replace every `TODO(release)` block in
   `scripts/release-notes-template.md` with the accepted features, the
   unverified or optional parts and the known limitations of this version (see
   the feature table in `docs/rewrite.md`). Set its marker line to exactly
   `<!-- notes-for: 0.y.z -->`. The workflow refuses to run while a
   `TODO(release)` marker is left or the marker names another version, so the
   notes are rewritten for every release.
2. Wait until the merge commit's CI gates on `main` are green. The workflow
   checks that the tag is a first-parent `main` commit and that every
   `<workflow>-ci-gate` check run on it succeeded.
3. Push the tag from an authenticated session. Tags pushed with `GITHUB_TOKEN`
   do not start workflows.

   ```sh
   git tag -a v0.y.z <main-sha> -m "FVOCI 0.y.z"
   git push origin v0.y.z
   # or, for an existing tag: gh workflow run release.yml -f tag=v0.y.z
   ```

4. First release only: GHCR creates an organization package as private, so the
   smoke jobs fail with an anonymous `docker manifest inspect` error. No tag
   exists at that point. An org owner opens the `fvoci` package (organization →
   Packages), then Package settings → Danger Zone → Change visibility → Public.
   This cannot be undone. Then use "Re-run failed jobs" on the same run; the
   images are not rebuilt and the smoke pulls the same digest.

Before anything is built, `verify` runs `scripts/release-preflight.sh`: the
notes marker above, `ARG FVOCI_BUILD_SHA` in the `rust-build` stage of
`infra/rust/Dockerfile`, and the user compose rendered with a dummy digest and
accepted by `docker compose config` from an empty directory with an empty
environment. `bun test ./tools/release/dist.test.ts` dry-runs the renderer,
`tools/release/provenance.ts` and the preflight against
`scripts/testdata/release/compose.user.yml` (a copy of the user compose) and
the real `infra/rust/compose.user.yml` when present.

## Jobs and permissions

The workflow default is `contents: read`. `bun tools/ci/verify-workflows.ts`
rejects other triggers or write scopes outside the jobs below.

| Job | Runner | Permissions | Does |
| --- | --- | --- | --- |
| verify | ubuntu-26.04 | contents, checks, packages: read | tag format, first-parent `main`, CI gates (`tools/release/check-ci.ts`), version policy, preflight, existing release/image |
| build-amd64 / build-arm64 | ubuntu-26.04 / ubuntu-26.04-arm | contents: read, packages: write | native build of the tagged SHA, OCI labels, push by digest; skipped when the version already has a tagged image |
| index | ubuntu-26.04 | contents: read, packages: write | pushes the two-platform index **by digest, without a tag** (`tools/release/release-api.ts push-index` from the workflow ref), records the index and per-arch digests |
| dist | ubuntu-26.04 | contents: read | `scripts/release-dist.sh` at the tag: `compose.yml`, `env.example`, `INSTALL.md`, `release.json`, `RELEASE-NOTES.md`, and `SHA256SUMS` over those five; then `tools/release/provenance.ts` from the workflow ref records the smoke tooling commit |
| smoke-amd64 / smoke-arm64 | ubuntu-26.04 / ubuntu-26.04-arm | contents: read | `scripts/release-smoke.sh` from the workflow ref against the digest, no registry login |
| publish | ubuntu-26.04 | contents: read, packages: write | tags the smoked index `:0.y.z` (never moved), then `:0.y` when this is the newest `v0.y.*` tag, with `tools/release/release-api.ts` from the workflow ref |
| release | ubuntu-26.04 | contents: write | `scripts/release-publish.sh`: pre-release for the existing git tag |

Only `publish` and `release` run after both smoke jobs passed, and they are the
only jobs that create a tag or a release. Order:

1. `index`: push the index by digest. No `:0.y.z` or `:0.y` tag moves.
2. `smoke-amd64` and `smoke-arm64`: pull `ghcr.io/aisflow/fvoci@sha256:<index>`
   anonymously and exercise it.
3. `publish`: tag `:0.y.z`, then retag `:0.y` when this is the newest `v0.y.*`
   tag. The newest tag is read from the repository when publishing, not at
   `verify`.
4. `release`: checks that `:0.y.z` points at the recorded digest, then creates
   the pre-release.

`release.json` records the same order in `publishOrder`. If a smoke fails, the
index stays untagged: no user-facing tag names an image nobody exercised, and a
fixed re-run builds again.

## Product commit and smoke tooling commit

`verify`, the builds, `dist` and `release` check out the tagged SHA (`verify`
resolves it) and build, render or publish from it: the image and its
`FVOCI_BUILD_SHA`, the OCI `version`/`revision` labels, `compose.yml`,
`env.example`, `INSTALL.md`, the notes and the version checks. The smoke jobs
are test tooling and check out `github.sha`, the commit the run started from:
the tag commit on a tag push, the dispatching branch head (normally `main`) on
`workflow_dispatch`. The smoke still tests only the tagged product: it pulls
the index digest from the `release-dist` record, installs from the `compose.yml` and `env.example` that
`dist` rendered at the tag, and takes the expected version and OCI revision
from `release.json` (`sourceSha`), never from its own checkout. The test
clients, browser specs, fixtures and the Bun lockfile come from the workflow ref.
`index` and `publish` also check out `github.sha` and run the registry tooling
(`tools/release/release-api.ts`) from it: they read no file of the tagged
tree, only the recorded digests.

`dist` records both commits: `release.json` keeps `sourceSha` (product) and
gains `toolingSha` and `toolingRef`; `RELEASE-NOTES.md` ends with a Provenance
section naming both; `SHA256SUMS` is recomputed over the same five files. On a
tag push the two SHAs are equal.

- A smoke tooling fix (test client, spec, fixture) does not need a new tag:
  merge it to `main`, then `gh workflow run release.yml --ref main -f
  tag=v0.y.z`. The tag and its SHA stay as they are; when no `:0.y.z` image
  exists yet (the smoke failed before `publish`), the image is built again from
  that same SHA and the smoke from `main` exercises it. "Re-run failed jobs"
  on the old run does not pick up the fix: a re-run keeps that run's
  `github.sha`.
- A product change (server, web app, image, compose, env example, install
  guide, notes) needs a new patch tag `v0.y.(z+1)`; the smoke never makes a
  later commit's product part of an existing tag.

Checkouts use `persist-credentials: false`. `verify` fetches full history
(every branch, so `origin/main` is present) without a later authenticated
fetch.

All release runs share one concurrency group, `release-ghcr-fvoci`, with
`cancel-in-progress: false`. Two patch tags therefore cannot race on `:0.y`.
GitHub keeps only one pending run per group, so a third run queued behind it
is cancelled and must be started again with `gh workflow run release.yml -f
tag=v0.y.z`.

Re-runs never move a recorded digest:

- `scripts/release-existing.sh` reuses the image a release (or an earlier run
  that tagged but stopped before the release) already carries for the same
  version and SHA. It fails when a release records another digest, when a
  draft release exists, or when the image was built from another commit.
- `release-api.ts tag` refuses to move an existing `:0.y.z` to another digest.
- `release-publish.sh` is a no-op for a complete release with the same digest,
  refuses to replace assets, and stops on a draft left by an interrupted upload.

Delete a broken or draft release by hand before re-running.

Registry and release state come from HTTP status codes and JSON
(`tools/release/release-api.ts`), not from error text. A package the job token
cannot read (401/403, for example before the first release creates it) counts
as untagged only while no release records a digest. `publish` checks the tag
again with its write token before tagging.

## User compose contract

`release-dist.sh` reads the first existing file of `infra/rust/compose.user.yml`
and `infra/rust/compose.yml` at the tag, with `<compose>.env.example` and
`<compose>.INSTALL.md` next to it (published as `env.example` and
`INSTALL.md`). It names the FVOCI image exactly once, as the anchor the product
services share:

```yaml
x-fvoci-image: &fvoci-image ${FVOCI_IMAGE:-ghcr.io/aisflow/fvoci:0.1.0}
services:
  fvoci:
    image: *fvoci-image
```

The anchor value becomes `ghcr.io/aisflow/fvoci:0.y.z@sha256:<index>` in the
release asset, which is never committed back. Rendering fails if:

- `FVOCI_IMAGE` or `ghcr.io/aisflow/fvoci` appears anywhere else;
- no service uses `*fvoci-image`;
- any other interpolation is not `${VAR:?message}` (`$$` is a literal), so an
  unfilled `.env` stops Compose before any container exists;
- `<compose>.env.example` does not assign exactly the variables the compose
  reads, or assigns one twice;
- any service has an `env_file`.

The preflight renders the files and, in an empty directory with an empty
environment, requires `docker compose config` to refuse the unfilled
`env.example` as `.env` and to accept a filled one. In the filled config,
exactly one service publishes container port 8080 and uses the product image,
a `postgres` service exists, and each value generated into `.env` (its empty
entries) appears only in a service's `environment`, never on a command line,
and outside the app only where that service needs it (`postgres` the owner
password, `meilisearch` the master key).

The smoke fills `env.example` as a user would (a fresh value for each empty
entry) in an empty directory, finds the app as the service publishing 8080 (no
service names are assumed), and expects:

- `docker compose up -d --wait` to start it; an unfilled `.env` to be refused
  before any container exists; a placeholder value to be refused by the app;
- the app's pid 1 to be `fvoci-server` with uid and gid 1000, no supplementary
  groups, no capabilities and `NoNewPrivs: 1`, and no setuid/setgid file in the
  image; each service's container environment to hold only its `.env` values
  (outside the app, `postgres` the owner password and any other service at
  most the master key; no `*_FILE` setting); neither the database owner
  password nor the Meilisearch master key in the server's process tree or
  `/run`; the keyrings in the server's environment; a root `docker exec`
  (as the healthcheck) to start with the configured values, and nothing uid
  1000 can read under `/proc` to hold any of them;
- a failed preparation (a read-only database) to keep the server down, and a
  restart after the fix to recover;
- the keys and data to survive a second `up` and `down`/`up`;
- a `postgres` service whose `POSTGRES_USER` can query the `fvoci` schema.

The image build passes `--build-arg FVOCI_BUILD_SHA=<sha>`. The Rust build stage
of `infra/rust/Dockerfile` needs `ARG FVOCI_BUILD_SHA` for
`fvoci-server --version` to report it. The preflight refuses a Dockerfile
without it, and the smoke fails while `--version` reports `unknown`.

## Smoke coverage

Each architecture pulls the index by digest, anonymously, onto a clean daemon, checks the
per-arch digests, OCI labels and `--version`, and then, against the rendered
compose: health and readiness, the uid and secret boundary above, `fvoci-migrate --doctor`,
first-admin setup (a second setup is refused) and login, document create and
collaborative save (`scripts/install-smoke-collab.mjs`), document
import/export and public PDF (`tools/release/smoke-documents.ts`), HWPX
upload, extraction and byte-exact download, workspace search, `down`/`up` with
the same volumes (existing session, password login, body, attachment,
extraction, imports and search survive), and a failed preparation that must
keep the server down.

Browser coverage is a subset. Most Playwright specs seed users through the
owner database and the debug `fvoci-e2e-fixture` binary, which a release stack
does not have, so only specs that create the first admin through the setup page
run, each on a fresh stack: task edit, project document revisions, task
attachments and HWPX edit. The full browser suite stays in `web.yml`.
