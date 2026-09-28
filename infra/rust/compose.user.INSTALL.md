# Installing FVOCI

Requirements: Docker Engine with the Compose plugin (v2.24 or later) and
`openssl` for generating secrets.

1. Put `compose.yml` and `env.example` from this release in an empty folder.
   Check them against `SHA256SUMS` (`sha256sum -c SHA256SUMS --ignore-missing`).
2. `cp env.example .env` and fill in every empty value with the command shown
   above it. Keep `.env` private and back it up: the database password, the
   password pepper and the encryption keys cannot be recovered or changed later.
3. `docker compose up -d --wait`
4. Open the address in `FVOCI_PUBLIC_ORIGIN` (default <http://localhost:8080>)
   and create the first administrator.

The server listens on `127.0.0.1` only. For a domain name, HTTPS, mail, S3
storage or other options, and for backup, restore and upgrades, see
`RUNNING.md` ("Install", "Backup and restore", "Upgrade") in the source
repository at the release tag. If the start fails, `docker compose logs fvoci`
names the setting to fix; the server does not start until preparation succeeds.
