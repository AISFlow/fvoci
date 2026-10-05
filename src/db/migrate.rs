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
    let bootstrapped: bool = sqlx::query_scalar(
        "SELECT EXISTS (
            SELECT 1 FROM information_schema.tables
            WHERE table_schema = 'fvoci' AND table_name = 'schema_migrations'
        )",
    )
    .fetch_one(&mut *conn)
    .await?;
    if !bootstrapped {
        return Ok(LedgerState::Unprepared);
    }
    let has_lineage: bool = sqlx::query_scalar(
        "SELECT EXISTS (
            SELECT 1 FROM information_schema.columns
            WHERE table_schema = 'fvoci' AND table_name = 'schema_migrations'
              AND column_name = 'lineage'
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
        assert_eq!(
            assert_sqlite_schema_current(&backend)
                .await
                .unwrap()
                .applied_steps,
            12
        );
        let name: String = sqlx::query_scalar("SELECT name FROM workspaces WHERE id=?1")
            .bind(workspace.as_bytes().as_slice())
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(name, "populated011 literal");
        let markers: Vec<i64> =
            sqlx::query_scalar("SELECT version FROM schema_migrations ORDER BY version")
                .fetch_all(&pool)
                .await
                .unwrap();
        assert_eq!(markers, (1..=12).collect::<Vec<i64>>());
        let count:i64=sqlx::query_scalar("SELECT count(*) FROM maintenance_job_claims WHERE owner_token IS NULL AND generation=0 AND expires_at IS NULL").fetch_one(&pool).await.unwrap();
        assert_eq!(count, 9);
        backend.close().await.unwrap();
        std::fs::remove_dir_all(root).unwrap();
    }
}
