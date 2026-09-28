# FVOCI @VERSION@ (trial pre-release)

<!-- notes-for: TODO(release) replace with the 0.y.z version these notes describe -->

This is a 0.x trial release. It is not covered by any compatibility promise:
a later 0.y release may change configuration, data layout or behaviour.
Installs are never updated automatically.

- Source commit: `@SHA@`
- Image (multi-arch index): `@IMAGE_REF@`
  - linux/amd64: `@AMD64_DIGEST@`
  - linux/arm64: `@ARM64_DIGEST@`
- Release smoke (pulled the digest above anonymously on native amd64 and arm64
  runners, fresh install, first admin, core flows, restart, bootstrap failure):
  @RUN_URL@

## Accepted in this release

<!-- TODO(release): list the features accepted on main at @SHA@ (docs/rewrite.md feature table). -->

## Not verified or optional

<!-- TODO(release): list optional integrations (SMTP, OIDC, GitHub app, AI, S3) and anything shipped but not verified. -->

## Known limitations

<!-- TODO(release): list known limitations and open issues for this version. -->

## Install

Requires Docker Engine with the Compose plugin (v2.20+), on linux/amd64 or
linux/arm64. From an empty directory:

```sh
curl -fsSLO https://github.com/@REPOSITORY@/releases/download/v@VERSION@/compose.yml
curl -fsSLO https://github.com/@REPOSITORY@/releases/download/v@VERSION@/SHA256SUMS
sha256sum --ignore-missing -c SHA256SUMS
docker compose up -d --wait
```

`compose.yml` pins the image by digest, so the tag cannot be moved under an
existing install. The first start generates the instance secrets into a Docker
volume; no `.env` file is needed. Open the published address and create the
first administrator on the setup page.

## Start, stop, restart

```sh
docker compose stop            # stop, keep everything
docker compose up -d --wait    # start again
docker compose restart server  # restart the application only
docker compose down            # remove containers, keep volumes (data and keys)
```

`docker compose down -v` deletes every volume: the database, uploaded files,
the search index and the generated keys. Without the keys, encrypted data
from a backup cannot be read. Only use it to throw an install away.

## Data, keys and upgrades

- Back up before any upgrade (see `RUNNING.md` at `@SHA@` for backup and
  restore). Keep the backup of the key volume separate from the database backup.
- To upgrade, stop the stack, replace `compose.yml` with the one from the new
  release (check its `SHA256SUMS`), and run `docker compose up -d --wait`. The
  one-shot init service applies database migrations before the server starts;
  if it fails the server does not start.
- Migrations only move forward. Going back to an older 0.y release means
  restoring the backup taken before the upgrade.
- Keep the same Compose project name (the directory name by default), or the
  new stack starts with empty volumes.
