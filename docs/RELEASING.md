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
environment. `python3 scripts/test_release_dist.py` dry-runs the renderer and
the preflight against `scripts/testdata/release/compose.user.yml` (a copy of the
user compose) and the real `infra/rust/compose.user.yml` when present.

## Jobs and permissions

The workflow default is `contents: read`. `scripts/ci_selection.py
verify-workflows` rejects other triggers or write scopes outside the jobs below.

| Job | Runner | Permissions | Does |
| --- | --- | --- | --- |
| verify | ubuntu-24.04 | contents, checks, packages: read | tag format, first-parent `main`, CI gates, version policy, preflight, existing release/image |
| build-amd64 / build-arm64 | ubuntu-24.04 / ubuntu-24.04-arm | contents: read, packages: write | native build of the tagged SHA, OCI labels, push by digest; skipped when the version already has a tagged image |
| index | ubuntu-24.04 | contents: read, packages: write | pushes the two-platform index **by digest, without a tag** (`scripts/release-api.py push-index`), records the index and per-arch digests |
| dist | ubuntu-24.04 | contents: read | `scripts/release-dist.sh`: `compose.yml`, `release.json`, `RELEASE-NOTES.md`, `SHA256SUMS` |
| smoke-amd64 / smoke-arm64 | ubuntu-24.04 / ubuntu-24.04-arm | contents: read | `scripts/release-smoke.sh` against the digest, no registry login |
| publish | ubuntu-24.04 | contents: read, packages: write | tags the smoked index `:0.y.z` (never moved), then `:0.y` when this is the newest `v0.y.*` tag |
| release | ubuntu-24.04 | contents: write | `scripts/release-publish.sh`: pre-release for the existing git tag |

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
- `release-api.py tag` refuses to move an existing `:0.y.z` to another digest.
- `release-publish.sh` is a no-op for a complete release with the same digest,
  refuses to replace assets, and stops on a draft left by an interrupted upload.

Delete a broken or draft release by hand before re-running.

Registry and release state come from HTTP status codes and JSON
(`scripts/release-api.py`), not from error text. A package the job token
cannot read (401/403, for example before the first release creates it) counts
as untagged only while no release records a digest. `publish` checks the tag
again with its write token before tagging.

## User compose contract

`release-dist.sh` reads the first existing file of `infra/rust/compose.user.yml`
and `infra/rust/compose.yml` at the tag. It names the FVOCI image exactly once,
as the anchor the product services share:

```yaml
x-fvoci-image: &fvoci-image ${FVOCI_IMAGE:-ghcr.io/aisflow/fvoci:0.1.0}
services:
  bootstrap:
    image: *fvoci-image
```

The anchor value becomes `ghcr.io/aisflow/fvoci:0.y.z@sha256:<index>` in the
release asset, which is never committed back. Rendering fails if:

- `FVOCI_IMAGE` or `ghcr.io/aisflow/fvoci` appears anywhere else;
- no service uses `*fvoci-image`;
- any other `${...}` or `$VAR` interpolation is left (`$$` is a literal);
- any service has an `env_file`.

The smoke expects the stack to start with `docker compose up -d --wait` from an
empty directory without any environment or `.env`, with:

- the one-shot `bootstrap` service generating the install secrets;
- `server` and `bootstrap` on the product image;
- a `server` service publishing container port 8080;
- one-shot services other services wait on with
  `service_completed_successfully`: all of them must exit 0, and a forced
  failure of each must keep the server down;
- a `postgres` service whose `POSTGRES_USER` can query the `fvoci` schema.

The image build passes `--build-arg FVOCI_BUILD_SHA=<sha>`. The Rust build stage
of `infra/rust/Dockerfile` needs `ARG FVOCI_BUILD_SHA` for
`fvoci-server --version` to report it. The preflight refuses a Dockerfile
without it, and the smoke fails while `--version` reports `unknown`.

## Smoke coverage

Each architecture pulls the index by digest, anonymously, onto a clean daemon, checks the
per-arch digests, OCI labels and `--version`, and then, against the rendered
compose: health and readiness, one-shot services, `fvoci-migrate --doctor`,
first-admin setup (a second setup is refused) and login, document create and
collaborative save (`scripts/install-smoke-collab.mjs`), document
import/export and public PDF (`scripts/install-smoke-documents.py`), HWPX
upload, extraction and byte-exact download, workspace search, `down`/`up` with
the same volumes (existing session, password login, body, attachment,
extraction, imports and search survive), and a forced failure of each one-shot
service that must keep the server down.

Browser coverage is a subset. Most Playwright specs seed users through the
owner database and the debug `fvoci-e2e-fixture` binary, which a release stack
does not have, so only specs that create the first admin through the setup page
run, each on a fresh stack: task edit, project document revisions, task
attachments and HWPX edit. The full browser suite stays in `web.yml`.
