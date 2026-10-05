# Schema baseline tools

Lineages: PostgreSQL `fvoci-postgres-060` (`migrations/postgres/060/`), SQLite-family
`fvoci-sqlite-060` (`migrations/sqlite/060/`). Both are registered in
`src/db/migrate.rs` with the SHA-256 of each step's exact text; the runner
records `(version, lineage, sql_sha256)` in the same transaction as the step
and refuses anything that is not an exact prefix of the compiled lineage. The
retired development lineages (PostgreSQL 001..056, SQLite `fvoci-sqlite-current-v1`)
are refused explicitly and never rewritten: 0.6 installs into an empty database
and data moves between installs with a current-format native archive.

## One-time semantic catalog comparison (PostgreSQL)

1. Install database A with `fvoci-migrate` built from the retired lineage's
   fixed tree (the pre-transition source) and apply `--grant-app-role`.
2. Install database B with `fvoci-migrate` built from this tree and apply
   `--grant-app-role` with the same role name.
3. Dump both catalogs with the extractor (any build of this tree):

       FVOCI_SCHEMA_CATALOG_DATABASE_URL=<owner url of A> FVOCI_SCHEMA_CATALOG_APP_ROLE=<role> \
       FVOCI_SCHEMA_CATALOG_OUT=/path/a.json cargo test --features db-tests --test schema_baseline_integration postgres_catalog_dump
       (same for B)

4. Compare: `python3 scripts/schema-baseline/compare-catalogs.py a.json b.json --report report.md`.
   Only the ledger may differ. The comparison is catalog-fact based
   (`pg_get_constraintdef`, `pg_get_indexdef`, `pg_get_triggerdef`, policies,
   `format_type`, defaults, ACLs, function bodies/options, seeds, row counts);
   raw DDL text is never compared.

## SQLite-family controls (no database)

`cargo test --features db-tests --test schema_baseline_integration` also runs:

- `sdk_rendering_reproduces_archived_remote_sqlite_schema_rows`: the archived
  actual loopback-SDK `sqlite_schema` rows of the retired SQLite lineage
  (`fixtures/legacy-sqlite-001-004-actual-sdk-schema-response.json`, exchange 10,
  236 rows) are reproduced byte-for-byte by `migrate::sdk_rendered_statements`
  applied to the frozen legacy texts (`fixtures/legacy-sqlite-00[1-4]_*.sql`).
  This is the positive control for the remote schema admission mode.
- `baseline_sqlite_steps_render_to_the_same_structural_catalog`: the baseline
  steps build the same tables/columns/FKs/indexes whether the engine receives
  the raw text (local SQLite) or the SDK rendering (remote libSQL).

The fixtures are frozen oracle inputs; they are not applied by any product path.
