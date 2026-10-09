-- FVOCI SQLite-family new-install baseline, lineage fvoci-sqlite-060, step 01: core.
-- the step ledger of this lineage.
-- Derived from the FINAL catalog of the retired development lineage (SQLite 001..006);
-- installs into an empty database only. A file carrying the retired lineage
-- fvoci-sqlite-current-v1 is refused by the runner and never rewritten.
-- The runner owns BEGIN IMMEDIATE, FK=ON, WAL/FULL, exact SQL digests and the
-- applied-step marker in the SAME transaction as each step's DDL.
-- STRICT enforces storage classes; UUIDs are exact 16-byte BLOBs. UTC instants
-- are signed epoch microseconds, decoded/encoded by checked Rust codecs.
-- Defaults use SQLite's millisecond clock without a floating-point conversion.
-- TEXT uses BINARY ordering. Decimal TEXT has no SQLite numeric comparison/sum
-- contract: named consumers must use exact decimal policy. RLS/definers,
-- current credentials/ACL and import event routing belong to named Rust
-- operations; a DB operator is trusted. Statement text is kept byte-identical
-- to the retired lineage so stored sqlite_schema definitions do not change.

CREATE TABLE schema_migrations (
    version INTEGER PRIMARY KEY NOT NULL CHECK (version>0),
    lineage TEXT NOT NULL CHECK (lineage='fvoci-sqlite-060'),
    sql_sha256 TEXT NOT NULL CHECK (length(sql_sha256)=64 AND sql_sha256 NOT GLOB '*[^0-9a-f]*'),
    applied_at INTEGER NOT NULL
) STRICT;
