use sqlx::postgres::PgPoolOptions;
use sqlx::{PgConnection, PgPool};

/// One compiled schema step of a lineage: its 1-based version, module name,
/// exact SQL text and the SHA-256 of that text pinned in the registry.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CompiledStep {
    pub version: i32,
    pub name: &'static str,
    pub sql: &'static str,
    pub sha256: &'static str,
}

/// A step receipt read from a database ledger.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AppliedStep {
    pub version: i32,
    pub lineage: String,
    pub sql_sha256: String,
}

/// What a PostgreSQL database's `fvoci.schema_migrations` says about itself.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum LedgerState {
    /// No `fvoci.schema_migrations` table: an empty database (or one that never
    /// completed step 01).
    Unprepared,
    /// The retired development lineage (a ledger without a `lineage` column).
    /// Its data is never converted in place.
    Retired { versions: Vec<i32> },
    /// Receipts of a baseline lineage, ascending by version.
    Applied(Vec<AppliedStep>),
}

/// PostgreSQL new-install baseline lineage. Every step of a lineage is applied in
/// order into an empty database; a later release appends steps to the same
/// lineage. A re-baseline starts a new lineage name.
pub const POSTGRES_LINEAGE: &str = "fvoci-postgres-060";

/// SQLite-family (local SQLite and remote libSQL) new-install baseline lineage.
/// These are executed steps, never PostgreSQL version markers.
pub const SQLITE_LINEAGE: &str = "fvoci-sqlite-060";
/// The retired SQLite-family development lineage, refused explicitly.
pub const RETIRED_SQLITE_LINEAGE: &str = "fvoci-sqlite-current-v1";

// Each step is one module of the baseline. The registry pins the SHA-256 of the
// exact compiled text; the runner refuses to apply or accept a step whose text
// no longer matches its pin, and records (version, lineage, sha256) in the same
// transaction as the step's DDL. Add a step at the end; never edit an accepted one.
const POSTGRES_STEPS: &[(&str, &str, &str)] = &[
    (
        "01_core",
        include_str!("../../migrations/postgres/060/01_core.sql"),
        "89d671b0af6aa87767810b3f9585d318d2f8a931400042c733ec4d9b9812e72d",
    ),
    (
        "02_identity",
        include_str!("../../migrations/postgres/060/02_identity.sql"),
        "5ba19c81003212a80f1865ddd2fde5f0e01fed4a5f40839c35c5e7b4b3bf5d5c",
    ),
    (
        "03_workspaces",
        include_str!("../../migrations/postgres/060/03_workspaces.sql"),
        "6460f24a11f8cc47e7e839a585c961aa52f766f837ea614a2e1d781c6b05db49",
    ),
    (
        "04_events",
        include_str!("../../migrations/postgres/060/04_events.sql"),
        "7e12b1711f3ed1c9c1c9c8751de7769b323edfda868901d23941632ea1c1aca6",
    ),
    (
        "05_projects",
        include_str!("../../migrations/postgres/060/05_projects.sql"),
        "54479862b9f3087dba696d55cf0a41213999f540ac1675e3ce1d3f3039bccb23",
    ),
    (
        "06_documents",
        include_str!("../../migrations/postgres/060/06_documents.sql"),
        "5b2d53388145e9dbe9509dfb8e7cc11d49d51f140a36eafaa669ad9fa12a1a30",
    ),
    (
        "07_attachments",
        include_str!("../../migrations/postgres/060/07_attachments.sql"),
        "038655eb9ebb657ee412ed8205ebd98566abff3c8679f66e910f354124784e8f",
    ),
    (
        "08_collections",
        include_str!("../../migrations/postgres/060/08_collections.sql"),
        "cdc15a0de9ce2fd38a169e3f7e611b359a43adc2c0f330f04eaa6580f19b4239",
    ),
    (
        "09_notifications",
        include_str!("../../migrations/postgres/060/09_notifications.sql"),
        "42643610bef66a1908bd72b9e07775e99a453d1e9f77cb4e6f1ecbbe0f432de0",
    ),
    (
        "10_integrations",
        include_str!("../../migrations/postgres/060/10_integrations.sql"),
        "3d55991ea9581bee6d63cb04b649c0b8bb01ecb235efb26152c10e14cb5464ca",
    ),
    (
        "11_imports",
        include_str!("../../migrations/postgres/060/11_imports.sql"),
        "a3a20d34e637a5f7a88851e3eef21ce96ca68b43a35583667ca80a6f57bf1f27",
    ),
    (
        "12_operations",
        include_str!("../../migrations/postgres/060/12_operations.sql"),
        "5783e5cd70fcf7e67d908c8088851354e5c9d8579cba9f51c3b7762795ffb5ba",
    ),
];

const SQLITE_STEPS: &[(&str, &str, &str)] = &[
    (
        "01_core",
        include_str!("../../migrations/sqlite/060/01_core.sql"),
        "d57046979d66dee08592750db106220bd244d8cd89faef2cbc682d2b15d9b903",
    ),
    (
        "02_identity",
        include_str!("../../migrations/sqlite/060/02_identity.sql"),
        "6ec58d924591a5c0d7e3c2ae9a0ed385e1517fd9cf5110abd4a3f5a4c36b83ed",
    ),
    (
        "03_workspaces",
        include_str!("../../migrations/sqlite/060/03_workspaces.sql"),
        "f35dd5bdbd22aea0f1f102e602121e5644a834aaca3c21329a2363edba5ce45d",
    ),
    (
        "04_events",
        include_str!("../../migrations/sqlite/060/04_events.sql"),
        "f0112ff64c73060cc6eba093e5d0f6f26a455438997e303bbdc62d36810b1c4c",
    ),
    (
        "05_projects",
        include_str!("../../migrations/sqlite/060/05_projects.sql"),
        "f49faf57942ba68126107c8e3e950663263fd14ae4dc2ae1e721aa2d59e8438d",
    ),
    (
        "06_documents",
        include_str!("../../migrations/sqlite/060/06_documents.sql"),
        "2ef7c969db051af6382e33cb0368d03b2dc7190f1c5954b1d853f80aee229cb3",
    ),
    (
        "07_attachments",
        include_str!("../../migrations/sqlite/060/07_attachments.sql"),
        "b1c475434ab3bd2189ffdd088d656bed28a8fe802d3847ec21a18c4a580e6d0d",
    ),
    (
        "08_collections",
        include_str!("../../migrations/sqlite/060/08_collections.sql"),
        "24d9684e45e74bc4dfac864f0ebbf1ac5723dbed0142f56eddce269b0b2a0154",
    ),
    (
        "09_notifications",
        include_str!("../../migrations/sqlite/060/09_notifications.sql"),
        "431c178474c5ea5149b3bb6e081259d72034fdf6ef61f0230c0fd680a53e717f",
    ),
    (
        "10_integrations",
        include_str!("../../migrations/sqlite/060/10_integrations.sql"),
        "58ba69967d340d81b0f42d16238645857548e0fcbdc78750c5163f6cc2465629",
    ),
    (
        "11_imports",
        include_str!("../../migrations/sqlite/060/11_imports.sql"),
        "29981388ae1330dbafb88252ac5e9b1cdde5a9ec4e9d94a3a383c5d72172a746",
    ),
    (
        "12_operations",
        include_str!("../../migrations/sqlite/060/12_operations.sql"),
        "090c90eb21943c93bd159c11423e9afdfd99a96f89ce849192fa705dc5b35423",
    ),
];

fn steps_of(registry: &'static [(&'static str, &'static str, &'static str)]) -> Vec<CompiledStep> {
    registry
        .iter()
        .enumerate()
        .map(|(index, (name, sql, sha256))| CompiledStep {
            version: (index + 1) as i32,
            name,
            sql,
            sha256,
        })
        .collect()
}

/// The PostgreSQL baseline steps compiled into this binary, ascending.
pub fn compiled_postgres_steps() -> Vec<CompiledStep> {
    steps_of(POSTGRES_STEPS)
}

/// The SQLite-family baseline steps compiled into this binary, ascending.
pub fn compiled_sqlite_steps() -> Vec<CompiledStep> {
    steps_of(SQLITE_STEPS)
}

/// The exact compiled SQLite-family DDL, in order. Test fixtures that build a
/// current-schema file without the runner execute these texts verbatim.
pub fn compiled_sqlite_sql() -> impl Iterator<Item = &'static str> {
    SQLITE_STEPS.iter().map(|(_, sql, _)| *sql)
}

/// Number of PostgreSQL steps compiled into this binary.
pub fn compiled_migration_count() -> usize {
    POSTGRES_STEPS.len()
}

/// Latest PostgreSQL step version compiled into this binary.
pub fn latest_migration_version() -> i32 {
    POSTGRES_STEPS.len() as i32
}

/// Every PostgreSQL step version compiled into this binary, ascending.
pub fn compiled_migration_versions() -> Vec<i32> {
    (1..=POSTGRES_STEPS.len() as i32).collect()
}

pub(crate) const MIGRATION_LOCK_KEY: i64 = 847_291_003_552;

const APP_ROLE_GRANTS: &str = include_str!("../../scripts/grant-app-role.sql");
const APP_ROLE_PLACEHOLDER: &str = ":\"app_role\"";

pub const SCHEMA_GATE_OPERATOR_HINT: &str =
    "run `fvoci-migrate` then `fvoci-migrate --grant-app-role <app-role>` before starting fvoci-server";

/// Operator message for a database carrying the retired development lineage.
pub const RETIRED_LINEAGE_HINT: &str = "this database carries the retired FVOCI development \
    migration lineage, which 0.6 does not upgrade in place; install into an empty database and \
    move the data with a current-format native archive (export from the old server, restore here)";

fn sha256_hex(bytes: &[u8]) -> String {
    use sha2::{Digest, Sha256};
    hex::encode(Sha256::digest(bytes))
}

fn verify_compiled_digest(step: &CompiledStep) -> Result<(), String> {
    if sha256_hex(step.sql.as_bytes()) != step.sha256 {
        return Err(format!(
            "compiled step {} ({}) does not match its registry digest; this binary is corrupt",
            step.version, step.name
        ));
    }
    Ok(())
}

/// Compares a ledger with the compiled lineage. Every compiled step must be
/// applied with the same lineage and digest, in order, and nothing else may be
/// recorded. The database ledger is never modified by this check.
pub fn schema_gate(ledger: &LedgerState, compiled: &[CompiledStep]) -> Result<(), String> {
    let expected = compiled.len() as i32;
    match ledger {
        LedgerState::Unprepared => Err(format!(
            "database has no applied migrations (expected {POSTGRES_LINEAGE} version {expected}); {SCHEMA_GATE_OPERATOR_HINT}"
        )),
        LedgerState::Retired { versions } => Err(format!(
            "database schema ledger has development migrations {versions:?} without a lineage; {RETIRED_LINEAGE_HINT}"
        )),
        LedgerState::Applied(applied) => {
            let pending = pending_steps(applied, compiled)?;
            if pending > 0 {
                let missing: Vec<i32> = compiled[compiled.len() - pending..]
                    .iter()
                    .map(|step| step.version)
                    .collect();
                return Err(format!(
                    "database schema is missing migrations {missing:?} and is behind compiled version {expected}; {SCHEMA_GATE_OPERATOR_HINT}"
                ));
            }
            Ok(())
        }
    }
}

/// Number of compiled steps a ledger of this lineage still lacks (0 when
/// current). Any receipt that is not an exact prefix of the compiled lineage
/// (foreign lineage, altered digest, gap, or a version newer than this binary)
/// is an error, never a count.
pub fn pending_steps(applied: &[AppliedStep], compiled: &[CompiledStep]) -> Result<usize, String> {
    let expected = compiled.len() as i32;
    if applied.is_empty() {
        return Err(format!(
            "database has no applied migrations (a schema ledger without receipts; expected {POSTGRES_LINEAGE} version {expected}); {SCHEMA_GATE_OPERATOR_HINT}"
        ));
    }
    if applied.len() > compiled.len() {
        let newer: Vec<i32> = applied[compiled.len()..]
            .iter()
            .map(|step| step.version)
            .collect();
        return Err(format!(
            "database schema has migrations {newer:?} newer than this binary ({expected}); deploy a matching fvoci-server"
        ));
    }
    for (index, (receipt, step)) in applied.iter().zip(compiled).enumerate() {
        let position = (index + 1) as i32;
        if receipt.lineage != POSTGRES_LINEAGE {
            return Err(format!(
                "database schema receipt {} belongs to lineage {:?}, not {POSTGRES_LINEAGE}; install into an empty database",
                receipt.version, receipt.lineage
            ));
        }
        if receipt.version != position {
            return Err(format!(
                "database schema receipts are not contiguous (receipt {} at position {position}); {SCHEMA_GATE_OPERATOR_HINT}",
                receipt.version
            ));
        }
        if receipt.sql_sha256 != step.sha256 {
            return Err(format!(
                "database schema step {} ({}) was applied from a different text (digest {}); deploy the fvoci-server that applied it",
                step.version, step.name, receipt.sql_sha256
            ));
        }
    }
    Ok(compiled.len() - applied.len())
}

/// Reads the ledger. Works inside a migration transaction or on a plain
/// connection of the app role (SELECT on fvoci.schema_migrations is granted).
pub async fn read_postgres_ledger(conn: &mut PgConnection) -> Result<LedgerState, sqlx::Error> {
    // Probe the catalog, not information_schema: information_schema only lists
    // relations and columns the current role has a privilege on, so an app role
    // without grants would see a migrated database as unprepared instead of
    // failing to read its ledger (permission denied surfaces below as "cannot read").
    let bootstrapped: bool =
        sqlx::query_scalar("SELECT to_regclass('fvoci.schema_migrations') IS NOT NULL")
            .fetch_one(&mut *conn)
            .await?;
    if !bootstrapped {
        return Ok(LedgerState::Unprepared);
    }
    let has_lineage: bool = sqlx::query_scalar(
        "SELECT EXISTS (
            SELECT 1 FROM pg_catalog.pg_attribute
            WHERE attrelid = 'fvoci.schema_migrations'::regclass
              AND attname = 'lineage' AND attnum > 0 AND NOT attisdropped
        )",
    )
    .fetch_one(&mut *conn)
    .await?;
    if !has_lineage {
        let versions: Vec<i32> =
            sqlx::query_scalar("SELECT version FROM fvoci.schema_migrations ORDER BY version")
                .fetch_all(&mut *conn)
                .await?;
        return Ok(LedgerState::Retired { versions });
    }
    let rows: Vec<(i32, String, String)> = sqlx::query_as(
        "SELECT version, lineage, sql_sha256 FROM fvoci.schema_migrations ORDER BY version",
    )
    .fetch_all(&mut *conn)
    .await?;
    Ok(LedgerState::Applied(
        rows.into_iter()
            .map(|(version, lineage, sql_sha256)| AppliedStep {
                version,
                lineage,
                sql_sha256,
            })
            .collect(),
    ))
}

/// Verifies the connected database matches the compiled lineage exactly.
pub async fn assert_schema_current(pool: &PgPool) -> Result<(), String> {
    let mut conn = pool.acquire().await.map_err(|error| {
        format!("cannot read fvoci.schema_migrations ({error}); {SCHEMA_GATE_OPERATOR_HINT}")
    })?;
    let ledger = read_postgres_ledger(&mut conn).await.map_err(|error| {
        format!("cannot read fvoci.schema_migrations ({error}); {SCHEMA_GATE_OPERATOR_HINT}")
    })?;
    schema_gate(&ledger, &compiled_postgres_steps())
}

// PostgreSQL grants EXECUTE to PUBLIC when a function is created. Revoke it for
// the migration owner's SECURITY DEFINER functions inside the same transaction
// that created them, so no window exists before grant-app-role.sql runs.
const REVOKE_PUBLIC_DEFINER_EXECUTE: &str = r#"
DO $$
DECLARE
    definer regprocedure;
BEGIN
    FOR definer IN
        SELECT p.oid::regprocedure
        FROM pg_proc p
        INNER JOIN pg_namespace n ON n.oid = p.pronamespace
        WHERE (n.nspname = 'fvoci' OR (n.nspname = 'public' AND p.proname LIKE 'app\_%'))
          AND p.prosecdef
          AND p.proowner = (SELECT oid FROM pg_roles WHERE rolname = current_user)
    LOOP
        EXECUTE format('REVOKE EXECUTE ON ROUTINE %s FROM PUBLIC', definer);
    END LOOP;
END
$$;
"#;

// Step 09 declares `uuidv7()` column defaults, a built-in only from
// PostgreSQL 18. On 16 and 17 this creates `public.uuidv7()` with the
// RFC 9562 section 5.7 layout: 48-bit Unix milliseconds from
// clock_timestamp(), version 7, and the RFC variant and remaining 74 bits
// taken from built-in gen_random_uuid() (CSPRNG). A clock outside the
// 48-bit millisecond range raises instead of truncating. PG18's per-backend
// sub-millisecond monotonicity is not reproduced: ids created in the same
// millisecond have no defined relative order, so ordering by id is only
// coarsely FIFO. An existing `uuidv7` on the migration search path is reused
// only when it is exactly this function owned by the migration role;
// anything else fails before it is changed. PostgreSQL 18 is left untouched.
const UUIDV7_COMPAT_PREFLIGHT: &str = r#"
DO $preflight$
DECLARE
    caller_schemas pg_catalog.name[] := pg_catalog.current_schemas(true);
    shim_body pg_catalog.text := $body$
DECLARE
    unix_ms pg_catalog.int8 := pg_catalog.floor(
        pg_catalog.extract('epoch', pg_catalog.clock_timestamp()) * 1000
    );
BEGIN
    IF unix_ms < 0 OR unix_ms > 281474976710655 THEN
        RAISE EXCEPTION 'uuidv7: clock % ms is outside the 48-bit Unix millisecond range', unix_ms;
    END IF;
    RETURN pg_catalog.encode(
        pg_catalog.set_bit(
            pg_catalog.set_bit(
                pg_catalog.overlay(
                    pg_catalog.uuid_send(pg_catalog.gen_random_uuid()),
                    pg_catalog.substr(pg_catalog.int8send(unix_ms), 3),
                    1,
                    6
                ),
                52,
                1
            ),
            53,
            1
        ),
        'hex'
    )::pg_catalog.uuid;
END
$body$;
    shim_config pg_catalog.text[] := ARRAY['search_path=pg_catalog, pg_temp'];
    existing record;
    found_shim boolean := false;
BEGIN
    PERFORM pg_catalog.set_config('search_path', 'pg_catalog, pg_temp', true);

    IF pg_catalog.current_setting('server_version_num')::integer >= 180000 THEN
        RETURN;
    END IF;

    IF NOT 'public' = ANY (caller_schemas) THEN
        RAISE EXCEPTION 'uuidv7 compatibility: schema "public" is not on the migration search_path (%)',
            pg_catalog.array_to_string(caller_schemas, ', ')
            USING HINT = 'run fvoci-migrate with the default search_path so migrations resolve public.uuidv7()';
    END IF;

    FOR existing IN
        SELECT p.oid::regprocedure AS signature,
               n.nspname = 'public' AND p.pronargs = 0 AS is_candidate,
               p.proowner = (SELECT r.oid FROM pg_roles r WHERE r.rolname = current_user) AS owned,
               pg_get_userbyid(p.proowner) AS owner_name,
               p.prokind = 'f'
                   AND p.prolang = (SELECT l.oid FROM pg_language l WHERE l.lanname = 'plpgsql')
                   AND p.prosrc = shim_body
                   AND p.prorettype = 'pg_catalog.uuid'::regtype
                   AND NOT p.proretset
                   AND p.provolatile = 'v'
                   AND p.proparallel = 's'
                   AND NOT p.prosecdef
                   AND NOT p.proleakproof
                   AND NOT p.proisstrict
                   AND p.pronargdefaults = 0
                   AND p.proconfig IS NOT DISTINCT FROM shim_config AS exact
        FROM pg_proc p
        INNER JOIN pg_namespace n ON n.oid = p.pronamespace
        WHERE p.proname = 'uuidv7'
          AND n.nspname = ANY (caller_schemas || 'public'::name)
        ORDER BY n.nspname, p.oid
    LOOP
        IF existing.is_candidate AND existing.owned AND existing.exact THEN
            found_shim := true;
        ELSE
            RAISE EXCEPTION 'uuidv7 compatibility: existing function % (owner %) is not the FVOCI shim owned by %',
                existing.signature, existing.owner_name, current_user
                USING HINT = 'drop or rename that function (or remove its schema from the migration search_path) and rerun fvoci-migrate; it was not changed';
        END IF;
    END LOOP;

    IF NOT found_shim THEN
        EXECUTE format(
            'CREATE FUNCTION public.uuidv7() RETURNS pg_catalog.uuid LANGUAGE plpgsql VOLATILE PARALLEL SAFE SET search_path = pg_catalog, pg_temp AS %L',
            shim_body
        );
    END IF;
END
$preflight$;
"#;

/// Runs the version-specific preflight under the migration lock in its own
/// transaction, before any step is applied.
async fn preflight(pool: &PgPool) -> Result<(), sqlx::Error> {
    let mut tx = pool.begin().await?;
    sqlx::query("SELECT pg_advisory_xact_lock($1)")
        .bind(MIGRATION_LOCK_KEY)
        .execute(&mut *tx)
        .await?;
    sqlx::raw_sql(UUIDV7_COMPAT_PREFLIGHT)
        .execute(&mut *tx)
        .await?;
    tx.commit().await
}

pub async fn run_migrations(url: &str) -> Result<(), sqlx::Error> {
    run_migrations_through(url, i32::MAX).await
}

/// Applies the compiled lineage up to `max_version` (every step when larger
/// than the lineage). A partially applied lineage is resumed from its next
/// step; anything that is not an exact prefix of the compiled lineage is refused.
pub async fn run_migrations_through(url: &str, max_version: i32) -> Result<(), sqlx::Error> {
    let pool = PgPoolOptions::new().max_connections(2).connect(url).await?;
    let result = apply_migrations(&pool, max_version).await;
    pool.close().await;
    result
}

fn ledger_error(message: String) -> sqlx::Error {
    sqlx::Error::Protocol(message)
}

async fn apply_migrations(pool: &PgPool, max_version: i32) -> Result<(), sqlx::Error> {
    preflight(pool).await?;
    let compiled = compiled_postgres_steps();
    for step in &compiled {
        if step.version > max_version {
            continue;
        }
        verify_compiled_digest(step).map_err(ledger_error)?;
        let mut tx = pool.begin().await?;
        sqlx::query("SELECT pg_advisory_xact_lock($1)")
            .bind(MIGRATION_LOCK_KEY)
            .execute(&mut *tx)
            .await?;

        let applied = match read_postgres_ledger(&mut tx).await? {
            LedgerState::Unprepared => Vec::new(),
            LedgerState::Retired { versions } => {
                tx.rollback().await?;
                return Err(ledger_error(format!(
                    "database schema ledger has development migrations {versions:?} without a lineage; {RETIRED_LINEAGE_HINT}"
                )));
            }
            LedgerState::Applied(applied) => applied,
        };
        if !applied.is_empty() || step.version > 1 {
            let pending = match pending_steps(&applied, &compiled).map_err(ledger_error) {
                Ok(pending) => pending,
                Err(error) => {
                    tx.rollback().await?;
                    return Err(error);
                }
            };
            let next = (compiled.len() - pending + 1) as i32;
            if step.version < next {
                tx.rollback().await?;
                continue;
            }
            if step.version != next {
                tx.rollback().await?;
                return Err(ledger_error(format!(
                    "database schema is at step {} but step {} was requested; {SCHEMA_GATE_OPERATOR_HINT}",
                    next - 1,
                    step.version
                )));
            }
        }

        sqlx::raw_sql(step.sql).execute(&mut *tx).await?;
        sqlx::raw_sql(REVOKE_PUBLIC_DEFINER_EXECUTE)
            .execute(&mut *tx)
            .await?;
        sqlx::query(
            "INSERT INTO fvoci.schema_migrations (version, lineage, sql_sha256) VALUES ($1, $2, $3)",
        )
        .bind(step.version)
        .bind(POSTGRES_LINEAGE)
        .bind(step.sha256)
        .execute(&mut *tx)
        .await?;
        tx.commit().await?;
    }

    Ok(())
}

#[derive(Debug)]
pub enum GrantError {
    Db(sqlx::Error),
    InvalidRole(String),
}

impl std::fmt::Display for GrantError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Db(error) => write!(
                f,
                "app role grant failed; no privileges were committed: {error}"
            ),
            Self::InvalidRole(reason) => write!(f, "app role grant refused: {reason}"),
        }
    }
}

impl std::error::Error for GrantError {}

impl From<sqlx::Error> for GrantError {
    fn from(error: sqlx::Error) -> Self {
        Self::Db(error)
    }
}

/// Returns scripts/grant-app-role.sql with the psql `:"app_role"` variable
/// replaced by a quoted identifier.
pub fn app_role_grant_sql(role: &str) -> String {
    let quoted = format!("\"{}\"", role.replace('"', "\"\""));
    APP_ROLE_GRANTS.replace(APP_ROLE_PLACEHOLDER, &quoted)
}

/// Applies scripts/grant-app-role.sql for `role` as one transaction through the
/// migration owner URL. Any failure rolls back every statement, so the broad
/// table grant never survives without the narrowing revokes that follow it.
pub async fn grant_app_role(url: &str, role: &str) -> Result<(), GrantError> {
    let pool = PgPoolOptions::new().max_connections(1).connect(url).await?;
    let result = apply_app_role_grants(&pool, role).await;
    pool.close().await;
    result
}

pub async fn apply_app_role_grants(pool: &PgPool, role: &str) -> Result<(), GrantError> {
    let mut tx = pool.begin().await?;
    sqlx::query("SELECT pg_advisory_xact_lock($1)")
        .bind(MIGRATION_LOCK_KEY)
        .execute(&mut *tx)
        .await?;
    let found: Option<(bool, bool)> =
        sqlx::query_as("SELECT rolsuper, rolbypassrls FROM pg_roles WHERE rolname = $1")
            .bind(role)
            .fetch_optional(&mut *tx)
            .await?;
    match found {
        None => {
            return Err(GrantError::InvalidRole(format!(
                "role {role:?} does not exist"
            )))
        }
        Some((true, _)) | Some((_, true)) => {
            return Err(GrantError::InvalidRole(format!(
                "role {role:?} must be non-superuser without BYPASSRLS"
            )))
        }
        Some(_) => {}
    }
    // The script revokes privileges from `role`; applied to an owner it would
    // strip the ACL that the SECURITY DEFINER functions run under.
    let owns_schema_objects: bool = sqlx::query_scalar(
        r#"
        SELECT $1 = current_user::text
            OR pg_has_role($1, n.nspowner, 'MEMBER')
            OR EXISTS (
                SELECT 1 FROM pg_class c
                WHERE c.relnamespace = n.oid AND pg_has_role($1, c.relowner, 'MEMBER')
            )
            OR EXISTS (
                SELECT 1 FROM pg_proc p
                INNER JOIN pg_namespace pn ON pn.oid = p.pronamespace
                WHERE (pn.oid = n.oid OR (pn.nspname = 'public' AND p.proname LIKE 'app\_%'))
                  AND pg_has_role($1, p.proowner, 'MEMBER')
            )
        FROM pg_namespace n
        WHERE n.nspname = 'fvoci'
        "#,
    )
    .bind(role)
    .fetch_one(&mut *tx)
    .await?;
    if owns_schema_objects {
        return Err(GrantError::InvalidRole(format!(
            "role {role:?} owns or inherits ownership of fvoci objects; use a separate app role"
        )));
    }
    sqlx::raw_sql(&app_role_grant_sql(role))
        .execute(&mut *tx)
        .await?;
    tx.commit().await?;
    Ok(())
}

pub async fn assert_app_role(pool: &PgPool) -> Result<(), String> {
    let row: (bool, bool) =
        sqlx::query_as("SELECT rolsuper, rolbypassrls FROM pg_roles WHERE rolname = current_user")
            .fetch_one(pool)
            .await
            .map_err(|e| e.to_string())?;
    if row.0 || row.1 {
        return Err("DATABASE_APP_URL must use a non-superuser role without BYPASSRLS".into());
    }

    let owns_schema: bool = sqlx::query_scalar(
        r#"
        SELECT EXISTS (
            SELECT 1
            FROM pg_namespace n
            WHERE n.nspname = 'fvoci'
              AND pg_get_userbyid(n.nspowner) = current_user
        )
        "#,
    )
    .fetch_one(pool)
    .await
    .map_err(|e| e.to_string())?;
    if owns_schema {
        return Err("application role must not own the fvoci schema".into());
    }

    let owns_protected: bool = sqlx::query_scalar(
        r#"
        SELECT EXISTS (
            SELECT 1
            FROM pg_class c
            INNER JOIN pg_namespace n ON n.oid = c.relnamespace
            WHERE n.nspname = 'fvoci'
              AND c.relname IN (
                  'schema_migrations', 'users', 'workspaces', 'memberships',
                  'sessions', 'events', 'audit_log', 'documents', 'document_states',
                  'document_collab_updates', 'document_collab_op_receipts', 'attachments',
                  'projects', 'project_members', 'workflows', 'statuses', 'tasks',
                  'invitations', 'revisions', 'comments', 'api_tokens', 'outbox_consumers',
                  'outbox_failures', 'processed_events', 'attachment_text', 'labels',
                  'task_assignees', 'task_labels', 'milestones', 'task_dependencies',
                  'notifications', 'notification_prefs', 'workspace_holidays', 'ics_tokens',
                  'magic_tokens', 'task_activity', 'webhooks', 'webhook_deliveries',
                  'github_installations', 'github_install_states', 'github_issue_links',
                  'github_deliveries', 'user_mfa', 'identity_links', 'workspace_oidc',
                  'mfa_challenges', 'oidc_states'
              )
              AND pg_get_userbyid(c.relowner) = current_user
        )
        "#,
    )
    .fetch_one(pool)
    .await
    .map_err(|e| e.to_string())?;
    if owns_protected {
        return Err("application role must not own protected fvoci tables".into());
    }

    let inherits_owner: bool = sqlx::query_scalar(
        r#"
        SELECT EXISTS (
            SELECT 1
            FROM pg_namespace n
            CROSS JOIN LATERAL (
                SELECT pg_get_userbyid(n.nspowner) AS owner_name
            ) owner
            WHERE n.nspname = 'fvoci'
              AND pg_has_role(current_user, owner.owner_name, 'MEMBER')
              AND owner.owner_name <> current_user
        )
        "#,
    )
    .fetch_one(pool)
    .await
    .map_err(|e| e.to_string())?;
    if inherits_owner {
        return Err("application role must not inherit the migration owner role".into());
    }

    Ok(())
}

/// Held by the actual server for its entire joined runtime, or exclusively
/// by preparation until every migration connection has closed. Locks use
/// the database inode, so equivalent paths cannot bypass admission.
pub struct SqliteAdmission(std::fs::File);
// Same maintained platform O_NOFOLLOW values already used by the install
// key-file boundary. std OpenOptionsExt exposes flags, not this constant.
#[cfg(all(
    target_os = "linux",
    any(
        target_arch = "arm",
        target_arch = "aarch64",
        target_arch = "powerpc",
        target_arch = "powerpc64"
    )
))]
pub(crate) const SQLITE_NOFOLLOW: i32 = 0o100000;
#[cfg(all(
    target_os = "linux",
    not(any(
        target_arch = "arm",
        target_arch = "aarch64",
        target_arch = "powerpc",
        target_arch = "powerpc64"
    ))
))]
pub(crate) const SQLITE_NOFOLLOW: i32 = 0o400000;
#[cfg(not(target_os = "linux"))]
pub(crate) const SQLITE_NOFOLLOW: i32 = 0x100;

impl SqliteAdmission {
    pub fn server(path: &std::path::Path) -> Result<Self, sqlx::Error> {
        Self::acquire(path, false, false)
    }
    fn migration(path: &std::path::Path) -> Result<Self, sqlx::Error> {
        Self::acquire(path, true, true)
    }
    /// Existing inode only, exclusive until the owned install handoff ends.
    /// This is not a new migration/registry authority or a DB creator.
    pub fn installation_handoff(path: &std::path::Path) -> Result<Self, sqlx::Error> {
        Self::acquire(path, false, true)
    }
    pub fn admitted_file(&self) -> &std::fs::File {
        &self.0
    }
    fn acquire(path: &std::path::Path, create: bool, exclusive: bool) -> Result<Self, sqlx::Error> {
        if !path.is_absolute() || path.file_name().is_none() {
            return Err(schema_error(
                "SQLite requires an absolute persistent database file",
            ));
        }
        let mut options = std::fs::OpenOptions::new();
        options.read(true).write(true).create(create);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600).custom_flags(SQLITE_NOFOLLOW);
        }
        let file = options.open(path).map_err(sqlx::Error::Io)?;
        let result = if exclusive {
            file.try_lock()
        } else {
            file.try_lock_shared()
        };
        result.map_err(|_| {
            schema_error(
                "SQLite server/migrator admission is held; stop live servers before migration",
            )
        })?;
        Ok(Self(file))
    }
}
impl Drop for SqliteAdmission {
    fn drop(&mut self) {
        // Closing the owned OS handle releases this lock, including process
        // termination. Explicit server shutdown joins consumers before here.
        let _ = self.0.unlock();
    }
}

fn schema_error(message: impl Into<String>) -> sqlx::Error {
    sqlx::Error::Protocol(message.into())
}

/// A migration step's DDL and marker share one reserved writer transaction.
/// Cancellation cannot publish a partial step. Restart checks actual receipts
/// and schema before deciding whether another step is needed.
pub async fn run_sqlite_migrations(path: &std::path::Path) -> Result<(), sqlx::Error> {
    start_sqlite_migration(path)?.wait().await
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SqliteMigrationDrain {
    Pending,
    Closed,
    Quarantined,
}

struct MigrationOwnerState {
    drain: tokio::sync::watch::Receiver<SqliteMigrationDrain>,
    join: std::sync::Mutex<Option<std::thread::JoinHandle<()>>>,
}
/// A cancellation observer can wait for the finite cleanup owner after the
/// request future is gone. Closed means all owned pool/worker closure finished.
#[derive(Clone)]
pub struct SqliteMigrationObserver(std::sync::Arc<MigrationOwnerState>);
impl SqliteMigrationObserver {
    pub async fn wait(&self) -> Result<SqliteMigrationDrain, sqlx::Error> {
        let mut drain = self.0.drain.clone();
        loop {
            let state = *drain.borrow();
            if state != SqliteMigrationDrain::Pending {
                let join = self
                    .0
                    .join
                    .lock()
                    .map_err(|_| schema_error("migration join lock poisoned"))?
                    .take();
                if let Some(join) = join {
                    join.join()
                        .map_err(|_| schema_error("migration owner thread panicked"))?;
                }
                return Ok(state);
            }
            drain
                .changed()
                .await
                .map_err(|_| schema_error("migration owner ended without a drain receipt"))?;
        }
    }
}

pub struct SqliteMigration {
    cancel: tokio_util::sync::CancellationToken,
    result: Option<tokio::sync::oneshot::Receiver<Result<(), sqlx::Error>>>,
    observer: SqliteMigrationObserver,
}

/// The original migration result and its actual cleanup/thread-join outcome.
/// A signal owner must retain both before it can exit or exec another process.
#[derive(Debug)]
pub struct SqliteMigrationOutcome {
    pub result: Result<(), sqlx::Error>,
    pub drain: Result<SqliteMigrationDrain, sqlx::Error>,
}

#[derive(Debug, thiserror::Error)]
#[error("SQLite migration join failed after original result {original:?}: {drain}")]
struct MigrationJoinFailure {
    original: Option<sqlx::Error>,
    #[source]
    drain: sqlx::Error,
}
impl SqliteMigration {
    pub fn observer(&self) -> SqliteMigrationObserver {
        self.observer.clone()
    }
    pub fn cancel(&self) {
        self.cancel.cancel();
    }
    pub async fn wait(self) -> Result<(), sqlx::Error> {
        let outcome = self
            .wait_with_cancel(&tokio_util::sync::CancellationToken::new())
            .await;
        match outcome.drain {
            Ok(_) => outcome.result,
            Err(drain) => Err(sqlx::Error::AnyDriverError(Box::new(
                MigrationJoinFailure {
                    original: outcome.result.err(),
                    drain,
                },
            ))),
        }
    }

    pub async fn wait_with_cancel(
        mut self,
        cancel: &tokio_util::sync::CancellationToken,
    ) -> SqliteMigrationOutcome {
        let mut receiver = self.result.take().expect("one migration result receiver");
        let result = tokio::select! {
            result = &mut receiver => result,
            _ = cancel.cancelled() => {
                self.cancel();
                // Await the original owner, including an already started
                // COMMIT; a signal cannot discard its result or cleanup.
                receiver.await
            }
        }
        .unwrap_or_else(|_| Err(schema_error("migration owner ended without a result")));
        let drain = self.observer.wait().await;
        SqliteMigrationOutcome { result, drain }
    }
}
impl Drop for SqliteMigration {
    fn drop(&mut self) {
        self.cancel.cancel();
    }
}

#[cfg(feature = "db-tests")]
pub struct SqliteMigrationTestControl {
    pub connected: tokio::sync::oneshot::Sender<sqlx::SqlitePool>,
    pub proceed: tokio::sync::oneshot::Receiver<()>,
    pub cleanup_started: Option<tokio::sync::oneshot::Sender<()>>,
}
#[cfg(feature = "db-tests")]
type MigrationControl = Option<SqliteMigrationTestControl>;
#[cfg(not(feature = "db-tests"))]
type MigrationControl = ();

pub fn start_sqlite_migration(path: &std::path::Path) -> Result<SqliteMigration, sqlx::Error> {
    start_sqlite_migration_owned(path, Default::default())
}
#[cfg(feature = "db-tests")]
pub fn start_sqlite_migration_controlled(
    path: &std::path::Path,
    control: SqliteMigrationTestControl,
) -> Result<SqliteMigration, sqlx::Error> {
    start_sqlite_migration_owned(path, Some(control))
}

// Unconfirmed connection initialization or a panicked cleanup cannot authorize
// another owner. Keep that inode quarantined for this process, rather than
// treating SQLx signal-only Drop as shutdown. A CLI restart ends this process.
static QUARANTINED_SQLITE_MIGRATIONS: std::sync::Mutex<Vec<std::sync::Arc<SqliteAdmission>>> =
    std::sync::Mutex::new(Vec::new());
fn quarantine_sqlite_admission(admission: std::sync::Arc<SqliteAdmission>) {
    match QUARANTINED_SQLITE_MIGRATIONS.lock() {
        Ok(mut quarantined) => quarantined.push(admission),
        Err(_) => std::mem::forget(admission), // fail closed even if quarantine bookkeeping poisoned
    }
}
#[derive(Debug, thiserror::Error)]
#[error("SQLite migration cleanup is unconfirmed; admission quarantined until process restart")]
struct MigrationCleanupUnconfirmed(#[source] sqlx::Error);
fn unconfirmed_migration_cleanup(error: sqlx::Error) -> sqlx::Error {
    sqlx::Error::AnyDriverError(Box::new(MigrationCleanupUnconfirmed(error)))
}
fn cleanup_is_unconfirmed(error: &sqlx::Error) -> bool {
    matches!(error, sqlx::Error::AnyDriverError(source) if source.downcast_ref::<MigrationCleanupUnconfirmed>().is_some())
}

// A reference worker without a shutdown receipt remains unconfirmed even
// when rollback of the separate preparation transaction also fails. Ordinary
// validation errors retain the existing rollback-error precedence.
fn sqlite_validation_error_after_rollback(
    primary: sqlx::Error,
    rollback: Result<(), sqlx::Error>,
) -> sqlx::Error {
    if cleanup_is_unconfirmed(&primary) {
        primary
    } else {
        rollback.err().unwrap_or(primary)
    }
}

fn start_sqlite_migration_owned(
    path: &std::path::Path,
    control: MigrationControl,
) -> Result<SqliteMigration, sqlx::Error> {
    let admission = std::sync::Arc::new(SqliteAdmission::migration(path)?);
    let path = path.to_path_buf();
    let cancel = tokio_util::sync::CancellationToken::new();
    let owned_cancel = cancel.clone();
    let (result_tx, result_rx) = tokio::sync::oneshot::channel();
    let (drain_tx, drain_rx) = tokio::sync::watch::channel(SqliteMigrationDrain::Pending);
    let state = std::sync::Arc::new(MigrationOwnerState {
        drain: drain_rx,
        join: std::sync::Mutex::new(None),
    });
    // This finite owner has its own runtime: aborting a caller or shutting down
    // its Tokio runtime cannot abort an in-flight COMMIT or skip worker close.
    // No scheduler/daemon or external orchestration process is involved.
    let join = std::thread::Builder::new()
        .name("fvoci-sqlite-migrate".into())
        .spawn(move || {
            let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                let runtime = tokio::runtime::Builder::new_current_thread()
                    .enable_all()
                    .build()
                    .map_err(sqlx::Error::Io)?;
                runtime.block_on(run_sqlite_migration_owned(&path, &owned_cancel, control))
            }));
            let result = match outcome {
                Ok(result) => result,
                Err(_) => Err(unconfirmed_migration_cleanup(schema_error(
                    "migration execution/cleanup panicked",
                ))),
            };
            let drain = if result.as_ref().err().is_some_and(cleanup_is_unconfirmed) {
                quarantine_sqlite_admission(admission);
                SqliteMigrationDrain::Quarantined
            } else {
                drop(admission);
                SqliteMigrationDrain::Closed
            };
            drain_tx.send_replace(drain);
            let _ = result_tx.send(result);
        })
        .map_err(sqlx::Error::Io)?;
    *state
        .join
        .lock()
        .map_err(|_| schema_error("migration join lock poisoned"))? = Some(join);
    Ok(SqliteMigration {
        cancel,
        result: Some(result_rx),
        observer: SqliteMigrationObserver(state),
    })
}

async fn run_sqlite_migration_owned(
    path: &std::path::Path,
    cancel: &tokio_util::sync::CancellationToken,
    control: MigrationControl,
) -> Result<(), sqlx::Error> {
    let preparation = super::pool::connect_sqlite_prepare(path)
        .await
        .map_err(unconfirmed_migration_cleanup)?;
    let backend = super::backend::Backend::Sqlite(preparation.pool.clone());
    #[cfg(feature = "db-tests")]
    let mut cleanup_started = None;
    #[cfg(feature = "db-tests")]
    if let Some(control) = control {
        let _ = control.connected.send(preparation.pool.clone());
        cleanup_started = control.cleanup_started;
        tokio::select! {
            _ = control.proceed => {},
            _ = cancel.cancelled() => {},
        }
    }
    #[cfg(not(feature = "db-tests"))]
    let _ = control;
    let result = apply_sqlite_migrations(&backend, Some(cancel)).await;
    // Request cancellation does not cancel any operation above or this drain.
    #[cfg(feature = "db-tests")]
    if let Some(started) = cleanup_started {
        let _ = started.send(());
    }
    preparation
        .close_confirmed()
        .await
        .map_err(unconfirmed_migration_cleanup)?;
    result
}

/// Exact old guard-in-request lifecycle, only for the barrier regression.
/// The test pauses a real SQLite COMMIT and observes its premature admission;
/// this is never a production backend/fallback or a new migration policy.
#[cfg(feature = "db-tests")]
pub async fn run_sqlite_migrations_legacy_control(
    path: &std::path::Path,
    control: SqliteMigrationTestControl,
) -> Result<(), sqlx::Error> {
    let _admission = SqliteAdmission::migration(path)?;
    let preparation = super::pool::connect_sqlite_prepare(path).await?;
    let backend = super::backend::Backend::Sqlite(preparation.pool.clone());
    let _ = control.connected.send(preparation.pool.clone());
    let _ = control.proceed.await;
    let result = apply_sqlite_migrations(&backend, None).await;
    backend.close().await?;
    result
}

/// Test-owned SQLite-family registry initialization for the actual loopback SDK.
/// Product remote installation still requires its separate admission owner.
#[cfg(all(test, feature = "db-tests"))]
pub(crate) async fn initialize_family_backend_for_test(
    backend: &super::backend::Backend,
) -> Result<(), sqlx::Error> {
    apply_sqlite_migrations(backend, None).await
}

/// Receipts and structural digest of the SQLite-family ledger as read on a
/// consumer's own writer stream (see `turso_test_schema_in_writer`).
#[cfg(all(test, feature = "db-tests"))]
#[allow(dead_code)] // consumed only by the ON migration-phase consumer in db::turso_test
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct TursoTestSchemaSnapshot {
    /// `(version, lineage, sql_sha256, applied_at)` rows in version order; the
    /// stored `applied_at` microseconds are validated as an instant but returned
    /// raw, never renormalized.
    pub receipts: Vec<(i64, String, String, i64)>,
    /// Same digest `assert_sqlite_schema_current` reports for a complete
    /// lineage; for a prefix it covers exactly the first `expected_steps`.
    pub schema_sha256: String,
}

/// The verified manual Turso job supplies exactly these values for the
/// mutating migration phase. They are consumer-test guards only; the product
/// never reads them and no credential or database identity is read here.
#[cfg(all(test, feature = "db-tests"))]
#[allow(dead_code)] // consumed only by the ON migration-phase consumer in db::turso_test
const TURSO_TEST_MIGRATION_FLAGS: [(&str, &str); 4] = [
    ("FVOCI_TEST_TURSO_MIGRATION_SELECTED", "1"),
    ("FVOCI_TEST_TURSO_PHASE", "migration"),
    ("FVOCI_TEST_TURSO_DESTRUCTIVE", "true"),
    ("FVOCI_TEST_TURSO_ALLOW_DESTRUCTIVE", "true"),
];

/// The one flag-value check both helpers share. `lookup` is the process
/// environment in the helpers; the pure negative proof below passes a map so
/// no test mutates the process environment.
#[cfg(all(test, feature = "db-tests"))]
fn turso_test_require_migration_flags(
    lookup: impl Fn(&str) -> Option<String>,
) -> Result<(), sqlx::Error> {
    for (name, expected) in TURSO_TEST_MIGRATION_FLAGS {
        if lookup(name).as_deref() != Some(expected) {
            return Err(schema_error(format!(
                "turso test helper refused: {name} must be exactly {expected:?} for the mutating migration phase"
            )));
        }
    }
    Ok(())
}

#[cfg(all(test, feature = "db-tests"))]
fn turso_test_process_flag(name: &str) -> Option<String> {
    std::env::var(name).ok()
}

/// Applies the first `through` compiled SQLite-family steps to the explicit
/// remote test database, then validates exactly that prefix.
///
/// `through` is `0` (blank: nothing applied, ledger absent), `11` (the prefix
/// the consumer resumes from) or `compiled_sqlite_steps().len()` (current).
/// Every guard runs before any effect: the backend must be `LibsqlRemote` and
/// the four job flags must be present with their exact values. Each step is
/// applied by the maintained `apply_sqlite_migration_step`, which owns its own
/// reserved stream (begin/DDL/receipt/commit or rollback) exactly as a product
/// installation does; a step already recorded is left untouched. The closing
/// validation reads the ledger on an own read stream and compares the actual
/// catalog with a reference built from the same `through` steps: for a prefix
/// this is deliberately the partial structure of those steps and is never
/// reported as a complete lineage, which only `verify_sqlite_applied(.., true)`
/// (the product gate) asserts.
#[cfg(all(test, feature = "db-tests"))]
#[allow(dead_code)] // consumed only by the ON migration-phase consumer in db::turso_test
pub(crate) async fn turso_test_apply_prefix(
    backend: &super::backend::Backend,
    through: usize,
) -> Result<(), sqlx::Error> {
    use super::backend::DbTransaction;
    if !matches!(backend, super::backend::Backend::LibsqlRemote(_)) {
        return Err(schema_error(
            "turso test helper requires the explicit remote libSQL backend",
        ));
    }
    turso_test_require_migration_flags(turso_test_process_flag)?;
    let compiled = compiled_sqlite_steps();
    if through > compiled.len() {
        return Err(schema_error(format!(
            "turso test prefix {through} exceeds the compiled lineage of {} steps",
            compiled.len()
        )));
    }
    for step in compiled.iter().take(through) {
        apply_sqlite_migration_step(backend, step, None).await?;
    }
    let mut tx = backend.begin_read().await?;
    let result = async {
        let DbTransaction::SqliteFamily(family) = &mut tx else {
            return Err(schema_error(
                "turso test prefix validation requires an actual SQLite-family handle",
            ));
        };
        let applied = sqlite_applied(family).await?;
        verify_sqlite_applied(&applied, through == compiled.len())?;
        if applied.len() != through {
            return Err(schema_error(format!(
                "turso test prefix expected {through} receipts, found {}",
                applied.len()
            )));
        }
        verify_sqlite_objects(family, through).await.map(|_| ())
    }
    .await;
    match result {
        Ok(()) => tx.rollback().await,
        Err(error) => Err(sqlite_validation_error_after_rollback(
            error,
            tx.rollback().await,
        )),
    }
}

/// Reads the ledger receipts and the structural digest on the consumer's own
/// already reserved writer stream. Nothing here begins, commits, rolls back or
/// detaches that stream: the consumer that owns the original request keeps the
/// finish (and any rollback after its own FK failure) and the readback.
///
/// Guards run before the first query: the stream must be remote, the four
/// manual-phase flags must be present with their exact values, and the stream
/// must hold the writer reservation.
///
/// `expected_steps` is the number of receipts the stream must see (`0` blank,
/// `11` prefix, `compiled_sqlite_steps().len()` current). The receipts are
/// verified against the compiled lineage; completeness is asserted only when
/// `expected_steps` is the whole lineage, and the digest covers exactly the
/// first `expected_steps` steps, so a prefix digest is a prefix fact, never a
/// current-schema claim.
#[cfg(all(test, feature = "db-tests"))]
#[allow(dead_code)] // consumed only by the ON migration-phase consumer in db::turso_test
pub(crate) async fn turso_test_schema_in_writer(
    family: &mut super::backend::FamilyTx,
    expected_steps: usize,
) -> Result<TursoTestSchemaSnapshot, sqlx::Error> {
    if !matches!(family, super::backend::FamilyTx::Remote(_)) {
        return Err(schema_error(
            "turso test writer snapshot requires the explicit remote libSQL stream",
        ));
    }
    // Same manual-phase flags as the prefix helper, checked before any query
    // on the borrowed stream (which is neither begun nor finished here).
    turso_test_require_migration_flags(turso_test_process_flag)?;
    family.require_writer()?;
    let compiled = compiled_sqlite_steps();
    if expected_steps > compiled.len() {
        return Err(schema_error(format!(
            "turso test snapshot expects {expected_steps} steps beyond the compiled lineage of {}",
            compiled.len()
        )));
    }
    let exists = family
        .query(
            "SELECT count(*) FROM sqlite_schema WHERE type='table' AND name='schema_migrations'",
            &[],
        )
        .await?;
    let receipts: Vec<(i64, String, String, i64)> = if exists[0].cell(0)?.integer()? == 0 {
        Vec::new()
    } else {
        family
            .query(
                "SELECT version,lineage,sql_sha256,applied_at FROM schema_migrations ORDER BY version",
                &[],
            )
            .await?
            .iter()
            .map(|row| {
                let applied_at = row.cell(3)?;
                // Validate the stored instant exactly as the product does, but
                // return the raw microseconds the engine stored.
                applied_at.datetime()?;
                Ok((
                    row.cell(0)?.integer()?,
                    row.cell(1)?.string()?,
                    row.cell(2)?.string()?,
                    applied_at.integer()?,
                ))
            })
            .collect::<Result<_, sqlx::Error>>()?
    };
    let applied: Vec<SqliteApplied> = receipts
        .iter()
        .map(|(version, lineage, digest, _)| (*version, lineage.clone(), digest.clone()))
        .collect();
    verify_sqlite_applied(&applied, expected_steps == compiled.len())?;
    if applied.len() != expected_steps {
        return Err(schema_error(format!(
            "turso test snapshot expected {expected_steps} receipts, found {}",
            applied.len()
        )));
    }
    let schema_sha256 = verify_sqlite_objects(family, expected_steps).await?;
    Ok(TursoTestSchemaSnapshot {
        receipts,
        schema_sha256,
    })
}

#[cfg(all(test, feature = "db-tests"))]
mod turso_test_helper_guards {
    use super::{turso_test_require_migration_flags, TURSO_TEST_MIGRATION_FLAGS};
    use std::collections::BTreeMap;

    fn lookup<'m>(
        map: &'m BTreeMap<&'static str, &'static str>,
    ) -> impl Fn(&str) -> Option<String> + 'm {
        move |name| map.get(name).map(|value| value.to_string())
    }

    #[test]
    fn exact_four_manual_phase_flags_are_required_before_any_query() {
        let complete: BTreeMap<_, _> = TURSO_TEST_MIGRATION_FLAGS.into_iter().collect();
        assert_eq!(complete.len(), 4);
        turso_test_require_migration_flags(lookup(&complete)).unwrap();
        // Absent flag, wrong phase, connection-phase values and a true-looking
        // but inexact spelling are each refused by name, with no data access.
        for (name, value) in [
            ("FVOCI_TEST_TURSO_MIGRATION_SELECTED", None),
            ("FVOCI_TEST_TURSO_PHASE", Some("connection")),
            ("FVOCI_TEST_TURSO_DESTRUCTIVE", Some("false")),
            ("FVOCI_TEST_TURSO_ALLOW_DESTRUCTIVE", Some("True")),
            ("FVOCI_TEST_TURSO_MIGRATION_SELECTED", Some("1 ")),
        ] {
            let mut map = complete.clone();
            match value {
                Some(value) => {
                    map.insert(name, value);
                }
                None => {
                    map.remove(name);
                }
            }
            let error = turso_test_require_migration_flags(lookup(&map))
                .unwrap_err()
                .to_string();
            assert!(error.contains(name), "{name} {value:?}: {error}");
            assert!(error.contains("mutating migration phase"), "{error}");
        }
        let empty = BTreeMap::new();
        let error = turso_test_require_migration_flags(lookup(&empty))
            .unwrap_err()
            .to_string();
        assert!(
            error.contains("FVOCI_TEST_TURSO_MIGRATION_SELECTED"),
            "{error}"
        );
    }
}

async fn apply_sqlite_migrations(
    backend: &super::backend::Backend,
    cancel: Option<&tokio_util::sync::CancellationToken>,
) -> Result<(), sqlx::Error> {
    for step in compiled_sqlite_steps() {
        apply_sqlite_migration_step(backend, &step, cancel).await?;
    }
    assert_sqlite_schema_current(backend).await.map(|_| ())
}

async fn apply_sqlite_migration_step(
    backend: &super::backend::Backend,
    step: &CompiledStep,
    cancel: Option<&tokio_util::sync::CancellationToken>,
) -> Result<(), sqlx::Error> {
    use super::backend::DbTransaction;
    use super::codec::Cell;
    let index = (step.version - 1) as usize;
    if cancel.is_some_and(|token| token.is_cancelled()) {
        return Err(schema_error(
            "SQLite migration cancelled after in-flight work settled",
        ));
    }
    let mut tx = backend.begin_write().await?;
    let result = async {
        if cancel.is_some_and(|token| token.is_cancelled()) { return Err(schema_error("SQLite migration cancelled before new step")); }
        let DbTransaction::SqliteFamily(family) = &mut tx else {
            return Err(schema_error("SQLite migrations require an actual SQLite-family handle"));
        };
        let applied = sqlite_applied(family).await?;
        verify_sqlite_applied(&applied, false)?;
        verify_sqlite_objects(family, applied.len()).await?;
        if index < applied.len() { return Ok(false); }
        if index != applied.len() { return Err(schema_error("SQLite migration gap")); }
        verify_compiled_digest(step).map_err(schema_error)?;
        if cancel.is_some_and(|token| token.is_cancelled()) { return Err(schema_error("SQLite migration cancelled before DDL")); }
        family.apply_migration_batch(step.sql).await?;
        if cancel.is_some_and(|token| token.is_cancelled()) { return Err(schema_error("SQLite migration cancelled after DDL; rollback before marker/commit")); }
        family.execute(
            "INSERT INTO schema_migrations(version,lineage,sql_sha256,applied_at) VALUES(?1,?2,?3,unixepoch()*1000000+CAST(substr(strftime('%f'),4,3) AS INTEGER)*1000)",
            &[Cell::Integer(step.version as i64), Cell::text(SQLITE_LINEAGE), Cell::text(step.sha256)],
        ).await?;
        verify_sqlite_objects(family, index + 1).await?;
        Ok(true)
    }.await;
    match result {
        Ok(true) => {
            if cancel.is_some_and(|token| token.is_cancelled()) {
                tx.rollback().await?;
                return Err(schema_error("SQLite migration cancelled before commit"));
            }
            tx.commit()
                .await
                .map_err(|error| sqlx::Error::AnyDriverError(Box::new(error)))?;
        }
        Ok(false) => tx.rollback().await?,
        Err(error) => {
            return Err(sqlite_validation_error_after_rollback(
                error,
                tx.rollback().await,
            ));
        }
    }
    Ok(())
}

/// How the compiled DDL text reached the engine, and therefore what text the
/// engine's `sqlite_schema` stores.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SqliteSchemaMode {
    /// Local SQLite: SQLx executes the compiled text verbatim.
    Raw,
    /// Remote libSQL (hrana): the pinned SDK parses every statement with
    /// libsql-sqlite3-parser and sends its re-rendered text. The reference
    /// engine is fed the same rendering (`sdk_rendered_statements`).
    SdkRendered,
}

impl SqliteSchemaMode {
    fn of(family: &super::backend::FamilyTx) -> Self {
        match family {
            super::backend::FamilyTx::Local(_) => Self::Raw,
            super::backend::FamilyTx::Remote(_) => Self::SdkRendered,
        }
    }
    fn provenance(self) -> &'static str {
        match self {
            Self::Raw => "raw",
            Self::SdkRendered => "sdk-rendered",
        }
    }
}

#[derive(Debug)]
pub struct SqliteCapability {
    pub lineage: &'static str,
    pub applied_steps: usize,
    pub mode: SqliteSchemaMode,
    pub schema_sha256: String,
}

pub async fn assert_sqlite_schema_current(
    backend: &super::backend::Backend,
) -> Result<SqliteCapability, sqlx::Error> {
    use super::backend::DbTransaction;
    let mut tx = backend.begin_read().await?;
    let result = async {
        let DbTransaction::SqliteFamily(family) = &mut tx else {
            return Err(schema_error(
                "SQLite schema gate requires an actual SQLite-family handle",
            ));
        };
        let applied = sqlite_applied(family).await?;
        verify_sqlite_applied(&applied, true)?;
        let schema_sha256 = verify_sqlite_objects(family, applied.len()).await?;
        Ok(SqliteCapability {
            lineage: SQLITE_LINEAGE,
            applied_steps: applied.len(),
            mode: SqliteSchemaMode::of(family),
            schema_sha256,
        })
    }
    .await;
    match result {
        Ok(capability) => {
            tx.rollback().await?;
            Ok(capability)
        }
        Err(error) => Err(sqlite_validation_error_after_rollback(
            error,
            tx.rollback().await,
        )),
    }
}

type SqliteApplied = (i64, String, String);
async fn sqlite_applied(
    family: &mut super::backend::FamilyTx,
) -> Result<Vec<SqliteApplied>, sqlx::Error> {
    let exists = family
        .query(
            "SELECT count(*) FROM sqlite_schema WHERE type='table' AND name='schema_migrations'",
            &[],
        )
        .await?;
    if exists[0].cell(0)?.integer()? == 0 {
        return Ok(Vec::new());
    }
    family
        .query(
            "SELECT version,lineage,sql_sha256,applied_at FROM schema_migrations ORDER BY version",
            &[],
        )
        .await?
        .iter()
        .map(|row| {
            // Reject a malformed storage class/time as well as an invalid set.
            row.cell(3)?.datetime()?;
            Ok((
                row.cell(0)?.integer()?,
                row.cell(1)?.string()?,
                row.cell(2)?.string()?,
            ))
        })
        .collect()
}

fn verify_sqlite_applied(applied: &[SqliteApplied], complete: bool) -> Result<(), sqlx::Error> {
    let compiled = compiled_sqlite_steps();
    if applied
        .iter()
        .any(|(_, lineage, _)| lineage == RETIRED_SQLITE_LINEAGE)
    {
        return Err(schema_error(format!(
            "SQLite schema carries the retired lineage {RETIRED_SQLITE_LINEAGE}; {RETIRED_LINEAGE_HINT}"
        )));
    }
    if applied.len() > compiled.len() || (complete && applied.len() != compiled.len()) {
        return Err(schema_error(
            "SQLite schema is ahead, incomplete or unprepared",
        ));
    }
    for (index, (version, lineage, digest)) in applied.iter().enumerate() {
        let step = &compiled[index];
        verify_compiled_digest(step).map_err(schema_error)?;
        if *version != step.version as i64 || lineage != SQLITE_LINEAGE || digest != step.sha256 {
            return Err(schema_error(
                "SQLite schema has a gap, foreign lineage or changed digest",
            ));
        }
    }
    Ok(())
}

/// The statements the pinned libsql SDK (0.9.30, hrana) sends for one compiled
/// step. Its `parser::Statement::parse` parses the whole text with
/// libsql-sqlite3-parser and sends `Cmd::to_string()` of every statement; the
/// one exception is a text that holds exactly one statement which is a
/// `CREATE TABLE`, sent as the original text. Nothing here lowercases,
/// strips or normalizes SQL: the rendering is the maintained parser's own, and
/// the remote engine stores exactly that text in `sqlite_schema`.
pub fn sdk_rendered_statements(sql: &str) -> Result<Vec<String>, sqlx::Error> {
    use fallible_iterator::FallibleIterator;
    use libsql_sqlite3_parser::ast::{Cmd, Stmt};
    use libsql_sqlite3_parser::lexer::sql::Parser;
    let mut parser = Box::new(Parser::new(sql.as_bytes()));
    let mut commands = Vec::new();
    while let Some(command) = parser
        .next()
        .map_err(|error| schema_error(format!("SDK parser rejected compiled DDL: {error}")))?
    {
        commands.push(command);
    }
    if commands.len() == 1 && matches!(commands[0], Cmd::Stmt(Stmt::CreateTable { .. })) {
        return Ok(vec![sql.to_string()]);
    }
    Ok(commands.iter().map(|command| command.to_string()).collect())
}

const SQLITE_OBJECTS: &str = "SELECT type,name,tbl_name,sql FROM sqlite_schema WHERE name NOT GLOB 'sqlite_*' ORDER BY type COLLATE BINARY,name COLLATE BINARY";
type SqliteObject = (String, String, String, String);

/// Exact comparison of the engine's stored definitions with the reference.
/// Every row must match byte for byte: a changed CHECK/FK/default/predicate/
/// trigger body, a missing, extra or renamed object all differ in the stored
/// text. Identifier case never differs between the two sides because both
/// receive the same text.
fn compare_sqlite_objects(
    expected: &[SqliteObject],
    actual: &[SqliteObject],
) -> Result<(), sqlx::Error> {
    if actual != expected {
        return Err(schema_error("SQLite schema definitions differ from compiled capability; unmarked/populated or altered schema refused"));
    }
    Ok(())
}

/// Ask the pinned engine to compile the fixed DDL in a separate reference
/// connection. This validates every actual table/index/trigger definition;
/// no handwritten SQL parser, guessed column inventory or marker-only gate.
/// For a remote engine the reference receives the SDK's own rendering of the
/// same steps, so the comparison stays byte-exact without any normalizer.
async fn verify_sqlite_objects(
    family: &mut super::backend::FamilyTx,
    steps: usize,
) -> Result<String, sqlx::Error> {
    use sqlx::Connection;
    let mode = SqliteSchemaMode::of(family);
    let mut reference = sqlx::SqliteConnection::connect_with(
        &sqlx::sqlite::SqliteConnectOptions::new()
            .in_memory(true)
            .foreign_keys(true),
    )
    .await
    .map_err(unconfirmed_migration_cleanup)?;
    let expected = async {
        let pin: (String, String) = sqlx::query_as("SELECT sqlite_version(),sqlite_source_id()")
            .fetch_one(&mut reference)
            .await?;
        if pin.0 != super::pool::SQLITE_VERSION || pin.1 != super::pool::SQLITE_SOURCE_ID {
            return Err(schema_error("SQLite schema reference engine pin mismatch"));
        }
        for step in compiled_sqlite_steps().iter().take(steps) {
            verify_compiled_digest(step).map_err(schema_error)?;
            match mode {
                SqliteSchemaMode::Raw => {
                    sqlx::raw_sql(step.sql).execute(&mut reference).await?;
                }
                SqliteSchemaMode::SdkRendered => {
                    for statement in sdk_rendered_statements(step.sql)? {
                        sqlx::raw_sql(&statement).execute(&mut reference).await?;
                    }
                }
            }
        }
        sqlx::query_as::<_, SqliteObject>(SQLITE_OBJECTS)
            .fetch_all(&mut reference)
            .await
    }
    .await;
    reference
        .close()
        .await
        .map_err(unconfirmed_migration_cleanup)?;
    let expected = expected?;
    let actual = family
        .query(SQLITE_OBJECTS, &[])
        .await?
        .iter()
        .map(|row| {
            Ok((
                row.cell(0)?.string()?,
                row.cell(1)?.string()?,
                row.cell(2)?.string()?,
                row.cell(3)?.string()?,
            ))
        })
        .collect::<Result<Vec<SqliteObject>, sqlx::Error>>()?;
    compare_sqlite_objects(&expected, &actual)?;
    let bytes = serde_json::to_vec(&(SQLITE_LINEAGE, steps, mode.provenance(), actual))
        .map_err(|e| sqlx::Error::Encode(Box::new(e)))?;
    Ok(hex::encode({
        use sha2::{Digest, Sha256};
        Sha256::digest(bytes)
    }))
}

#[cfg(test)]
mod sqlite_rollback_tests {
    use super::*;

    fn unconfirmed_reference() -> sqlx::Error {
        unconfirmed_migration_cleanup(schema_error("reference worker shutdown unconfirmed"))
    }

    #[test]
    fn unconfirmed_primary_survives_failed_rollback() {
        // The original rollback-first `?` loses the typed quarantine reason.
        let legacy_primary = unconfirmed_reference();
        let legacy = Err::<(), _>(schema_error("rollback failed"))
            .err()
            .unwrap_or(legacy_primary);
        assert!(!cleanup_is_unconfirmed(&legacy));
        let fixed = sqlite_validation_error_after_rollback(
            unconfirmed_reference(),
            Err(schema_error("rollback failed")),
        );
        assert!(cleanup_is_unconfirmed(&fixed));
    }

    #[test]
    fn ordinary_primary_retains_failed_rollback_precedence() {
        let fixed = sqlite_validation_error_after_rollback(
            schema_error("schema validation failed"),
            Err(schema_error("rollback failed")),
        );
        assert!(!cleanup_is_unconfirmed(&fixed));
        assert_eq!(
            fixed.to_string(),
            schema_error("rollback failed").to_string()
        );
    }

    #[test]
    fn successful_rollback_retains_primary_classification() {
        let unconfirmed = sqlite_validation_error_after_rollback(unconfirmed_reference(), Ok(()));
        assert!(cleanup_is_unconfirmed(&unconfirmed));
        let ordinary = sqlite_validation_error_after_rollback(
            schema_error("schema validation failed"),
            Ok(()),
        );
        assert!(!cleanup_is_unconfirmed(&ordinary));
        assert_eq!(
            ordinary.to_string(),
            schema_error("schema validation failed").to_string()
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // Applied steps are skipped by version and verified by digest, so editing an
    // accepted step would refuse every existing installation. Add a new step
    // instead and pin its digest in the registry when it is accepted.
    #[test]
    fn compiled_steps_match_their_registry_digests_and_are_contiguous() {
        for (lineage, steps) in [
            (POSTGRES_LINEAGE, compiled_postgres_steps()),
            (SQLITE_LINEAGE, compiled_sqlite_steps()),
        ] {
            assert_eq!(steps.len(), 12, "{lineage} has twelve baseline modules");
            let mut names = std::collections::BTreeSet::new();
            for (index, step) in steps.iter().enumerate() {
                assert_eq!(
                    step.version,
                    index as i32 + 1,
                    "{lineage} versions are contiguous"
                );
                assert!(
                    names.insert(step.name),
                    "{lineage} step name {} repeats",
                    step.name
                );
                assert!(
                    step.name.starts_with(&format!("{:02}_", step.version)),
                    "{lineage} step {} is named {}",
                    step.version,
                    step.name
                );
                verify_compiled_digest(step)
                    .unwrap_or_else(|error| panic!("{lineage}: {error}; add a new step instead"));
            }
        }
        assert_eq!(compiled_migration_count(), 12);
        assert_eq!(latest_migration_version(), 12);
        assert_eq!(compiled_migration_versions(), (1..=12).collect::<Vec<_>>());
        let pg: Vec<&str> = compiled_postgres_steps().iter().map(|s| s.name).collect();
        let sq: Vec<&str> = compiled_sqlite_steps().iter().map(|s| s.name).collect();
        assert_eq!(pg, sq, "both engines use the same module names and order");
    }

    #[test]
    fn baseline_ledger_texts_pin_their_lineage() {
        let pg = compiled_postgres_steps()[0].sql;
        assert!(
            pg.contains("CREATE TABLE fvoci.schema_migrations ("),
            "{pg}"
        );
        assert!(pg.contains(&format!("lineage = '{POSTGRES_LINEAGE}'")));
        let sq = compiled_sqlite_steps()[0].sql;
        assert!(sq.contains("CREATE TABLE schema_migrations ("), "{sq}");
        assert!(sq.contains(&format!("lineage='{SQLITE_LINEAGE}'")));
        assert_ne!(SQLITE_LINEAGE, RETIRED_SQLITE_LINEAGE);
    }

    fn receipts(steps: &[CompiledStep]) -> Vec<AppliedStep> {
        steps
            .iter()
            .map(|step| AppliedStep {
                version: step.version,
                lineage: POSTGRES_LINEAGE.into(),
                sql_sha256: step.sha256.into(),
            })
            .collect()
    }

    #[test]
    fn schema_gate_distinguishes_current_behind_ahead_altered_foreign_retired_and_empty() {
        let compiled = compiled_postgres_steps();
        let expected = latest_migration_version();
        let current = receipts(&compiled);
        assert!(schema_gate(&LedgerState::Applied(current.clone()), &compiled).is_ok());
        assert_eq!(pending_steps(&current, &compiled).unwrap(), 0);

        let mut behind = current.clone();
        behind.pop();
        assert_eq!(pending_steps(&behind, &compiled).unwrap(), 1);
        let behind = schema_gate(&LedgerState::Applied(behind), &compiled).unwrap_err();
        assert!(behind.contains("missing migrations [12]"), "{behind}");
        assert!(
            behind.contains(&format!("behind compiled version {expected}")),
            "{behind}"
        );
        assert!(behind.contains(SCHEMA_GATE_OPERATOR_HINT), "{behind}");

        // A receipt set that is not a prefix (a middle step missing) is never a count.
        let mut gap = current.clone();
        gap.remove(gap.len() - 2);
        let gap = schema_gate(&LedgerState::Applied(gap), &compiled).unwrap_err();
        assert!(gap.contains("not contiguous"), "{gap}");
        assert!(gap.contains(SCHEMA_GATE_OPERATOR_HINT), "{gap}");

        let mut ahead = current.clone();
        ahead.push(AppliedStep {
            version: expected + 1,
            lineage: POSTGRES_LINEAGE.into(),
            sql_sha256: "0".repeat(64),
        });
        let ahead = schema_gate(&LedgerState::Applied(ahead), &compiled).unwrap_err();
        assert!(
            ahead.contains(&format!("newer than this binary ({expected})")),
            "{ahead}"
        );
        assert!(ahead.contains("deploy a matching fvoci-server"), "{ahead}");
        assert!(!ahead.contains("fvoci-migrate"), "{ahead}");

        let mut altered = current.clone();
        altered[3].sql_sha256 = "f".repeat(64);
        let altered = schema_gate(&LedgerState::Applied(altered), &compiled).unwrap_err();
        assert!(
            altered.contains("step 4 (04_events) was applied from a different text"),
            "{altered}"
        );

        let mut foreign = current.clone();
        foreign[0].lineage = "fvoci-postgres-999".into();
        let foreign = schema_gate(&LedgerState::Applied(foreign), &compiled).unwrap_err();
        assert!(
            foreign.contains("belongs to lineage \"fvoci-postgres-999\""),
            "{foreign}"
        );

        let empty = schema_gate(&LedgerState::Unprepared, &compiled).unwrap_err();
        assert!(empty.contains("no applied migrations"), "{empty}");
        assert!(empty.contains(SCHEMA_GATE_OPERATOR_HINT), "{empty}");

        let receipts_only = schema_gate(&LedgerState::Applied(Vec::new()), &compiled).unwrap_err();
        assert!(
            receipts_only.contains("without receipts"),
            "{receipts_only}"
        );

        let retired = schema_gate(
            &LedgerState::Retired {
                versions: (1..=55).collect(),
            },
            &compiled,
        )
        .unwrap_err();
        assert!(
            retired.contains("development migrations [1, 2"),
            "{retired}"
        );
        assert!(retired.contains(RETIRED_LINEAGE_HINT), "{retired}");
        assert!(
            !retired.contains("fvoci-migrate --grant-app-role"),
            "{retired}"
        );
    }

    #[test]
    fn grant_sql_quotes_role_and_replaces_every_placeholder() {
        let sql = app_role_grant_sql("app\"x");
        assert!(!sql.contains(APP_ROLE_PLACEHOLDER));
        assert!(sql.contains("TO \"app\"\"x\";"));
        assert!(!sql.contains("BEGIN") && !sql.contains("COMMIT"));
    }

    #[test]
    fn retired_sqlite_lineage_is_refused_by_name_before_any_digest_check() {
        let error =
            verify_sqlite_applied(&[(1, RETIRED_SQLITE_LINEAGE.into(), "0".repeat(64))], false)
                .unwrap_err()
                .to_string();
        assert!(error.contains(RETIRED_SQLITE_LINEAGE), "{error}");
        assert!(error.contains(RETIRED_LINEAGE_HINT), "{error}");
        let foreign = verify_sqlite_applied(&[(1, "other".into(), "0".repeat(64))], false)
            .unwrap_err()
            .to_string();
        assert!(foreign.contains("foreign lineage"), "{foreign}");
    }

    #[test]
    fn sdk_rendering_keeps_a_lone_create_table_verbatim_and_rerenders_batches() {
        let lone = compiled_sqlite_steps()[0].sql;
        assert_eq!(
            sdk_rendered_statements(lone).unwrap(),
            vec![lone.to_string()]
        );
        let batch = "CREATE TABLE groups (id BLOB PRIMARY KEY NOT NULL) STRICT;\nCREATE INDEX groups_idx ON groups(id) WHERE id IS NOT NULL;";
        let rendered = sdk_rendered_statements(batch).unwrap();
        assert_eq!(rendered.len(), 2);
        // The maintained parser may render a keyword-fallback identifier with the
        // keyword's spelling; SQLite identifiers are case-insensitive, so both
        // texts name the same table. The reference and the remote receive this
        // same text, so only its determinism and shape matter here.
        assert!(
            rendered[0]
                .to_lowercase()
                .starts_with("create table groups"),
            "{}",
            rendered[0]
        );
        assert!(
            rendered[1].to_lowercase().contains("where"),
            "{}",
            rendered[1]
        );
        assert_eq!(
            sdk_rendered_statements(batch).unwrap(),
            rendered,
            "rendering is deterministic"
        );
        let error = sdk_rendered_statements("CREATE TABLE (")
            .unwrap_err()
            .to_string();
        assert!(error.contains("SDK parser rejected"), "{error}");
        for step in compiled_sqlite_steps() {
            let rendered = sdk_rendered_statements(step.sql).unwrap();
            assert!(!rendered.is_empty(), "{} renders", step.name);
        }
    }

    #[test]
    fn sqlite_object_comparison_refuses_any_changed_missing_extra_or_renamed_definition() {
        let row = |t: &str, n: &str, tbl: &str, sql: &str| {
            (
                t.to_string(),
                n.to_string(),
                tbl.to_string(),
                sql.to_string(),
            )
        };
        let expected = vec![
            row(
                "index",
                "a_idx",
                "a",
                "CREATE INDEX a_idx ON a(x) WHERE x>0",
            ),
            row("table", "a", "a", "CREATE TABLE a (x INTEGER CHECK (x>0))"),
            row(
                "trigger",
                "a_t",
                "a",
                "CREATE TRIGGER a_t BEFORE DELETE ON a BEGIN SELECT RAISE(ABORT,'no'); END",
            ),
        ];
        assert!(compare_sqlite_objects(&expected, &expected).is_ok());
        let mut altered_check = expected.clone();
        altered_check[1].3 = "CREATE TABLE a (x INTEGER CHECK (x>=0))".into();
        assert!(compare_sqlite_objects(&expected, &altered_check).is_err());
        let mut altered_predicate = expected.clone();
        altered_predicate[0].3 = "CREATE INDEX a_idx ON a(x)".into();
        assert!(compare_sqlite_objects(&expected, &altered_predicate).is_err());
        let mut altered_trigger = expected.clone();
        altered_trigger[2].3 = altered_trigger[2].3.replace("ABORT", "IGNORE");
        assert!(compare_sqlite_objects(&expected, &altered_trigger).is_err());
        let missing = expected[..2].to_vec();
        assert!(compare_sqlite_objects(&expected, &missing).is_err());
        let mut extra = expected.clone();
        extra.push(row("table", "b", "b", "CREATE TABLE b (y)"));
        assert!(compare_sqlite_objects(&expected, &extra).is_err());
        let mut renamed = expected.clone();
        renamed[1].1 = "A".into();
        assert!(compare_sqlite_objects(&expected, &renamed).is_err());
    }
}

#[cfg(all(test, feature = "db-tests"))]
mod maintenance_claim_migration_tests {
    use super::super::backend::Backend;
    use super::*;
    #[tokio::test]
    async fn maintenance_claim_migration_current_populated_restart_and_gap_refusal() {
        let root = std::env::temp_dir().join(format!("fvoci-s16-migrate-{}", uuid::Uuid::now_v7()));
        std::fs::create_dir_all(&root).unwrap();
        let path = root.join("app.sqlite");
        run_sqlite_migrations(&path).await.unwrap();
        let pool = super::super::pool::connect_sqlite_app(&path, 1)
            .await
            .unwrap();
        let backend = Backend::Sqlite(pool.clone());
        let before = assert_sqlite_schema_current(&backend).await.unwrap();
        assert_eq!(before.applied_steps, 12);
        let workspace = uuid::Uuid::now_v7();
        sqlx::query(
            "INSERT INTO workspaces(id,slug,name) VALUES(?1,'s16-populated','preserved literal')",
        )
        .bind(workspace.as_bytes().as_slice())
        .execute(&pool)
        .await
        .unwrap();
        sqlx::query("UPDATE maintenance_job_claims SET generation=7 WHERE job_key=8")
            .execute(&pool)
            .await
            .unwrap();
        backend.close().await.unwrap();
        run_sqlite_migrations(&path).await.unwrap();
        let pool = super::super::pool::connect_sqlite_app(&path, 1)
            .await
            .unwrap();
        let backend = Backend::Sqlite(pool.clone());
        let current = assert_sqlite_schema_current(&backend).await.unwrap();
        assert_eq!(current.applied_steps, 12);
        assert_eq!(current.schema_sha256, before.schema_sha256);
        let preserved: String = sqlx::query_scalar("SELECT name FROM workspaces WHERE id=?1")
            .bind(workspace.as_bytes().as_slice())
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(preserved, "preserved literal");
        let rows: Vec<(i64, i64)> = sqlx::query_as(
            "SELECT job_key,generation FROM maintenance_job_claims ORDER BY job_key",
        )
        .fetch_all(&pool)
        .await
        .unwrap();
        assert_eq!(rows.len(), 9);
        assert_eq!(
            rows.iter().map(|r| r.0).collect::<Vec<_>>(),
            (1..=9).collect::<Vec<_>>()
        );
        assert_eq!(
            rows[7].1, 7,
            "restart cannot reseed/reset released generation"
        );
        let fk: i64 = sqlx::query_scalar("PRAGMA foreign_keys")
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(fk, 1);
        sqlx::query("DELETE FROM schema_migrations WHERE version=12")
            .execute(&pool)
            .await
            .unwrap();
        assert!(assert_sqlite_schema_current(&backend).await.is_err());
        backend.close().await.unwrap();
        assert!(run_sqlite_migrations(&path).await.is_err());
        let pool = super::super::pool::connect_sqlite_app(&path, 1)
            .await
            .unwrap();
        let name: String = sqlx::query_scalar("SELECT name FROM workspaces WHERE id=?1")
            .bind(workspace.as_bytes().as_slice())
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(name, "preserved literal");
        pool.close().await;
        std::fs::remove_dir_all(root).unwrap();
    }
    #[tokio::test]
    async fn maintenance_claim_migration_refuses_changed_definition_without_reset() {
        let root = std::env::temp_dir().join(format!("fvoci-s16-migrate-{}", uuid::Uuid::now_v7()));
        std::fs::create_dir_all(&root).unwrap();
        let path = root.join("app.sqlite");
        run_sqlite_migrations(&path).await.unwrap();
        let pool = super::super::pool::connect_sqlite_app(&path, 1)
            .await
            .unwrap();
        sqlx::query("ALTER TABLE maintenance_job_claims ADD COLUMN unexpected TEXT")
            .execute(&pool)
            .await
            .unwrap();
        let backend = Backend::Sqlite(pool.clone());
        assert!(assert_sqlite_schema_current(&backend).await.is_err());
        backend.close().await.unwrap();
        assert!(run_sqlite_migrations(&path).await.is_err());
        let pool = super::super::pool::connect_sqlite_app(&path, 1)
            .await
            .unwrap();
        let rows: i64 = sqlx::query_scalar("SELECT count(*) FROM maintenance_job_claims")
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(rows, 9);
        let unexpected:i64=sqlx::query_scalar("SELECT count(*) FROM pragma_table_info('maintenance_job_claims') WHERE name='unexpected'").fetch_one(&pool).await.unwrap();
        assert_eq!(unexpected, 1);
        pool.close().await;
        std::fs::remove_dir_all(root).unwrap();
    }
}

#[cfg(all(test, feature = "db-tests"))]
mod baseline_prefix_resume_tests {
    use super::super::backend::Backend;
    use super::*;
    #[tokio::test]
    async fn populated_eleven_step_prefix_resumes_to_the_complete_lineage_atomically() {
        let root = std::env::temp_dir().join(format!("fvoci-s16-upgrade-{}", uuid::Uuid::now_v7()));
        std::fs::create_dir_all(&root).unwrap();
        let path = root.join("app.sqlite");
        // An interrupted install: an exact prefix of this lineage, executed by the
        // SAME maintained per-step initializer and actual preparation/admission owner.
        // This is modern install interruption/resume, not an old-lineage upgrade.
        let admission = SqliteAdmission::migration(&path).unwrap();
        let preparation = super::super::pool::connect_sqlite_prepare(&path)
            .await
            .unwrap();
        let backend = Backend::Sqlite(preparation.pool.clone());
        for step in compiled_sqlite_steps().iter().take(11) {
            apply_sqlite_migration_step(&backend, step, None)
                .await
                .unwrap();
        }
        let workspace = uuid::Uuid::now_v7();
        sqlx::query(
            "INSERT INTO workspaces(id,slug,name) VALUES(?1,'s16-upgrade','populated011 literal')",
        )
        .bind(workspace.as_bytes().as_slice())
        .execute(&preparation.pool)
        .await
        .unwrap();
        // Carried from the retired 005->006 fixture: a live monotonic fence counter
        // written before the resume must survive it (step 06 seeds next_fence=1).
        sqlx::query("UPDATE collab_fence_counter SET next_fence=17 WHERE id=1")
            .execute(&preparation.pool)
            .await
            .unwrap();
        let old_markers: Vec<i64> =
            sqlx::query_scalar("SELECT version FROM schema_migrations ORDER BY version")
                .fetch_all(&preparation.pool)
                .await
                .unwrap();
        assert_eq!(old_markers, (1..=11).collect::<Vec<i64>>());
        assert!(
            assert_sqlite_schema_current(&backend).await.is_err(),
            "old prefix is never the complete lineage"
        );
        preparation.close_confirmed().await.unwrap();
        drop(admission);
        run_sqlite_migrations(&path).await.unwrap();
        let pool = super::super::pool::connect_sqlite_app(&path, 1)
            .await
            .unwrap();
        let backend = Backend::Sqlite(pool.clone());
        let current = assert_sqlite_schema_current(&backend).await.unwrap();
        assert_eq!(current.applied_steps, 12);
        let preserved: (String, i64) = sqlx::query_as(
            "SELECT name,(SELECT next_fence FROM collab_fence_counter WHERE id=1) FROM workspaces WHERE id=?1",
        )
        .bind(workspace.as_bytes().as_slice())
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(preserved, ("populated011 literal".into(), 17));
        let markers: Vec<i64> =
            sqlx::query_scalar("SELECT version FROM schema_migrations ORDER BY version")
                .fetch_all(&pool)
                .await
                .unwrap();
        assert_eq!(markers, (1..=12).collect::<Vec<i64>>());
        let count: i64 = sqlx::query_scalar("SELECT count(*) FROM maintenance_job_claims WHERE owner_token IS NULL AND generation=0 AND expires_at IS NULL")
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(count, 9);
        // Carried controls: the app connection enforces foreign keys, and a fence
        // row for an unknown task has no effect (FK violation, nothing written).
        let fk: i64 = sqlx::query_scalar("PRAGMA foreign_keys")
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(fk, 1);
        let wrong_task = uuid::Uuid::now_v7();
        let owner = uuid::Uuid::now_v7();
        let error = sqlx::query("INSERT INTO task_collab_room_fences(workspace_id,task_id,owner_token,fence,expires_at) VALUES(?1,?2,?3,17,1)")
            .bind(workspace.as_bytes().as_slice())
            .bind(wrong_task.as_bytes().as_slice())
            .bind(owner.as_bytes().as_slice())
            .execute(&pool)
            .await
            .unwrap_err();
        assert!(error
            .as_database_error()
            .unwrap()
            .is_foreign_key_violation());
        let fences: i64 = sqlx::query_scalar("SELECT count(*) FROM task_collab_room_fences")
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(fences, 0);
        // A claim generation advanced by the product (step 12 seeds generation 0)
        // must survive an installer rerun on the complete lineage, which must be
        // a no-op: same receipts, same schema digest, same counter.
        sqlx::query("UPDATE maintenance_job_claims SET generation=7 WHERE job_key=8")
            .execute(&pool)
            .await
            .unwrap();
        backend.close().await.unwrap();
        run_sqlite_migrations(&path).await.unwrap();
        let pool = super::super::pool::connect_sqlite_app(&path, 1)
            .await
            .unwrap();
        let backend = Backend::Sqlite(pool.clone());
        let restarted = assert_sqlite_schema_current(&backend).await.unwrap();
        assert_eq!(restarted.applied_steps, 12);
        assert_eq!(restarted.schema_sha256, current.schema_sha256);
        let markers: Vec<i64> =
            sqlx::query_scalar("SELECT version FROM schema_migrations ORDER BY version")
                .fetch_all(&pool)
                .await
                .unwrap();
        assert_eq!(markers, (1..=12).collect::<Vec<i64>>());
        let (counter, generation, untouched): (i64, i64, i64) = sqlx::query_as(
            "SELECT (SELECT next_fence FROM collab_fence_counter WHERE id=1),(SELECT generation FROM maintenance_job_claims WHERE job_key=8),(SELECT count(*) FROM maintenance_job_claims WHERE generation=0)",
        )
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!((counter, generation, untouched), (17, 7, 8));
        backend.close().await.unwrap();
        std::fs::remove_dir_all(root).unwrap();
    }
}

// ---------------------------------------------------------------------------
// Remote (libsql-remote) primary preparation and normal startup gate.
//
// Contract: evidence record 323 (sql-baseline author), root msg_83b8ec1b25f4,
// review fixes msg_f1cbaac09db8. Everything below reuses the existing
// SQLite-family lineage, the existing per-step applier
// (`apply_sqlite_migration_step`: one reserved writer stream per step, ledger
// gap check inside that stream, cancellation checkpoints, receipt and commit)
// and the existing gate (`assert_sqlite_schema_current`). Nothing here adds a
// schema, a parser, a lease framework or an in-place upgrade path: the remote
// backend installs the 0.6 baseline into an empty database, or resumes an
// interrupted install that is an exact prefix of the compiled lineage. The
// three production refusals (server startup, migrator binary, preparation)
// stay in place until the wiring is independently reviewed; this module only
// offers the replacement.
//
// Honest limits (G1/G2 of the contract): libSQL remote has no file lock and no
// advisory lock. The process-local admission semaphore and lease accounting
// bound this process only; they are not cross-server exclusion and there is no
// `pg_stat_activity` equivalent, so live application writers cannot be
// detected. Step-level safety still holds because each step runs inside one
// `BEGIN IMMEDIATE` stream whose ledger gap check executes in the same write
// transaction: two migrators cannot double-apply or interleave a step; the
// loser sees the gap refusal or the already-applied skip. A single token also
// means there is no owner/app privilege split on the remote backend; tenant
// isolation stays the in-process `FamilyTx` discipline, never a database role.
// ---------------------------------------------------------------------------

/// Ledger state of a SQLite-family database as read by the existing gate
/// helpers on one read stream (nothing is written).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum SqliteFamilyState {
    /// No ledger and no compiled objects: a fresh 0.6 install target.
    Blank,
    /// An exact, incomplete prefix of the compiled lineage: an interrupted
    /// install that resumes from the next step.
    Prefix(usize),
    /// Every compiled step with matching digests and an exact catalog.
    Current(usize),
}

/// Classifies the database without changing it. Ahead, gap, foreign lineage,
/// changed digest, unmarked or populated objects and the retired development
/// lineage are refused with the existing gate messages; they are never states.
async fn classify_sqlite_family(
    backend: &super::backend::Backend,
) -> Result<SqliteFamilyState, sqlx::Error> {
    use super::backend::DbTransaction;
    let mut tx = backend.begin_read().await?;
    let result = async {
        let DbTransaction::SqliteFamily(family) = &mut tx else {
            return Err(schema_error(
                "SQLite schema gate requires an actual SQLite-family handle",
            ));
        };
        let applied = sqlite_applied(family).await?;
        verify_sqlite_applied(&applied, false)?;
        verify_sqlite_objects(family, applied.len()).await?;
        let compiled = compiled_sqlite_steps().len();
        Ok(match applied.len() {
            0 => SqliteFamilyState::Blank,
            count if count == compiled => SqliteFamilyState::Current(count),
            count => SqliteFamilyState::Prefix(count),
        })
    }
    .await;
    match result {
        Ok(state) => {
            tx.rollback().await?;
            Ok(state)
        }
        Err(error) => Err(sqlite_validation_error_after_rollback(
            error,
            tx.rollback().await,
        )),
    }
}

/// Outcome of one remote primary preparation / migration run. Step counts are
/// the receipts validated by the gate after the run, not the steps this
/// process believes it wrote: another migrator may have advanced the ledger.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RemoteMigrationOutcome {
    /// Ledger and catalog were already exact; nothing was changed.
    Current,
    /// A blank database received the whole compiled lineage.
    Installed { steps: usize },
    /// An exact incomplete prefix of `from` receipts was completed to `to`.
    Resumed { from: usize, to: usize },
}

/// Bounded, typed failure of the remote helper. Neither `Display` nor `Debug`
/// ever includes the endpoint, the token, a raw driver message or any text
/// that is not one of the closed gate texts of [`gate_code`]. The underlying
/// `sqlx::Error` stays reachable through [`Self::driver_source`] for a
/// boundary that redacts, deliberately not through `Error::source`, and no
/// log-safety claim is made for that raw value.
/// Where the existing gate refused: before any write stream was opened, at
/// normal startup (never writes), or after migration steps were committed.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum GateStage {
    /// Classification on one read stream before the first write stream.
    PreWriteClassification,
    /// The normal startup gate of `connect_remote_app`; nothing is migrated.
    StartupGate,
    /// The full gate after the applier committed one or more steps: writes
    /// may have committed and only the ledger says which.
    PostMigrationValidation,
}

pub enum RemoteMigrationError {
    /// `RemoteDatabase::connect` refused the configured endpoint or token, or
    /// the backend was not `libsql-remote`.
    Connect { source: sqlx::Error },
    /// The existing gate refused the database (ahead, gap, foreign lineage,
    /// changed digest, unmarked or populated objects, retired lineage, or a
    /// catalog that differs from the compiled capability after the run).
    Gate {
        stage: GateStage,
        source: sqlx::Error,
        drain: Option<sqlx::Error>,
    },
    /// DDL or the receipt of one step failed. The existing applier attempts
    /// the rollback and folds its result into `source`, so no rollback
    /// confirmation exists here: the step's settlement is withheld (or an
    /// unconfirmed cleanup when the typed marker is present).
    Step {
        version: i32,
        source: sqlx::Error,
        drain: Option<sqlx::Error>,
    },
    /// The COMMIT of one step did not settle: the receipt may or may not exist.
    /// The next run re-reads the ledger and resumes safely either way.
    CommitUnknown {
        version: i32,
        source: sqlx::Error,
        drain: Option<sqlx::Error>,
    },
    /// Cancellation was observed at a checkpoint; `last_settled` receipts were
    /// validated before the stop.
    Cancelled {
        last_settled: usize,
        drain: Option<sqlx::Error>,
    },
    /// The run itself succeeded but closing the remote handle reported a
    /// failed or unconfirmed stream cleanup.
    Drain { source: sqlx::Error },
}

/// Closed set of gate texts this module repeats. The code is stable for
/// receipts; the text is the canonical message, never the error's own string.
/// A `Protocol` message outside this set is withheld like any other error.
const GATE_TEXTS: [(&str, &str, &str); 13] = [
    (
        "GATE_AHEAD_INCOMPLETE_UNPREPARED",
        "SQLite schema is ahead, incomplete or unprepared",
        "SQLite schema is ahead, incomplete or unprepared",
    ),
    (
        "GATE_GAP_FOREIGN_DIGEST",
        "SQLite schema has a gap, foreign lineage or changed digest",
        "SQLite schema has a gap, foreign lineage or changed digest",
    ),
    (
        "GATE_CATALOG_DIFFERS",
        "SQLite schema definitions differ from compiled capability; unmarked/populated or altered schema refused",
        "SQLite schema definitions differ from compiled capability; unmarked/populated or altered schema refused",
    ),
    (
        "GATE_RETIRED_LINEAGE",
        "SQLite schema carries the retired lineage ",
        "SQLite schema carries the retired development lineage; install into an empty database",
    ),
    (
        "GATE_STEP_GAP",
        "SQLite migration gap",
        "SQLite migration gap",
    ),
    (
        "GATE_REFERENCE_PIN",
        "SQLite schema reference engine pin mismatch",
        "SQLite schema reference engine pin mismatch",
    ),
    (
        "GATE_FAMILY_HANDLE",
        "SQLite schema gate requires an actual SQLite-family handle",
        "SQLite schema gate requires an actual SQLite-family handle",
    ),
    (
        "GATE_FAMILY_HANDLE",
        "SQLite migrations require an actual SQLite-family handle",
        "SQLite migrations require an actual SQLite-family handle",
    ),
    (
        "GATE_BACKEND_KIND",
        "remote migrator requires the libsql-remote backend",
        "remote migrator requires the libsql-remote backend",
    ),
    (
        "GATE_BACKEND_KIND",
        "remote startup requires the libsql-remote backend",
        "remote startup requires the libsql-remote backend",
    ),
    (
        "GATE_ENDPOINT_SHAPE",
        "remote libSQL requires a TLS primary endpoint and token",
        "remote libSQL requires a TLS primary endpoint and token",
    ),
    (
        "GATE_CANCELLED",
        "SQLite migration cancelled",
        "SQLite migration cancelled at a checkpoint",
    ),
    (
        "GATE_VALIDATION_FAILED",
        "schema validation failed",
        "schema validation failed",
    ),
];

/// Maps an error to its closed gate code and canonical text, if it is one of
/// this module's own fixed `Protocol` messages (exact, or the fixed prefix of
/// the retired-lineage and cancellation messages). Everything else is `None`.
fn gate_code(error: &sqlx::Error) -> Option<(&'static str, &'static str)> {
    let sqlx::Error::Protocol(message) = error else {
        return None;
    };
    GATE_TEXTS
        .iter()
        .find(|(_, needle, _)| {
            message == needle
                || (matches!(
                    *needle,
                    "SQLite schema carries the retired lineage " | "SQLite migration cancelled"
                ) && message.starts_with(needle))
        })
        .map(|(code, _, text)| (*code, *text))
}

fn bounded_gate_text(error: &sqlx::Error) -> &'static str {
    gate_code(error).map_or("driver error withheld", |(_, text)| text)
}

impl RemoteMigrationError {
    /// Stable code for operators and receipts.
    pub fn code(&self) -> &'static str {
        match self {
            Self::Connect { .. } => "REMOTE_CONNECT_REFUSED",
            Self::Gate { .. } => "REMOTE_SCHEMA_GATE_REFUSED",
            Self::Step { .. } => "REMOTE_MIGRATION_STEP_FAILED",
            Self::CommitUnknown { .. } => "REMOTE_MIGRATION_COMMIT_UNKNOWN",
            Self::Cancelled { .. } => "REMOTE_MIGRATION_CANCELLED",
            Self::Drain { .. } => "REMOTE_DRAIN_FAILED",
        }
    }
    /// The closed gate code of the source when it is one of this module's own
    /// fixed messages; `None` for driver errors and for `Cancelled`.
    pub fn gate(&self) -> Option<&'static str> {
        self.driver_source()
            .and_then(gate_code)
            .map(|(code, _)| code)
    }
    /// The underlying driver or gate error, for a boundary that redacts.
    /// `Cancelled` carries no source unless the drain at close failed.
    pub fn driver_source(&self) -> Option<&sqlx::Error> {
        match self {
            Self::Connect { source }
            | Self::Gate { source, .. }
            | Self::Step { source, .. }
            | Self::CommitUnknown { source, .. }
            | Self::Drain { source } => Some(source),
            Self::Cancelled { drain, .. } => drain.as_ref(),
        }
    }
    /// Settlement of the failed work as far as a typed receipt proves it.
    /// `Step` never carries a rollback receipt (the existing applier folds a
    /// failed rollback into its error), so it is `rollback-confirmation-withheld`
    /// unless the typed unconfirmed-cleanup marker is present.
    /// A failed drain at close takes priority: no settlement is claimed when
    /// the owned streams did not retire cleanly.
    pub fn settlement(&self) -> &'static str {
        if self.drain_failed() {
            return "drain-failed";
        }
        match self {
            Self::Step { source, .. } if cleanup_is_unconfirmed(source) => "cleanup-unconfirmed",
            Self::Step { .. } => "rollback-confirmation-withheld",
            Self::CommitUnknown { .. } => "commit-unknown",
            Self::Cancelled { .. } => "cancel-checkpoint-settled",
            Self::Gate {
                stage: GateStage::PostMigrationValidation,
                ..
            } => "writes-may-have-committed",
            Self::Gate { .. } | Self::Connect { .. } => "no-write-opened",
            Self::Drain { .. } => "drain-failed",
        }
    }
    /// The gate stage of a `Gate` refusal.
    pub fn gate_stage(&self) -> Option<GateStage> {
        match self {
            Self::Gate { stage, .. } => Some(*stage),
            _ => None,
        }
    }
    /// Whether the owned stream drain at close reported a failure.
    pub fn drain_failed(&self) -> bool {
        match self {
            Self::Gate { drain, .. }
            | Self::Step { drain, .. }
            | Self::CommitUnknown { drain, .. }
            | Self::Cancelled { drain, .. } => drain.is_some(),
            Self::Drain { .. } => true,
            Self::Connect { .. } => false,
        }
    }
}

impl std::fmt::Display for RemoteMigrationError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let drain = if self.drain_failed() {
            "; remote stream drain failed at close"
        } else {
            ""
        };
        match self {
            Self::Connect { .. } => write!(
                f,
                "remote libSQL primary connect refused (TLS endpoint and token required)"
            ),
            Self::Gate { stage, source, .. } => write!(
                f,
                "{}: {}{drain}",
                match stage {
                    GateStage::PreWriteClassification => {
                        "remote schema gate refused before any write"
                    }
                    GateStage::StartupGate => "remote startup gate refused",
                    GateStage::PostMigrationValidation => {
                        "remote schema gate refused after migration steps committed"
                    }
                },
                bounded_gate_text(source)
            ),
            Self::Step { version, source, .. } => write!(
                f,
                "remote migration step {version} failed; {}: {}{drain}",
                if cleanup_is_unconfirmed(source) {
                    "cleanup unconfirmed, admission quarantined"
                } else {
                    "rollback confirmation withheld"
                },
                bounded_gate_text(source)
            ),
            Self::CommitUnknown { version, .. } => write!(
                f,
                "remote migration step {version} commit outcome is unknown; settlement receipt retained; rerun resumes from the ledger{drain}"
            ),
            Self::Cancelled { last_settled, .. } => write!(
                f,
                "remote migration cancelled after {last_settled} settled step(s){drain}"
            ),
            Self::Drain { .. } => write!(f, "remote stream drain failed at close"),
        }
    }
}

/// Redacted structural `Debug`: variant, numbers, codes and withheld markers
/// only; the raw `sqlx::Error` values are never formatted.
impl std::fmt::Debug for RemoteMigrationError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let mut debug = f.debug_struct("RemoteMigrationError");
        debug.field("code", &self.code());
        match self {
            Self::Step { version, .. } | Self::CommitUnknown { version, .. } => {
                debug.field("version", version);
            }
            Self::Cancelled { last_settled, .. } => {
                debug.field("last_settled", last_settled);
            }
            Self::Connect { .. } | Self::Gate { .. } | Self::Drain { .. } => {}
        }
        debug.field("gate", &self.gate());
        debug.field("gate_stage", &self.gate_stage());
        debug.field("settlement", &self.settlement());
        debug.field("source", &"<withheld>");
        debug.field("drain_failed", &self.drain_failed());
        debug.finish()
    }
}

impl std::error::Error for RemoteMigrationError {}

fn is_commit_unknown(error: &sqlx::Error) -> bool {
    matches!(
        error,
        sqlx::Error::AnyDriverError(source)
            if source.downcast_ref::<super::backend::CommitUnknown>().is_some()
    )
}

/// Classifies one failed step of the existing applier. Only an unknown COMMIT
/// (the typed `CommitUnknown` receipt) is distinguished; every other applier
/// failure, including the applier's own cancellation checkpoint message, stays
/// a `Step` failure that retains its primary source and closed gate code,
/// because a message prefix is not a typed receipt. `Cancelled` is produced
/// only by this module's own pre-step token checkpoint, before the applier
/// opened a write stream. The applier folds a failed rollback into its error
/// (`sqlite_validation_error_after_rollback`), so a `Step` failure carries no
/// rollback receipt: its settlement is reported as withheld, or as an
/// unconfirmed cleanup when the typed `MigrationCleanupUnconfirmed` is present.
fn classify_step_failure(version: i32, source: sqlx::Error) -> RemoteMigrationError {
    if is_commit_unknown(&source) {
        RemoteMigrationError::CommitUnknown {
            version,
            source,
            drain: None,
        }
    } else {
        RemoteMigrationError::Step {
            version,
            source,
            drain: None,
        }
    }
}

/// Applies the policy of the contract on an already admitted remote backend
/// and always closes it afterwards. Blank or exact-prefix states are migrated
/// with the existing per-step applier; `Current` is a no-op; everything else is
/// refused by the existing gate before any write stream is opened.
async fn remote_migrate_backend(
    backend: super::backend::Backend,
    cancel: &tokio_util::sync::CancellationToken,
) -> Result<RemoteMigrationOutcome, RemoteMigrationError> {
    let outcome = remote_migrate_admitted(&backend, cancel).await;
    let drain = backend.close().await.err();
    match (outcome, drain) {
        (Ok(outcome), None) => Ok(outcome),
        (Ok(_), Some(source)) => Err(RemoteMigrationError::Drain { source }),
        (Err(error), drain) => Err(match error {
            RemoteMigrationError::Gate { stage, source, .. } => RemoteMigrationError::Gate {
                stage,
                source,
                drain,
            },
            RemoteMigrationError::Step {
                version, source, ..
            } => RemoteMigrationError::Step {
                version,
                source,
                drain,
            },
            RemoteMigrationError::CommitUnknown {
                version, source, ..
            } => RemoteMigrationError::CommitUnknown {
                version,
                source,
                drain,
            },
            RemoteMigrationError::Cancelled { last_settled, .. } => {
                RemoteMigrationError::Cancelled {
                    last_settled,
                    drain,
                }
            }
            other => other,
        }),
    }
}

async fn remote_migrate_admitted(
    backend: &super::backend::Backend,
    cancel: &tokio_util::sync::CancellationToken,
) -> Result<RemoteMigrationOutcome, RemoteMigrationError> {
    let initial = match classify_sqlite_family(backend).await.map_err(|source| {
        RemoteMigrationError::Gate {
            stage: GateStage::PreWriteClassification,
            source,
            drain: None,
        }
    })? {
        SqliteFamilyState::Current(_) => return Ok(RemoteMigrationOutcome::Current),
        SqliteFamilyState::Blank => 0,
        SqliteFamilyState::Prefix(count) => count,
    };
    let compiled = compiled_sqlite_steps();
    let mut settled = initial;
    for step in compiled.iter().skip(initial) {
        if cancel.is_cancelled() {
            return Err(RemoteMigrationError::Cancelled {
                last_settled: settled,
                drain: None,
            });
        }
        match apply_sqlite_migration_step(backend, step, Some(cancel)).await {
            Ok(()) => settled += 1,
            Err(source) => return Err(classify_step_failure(step.version, source)),
        }
    }
    // Steps may have committed above: a refusal here is a post-migration
    // validation failure, never a pre-write classification.
    let current = assert_sqlite_schema_current(backend)
        .await
        .map_err(|source| RemoteMigrationError::Gate {
            stage: GateStage::PostMigrationValidation,
            source,
            drain: None,
        })?;
    Ok(if initial == 0 {
        RemoteMigrationOutcome::Installed {
            steps: current.applied_steps,
        }
    } else {
        RemoteMigrationOutcome::Resumed {
            from: initial,
            to: current.applied_steps,
        }
    })
}

/// Remote primary preparation and migrator for the SQLite-family lineage.
///
/// Opens ONE admitted stream (`max_connections = 1`), classifies the ledger
/// with the existing gate, applies only `Blank` or exact-prefix states through
/// the existing per-step applier under `cancel`, re-runs the full gate, then
/// always closes and drains the handle. The settings are read only to connect;
/// neither the endpoint nor the token appears in any returned text.
pub async fn run_remote_migrations(
    settings: &crate::config::DatabaseSettings,
    cancel: &tokio_util::sync::CancellationToken,
) -> Result<RemoteMigrationOutcome, RemoteMigrationError> {
    let crate::config::DatabaseSettings::LibsqlRemote {
        primary_url,
        auth_token,
    } = settings
    else {
        return Err(RemoteMigrationError::Connect {
            source: schema_error("remote migrator requires the libsql-remote backend"),
        });
    };
    let database =
        super::backend::RemoteDatabase::connect(primary_url.clone(), auth_token.clone(), 1)
            .await
            .map_err(|source| RemoteMigrationError::Connect { source })?;
    remote_migrate_backend(super::backend::Backend::LibsqlRemote(database), cancel).await
}

/// Normal startup gate for the remote backend: connects with the server pool
/// size and returns the backend only when the existing gate reports the exact
/// compiled lineage. Any other state closes the handle and refuses; nothing is
/// ever migrated at startup.
pub async fn connect_remote_app(
    settings: &crate::config::DatabaseSettings,
    app_pool_max: u32,
) -> Result<super::backend::Backend, RemoteMigrationError> {
    let crate::config::DatabaseSettings::LibsqlRemote {
        primary_url,
        auth_token,
    } = settings
    else {
        return Err(RemoteMigrationError::Connect {
            source: schema_error("remote startup requires the libsql-remote backend"),
        });
    };
    let database = super::backend::RemoteDatabase::connect(
        primary_url.clone(),
        auth_token.clone(),
        app_pool_max,
    )
    .await
    .map_err(|source| RemoteMigrationError::Connect { source })?;
    let backend = super::backend::Backend::LibsqlRemote(database);
    match assert_sqlite_schema_current(&backend).await {
        Ok(_) => Ok(backend),
        Err(source) => {
            let drain = backend.close().await.err();
            Err(RemoteMigrationError::Gate {
                stage: GateStage::StartupGate,
                source,
                drain,
            })
        }
    }
}

#[cfg(test)]
mod remote_helper_display_tests {
    use super::*;

    const SECRET: &str = "https://primary.secret.example/?authToken=TOKEN-VALUE";

    fn driver_error() -> sqlx::Error {
        sqlx::Error::AnyDriverError(SECRET.into())
    }
    fn protocol_canary() -> sqlx::Error {
        schema_error(format!("gate message leaking {SECRET}"))
    }
    fn assert_secret_free(text: &str) {
        for needle in ["secret.example", "TOKEN-VALUE", "authToken", "leaking"] {
            assert!(!text.contains(needle), "{needle} leaked: {text}");
        }
    }

    #[test]
    fn display_and_debug_never_repeat_endpoint_token_or_driver_text() {
        let errors = [
            RemoteMigrationError::Connect {
                source: driver_error(),
            },
            RemoteMigrationError::Connect {
                source: protocol_canary(),
            },
            RemoteMigrationError::Gate {
                stage: GateStage::PreWriteClassification,
                source: driver_error(),
                drain: Some(protocol_canary()),
            },
            RemoteMigrationError::Gate {
                stage: GateStage::StartupGate,
                source: protocol_canary(),
                drain: None,
            },
            RemoteMigrationError::Gate {
                stage: GateStage::PostMigrationValidation,
                source: schema_error("SQLite schema is ahead, incomplete or unprepared"),
                drain: None,
            },
            RemoteMigrationError::Step {
                version: 7,
                source: protocol_canary(),
                drain: Some(driver_error()),
            },
            RemoteMigrationError::CommitUnknown {
                version: 3,
                source: driver_error(),
                drain: None,
            },
            RemoteMigrationError::Cancelled {
                last_settled: 5,
                drain: Some(protocol_canary()),
            },
            RemoteMigrationError::Drain {
                source: protocol_canary(),
            },
        ];
        for error in &errors {
            assert_secret_free(&error.to_string());
            assert_secret_free(&format!("{error:?}"));
            assert_secret_free(&format!("{error:#?}"));
            assert!(std::error::Error::source(error).is_none());
            assert!(!error.code().is_empty());
        }
        assert_eq!(
            errors[4].to_string(),
            "remote schema gate refused after migration steps committed: SQLite schema is ahead, incomplete or unprepared"
        );
        assert_eq!(errors[4].gate(), Some("GATE_AHEAD_INCOMPLETE_UNPREPARED"));
        assert_eq!(errors[4].settlement(), "writes-may-have-committed");
        assert_eq!(
            errors[4].gate_stage(),
            Some(GateStage::PostMigrationValidation)
        );
        assert_eq!(
            errors[2].to_string(),
            "remote schema gate refused before any write: driver error withheld; remote stream drain failed at close"
        );
        assert_eq!(errors[2].settlement(), "drain-failed");
        assert_eq!(
            errors[3].to_string(),
            "remote startup gate refused: driver error withheld"
        );
        assert_eq!(errors[3].gate(), None);
        assert_eq!(errors[3].settlement(), "no-write-opened");
        assert_eq!(errors[7].settlement(), "drain-failed");
        assert_eq!(errors[6].gate_stage(), None);
        assert_eq!(
            errors[6].to_string(),
            "remote migration step 3 commit outcome is unknown; settlement receipt retained; rerun resumes from the ledger"
        );
        assert!(errors[5].drain_failed() && !errors[6].drain_failed());
        assert_eq!(
            format!("{:?}", errors[6]),
            "RemoteMigrationError { code: \"REMOTE_MIGRATION_COMMIT_UNKNOWN\", version: 3, gate: None, gate_stage: None, settlement: \"commit-unknown\", source: \"<withheld>\", drain_failed: false }"
        );
        assert_eq!(
            errors[5].to_string(),
            "remote migration step 7 failed; rollback confirmation withheld: driver error withheld; remote stream drain failed at close"
        );
        // A Step whose drain failed claims no settlement at all; the withheld
        // confirmation is reported only for a Step with a clean drain.
        assert_eq!(errors[5].settlement(), "drain-failed");
        let withheld = RemoteMigrationError::Step {
            version: 7,
            source: protocol_canary(),
            drain: None,
        };
        assert_eq!(withheld.settlement(), "rollback-confirmation-withheld");
        assert_eq!(
            withheld.to_string(),
            "remote migration step 7 failed; rollback confirmation withheld: driver error withheld"
        );
        assert_secret_free(&format!("{withheld:?}"));
        let unconfirmed = RemoteMigrationError::Step {
            version: 2,
            source: unconfirmed_migration_cleanup(protocol_canary()),
            drain: None,
        };
        assert_eq!(unconfirmed.settlement(), "cleanup-unconfirmed");
        assert_secret_free(&unconfirmed.to_string());
        assert_secret_free(&format!("{unconfirmed:?}"));
        assert_eq!(
            unconfirmed.to_string(),
            "remote migration step 2 failed; cleanup unconfirmed, admission quarantined: driver error withheld"
        );
    }

    #[test]
    fn step_failures_keep_their_primary_cause_and_only_the_local_checkpoint_cancels() {
        // The applier's cancellation message is a string, not a typed receipt:
        // it stays a Step failure with its closed gate code and withheld settlement.
        let applier_cancel =
            classify_step_failure(4, schema_error("SQLite migration cancelled before DDL"));
        assert!(matches!(
            &applier_cancel,
            RemoteMigrationError::Step { version: 4, .. }
        ));
        assert_eq!(applier_cancel.gate(), Some("GATE_CANCELLED"));
        assert_eq!(
            applier_cancel.settlement(),
            "rollback-confirmation-withheld"
        );
        // Cancelled exists only from the local pre-step checkpoint, and claims
        // nothing once the drain failed.
        let local = RemoteMigrationError::Cancelled {
            last_settled: 3,
            drain: None,
        };
        assert_eq!(local.settlement(), "cancel-checkpoint-settled");
        let local_drain_failed = RemoteMigrationError::Cancelled {
            last_settled: 3,
            drain: Some(driver_error()),
        };
        assert_eq!(local_drain_failed.settlement(), "drain-failed");
        // A primary failure is never erased, whatever a token says concurrently.
        let primary = classify_step_failure(4, schema_error("SQLite migration gap"));
        assert!(matches!(
            &primary,
            RemoteMigrationError::Step { version: 4, .. }
        ));
        assert_eq!(primary.gate(), Some("GATE_STEP_GAP"));
        assert_eq!(primary.settlement(), "rollback-confirmation-withheld");
        // An unknown settlement keeps precedence over any other classification.
        let unknown = classify_step_failure(
            5,
            sqlx::Error::AnyDriverError(Box::new(super::super::backend::CommitUnknown {
                source: driver_error(),
            })),
        );
        assert!(matches!(
            &unknown,
            RemoteMigrationError::CommitUnknown { version: 5, .. }
        ));
        assert_secret_free(&unknown.to_string());
        assert_secret_free(&format!("{unknown:?}"));
        // A folded rollback failure or an unconfirmed cleanup stays a Step
        // failure with its typed settlement, never Cancelled.
        let folded = classify_step_failure(6, unconfirmed_migration_cleanup(driver_error()));
        assert_eq!(folded.settlement(), "cleanup-unconfirmed");
        assert_eq!(folded.code(), "REMOTE_MIGRATION_STEP_FAILED");
    }

    #[test]
    fn gate_codes_are_a_closed_set() {
        let retired = schema_error(format!(
            "SQLite schema carries the retired lineage {RETIRED_SQLITE_LINEAGE}; {RETIRED_LINEAGE_HINT}"
        ));
        assert_eq!(
            gate_code(&retired),
            Some((
                "GATE_RETIRED_LINEAGE",
                "SQLite schema carries the retired development lineage; install into an empty database"
            ))
        );
        assert_eq!(
            gate_code(&schema_error("SQLite migration cancelled before DDL")),
            Some((
                "GATE_CANCELLED",
                "SQLite migration cancelled at a checkpoint"
            ))
        );
        assert_eq!(
            gate_code(&schema_error("SQLite migration gap")).map(|(c, _)| c),
            Some("GATE_STEP_GAP")
        );
        assert_eq!(
            gate_code(&schema_error("SQLite migration gap elsewhere")),
            None
        );
        assert_eq!(gate_code(&protocol_canary()), None);
        assert_eq!(gate_code(&driver_error()), None);
        let cancelled = RemoteMigrationError::Cancelled {
            last_settled: 2,
            drain: None,
        };
        assert!(cancelled.driver_source().is_none() && cancelled.gate().is_none());
        let connect = RemoteMigrationError::Connect {
            source: schema_error("remote migrator requires the libsql-remote backend"),
        };
        assert_eq!(connect.gate(), Some("GATE_BACKEND_KIND"));
    }
}

/// Actual remote-family execution of the helper against the maintained
/// libsql 0.9.30 -> proxy -> official sqld fixture (`FVOCI_TEST_SQLD`, no
/// skip), through the maintained `RemoteDatabase::from_test_driver` admission.
/// Explicitly selected only; never part of the registered cohorts.
#[cfg(all(test, feature = "db-tests"))]
mod remote_primary_helper_tests {
    use super::super::backend::{Backend, DbTransaction, RemoteDatabase};
    use super::super::codec::Cell;
    use super::super::libsql_finish_fixture::LibsqlFinishFixture;
    use super::*;
    use std::num::NonZeroU32;
    use std::path::PathBuf;
    use tokio_util::sync::CancellationToken;

    struct Fixture {
        driver: LibsqlFinishFixture,
        root: PathBuf,
    }

    impl Fixture {
        async fn start() -> Self {
            let sqld = PathBuf::from(
                std::env::var_os("FVOCI_TEST_SQLD")
                    .expect("allocated pinned official sqld required; no SDK skip"),
            );
            // The maintained fixture creates the run directory itself
            // (`create_dir`, never `create_dir_all`): the caller only provides a
            // nonexistent unique path and reaps it after the receipt.
            let root =
                std::env::temp_dir().join(format!("fvoci-remote-helper-{}", uuid::Uuid::now_v7()));
            let driver = LibsqlFinishFixture::start(&sqld, &root).await.unwrap();
            Self { driver, root }
        }
        /// A fresh admitted backend over the same fixture database, exactly
        /// like the helper's own single-stream admission.
        async fn backend(&self) -> Backend {
            let database = self.driver.database().await.unwrap();
            Backend::LibsqlRemote(std::sync::Arc::new(RemoteDatabase::from_test_driver(
                database,
                NonZeroU32::new(1).unwrap(),
            )))
        }
        async fn finish(self) {
            let receipt = self.driver.finish().await.unwrap();
            assert!(receipt.sqld_reaped, "{receipt:?}");
            std::fs::remove_dir_all(self.root).unwrap();
        }
    }

    /// One bound write on a fresh single stream, committed; the system
    /// context mirrors the maintained migration fixtures.
    async fn write(backend: &Backend, statement: &'static str, args: &[Cell]) {
        let mut tx = backend.begin_write().await.unwrap();
        let DbTransaction::SqliteFamily(family) = &mut tx else {
            panic!("remote family handle expected");
        };
        family.replace_system_context(true);
        family.execute(statement, args).await.unwrap();
        tx.commit().await.unwrap();
    }

    async fn gate_of(f: &Fixture) -> RemoteMigrationError {
        remote_migrate_backend(f.backend().await, &CancellationToken::new())
            .await
            .unwrap_err()
    }

    fn assert_gate(error: &RemoteMigrationError, code: &str) {
        assert_eq!(error.code(), "REMOTE_SCHEMA_GATE_REFUSED", "{error:?}");
        assert_eq!(error.gate(), Some(code), "{error:?}");
        assert!(!error.drain_failed(), "{error:?}");
    }

    async fn assert_current(f: &Fixture) {
        assert_eq!(
            remote_migrate_backend(f.backend().await, &CancellationToken::new())
                .await
                .unwrap(),
            RemoteMigrationOutcome::Current
        );
    }

    /// Bounded observation of everything a refusal must leave untouched: the
    /// entire ledger (with applied_at), the full catalog as the existing
    /// objects query lists it, and the seeded business row.
    type Snapshot = (
        Vec<(i64, String, String, i64)>,
        Vec<SqliteObject>,
        Vec<(uuid::Uuid, String, String)>,
    );
    async fn snapshot(f: &Fixture) -> Snapshot {
        let backend = f.backend().await;
        let mut tx = backend.begin_read().await.unwrap();
        let DbTransaction::SqliteFamily(family) = &mut tx else {
            panic!("remote family handle expected");
        };
        let ledger = family
            .query(
                "SELECT version,lineage,sql_sha256,applied_at FROM schema_migrations ORDER BY version",
                &[],
            )
            .await
            .unwrap()
            .iter()
            .map(|row| {
                (
                    row.cell(0).unwrap().integer().unwrap(),
                    row.cell(1).unwrap().string().unwrap(),
                    row.cell(2).unwrap().string().unwrap(),
                    row.cell(3).unwrap().integer().unwrap(),
                )
            })
            .collect();
        let catalog = family
            .query(SQLITE_OBJECTS, &[])
            .await
            .unwrap()
            .iter()
            .map(|row| {
                (
                    row.cell(0).unwrap().string().unwrap(),
                    row.cell(1).unwrap().string().unwrap(),
                    row.cell(2).unwrap().string().unwrap(),
                    row.cell(3).unwrap().string().unwrap(),
                )
            })
            .collect();
        let rows = family
            .query("SELECT id,slug,name FROM workspaces ORDER BY slug", &[])
            .await
            .unwrap()
            .iter()
            .map(|row| {
                (
                    row.cell(0).unwrap().id().unwrap(),
                    row.cell(1).unwrap().string().unwrap(),
                    row.cell(2).unwrap().string().unwrap(),
                )
            })
            .collect();
        tx.rollback().await.unwrap();
        backend.close().await.unwrap();
        (ledger, catalog, rows)
    }

    async fn receipts(f: &Fixture) -> Vec<i64> {
        let backend = f.backend().await;
        let mut tx = backend.begin_read().await.unwrap();
        let DbTransaction::SqliteFamily(family) = &mut tx else {
            panic!("remote family handle expected");
        };
        let rows = family
            .query(
                "SELECT version FROM schema_migrations ORDER BY version",
                &[],
            )
            .await
            .unwrap();
        let versions = rows
            .iter()
            .map(|row| row.cell(0).unwrap().integer().unwrap())
            .collect();
        tx.rollback().await.unwrap();
        backend.close().await.unwrap();
        versions
    }

    #[tokio::test]
    async fn blank_installs_the_whole_lineage_then_reports_current() {
        let f = Fixture::start().await;
        let cancel = CancellationToken::new();
        let first = remote_migrate_backend(f.backend().await, &cancel)
            .await
            .unwrap();
        assert_eq!(first, RemoteMigrationOutcome::Installed { steps: 12 });
        assert_eq!(receipts(&f).await, (1..=12).collect::<Vec<i64>>());
        assert_current(&f).await;
        let backend = f.backend().await;
        let capability = assert_sqlite_schema_current(&backend).await.unwrap();
        assert_eq!(capability.applied_steps, 12);
        backend.close().await.unwrap();
        f.finish().await;
    }

    #[tokio::test]
    async fn exact_incomplete_prefix_resumes_and_preserves_rows() {
        let f = Fixture::start().await;
        let backend = f.backend().await;
        for step in compiled_sqlite_steps().iter().take(7) {
            apply_sqlite_migration_step(&backend, step, None)
                .await
                .unwrap();
        }
        let workspace = uuid::Uuid::now_v7();
        write(
            &backend,
            "INSERT INTO workspaces(id,slug,name) VALUES(?1,'remote-resume','interrupted install literal')",
            &[Cell::uuid(workspace)],
        )
        .await;
        backend.close().await.unwrap();
        let outcome = remote_migrate_backend(f.backend().await, &CancellationToken::new())
            .await
            .unwrap();
        assert_eq!(outcome, RemoteMigrationOutcome::Resumed { from: 7, to: 12 });
        let backend = f.backend().await;
        let mut tx = backend.begin_read().await.unwrap();
        let DbTransaction::SqliteFamily(family) = &mut tx else {
            panic!("remote family handle expected");
        };
        let rows = family
            .query(
                "SELECT id,name FROM workspaces WHERE slug='remote-resume'",
                &[],
            )
            .await
            .unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].cell(0).unwrap().id().unwrap(), workspace);
        assert_eq!(
            rows[0].cell(1).unwrap().string().unwrap(),
            "interrupted install literal"
        );
        tx.rollback().await.unwrap();
        backend.close().await.unwrap();
        f.finish().await;
    }

    #[tokio::test]
    async fn ahead_gap_foreign_digest_drift_unmarked_and_retired_are_refused_without_writes() {
        let f = Fixture::start().await;
        remote_migrate_backend(f.backend().await, &CancellationToken::new())
            .await
            .unwrap();
        let steps = compiled_sqlite_steps();
        let seeded = uuid::Uuid::now_v7();
        let backend = f.backend().await;
        write(
            &backend,
            "INSERT INTO workspaces(id,slug,name) VALUES(?1,'remote-seed','seeded business row')",
            &[Cell::uuid(seeded)],
        )
        .await;
        backend.close().await.unwrap();
        assert_current(&f).await;
        let baseline = snapshot(&f).await;
        assert_eq!(baseline.0.len(), 12);
        assert_eq!(
            baseline.2,
            vec![(
                seeded,
                "remote-seed".to_string(),
                "seeded business row".to_string()
            )]
        );
        // The ledger CHECK pins lineage to the compiled value, so a foreign or
        // retired lineage can only exist in a ledger table whose definition is
        // not this binary's: exactly what a foreign database carries. Those two
        // vectors replace the table (rows copied through a hold table) and the
        // repair recreates it from its own stored definition, byte for byte.
        let ledger_ddl: &'static str = Box::leak(
            baseline
                .1
                .iter()
                .find(|object| object.0 == "table" && object.1 == "schema_migrations")
                .expect("ledger definition in the catalog")
                .3
                .clone()
                .into_boxed_str(),
        );
        const HOLD: &str = "CREATE TABLE schema_migrations_hold AS SELECT version,lineage,sql_sha256,applied_at FROM schema_migrations";
        const DROP: &str = "DROP TABLE schema_migrations";
        const DROP_HOLD: &str = "DROP TABLE schema_migrations_hold";
        const FOREIGN_DDL: &str = "CREATE TABLE schema_migrations (version INTEGER PRIMARY KEY NOT NULL, lineage TEXT NOT NULL, sql_sha256 TEXT NOT NULL, applied_at INTEGER NOT NULL) STRICT";
        const RESTORE_ROWS: &str = "INSERT INTO schema_migrations SELECT version,'fvoci-sqlite-060',sql_sha256,applied_at FROM schema_migrations_hold";
        type Statements = Vec<(&'static str, Vec<Cell>)>;
        // (deviation sequence, expected closed gate code, repair sequence)
        let vectors: Vec<(Statements, &'static str, Statements)> = vec![
            (
                // Ahead: a thirteenth receipt beyond this binary's lineage.
                vec![(
                    "INSERT INTO schema_migrations(version,lineage,sql_sha256,applied_at) VALUES(13,'fvoci-sqlite-060',?1,unixepoch()*1000000)",
                    vec![Cell::text(steps[11].sha256)],
                )],
                "GATE_AHEAD_INCOMPLETE_UNPREPARED",
                vec![("DELETE FROM schema_migrations WHERE version=13", vec![])],
            ),
            (
                // Gap: receipt 6 missing between 5 and 7.
                vec![("DELETE FROM schema_migrations WHERE version=6", vec![])],
                "GATE_GAP_FOREIGN_DIGEST",
                vec![(
                    "INSERT INTO schema_migrations(version,lineage,sql_sha256,applied_at) VALUES(6,'fvoci-sqlite-060',?1,unixepoch()*1000000)",
                    vec![Cell::text(steps[5].sha256)],
                )],
            ),
            (
                // Foreign lineage on one receipt, inside a foreign ledger definition.
                vec![
                    (HOLD, vec![]),
                    (DROP, vec![]),
                    (FOREIGN_DDL, vec![]),
                    (
                        "INSERT INTO schema_migrations SELECT version,CASE WHEN version=3 THEN 'foreign-lineage' ELSE lineage END,sql_sha256,applied_at FROM schema_migrations_hold",
                        vec![],
                    ),
                    (DROP_HOLD, vec![]),
                ],
                "GATE_GAP_FOREIGN_DIGEST",
                vec![
                    (HOLD, vec![]),
                    (DROP, vec![]),
                    (ledger_ddl, vec![]),
                    (RESTORE_ROWS, vec![]),
                    (DROP_HOLD, vec![]),
                ],
            ),
            (
                // Changed digest on one receipt.
                vec![(
                    "UPDATE schema_migrations SET sql_sha256='0000000000000000000000000000000000000000000000000000000000000000' WHERE version=1",
                    vec![],
                )],
                "GATE_GAP_FOREIGN_DIGEST",
                vec![(
                    "UPDATE schema_migrations SET sql_sha256=?1 WHERE version=1",
                    vec![Cell::text(steps[0].sha256)],
                )],
            ),
            (
                // Incomplete ledger with catalog drift: receipt 12 deleted while
                // the objects of step 12 remain; the eleven receipts are an exact
                // prefix, so the objects verifier (not the ahead check) refuses.
                vec![("DELETE FROM schema_migrations WHERE version=12", vec![])],
                "GATE_CATALOG_DIFFERS",
                vec![(
                    "INSERT INTO schema_migrations(version,lineage,sql_sha256,applied_at) VALUES(12,'fvoci-sqlite-060',?1,unixepoch()*1000000)",
                    vec![Cell::text(steps[11].sha256)],
                )],
            ),
            (
                // Unmarked object next to a complete ledger.
                vec![(
                    "CREATE TABLE remote_unmarked_object(value INTEGER) STRICT",
                    vec![],
                )],
                "GATE_CATALOG_DIFFERS",
                vec![("DROP TABLE remote_unmarked_object", vec![])],
            ),
            (
                // Retired development lineage inside a foreign ledger definition:
                // refused by its own message before any catalog comparison, never rewritten.
                vec![
                    (HOLD, vec![]),
                    (DROP, vec![]),
                    (FOREIGN_DDL, vec![]),
                    (
                        "INSERT INTO schema_migrations SELECT version,CASE WHEN version=12 THEN 'fvoci-sqlite-current-v1' ELSE lineage END,sql_sha256,applied_at FROM schema_migrations_hold",
                        vec![],
                    ),
                    (DROP_HOLD, vec![]),
                ],
                "GATE_RETIRED_LINEAGE",
                vec![
                    (HOLD, vec![]),
                    (DROP, vec![]),
                    (ledger_ddl, vec![]),
                    (RESTORE_ROWS, vec![]),
                    (DROP_HOLD, vec![]),
                ],
            ),
        ];
        for (deviate, code, repair) in vectors {
            let label = deviate[0].0;
            let backend = f.backend().await;
            for (statement, args) in &deviate {
                write(&backend, statement, args).await;
            }
            backend.close().await.unwrap();
            let before = snapshot(&f).await;
            assert_ne!(before, baseline, "{label}");
            let error = gate_of(&f).await;
            assert_gate(&error, code);
            assert_eq!(error.gate_stage(), Some(GateStage::PreWriteClassification));
            assert_eq!(error.settlement(), "no-write-opened");
            // The refusal left the ledger, the catalog and the business row exactly as found.
            let after = snapshot(&f).await;
            assert_eq!(after, before, "refusal must not write: {label}");
            let backend = f.backend().await;
            for (statement, args) in &repair {
                write(&backend, statement, args).await;
            }
            backend.close().await.unwrap();
            assert_current(&f).await;
            let repaired = snapshot(&f).await;
            assert_eq!(repaired.1, baseline.1, "catalog after repair: {label}");
            assert_eq!(repaired.2, baseline.2, "business row after repair: {label}");
        }
        assert_eq!(receipts(&f).await, (1..=12).collect::<Vec<i64>>());
        f.finish().await;
    }

    #[tokio::test]
    async fn cancellation_before_the_first_step_leaves_a_blank_database() {
        let f = Fixture::start().await;
        let cancel = CancellationToken::new();
        cancel.cancel();
        let error = remote_migrate_backend(f.backend().await, &cancel)
            .await
            .unwrap_err();
        assert!(matches!(
            error,
            RemoteMigrationError::Cancelled {
                last_settled: 0,
                drain: None
            }
        ));
        let backend = f.backend().await;
        assert_eq!(
            classify_sqlite_family(&backend).await.unwrap(),
            SqliteFamilyState::Blank
        );
        backend.close().await.unwrap();
        f.finish().await;
    }

    #[tokio::test]
    async fn lost_commit_reply_reports_unknown_and_the_rerun_resumes_from_the_real_receipt() {
        let f = Fixture::start().await;
        let gate = f.driver.arm_commit_response_loss();
        let cancel = CancellationToken::new();
        let backend = f.backend().await;
        // The real helper future is pinned on this test task and driven
        // concurrently with the fixture gate; no thread spawn, so no Send +
        // 'static bound is imposed on the product future (the intended
        // callers await it inline as well).
        let run = remote_migrate_backend(backend, &cancel);
        tokio::pin!(run);
        tokio::select! {
            response = gate.wait_upstream_response() => response.unwrap(),
            early = &mut run => panic!("helper finished before the upstream COMMIT fault gate: {early:?}"),
        }
        // Explicit pre-release check: one actual poll of the pinned real
        // future must still be Pending while the COMMIT reply is withheld.
        {
            use std::future::Future;
            use std::task::{Context, Poll, Waker};
            let mut context = Context::from_waker(Waker::noop());
            assert!(
                matches!(run.as_mut().poll(&mut context), Poll::Pending),
                "helper settled before the withheld COMMIT reply was released"
            );
        }
        gate.release_lost_reply();
        let error = run.await.unwrap_err();
        let RemoteMigrationError::CommitUnknown { version, .. } = &error else {
            panic!("expected CommitUnknown, got {error:?}");
        };
        assert_eq!(*version, 1, "the first step's COMMIT reply was withheld");
        assert_eq!(error.code(), "REMOTE_MIGRATION_COMMIT_UNKNOWN");
        // The maintained fixture records that the upstream accepted the COMMIT
        // before the reply was cut: a fresh connection must observe exactly
        // receipt 1, and only then may the rerun resume from 1.
        let lost = f
            .driver
            .exchanges()
            .into_iter()
            .find(|exchange| exchange.reply_lost)
            .expect("one withheld reply recorded");
        assert!(lost.has_commit, "{lost:?}");
        assert_eq!(lost.upstream_status, Some(200), "{lost:?}");
        assert_eq!(receipts(&f).await, vec![1]);
        let backend = f.backend().await;
        assert_eq!(
            classify_sqlite_family(&backend).await.unwrap(),
            SqliteFamilyState::Prefix(1)
        );
        backend.close().await.unwrap();
        let rerun = remote_migrate_backend(f.backend().await, &CancellationToken::new())
            .await
            .unwrap();
        assert_eq!(rerun, RemoteMigrationOutcome::Resumed { from: 1, to: 12 });
        assert_eq!(receipts(&f).await, (1..=12).collect::<Vec<i64>>());
        f.finish().await;
    }
}
