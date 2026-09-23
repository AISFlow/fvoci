# Running the Rust slice

## Toolchain

Install Rust 1.98.1 (see `rust-toolchain.toml`) or point `CARGO_HOME`, `RUSTUP_HOME`, and `PATH` at your toolchain.

## Environment

| Variable | Purpose |
| --- | --- |
| `DATABASE_URL` | Migration owner connection (superuser or schema owner). Used only by `fvoci-migrate` and startup migration — **never** for request handling. |
| `DATABASE_APP_URL` | Application DML role (non-superuser, no `BYPASSRLS`). Required; must differ from `DATABASE_URL` in URL and role name. |
| `PASSWORD_PEPPER_KEYS` | JSON map of pepper key id → 64-char hex. |
| `PASSWORD_PEPPER_ACTIVE_KEY_ID` | Active pepper id. |
| `FVOCI_BIND` | Listen address (default `127.0.0.1:0`). |
| `FVOCI_PUBLIC_ORIGIN` | Expected browser `Origin` for mutating routes (default `http://localhost:5173`). |
| `FVOCI_COOKIE_SECURE` | `true`/`1` to set `Secure` on session cookies; defaults from `FVOCI_PUBLIC_ORIGIN` scheme. |
| `FVOCI_BRANDING_NAME` | Setup status branding (default `FVOCI`). |

Remote PostgreSQL with TLS: use `sslmode=require` (or stricter) in both URLs. The crate uses SQLx `runtime-tokio-rustls`.

## Provision app role (after migrate)

```sh
export DATABASE_URL='postgres://owner@host:5432/fvoci?sslmode=require'
cargo run --bin fvoci-migrate
psql "$DATABASE_URL" -v app_role=fvoci_app_prod -f scripts/grant-app-role.sql
# create LOGIN role separately with a unique password; never commit credentials
export DATABASE_APP_URL='postgres://fvoci_app_prod:***@host:5432/fvoci?sslmode=require'
```

## Start server

```sh
cargo run --bin fvoci-server
```

Migrations run once at startup via the owner URL; the server connects only through `DATABASE_APP_URL`.

## Tests

Pure unit tests:

```sh
cargo test --lib
```

DB integration tests (isolated DB + unique app role per run):

```sh
export TEST_DATABASE_URL='postgres://admin@host:5432/postgres?sslmode=require'
cargo test --features db-tests --test db_integration
```

Optional: `scripts/provision-test-db.sh` creates a throwaway database and exports `TEST_DATABASE_URL` / `TEST_APP_DATABASE_URL` when `psql` is available.
