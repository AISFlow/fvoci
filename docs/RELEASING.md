# Releasing 0.y.z trial builds

`.github/workflows/release.yml` turns a `v0.y.z` tag on a green `main` commit
into multi-arch images `ghcr.io/aisflow/fvoci:0.y.z` (and `:0.y`), a smoke run
against the published digest on both architectures, and a GitHub pre-release
with a digest-pinned `compose.yml`. There is no 1.0.0+ release, no automatic
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
   the feature table in `docs/rewrite.md`). The workflow refuses to run while a
   marker is left. Rewrite these sections for every release.
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
   smoke jobs fail with an anonymous `docker manifest inspect` error. An org
   owner opens the `fvoci` package (organization → Packages), then Package
   settings → Danger Zone → Change visibility → Public. This cannot be undone.
   Then use "Re-run failed jobs" on the same run; the images are not rebuilt.

## Jobs and permissions

The workflow default is `contents: read`. `scripts/ci_selection.py
verify-workflows` rejects other triggers or write scopes outside the jobs below.

| Job | Runner | Permissions | Does |
| --- | --- | --- | --- |
| verify | ubuntu-24.04 | contents, checks, packages: read | tag format, first-parent `main`, CI gates, version policy, notes markers, existing release/image |
| build-amd64 / build-arm64 | ubuntu-24.04 / ubuntu-24.04-arm | contents: read, packages: write | native build of the tagged SHA, OCI labels, push by digest; skipped when the version already has an image |
| manifest | ubuntu-24.04 | contents: read, packages: write | `imagetools create` of `:0.y.z` (and `:0.y` for the newest patch), records the index and per-arch digests |
| dist | ubuntu-24.04 | contents: read | `scripts/release-dist.sh`: `compose.yml`, `release.json`, `RELEASE-NOTES.md`, `SHA256SUMS` |
| smoke-amd64 / smoke-arm64 | ubuntu-24.04 / ubuntu-24.04-arm | contents: read | `scripts/release-smoke.sh`, no registry login |
| release | ubuntu-24.04 | contents: write | `scripts/release-publish.sh`: pre-release for the existing tag |

The GitHub release is created only after both smoke jobs pass, so a published
release always names a digest that was pulled and exercised.

Re-runs never move a recorded digest. `scripts/release-existing.sh` reuses the
image a release (or an earlier, unfinished run) already published for the same
version and SHA, and fails when a release records another digest or the image
was built from another commit. `release-publish.sh` is a no-op for a complete
release with the same digest and refuses to replace assets otherwise; delete a
broken release by hand before re-running.

## User compose contract

`release-dist.sh` reads the first existing file of `infra/rust/compose.user.yml`
and `infra/rust/compose.yml` at the tag. It must name the FVOCI image only as
`${FVOCI_IMAGE...}` (for example `${FVOCI_IMAGE:-ghcr.io/aisflow/fvoci:0.1.0}`);
every occurrence becomes `ghcr.io/aisflow/fvoci:0.y.z@sha256:<index>` in the
release asset, which is never committed back. The smoke expects the stack to
start with `docker compose up -d --wait` from an empty directory without any
environment or `.env`, a `server` service publishing container port 8080, at
least one one-shot service `server` depends on with
`service_completed_successfully`, and a `postgres` service whose
`POSTGRES_USER` can query the `fvoci` schema.

The image build passes `--build-arg FVOCI_BUILD_SHA=<sha>`. The Rust build stage
of `infra/rust/Dockerfile` needs `ARG FVOCI_BUILD_SHA` for
`fvoci-server --version` to report it; the smoke fails while it reports
`unknown`.

## Smoke coverage

Each architecture pulls the index anonymously onto a clean daemon, checks the
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
