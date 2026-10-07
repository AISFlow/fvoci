//! Explicit manual primary consumers, never part of automatic DB suites.
//! Connection is read-only; migration requires both explicit destructive gates.
//! No synthetic transport, retry, or normal startup unlock is used.
//! Disposable reset is separately selected; ordinary consumers never reset.
use super::backend::{Backend, DbTx, FamilyTx, RemoteDatabase};
use super::codec::Cell;
use crate::config::DatabaseSettings;

const TEST_NAME: &str = "db::turso_test::turso_primary_connection";

fn configuration(url: String, token: String) -> Result<(String, String), &'static str> {
    if url.is_empty() || token.is_empty() {
        return Err("MISSING_SECRET");
    }
    if url.len() > 2048
        || token.len() > 16384
        || url
            .bytes()
            .chain(token.bytes())
            .any(|c| c <= 32 || c == 127)
    {
        return Err("INVALID_SECRET_FORMAT");
    }
    let parsed = url::Url::parse(&url).map_err(|_| "INVALID_PRIMARY_URL")?;
    let host = parsed.host_str().ok_or("INVALID_PRIMARY_URL")?;
    if !matches!(parsed.scheme(), "libsql" | "https")
        || !host.ends_with(".turso.io")
        || host.len() > 253
        || !host.split('.').all(|label| {
            !label.is_empty()
                && !label.starts_with('-')
                && !label.ends_with('-')
                && label
                    .bytes()
                    .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == b'-')
        })
        || (url != format!("{}://{host}", parsed.scheme())
            && url != format!("{}://{host}/", parsed.scheme()))
        || !parsed.username().is_empty()
        || parsed.password().is_some()
        || parsed.port().is_some()
        || !matches!(parsed.path(), "" | "/")
        || parsed.query().is_some()
        || parsed.fragment().is_some()
    {
        return Err("INVALID_PRIMARY_URL");
    }
    Ok((url, token))
}

async fn same_stream_readback(tx: &mut DbTx) -> Result<(), &'static str> {
    let DbTx::SqliteFamily(family @ FamilyTx::Remote(_)) = tx else {
        return Err("WRONG_BACKEND");
    };
    let fk = family
        .query("PRAGMA foreign_keys", &[])
        .await
        .map_err(|_| "FK_QUERY_FAILED")?;
    if fk.len() != 1 || fk[0].cell(0).map_err(|_| "FK_DECODE_FAILED")? != Cell::Integer(1) {
        return Err("FOREIGN_KEYS_NOT_ONE");
    }
    let literal = family
        .query("SELECT 353, 'fvoci-turso-connection'", &[])
        .await
        .map_err(|_| "LITERAL_QUERY_FAILED")?;
    if literal.len() != 1
        || literal[0].cell(0).map_err(|_| "LITERAL_DECODE_FAILED")? != Cell::Integer(353)
        || literal[0].cell(1).map_err(|_| "LITERAL_DECODE_FAILED")?
            != Cell::Text("fvoci-turso-connection".into())
    {
        return Err("LITERAL_MISMATCH");
    }
    Ok(())
}

#[tokio::test]
#[ignore = "manual trusted-main Turso job selects this exact test with --ignored"]
async fn turso_primary_connection() -> Result<(), &'static str> {
    // Explicit selection is required; selecting this ignored test without the
    // consuming wrapper fails, including when credentials are missing.
    let args: Vec<String> = std::env::args().collect();
    if std::env::var("FVOCI_TEST_TURSO_CONNECTION_SELECTED").as_deref() != Ok("1")
        || !args.iter().any(|arg| arg == "--exact")
        || !args.iter().any(|arg| arg == "--ignored")
        || !args.iter().any(|arg| arg == TEST_NAME)
    {
        return Err("EXPLICIT_SELECTION_REQUIRED");
    }
    let settings = DatabaseSettings::from_env().map_err(|_| "PRODUCT_CONFIGURATION_FAILED")?;
    let DatabaseSettings::LibsqlRemote {
        primary_url,
        auth_token,
    } = settings
    else {
        return Err("WRONG_PRODUCT_BACKEND");
    };
    let (url, token) = configuration(primary_url, auth_token)?;
    let remote = match RemoteDatabase::connect(url, token, 1).await {
        Ok(remote) => remote,
        Err(_) => {
            println!(
                "FVOCI_TURSO_RECEIPT primary=CONNECT_FAILED rollback=NOT_STARTED close=NOT_STARTED leases=NOT_OBSERVED"
            );
            return Err("CONNECT_FAILED");
        }
    };
    let backend = Backend::LibsqlRemote(remote);
    let mut tx = match backend.begin_read().await {
        Ok(tx) => tx,
        Err(_) => {
            let close = backend.close().await;
            println!(
                "FVOCI_TURSO_RECEIPT primary=BEGIN_FAILED rollback=NOT_STARTED close={} leases=NOT_OBSERVED",
                if close.is_ok() { "OK" } else { "FAILED" }
            );
            return Err("BEGIN_FAILED");
        }
    };
    let primary = same_stream_readback(&mut tx).await;
    // Always await the original transaction rollback, including read failure.
    // Backend close awaits the product cleanup owner. SDK Drop Close receipt is
    // not exposed; successful owner drain is not a server Close ACK claim.
    let rollback = tx.rollback().await;
    let close = backend.close().await;
    let leases_closed = backend
        .connection_stats()
        .map(|stats| stats.size == 0)
        .unwrap_or(false);
    println!(
        "FVOCI_TURSO_RECEIPT primary={} rollback={} close={} leases={}",
        primary.err().unwrap_or("OK"),
        if rollback.is_ok() { "OK" } else { "FAILED" },
        if close.is_ok() { "OK" } else { "FAILED" },
        if leases_closed { "ZERO" } else { "FAILED" },
    );
    primary?;
    if rollback.is_err() || close.is_err() || !leases_closed {
        return Err("CLEANUP_NOT_CONFIRMED");
    }
    Ok(())
}

#[test]
fn connection_configuration_refuses_missing_or_foreign_target() {
    let host = "isolated-owner.aws-us-east-1.turso.io";
    assert_eq!(
        configuration(String::new(), "FAKE".into()).err(),
        Some("MISSING_SECRET")
    );
    for url in [
        "http://isolated-owner.aws-us-east-1.turso.io",
        "libsql://other.example.org",
        "libsql://localhost",
        "https://fake:fake@isolated-owner.aws-us-east-1.turso.io",
        "https://isolated-owner.aws-us-east-1.turso.io?sync=true",
    ] {
        assert_eq!(
            configuration(url.into(), "FAKE".into()).err(),
            Some("INVALID_PRIMARY_URL")
        );
    }
    assert!(configuration(format!("libsql://{host}"), "FAKE".into()).is_ok());
}

const MIGRATION_TEST_NAME: &str = "db::turso_test::turso_primary_current12_install_resume";

fn migration_selection() -> Result<(), &'static str> {
    let args: Vec<String> = std::env::args().collect();
    for (name, expected) in [
        ("FVOCI_TEST_TURSO_MIGRATION_SELECTED", "1"),
        ("FVOCI_TEST_TURSO_PHASE", "migration"),
        ("FVOCI_TEST_TURSO_DESTRUCTIVE", "true"),
        ("FVOCI_TEST_TURSO_ALLOW_DESTRUCTIVE", "true"),
    ] {
        if std::env::var(name).as_deref() != Ok(expected) {
            return Err("EXPLICIT_MIGRATION_SELECTION_REQUIRED");
        }
    }
    if !args.iter().any(|arg| arg == "--exact")
        || !args.iter().any(|arg| arg == "--ignored")
        || !args.iter().any(|arg| arg == MIGRATION_TEST_NAME)
    {
        return Err("EXPLICIT_MIGRATION_SELECTION_REQUIRED");
    }
    Ok(())
}

fn remote_family(tx: &mut DbTx) -> Result<&mut FamilyTx, &'static str> {
    match tx {
        DbTx::SqliteFamily(family @ FamilyTx::Remote(_)) => Ok(family),
        _ => Err("WRONG_BACKEND"),
    }
}

async fn migration_snapshot(
    backend: &Backend,
    steps: usize,
) -> Result<super::migrate::TursoTestSchemaSnapshot, &'static str> {
    let mut tx = backend.begin_write().await.map_err(|_| "BEGIN_FAILED")?;
    let result = async {
        same_stream_readback(&mut tx).await?;
        super::migrate::turso_test_schema_in_writer(remote_family(&mut tx)?, steps)
            .await
            .map_err(|_| "SCHEMA_VALIDATION_FAILED")
    }
    .await;
    let rollback = tx.rollback().await;
    if rollback.is_err() {
        return Err("ROLLBACK_UNCONFIRMED");
    }
    result
}

async fn migration_populate(backend: &Backend, workspace: uuid::Uuid) -> Result<(), &'static str> {
    let mut tx = backend.begin_write().await.map_err(|_| "BEGIN_FAILED")?;
    let result = async {
        same_stream_readback(&mut tx).await?;
        let family = remote_family(&mut tx)?;
        super::migrate::turso_test_schema_in_writer(family, 11)
            .await
            .map_err(|_| "PREFIX_VALIDATION_FAILED")?;
        let rows = family
            .query("SELECT count(*) FROM workspaces", &[])
            .await
            .map_err(|_| "DATA_QUERY_FAILED")?;
        if rows.len() != 1 || rows[0].cell(0).map_err(|_| "DATA_DECODE_FAILED")? != Cell::Integer(0) {
            return Err("UNEXPECTED_TARGET_DATA");
        }
        if family
            .execute(
                "INSERT INTO workspaces(id,slug,name) VALUES(?1,'turso-migration','Turso populated011 literal')",
                &[Cell::uuid(workspace)],
            )
            .await
            .map_err(|_| "DATA_WRITE_FAILED")?
            != 1
            || family
                .execute("UPDATE collab_fence_counter SET next_fence=17 WHERE id=1 AND next_fence=1", &[])
                .await
                .map_err(|_| "FENCE_WRITE_FAILED")?
                != 1
        {
            return Err("DATA_WRITE_MISMATCH");
        }
        Ok(())
    }
    .await;
    if let Err(error) = result {
        return match tx.rollback().await {
            Ok(()) => Err(error),
            Err(_) => Err("ROLLBACK_UNCONFIRMED"),
        };
    }
    // Remote COMMIT uncertainty is terminal. No fresh observer reconciles it.
    tx.commit_with_cleanup()
        .await
        .map_err(|_| "COMMIT_UNCONFIRMED")
}

fn genuine_remote_fk_failure(error: &sqlx::Error) -> Result<bool, &'static str> {
    let sqlx::Error::AnyDriverError(original) = error else {
        return Ok(false);
    };
    match original.downcast_ref::<libsql::Error>() {
        Some(libsql::Error::SqliteFailure(787, _))
        | Some(libsql::Error::RemoteSqliteFailure(_, 787, _)) => Ok(true),
        Some(error @ libsql::Error::Hrana(_)) => {
            // No arbitrary-box/source-chain numeric error can substitute for a
            // maintained Hrana statement rejection. Never inspect message text.
            Ok(error.hrana_error_code() == Some("SQLITE_CONSTRAINT_FOREIGNKEY"))
        }
        _ => Ok(false),
    }
}

/// `true` only for the maintained Hrana statement rejection whose exposed machine
/// code is exactly the primary `SQLITE_CONSTRAINT`. The pinned sqld serializes a
/// constraint failure as that primary code and discards the extended code, so the
/// code alone never proves a foreign-key failure: it only admits the same-writer
/// causal witness below. Every other kind, code or message stays refused.
fn primary_only_hrana_constraint(error: &sqlx::Error) -> bool {
    let sqlx::Error::AnyDriverError(original) = error else {
        return false;
    };
    matches!(
        original.downcast_ref::<libsql::Error>(),
        Some(error @ libsql::Error::Hrana(_))
            if error.hrana_error_code() == Some("SQLITE_CONSTRAINT")
    )
}

const FENCE_TABLE: &str = "task_collab_room_fences";

/// Baseline read on the SAME writer before the original immediate failure: the fence
/// table holds no row, the referenced parent (workspace_id, id) of exactly these cells
/// is absent and `defer_foreign_keys` reads 0, so the later deferred witness can be
/// explained only by the composite foreign key of exactly these cells.
async fn migration_fk_baseline(family: &mut FamilyTx, cells: &[Cell]) -> Result<(), &'static str> {
    if witness_integer(family, "PRAGMA defer_foreign_keys", &[]).await? != 0 {
        return Err("DEFER_PRAGMA_REFUSED");
    }
    if witness_integer(family, "SELECT count(*) FROM task_collab_room_fences", &[]).await? != 0 {
        return Err("FENCE_BASELINE_NOT_EMPTY");
    }
    if witness_integer(
        family,
        "SELECT count(*) FROM tasks WHERE workspace_id=?1 AND id=?2",
        &cells[..2],
    )
    .await?
        != 0
    {
        return Err("PARENT_PRESENT");
    }
    Ok(())
}

/// Same-writer causal witness for a primary-only `SQLITE_CONSTRAINT` rejection of
/// `MIGRATION_UNKNOWN_TASK_FENCE_SQL`, run on the SAME writer that already holds the
/// original immediate failure and the baseline above: `defer_foreign_keys` must read
/// 0, be enabled and read back 1 while `foreign_keys` stays 1; the IDENTICAL statement
/// with the IDENTICAL cells must then succeed, so every non-deferrable constraint of
/// the fence table (NOT NULL, CHECK, STRICT typing, PRIMARY KEY) is satisfied by
/// exactly these values; the table must hold exactly that one row, whose rowid is read
/// back by the exact tuple; `foreign_key_check` must identify exactly that rowid
/// against the composite `tasks` key, cross-checked by `foreign_key_list`. The deferred
/// violation is left for the caller's unconditional rollback and is never committed.
/// Refused pragmas, an altered outcome, unexpected rows or a missing mapping fail.
async fn migration_fk_witness(family: &mut FamilyTx, cells: &[Cell]) -> Result<(), &'static str> {
    if witness_integer(family, "PRAGMA defer_foreign_keys", &[]).await? != 0 {
        return Err("DEFER_PRAGMA_REFUSED");
    }
    family
        .execute("PRAGMA defer_foreign_keys=ON", &[])
        .await
        .map_err(|_| "DEFER_PRAGMA_REFUSED")?;
    if witness_integer(family, "PRAGMA defer_foreign_keys", &[]).await? != 1 {
        return Err("DEFER_PRAGMA_REFUSED");
    }
    if witness_integer(family, "PRAGMA foreign_keys", &[]).await? != 1 {
        return Err("FOREIGN_KEYS_NOT_ONE");
    }
    if family
        .execute(MIGRATION_UNKNOWN_TASK_FENCE_SQL, cells)
        .await
        .map_err(|_| "NOT_FK_ONLY")?
        != 1
    {
        return Err("NOT_FK_ONLY");
    }
    if witness_integer(family, "SELECT count(*) FROM task_collab_room_fences", &[]).await? != 1 {
        return Err("FENCE_ROW_UNBOUND");
    }
    let rowid = witness_integer(
        family,
        "SELECT rowid FROM task_collab_room_fences WHERE workspace_id=?1 AND task_id=?2 AND owner_token=?3 AND fence=17 AND expires_at=1",
        cells,
    )
    .await
    .map_err(|_| "FENCE_ROW_UNBOUND")?;
    let check = witness_rows(
        family,
        "PRAGMA foreign_key_check(task_collab_room_fences)",
        4,
    )
    .await?;
    let list = witness_rows(
        family,
        "PRAGMA foreign_key_list(task_collab_room_fences)",
        5,
    )
    .await?;
    fk_witness_mapping(rowid, &check, &list)
}

/// One real unrelated-constraint control on the same writer (the observed ambiguity):
/// the identical canonical statement with the given cells must fail immediately with
/// the primary-only Hrana code, and the same-writer witness must refuse it as
/// NOT_FK_ONLY. Any other outcome is reported by its closed category.
async fn migration_fk_negative(family: &mut FamilyTx, cells: &[Cell]) -> &'static str {
    let Err(error) = family
        .execute(MIGRATION_UNKNOWN_TASK_FENCE_SQL, cells)
        .await
    else {
        return "ACCEPTED";
    };
    if genuine_remote_fk_failure(&error) != Ok(false) || !primary_only_hrana_constraint(&error) {
        return "INCONCLUSIVE";
    }
    match migration_fk_witness(family, cells).await {
        Err("NOT_FK_ONLY") => "REFUSED",
        Err(code) => code,
        Ok(()) => "PROVEN_WRONGLY",
    }
}

async fn witness_integer(
    family: &mut FamilyTx,
    statement: &'static str,
    args: &[Cell],
) -> Result<i64, &'static str> {
    let rows = family
        .query(statement, args)
        .await
        .map_err(|_| "WITNESS_QUERY_FAILED")?;
    match (rows.len(), rows.first().map(|row| row.cell(0))) {
        (1, Some(Ok(Cell::Integer(value)))) => Ok(value),
        _ => Err("WITNESS_DECODE_FAILED"),
    }
}

async fn witness_rows(
    family: &mut FamilyTx,
    statement: &'static str,
    columns: usize,
) -> Result<Vec<Vec<Cell>>, &'static str> {
    let rows = family
        .query(statement, &[])
        .await
        .map_err(|_| "WITNESS_QUERY_FAILED")?;
    rows.iter()
        .map(|row| {
            (0..columns)
                .map(|index| row.cell(index).map_err(|_| "WITNESS_DECODE_FAILED"))
                .collect()
        })
        .collect()
}

/// Pure witness mapping: exactly one violating row of the fence table, the rowid read
/// back by the exact tuple, against the parent `tasks` through foreign key 0, and that
/// key is exactly the composite (workspace_id,task_id) -> tasks(workspace_id,id)
/// mapping with no other foreign key; anything else fails.
fn fk_witness_mapping(
    rowid: i64,
    check: &[Vec<Cell>],
    list: &[Vec<Cell>],
) -> Result<(), &'static str> {
    let [row] = check else {
        return Err("WITNESS_MISMATCH");
    };
    let [table, violating, parent, fkid] = row.as_slice() else {
        return Err("WITNESS_MISMATCH");
    };
    if *table != Cell::text(FENCE_TABLE)
        || *violating != Cell::Integer(rowid)
        || *parent != Cell::text("tasks")
        || *fkid != Cell::Integer(0)
    {
        return Err("WITNESS_MISMATCH");
    }
    let expected = [("workspace_id", "workspace_id"), ("task_id", "id")];
    if list.len() != expected.len() {
        return Err("WITNESS_MISMATCH");
    }
    for (sequence, (row, (child, parent_column))) in list.iter().zip(expected).enumerate() {
        let [id, seq, table, from, to, ..] = row.as_slice() else {
            return Err("WITNESS_MISMATCH");
        };
        if *id != Cell::Integer(0)
            || *seq != Cell::Integer(sequence as i64)
            || *table != Cell::text("tasks")
            || *from != Cell::text(child)
            || *to != Cell::text(parent_column)
        {
            return Err("WITNESS_MISMATCH");
        }
    }
    Ok(())
}

#[test]
fn migration_fk_classification_never_uses_arbitrary_error_or_private_message() {
    let sqlite = sqlx::Error::AnyDriverError(Box::new(libsql::Error::SqliteFailure(
        787,
        "literal fixture".into(),
    )));
    assert_eq!(genuine_remote_fk_failure(&sqlite), Ok(true));
    let remote = sqlx::Error::AnyDriverError(Box::new(libsql::Error::RemoteSqliteFailure(
        19,
        787,
        "literal fixture".into(),
    )));
    assert_eq!(genuine_remote_fk_failure(&remote), Ok(true));
    let constraint = sqlx::Error::AnyDriverError(Box::new(libsql::Error::SqliteFailure(
        19,
        "FOREIGN KEY constraint failed SQLITE_CONSTRAINT_FOREIGNKEY".into(),
    )));
    assert_eq!(genuine_remote_fk_failure(&constraint), Ok(false));
    let hidden = sqlx::Error::AnyDriverError(Box::new(libsql::Error::Hrana(Box::new(
        std::io::Error::other("SQLITE_CONSTRAINT_FOREIGNKEY literal private-message fixture"),
    ))));
    assert_eq!(genuine_remote_fk_failure(&hidden), Ok(false));
    // Numeric success is accepted only at the original public SDK boundary.
    // A boxed error inside Hrana is not a maintained statement rejection.
    let nested = sqlx::Error::AnyDriverError(Box::new(libsql::Error::Hrana(Box::new(
        libsql::Error::SqliteFailure(787, "literal nested numeric fixture".into()),
    ))));
    assert_eq!(genuine_remote_fk_failure(&nested), Ok(false));
    let arbitrary = sqlx::Error::AnyDriverError(Box::new(std::io::Error::other(
        "SQLITE_CONSTRAINT_FOREIGNKEY arbitrary box fixture",
    )));
    assert_eq!(genuine_remote_fk_failure(&arbitrary), Ok(false));
    assert_eq!(
        genuine_remote_fk_failure(&sqlx::Error::PoolTimedOut),
        Ok(false)
    );
}

// One canonical statement for the original consumer and allocated local control.
const MIGRATION_UNKNOWN_TASK_FENCE_SQL: &str = "INSERT INTO task_collab_room_fences(workspace_id,task_id,owner_token,fence,expires_at) VALUES(?1,?2,?3,17,1)";

/// Closed proof kinds of the shared FK rollback: `EXTENDED` for the typed proof of
/// the unchanged strict classifier, `SAME_WRITER_PRIMARY_HRANA` only for the exact
/// original SDK typed primary-only Hrana `SQLITE_CONSTRAINT` plus the full same-writer
/// row witness. Nothing else is ever reported as a proof.
const FK_PROOF_EXTENDED: &str = "EXTENDED";
const FK_PROOF_SAME_WRITER_PRIMARY_HRANA: &str = "SAME_WRITER_PRIMARY_HRANA";
const FK_PROOF_NOT_CONFIRMED: &str = "NOT_CONFIRMED";

async fn migration_fk_rollback(
    backend: &Backend,
    workspace: uuid::Uuid,
) -> Result<&'static str, &'static str> {
    let steps = super::migrate::compiled_sqlite_steps();
    if steps.len() != 12 || steps[11].version != 12 {
        return Err("CURRENT_LINEAGE_CHANGED");
    }
    let mut tx = backend.begin_write().await.map_err(|_| "BEGIN_FAILED")?;
    let mut original_fk_error = None;
    let result = async {
        same_stream_readback(&mut tx).await?;
        let family = remote_family(&mut tx)?;
        super::migrate::turso_test_schema_in_writer(family, 11)
            .await
            .map_err(|_| "PREFIX_VALIDATION_FAILED")?;
        // Use the actual immutable registry SQL; no copied DDL or fake failure.
        family
            .apply_migration_batch(steps[11].sql)
            .await
            .map_err(|_| "DDL_FAILED")?;
        let cells = [
            Cell::uuid(workspace),
            Cell::uuid(uuid::Uuid::now_v7()),
            Cell::uuid(uuid::Uuid::now_v7()),
        ];
        migration_fk_baseline(family, &cells).await?;
        original_fk_error = family
            .execute(MIGRATION_UNKNOWN_TASK_FENCE_SQL, &cells)
            .await
            .err();
        let error = original_fk_error.as_ref().ok_or("FK_FAILURE_MISSING")?;
        if genuine_remote_fk_failure(error)? {
            return Ok(FK_PROOF_EXTENDED);
        }
        // Only the primary-only Hrana code is admitted, and only through the
        // same-writer causal witness; any other kind or code stays refused.
        if !primary_only_hrana_constraint(error) {
            return Err("WRONG_FK_FAILURE");
        }
        migration_fk_witness(family, &cells).await?;
        Ok(FK_PROOF_SAME_WRITER_PRIMARY_HRANA)
    }
    .await;
    // Always finish the SAME writer after the real FK statement, including an
    // unexpected statement/DDL failure. Drop/Close is never a rollback receipt.
    let rollback = tx.rollback().await;
    // Preserve exact SDK error through original-stream settlement; reporting
    // never formats or substitutes its private source.
    drop(original_fk_error);
    if rollback.is_err() {
        return Err("ROLLBACK_UNCONFIRMED");
    }
    result
}

async fn migration_preserved_data(
    backend: &Backend,
    workspace: uuid::Uuid,
    steps: usize,
    generation: i64,
) -> Result<(), &'static str> {
    let mut tx = backend.begin_write().await.map_err(|_| "BEGIN_FAILED")?;
    let result = async {
        same_stream_readback(&mut tx).await?;
        let family = remote_family(&mut tx)?;
        super::migrate::turso_test_schema_in_writer(family, steps)
            .await
            .map_err(|_| "SCHEMA_VALIDATION_FAILED")?;
        let rows = family.query(
            "SELECT name,(SELECT next_fence FROM collab_fence_counter WHERE id=1),(SELECT count(*) FROM task_collab_room_fences),(SELECT count(*) FROM workspaces) FROM workspaces WHERE id=?1",
            &[Cell::uuid(workspace)],
        ).await.map_err(|_| "DATA_QUERY_FAILED")?;
        if rows.len() != 1
            || rows[0].cell(0).map_err(|_| "DATA_DECODE_FAILED")? != Cell::text("Turso populated011 literal")
            || rows[0].cell(1).map_err(|_| "DATA_DECODE_FAILED")? != Cell::Integer(17)
            || rows[0].cell(2).map_err(|_| "DATA_DECODE_FAILED")? != Cell::Integer(0)
            || rows[0].cell(3).map_err(|_| "DATA_DECODE_FAILED")? != Cell::Integer(1)
        { return Err("PRESERVED_DATA_MISMATCH"); }
        if steps == 12 {
            let rows = family.query("SELECT job_key,generation,owner_token,expires_at FROM maintenance_job_claims ORDER BY job_key", &[])
                .await.map_err(|_| "SEED_QUERY_FAILED")?;
            if rows.len() != 9 { return Err("SEED_MISMATCH"); }
            for (index, row) in rows.iter().enumerate() {
                let key = index as i64 + 1;
                if row.cell(0).map_err(|_| "SEED_DECODE_FAILED")? != Cell::Integer(key)
                    || row.cell(1).map_err(|_| "SEED_DECODE_FAILED")? != Cell::Integer(if key == 8 { generation } else { 0 })
                    || row.cell(2).map_err(|_| "SEED_DECODE_FAILED")? != Cell::Null
                    || row.cell(3).map_err(|_| "SEED_DECODE_FAILED")? != Cell::Null
                { return Err("SEED_MISMATCH"); }
            }
        }
        Ok(())
    }.await;
    let rollback = tx.rollback().await;
    if rollback.is_err() {
        return Err("ROLLBACK_UNCONFIRMED");
    }
    result
}

async fn migration_advance_generation(backend: &Backend) -> Result<(), &'static str> {
    let mut tx = backend.begin_write().await.map_err(|_| "BEGIN_FAILED")?;
    let result = async {
        let family = remote_family(&mut tx)?;
        let changed = family.execute("UPDATE maintenance_job_claims SET generation=7 WHERE job_key=8 AND generation=0 AND owner_token IS NULL AND expires_at IS NULL", &[])
            .await.map_err(|_| "GENERATION_WRITE_FAILED")?;
        if changed != 1 { return Err("GENERATION_WRITE_MISMATCH"); }
        Ok(())
    }.await;
    if let Err(error) = result {
        return match tx.rollback().await {
            Ok(()) => Err(error),
            Err(_) => Err("ROLLBACK_UNCONFIRMED"),
        };
    }
    tx.commit_with_cleanup()
        .await
        .map_err(|_| "COMMIT_UNCONFIRMED")
}

async fn migration_catalog_negatives(backend: &Backend) -> Result<(), &'static str> {
    for (statement, refusal) in [
        (
            "DELETE FROM schema_migrations WHERE version=6",
            "SQLite schema is ahead, incomplete or unprepared",
        ),
        (
            "UPDATE schema_migrations SET sql_sha256='0000000000000000000000000000000000000000000000000000000000000000' WHERE version=1",
            "SQLite schema has a gap, foreign lineage or changed digest",
        ),
        (
            "CREATE TABLE turso_migration_unmarked_object(value INTEGER) STRICT",
            "SQLite schema definitions differ from compiled capability; unmarked/populated or altered schema refused",
        ),
    ] {
        let mut tx = backend.begin_write().await.map_err(|_| "BEGIN_FAILED")?;
        let result = async {
            same_stream_readback(&mut tx).await?;
            let family = remote_family(&mut tx)?;
            super::migrate::turso_test_schema_in_writer(family, 12)
                .await
                .map_err(|_| "CURRENT_GATE_FAILED")?;
            family
                .execute(statement, &[])
                .await
                .map_err(|_| "NEGATIVE_WRITE_FAILED")?;
            match super::migrate::turso_test_schema_in_writer(family, 12).await {
                Err(sqlx::Error::Protocol(message)) if message == refusal => Ok(()),
                _ => Err("NEGATIVE_REFUSAL_NOT_CONFIRMED"),
            }
        }
        .await;
        let rollback = tx.rollback().await;
        if rollback.is_err() {
            return Err("ROLLBACK_UNCONFIRMED");
        }
        result?;
        // Healthy actual new stream after ORIGINAL rollback; no mutated ledger
        // or catalog is ever committed, and a failed rollback cannot advance.
        migration_snapshot(backend, 12).await?;
    }
    Ok(())
}

// This boundary accepts only consumer-owned static codes, never SDK errors.
fn migration_diagnostic_code(result: Result<(), &'static str>) -> Option<&'static str> {
    match result {
        Ok(()) => Some("OK"),
        Err("BEGIN_FAILED") => Some("BEGIN_FAILED"),
        Err("CLOSE_FAILED") => Some("CLOSE_FAILED"),
        Err("COMMIT_UNCONFIRMED") => Some("COMMIT_UNCONFIRMED"),
        Err("CURRENT_APPLY_FAILED") => Some("CURRENT_APPLY_FAILED"),
        Err("CURRENT_GATE_FAILED") => Some("CURRENT_GATE_FAILED"),
        Err("CURRENT_GATE_MISMATCH") => Some("CURRENT_GATE_MISMATCH"),
        Err("CURRENT_LINEAGE_CHANGED") => Some("CURRENT_LINEAGE_CHANGED"),
        Err("DATA_DECODE_FAILED") => Some("DATA_DECODE_FAILED"),
        Err("DATA_QUERY_FAILED") => Some("DATA_QUERY_FAILED"),
        Err("DATA_WRITE_FAILED") => Some("DATA_WRITE_FAILED"),
        Err("DATA_WRITE_MISMATCH") => Some("DATA_WRITE_MISMATCH"),
        Err("DDL_FAILED") => Some("DDL_FAILED"),
        Err("DEFER_PRAGMA_REFUSED") => Some("DEFER_PRAGMA_REFUSED"),
        Err("FENCE_BASELINE_NOT_EMPTY") => Some("FENCE_BASELINE_NOT_EMPTY"),
        Err("FENCE_ROW_UNBOUND") => Some("FENCE_ROW_UNBOUND"),
        Err("FENCE_WRITE_FAILED") => Some("FENCE_WRITE_FAILED"),
        Err("FK_DECODE_FAILED") => Some("FK_DECODE_FAILED"),
        Err("FK_FAILURE_MISSING") => Some("FK_FAILURE_MISSING"),
        Err("FK_QUERY_FAILED") => Some("FK_QUERY_FAILED"),
        Err("FK_ROLLBACK_PREFIX_CHANGED") => Some("FK_ROLLBACK_PREFIX_CHANGED"),
        Err("FOREIGN_KEYS_NOT_ONE") => Some("FOREIGN_KEYS_NOT_ONE"),
        Err("GENERATION_WRITE_FAILED") => Some("GENERATION_WRITE_FAILED"),
        Err("GENERATION_WRITE_MISMATCH") => Some("GENERATION_WRITE_MISMATCH"),
        Err("INCOMPLETE_PREFIX_REFUSAL_NOT_CONFIRMED") => {
            Some("INCOMPLETE_PREFIX_REFUSAL_NOT_CONFIRMED")
        }
        Err("LEASES_NOT_ZERO") => Some("LEASES_NOT_ZERO"),
        Err("LITERAL_DECODE_FAILED") => Some("LITERAL_DECODE_FAILED"),
        Err("LITERAL_MISMATCH") => Some("LITERAL_MISMATCH"),
        Err("LITERAL_QUERY_FAILED") => Some("LITERAL_QUERY_FAILED"),
        Err("NEGATIVE_REFUSAL_NOT_CONFIRMED") => Some("NEGATIVE_REFUSAL_NOT_CONFIRMED"),
        Err("NEGATIVE_ROLLBACK_CHANGED_CURRENT") => Some("NEGATIVE_ROLLBACK_CHANGED_CURRENT"),
        Err("NEGATIVE_WRITE_FAILED") => Some("NEGATIVE_WRITE_FAILED"),
        Err("NOT_FK_ONLY") => Some("NOT_FK_ONLY"),
        Err("PARENT_PRESENT") => Some("PARENT_PRESENT"),
        Err("PREFIX_APPLY_FAILED") => Some("PREFIX_APPLY_FAILED"),
        Err("PREFIX_RECEIPTS_CHANGED") => Some("PREFIX_RECEIPTS_CHANGED"),
        Err("PREFIX_VALIDATION_FAILED") => Some("PREFIX_VALIDATION_FAILED"),
        Err("PRESERVED_DATA_MISMATCH") => Some("PRESERVED_DATA_MISMATCH"),
        Err("RECONNECT_FAILED") => Some("RECONNECT_FAILED"),
        Err("RESTART_APPLY_FAILED") => Some("RESTART_APPLY_FAILED"),
        Err("RESTART_RECEIPTS_OR_SCHEMA_CHANGED") => Some("RESTART_RECEIPTS_OR_SCHEMA_CHANGED"),
        Err("ROLLBACK_UNCONFIRMED") => Some("ROLLBACK_UNCONFIRMED"),
        Err("SCHEMA_VALIDATION_FAILED") => Some("SCHEMA_VALIDATION_FAILED"),
        Err("SEED_DECODE_FAILED") => Some("SEED_DECODE_FAILED"),
        Err("SEED_MISMATCH") => Some("SEED_MISMATCH"),
        Err("SEED_QUERY_FAILED") => Some("SEED_QUERY_FAILED"),
        Err("UNEXPECTED_TARGET_DATA") => Some("UNEXPECTED_TARGET_DATA"),
        Err("WITNESS_DECODE_FAILED") => Some("WITNESS_DECODE_FAILED"),
        Err("WITNESS_MISMATCH") => Some("WITNESS_MISMATCH"),
        Err("WITNESS_QUERY_FAILED") => Some("WITNESS_QUERY_FAILED"),
        Err("WRONG_BACKEND") => Some("WRONG_BACKEND"),
        Err("WRONG_FK_FAILURE") => Some("WRONG_FK_FAILURE"),
        Err(_) => None,
    }
}

fn migration_failure_diagnostic(
    primary: Result<(), &'static str>,
    close: Result<(), &'static str>,
) -> Option<(&'static str, &'static str)> {
    if primary.is_ok() && close.is_ok() {
        return None;
    }
    let primary = migration_diagnostic_code(primary)?;
    // Final owner cleanup has only these two existing failure codes.
    let close = match close {
        Ok(()) => "OK",
        Err("CLOSE_FAILED") => "CLOSE_FAILED",
        Err("LEASES_NOT_ZERO") => "LEASES_NOT_ZERO",
        Err(_) => return None,
    };
    Some((primary, close))
}

#[test]
fn migration_diagnostics_disclose_only_known_static_failures() {
    for code in [
        "BEGIN_FAILED",
        "CLOSE_FAILED",
        "COMMIT_UNCONFIRMED",
        "CURRENT_APPLY_FAILED",
        "CURRENT_GATE_FAILED",
        "CURRENT_GATE_MISMATCH",
        "CURRENT_LINEAGE_CHANGED",
        "DATA_DECODE_FAILED",
        "DATA_QUERY_FAILED",
        "DATA_WRITE_FAILED",
        "DATA_WRITE_MISMATCH",
        "DDL_FAILED",
        "DEFER_PRAGMA_REFUSED",
        "FENCE_BASELINE_NOT_EMPTY",
        "FENCE_ROW_UNBOUND",
        "FENCE_WRITE_FAILED",
        "FK_DECODE_FAILED",
        "FK_FAILURE_MISSING",
        "FK_QUERY_FAILED",
        "FK_ROLLBACK_PREFIX_CHANGED",
        "FOREIGN_KEYS_NOT_ONE",
        "GENERATION_WRITE_FAILED",
        "GENERATION_WRITE_MISMATCH",
        "INCOMPLETE_PREFIX_REFUSAL_NOT_CONFIRMED",
        "LEASES_NOT_ZERO",
        "LITERAL_DECODE_FAILED",
        "LITERAL_MISMATCH",
        "LITERAL_QUERY_FAILED",
        "NEGATIVE_REFUSAL_NOT_CONFIRMED",
        "NEGATIVE_ROLLBACK_CHANGED_CURRENT",
        "NEGATIVE_WRITE_FAILED",
        "NOT_FK_ONLY",
        "PARENT_PRESENT",
        "PREFIX_APPLY_FAILED",
        "PREFIX_RECEIPTS_CHANGED",
        "PREFIX_VALIDATION_FAILED",
        "PRESERVED_DATA_MISMATCH",
        "RECONNECT_FAILED",
        "RESTART_APPLY_FAILED",
        "RESTART_RECEIPTS_OR_SCHEMA_CHANGED",
        "ROLLBACK_UNCONFIRMED",
        "SCHEMA_VALIDATION_FAILED",
        "SEED_DECODE_FAILED",
        "SEED_MISMATCH",
        "SEED_QUERY_FAILED",
        "UNEXPECTED_TARGET_DATA",
        "WITNESS_DECODE_FAILED",
        "WITNESS_MISMATCH",
        "WITNESS_QUERY_FAILED",
        "WRONG_BACKEND",
        "WRONG_FK_FAILURE",
    ] {
        assert_eq!(migration_diagnostic_code(Err(code)), Some(code));
        assert_eq!(
            migration_failure_diagnostic(Err(code), Ok(())),
            Some((code, "OK"))
        );
        assert_eq!(
            migration_failure_diagnostic(Err(code), Err("CLOSE_FAILED")),
            Some((code, "CLOSE_FAILED"))
        );
    }
    assert_eq!(migration_diagnostic_code(Ok(())), Some("OK"));
    assert_eq!(migration_failure_diagnostic(Ok(()), Ok(())), None);
    assert_eq!(
        migration_failure_diagnostic(Ok(()), Err("CLOSE_FAILED")),
        Some(("OK", "CLOSE_FAILED"))
    );
    assert_eq!(
        migration_failure_diagnostic(Ok(()), Err("LEASES_NOT_ZERO")),
        Some(("OK", "LEASES_NOT_ZERO"))
    );
    // These errors are outside the post-owner migration aggregate or private
    // transport/message values; no unknown string can be reflected in output.
    for private in [
        "",
        "UNKNOWN_ERROR",
        "CONNECT_FAILED",
        "PRODUCT_CONFIGURATION_FAILED",
        "MISSING_SECRET",
        "INVALID_PRIMARY_URL",
        "WRONG_FK_FAILURE_EXTRA",
        "wrong_fk_failure",
        "FK_QUERY_FAILED\nFAKE_PRIVATE_TOKEN",
        "libsql://FAKE_PRIVATE_TOKEN.example.org",
        "SDK: FAKE_PRIVATE_TOKEN",
    ] {
        assert_eq!(migration_diagnostic_code(Err(private)), None);
        assert_eq!(migration_failure_diagnostic(Err(private), Ok(())), None);
        assert_eq!(migration_failure_diagnostic(Ok(()), Err(private)), None);
    }
    // A valid primary code cannot be reused in the narrower close position.
    assert_eq!(
        migration_failure_diagnostic(Ok(()), Err("BEGIN_FAILED")),
        None
    );
    assert_eq!(
        migration_failure_diagnostic(Err("SCHEMA_VALIDATION_FAILED"), Err("BEGIN_FAILED")),
        None
    );
}

async fn close_migration_owner(backend: &Backend) -> Result<(), &'static str> {
    backend.close().await.map_err(|_| "CLOSE_FAILED")?;
    if !backend
        .connection_stats()
        .is_ok_and(|stats| stats.size == 0)
    {
        return Err("LEASES_NOT_ZERO");
    }
    Ok(())
}

#[tokio::test]
#[ignore = "manual trusted-ref isolated Turso migration job selects exactly this test"]
async fn turso_primary_current12_install_resume() -> Result<(), &'static str> {
    migration_selection()?;
    let settings = DatabaseSettings::from_env().map_err(|_| "PRODUCT_CONFIGURATION_FAILED")?;
    let DatabaseSettings::LibsqlRemote {
        primary_url,
        auth_token,
    } = settings
    else {
        return Err("WRONG_PRODUCT_BACKEND");
    };
    let (url, token) = configuration(primary_url, auth_token)?;
    let backend = Backend::LibsqlRemote(
        RemoteDatabase::connect(url.clone(), token.clone(), 1)
            .await
            .map_err(|_| "CONNECT_FAILED")?,
    );
    let workspace = uuid::Uuid::now_v7();
    let mut fk_proof = FK_PROOF_NOT_CONFIRMED;
    let primary = async {
        if super::migrate::compiled_sqlite_steps().len() != 12 {
            return Err("CURRENT_LINEAGE_CHANGED");
        }
        // An exact blank catalog is required BEFORE DDL. Unknown/current/mixed
        // targets refuse, never reset. ROOT owns any authorized disposable reset.
        migration_snapshot(&backend, 0).await?;
        super::migrate::turso_test_apply_prefix(&backend, 11)
            .await
            .map_err(|_| "PREFIX_APPLY_FAILED")?;
        let prefix = migration_snapshot(&backend, 11).await?;
        migration_populate(&backend, workspace).await?;
        match super::migrate::assert_sqlite_schema_current(&backend).await {
            Err(sqlx::Error::Protocol(message))
                if message == "SQLite schema is ahead, incomplete or unprepared" => {}
            _ => return Err("INCOMPLETE_PREFIX_REFUSAL_NOT_CONFIRMED"),
        }
        fk_proof = migration_fk_rollback(&backend, workspace).await?;
        if migration_snapshot(&backend, 11).await? != prefix {
            return Err("FK_ROLLBACK_PREFIX_CHANGED");
        }
        migration_preserved_data(&backend, workspace, 11, 0).await?;
        // Close the actual prefix owner before reconnecting; no old stream is borrowed.
        close_migration_owner(&backend).await?;
        let resumed = Backend::LibsqlRemote(
            RemoteDatabase::connect(url.clone(), token.clone(), 1)
                .await
                .map_err(|_| "RECONNECT_FAILED")?,
        );
        let result = async {
            super::migrate::turso_test_apply_prefix(&resumed, 12)
                .await
                .map_err(|_| "CURRENT_APPLY_FAILED")?;
            let current = migration_snapshot(&resumed, 12).await?;
            if current.receipts[..11] != prefix.receipts {
                return Err("PREFIX_RECEIPTS_CHANGED");
            }
            let gate = super::migrate::assert_sqlite_schema_current(&resumed)
                .await
                .map_err(|_| "CURRENT_GATE_FAILED")?;
            if gate.applied_steps != 12
                || gate.schema_sha256 != current.schema_sha256
                || gate.lineage != super::migrate::SQLITE_LINEAGE
                || gate.mode != super::migrate::SqliteSchemaMode::SdkRendered
            {
                return Err("CURRENT_GATE_MISMATCH");
            }
            migration_preserved_data(&resumed, workspace, 12, 0).await?;
            migration_catalog_negatives(&resumed).await?;
            if migration_snapshot(&resumed, 12).await? != current {
                return Err("NEGATIVE_ROLLBACK_CHANGED_CURRENT");
            }
            migration_advance_generation(&resumed).await?;
            close_migration_owner(&resumed).await?;
            let restarted = Backend::LibsqlRemote(
                RemoteDatabase::connect(url, token, 1)
                    .await
                    .map_err(|_| "RECONNECT_FAILED")?,
            );
            let restarted_result = async {
                super::migrate::turso_test_apply_prefix(&restarted, 12)
                    .await
                    .map_err(|_| "RESTART_APPLY_FAILED")?;
                if migration_snapshot(&restarted, 12).await? != current {
                    return Err("RESTART_RECEIPTS_OR_SCHEMA_CHANGED");
                }
                migration_preserved_data(&restarted, workspace, 12, 7).await
            }
            .await;
            let close = close_migration_owner(&restarted).await;
            restarted_result?;
            close
        }
        .await;
        let close = close_migration_owner(&resumed).await;
        result?;
        close
    }
    .await;
    let close = close_migration_owner(&backend).await;
    let ok = primary.is_ok() && close.is_ok();
    // The proof kind is one closed literal, printed only with a confirmed run and
    // after cleanup; raw SDK codes, messages or identifiers never reach the receipt.
    let fk_proof = match fk_proof {
        FK_PROOF_EXTENDED if ok => FK_PROOF_EXTENDED,
        FK_PROOF_SAME_WRITER_PRIMARY_HRANA if ok => FK_PROOF_SAME_WRITER_PRIMARY_HRANA,
        _ => FK_PROOF_NOT_CONFIRMED,
    };
    println!(
        "FVOCI_TURSO_MIGRATION_RECEIPT primary={} prefix={} fk_rollback={} fk_proof={} current={} restart={} close={} leases={}",
        if ok { "OK" } else { "FAILED" },
        if ok { "OK" } else { "NOT_CONFIRMED" },
        if ok { "OK" } else { "NOT_CONFIRMED" },
        fk_proof,
        if ok { "OK" } else { "NOT_CONFIRMED" },
        if ok { "OK" } else { "NOT_CONFIRMED" },
        if close.is_ok() { "OK" } else { "FAILED" },
        if backend
            .connection_stats()
            .is_ok_and(|stats| stats.size == 0)
        {
            "ZERO"
        } else {
            "FAILED"
        }
    );
    // Emit after original cleanup and receipt, without altering failure priority.
    if let Some((primary_code, close_code)) = migration_failure_diagnostic(primary, close) {
        println!(
            "FVOCI_TURSO_MIGRATION_DIAGNOSTIC primary={} close={}",
            primary_code, close_code,
        );
    }
    primary?;
    close
}
const INVENTORY_TEST_NAME: &str = "db::turso_test::turso_primary_migration_target_inventory";

fn inventory_selection(
    lookup: impl Fn(&str) -> Option<String>,
    args: &[String],
) -> Result<(), &'static str> {
    // Inventory reserves a writer for a snapshot but cannot authorize mutation.
    // Its exact read-only tuple is distinct from the mutating migration tuple.
    for (name, expected) in [
        ("FVOCI_TEST_TURSO_MIGRATION_SELECTED", "1"),
        ("FVOCI_TEST_TURSO_PHASE", "inventory"),
        ("FVOCI_TEST_TURSO_DESTRUCTIVE", "false"),
        ("FVOCI_TEST_TURSO_ALLOW_DESTRUCTIVE", "false"),
    ] {
        if lookup(name).as_deref() != Some(expected) {
            return Err("EXPLICIT_INVENTORY_SELECTION_REQUIRED");
        }
    }
    for required in [
        INVENTORY_TEST_NAME,
        "--ignored",
        "--exact",
        "--test-threads=1",
    ] {
        if args
            .iter()
            .skip(1)
            .filter(|arg| arg.as_str() == required)
            .count()
            != 1
        {
            return Err("EXPLICIT_INVENTORY_SELECTION_REQUIRED");
        }
    }
    if args.iter().skip(1).any(|arg| {
        !matches!(
            arg.as_str(),
            INVENTORY_TEST_NAME | "--ignored" | "--exact" | "--test-threads=1" | "--nocapture"
        )
    }) {
        return Err("EXPLICIT_INVENTORY_SELECTION_REQUIRED");
    }
    Ok(())
}

fn inventory_prefix(
    exists: i64,
    count: Option<i64>,
    compiled_steps: usize,
) -> Result<usize, &'static str> {
    if compiled_steps != 12 {
        return Err("CURRENT_LINEAGE_CHANGED");
    }
    match (exists, count) {
        (0, None) => Ok(0),
        (1, Some(count @ 0..=12)) => Ok(count as usize),
        _ => Err("INVENTORY_PREFIX_REFUSED"),
    }
}

struct TursoTargetInventory {
    prefix: u8,
    schema_sha256: String,
}

impl TursoTargetInventory {
    // This only checks disclosure shape. The caller MUST first obtain the
    // maintained full ledger/catalog snapshot on the SAME reserved writer.
    fn from_snapshot(
        prefix: usize,
        snapshot: super::migrate::TursoTestSchemaSnapshot,
    ) -> Result<Self, &'static str> {
        if prefix > 12 || snapshot.receipts.len() != prefix {
            return Err("INVENTORY_SNAPSHOT_MISMATCH");
        }
        if !inventory_hash(&snapshot.schema_sha256) {
            return Err("INVENTORY_HASH_INVALID");
        }
        Ok(Self {
            prefix: prefix as u8,
            schema_sha256: snapshot.schema_sha256,
        })
    }
}

fn inventory_hash(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|value| value.is_ascii_digit() || (b'a'..=b'f').contains(&value))
}

fn inventory_disclosure<'a>(
    primary: &'a Result<TursoTargetInventory, &'static str>,
    rollback: Option<Result<(), &'static str>>,
    close: Result<(), &'static str>,
    leases_zero: bool,
) -> Option<(&'static str, u8, &'a str)> {
    if rollback != Some(Ok(())) || close.is_err() || !leases_zero {
        return None;
    }
    let value = primary.as_ref().ok()?;
    if !inventory_hash(&value.schema_sha256) {
        return None;
    }
    let class = match value.prefix {
        0 => "BLANK",
        1..=11 => "PREFIX",
        12 => "CURRENT",
        _ => return None,
    };
    Some((class, value.prefix, &value.schema_sha256))
}

fn inventory_finish(result: Option<Result<(), &'static str>>) -> &'static str {
    match result {
        None => "NOT_STARTED",
        Some(Ok(())) => "OK",
        Some(Err(_)) => "FAILED",
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum InventoryPrimaryDiagnostic {
    Ok,
    BeginFailed,
    WrongProductBackend,
    WrongBackend,
    FkQueryFailed,
    FkDecodeFailed,
    ForeignKeysNotOne,
    LiteralQueryFailed,
    LiteralDecodeFailed,
    LiteralMismatch,
    CurrentLineageChanged,
    QueryFailed,
    DecodeFailed,
    PrefixRefused,
    SchemaRefused,
    SnapshotMismatch,
    HashInvalid,
}

impl InventoryPrimaryDiagnostic {
    fn from_result(result: Result<(), &'static str>) -> Option<Self> {
        Some(match result {
            Ok(()) => Self::Ok,
            Err("BEGIN_FAILED") => Self::BeginFailed,
            Err("WRONG_PRODUCT_BACKEND") => Self::WrongProductBackend,
            Err("WRONG_BACKEND") => Self::WrongBackend,
            Err("FK_QUERY_FAILED") => Self::FkQueryFailed,
            Err("FK_DECODE_FAILED") => Self::FkDecodeFailed,
            Err("FOREIGN_KEYS_NOT_ONE") => Self::ForeignKeysNotOne,
            Err("LITERAL_QUERY_FAILED") => Self::LiteralQueryFailed,
            Err("LITERAL_DECODE_FAILED") => Self::LiteralDecodeFailed,
            Err("LITERAL_MISMATCH") => Self::LiteralMismatch,
            Err("CURRENT_LINEAGE_CHANGED") => Self::CurrentLineageChanged,
            Err("INVENTORY_QUERY_FAILED") => Self::QueryFailed,
            Err("INVENTORY_DECODE_FAILED") => Self::DecodeFailed,
            Err("INVENTORY_PREFIX_REFUSED") => Self::PrefixRefused,
            Err("INVENTORY_SCHEMA_REFUSED") => Self::SchemaRefused,
            Err("INVENTORY_SNAPSHOT_MISMATCH") => Self::SnapshotMismatch,
            Err("INVENTORY_HASH_INVALID") => Self::HashInvalid,
            Err(_) => return None,
        })
    }

    fn code(self) -> &'static str {
        match self {
            Self::Ok => "OK",
            Self::BeginFailed => "BEGIN_FAILED",
            Self::WrongProductBackend => "WRONG_PRODUCT_BACKEND",
            Self::WrongBackend => "WRONG_BACKEND",
            Self::FkQueryFailed => "FK_QUERY_FAILED",
            Self::FkDecodeFailed => "FK_DECODE_FAILED",
            Self::ForeignKeysNotOne => "FOREIGN_KEYS_NOT_ONE",
            Self::LiteralQueryFailed => "LITERAL_QUERY_FAILED",
            Self::LiteralDecodeFailed => "LITERAL_DECODE_FAILED",
            Self::LiteralMismatch => "LITERAL_MISMATCH",
            Self::CurrentLineageChanged => "CURRENT_LINEAGE_CHANGED",
            Self::QueryFailed => "INVENTORY_QUERY_FAILED",
            Self::DecodeFailed => "INVENTORY_DECODE_FAILED",
            Self::PrefixRefused => "INVENTORY_PREFIX_REFUSED",
            Self::SchemaRefused => "INVENTORY_SCHEMA_REFUSED",
            Self::SnapshotMismatch => "INVENTORY_SNAPSHOT_MISMATCH",
            Self::HashInvalid => "INVENTORY_HASH_INVALID",
        }
    }
}

fn inventory_failure_diagnostic(
    primary: Result<(), &'static str>,
    rollback: Option<Result<(), &'static str>>,
    close: Result<(), &'static str>,
    leases_zero: bool,
) -> Option<(
    InventoryPrimaryDiagnostic,
    &'static str,
    &'static str,
    &'static str,
)> {
    if primary.is_ok() && rollback == Some(Ok(())) && close.is_ok() && leases_zero {
        return None;
    }
    let primary = InventoryPrimaryDiagnostic::from_result(primary)?;
    if matches!(
        primary,
        InventoryPrimaryDiagnostic::BeginFailed | InventoryPrimaryDiagnostic::WrongProductBackend
    ) != rollback.is_none()
    {
        return None;
    }
    let rollback = match rollback {
        None => "NOT_STARTED",
        Some(Ok(())) => "OK",
        Some(Err("ROLLBACK_UNCONFIRMED")) => "ROLLBACK_UNCONFIRMED",
        Some(Err(_)) => return None,
    };
    let close = match close {
        Ok(()) => "OK",
        Err("CLOSE_FAILED") => "CLOSE_FAILED",
        Err("LEASES_NOT_ZERO") => "LEASES_NOT_ZERO",
        Err(_) => return None,
    };
    Some((
        primary,
        rollback,
        close,
        if leases_zero { "ZERO" } else { "FAILED" },
    ))
}

fn inventory_settled_result(
    primary: Result<TursoTargetInventory, &'static str>,
    rollback: Option<Result<(), &'static str>>,
    close: Result<(), &'static str>,
    leases_zero: bool,
    admitted: bool,
) -> Result<(), &'static str> {
    // Same original primary -> rollback -> close -> lease -> disclosure priority.
    primary?;
    rollback.ok_or("ROLLBACK_NOT_STARTED")??;
    close?;
    if !leases_zero {
        return Err("LEASES_NOT_ZERO");
    }
    if !admitted {
        return Err("INVENTORY_DISCLOSURE_REFUSED");
    }
    Ok(())
}

async fn inventory_in_writer(tx: &mut DbTx) -> Result<TursoTargetInventory, &'static str> {
    same_stream_readback(tx).await?;
    let family = remote_family(tx)?;
    let exists = family
        .query(
            "SELECT count(*) FROM sqlite_schema WHERE type='table' AND name='schema_migrations'",
            &[],
        )
        .await
        .map_err(|_| "INVENTORY_QUERY_FAILED")?;
    if exists.len() != 1 {
        return Err("INVENTORY_DECODE_FAILED");
    }
    let exists = exists[0]
        .cell(0)
        .and_then(|cell| cell.integer())
        .map_err(|_| "INVENTORY_DECODE_FAILED")?;
    let count = if exists == 1 {
        let count = family
            .query("SELECT count(*) FROM schema_migrations", &[])
            .await
            .map_err(|_| "INVENTORY_QUERY_FAILED")?;
        if count.len() != 1 {
            return Err("INVENTORY_DECODE_FAILED");
        }
        Some(
            count[0]
                .cell(0)
                .and_then(|cell| cell.integer())
                .map_err(|_| "INVENTORY_DECODE_FAILED")?,
        )
    } else {
        None
    };
    // A count selects one expected prefix, never proves a ledger/catalog.
    let prefix = inventory_prefix(exists, count, super::migrate::compiled_sqlite_steps().len())?;
    let snapshot = super::migrate::turso_test_schema_in_writer(family, prefix)
        .await
        .map_err(|_| "INVENTORY_SCHEMA_REFUSED")?;
    TursoTargetInventory::from_snapshot(prefix, snapshot)
}

async fn inventory_observe(
    backend: &Backend,
) -> (
    Result<TursoTargetInventory, &'static str>,
    Option<Result<(), &'static str>>,
) {
    if !matches!(backend, Backend::LibsqlRemote(_)) {
        return (Err("WRONG_PRODUCT_BACKEND"), None);
    }
    let mut tx = match backend.begin_write().await {
        Ok(tx) => tx,
        Err(_) => return (Err("BEGIN_FAILED"), None),
    };
    let primary = inventory_in_writer(&mut tx).await;
    // Every query/schema/decode failure still settles the original stream.
    // No DDL, DML, commit, reset or alternate observer occurs here.
    let rollback = tx.rollback().await.map_err(|_| "ROLLBACK_UNCONFIRMED");
    (primary, Some(rollback))
}

#[tokio::test]
#[ignore = "manual trusted-ref target inventory; same migration gates, no schema/data mutation"]
async fn turso_primary_migration_target_inventory() -> Result<(), &'static str> {
    let args: Vec<String> = std::env::args().collect();
    inventory_selection(|name| std::env::var(name).ok(), &args)?;
    let settings = DatabaseSettings::from_env().map_err(|_| "PRODUCT_CONFIGURATION_FAILED")?;
    let DatabaseSettings::LibsqlRemote {
        primary_url,
        auth_token,
    } = settings
    else {
        return Err("WRONG_PRODUCT_BACKEND");
    };
    let (url, token) = configuration(primary_url, auth_token)?;
    let backend = Backend::LibsqlRemote(
        RemoteDatabase::connect(url, token, 1)
            .await
            .map_err(|_| "CONNECT_FAILED")?,
    );
    let (primary, rollback) = inventory_observe(&backend).await;
    let close = close_migration_owner(&backend).await;
    let leases_zero = backend
        .connection_stats()
        .is_ok_and(|stats| stats.size == 0);
    let disclosure = inventory_disclosure(&primary, rollback, close, leases_zero);
    let admitted = disclosure.is_some();
    let (classification, prefix, hash) = match disclosure {
        Some((class, prefix, hash)) => (class, prefix.to_string(), hash),
        None => ("REFUSED", "NONE".to_owned(), "NONE"),
    };
    println!(
        "FVOCI_TURSO_INVENTORY_RECEIPT classification={} prefix={} schema_sha256={} rollback={} close={} leases={}",
        classification, prefix, hash, inventory_finish(rollback), inventory_finish(Some(close)),
        if leases_zero { "ZERO" } else { "FAILED" },
    );
    if !admitted {
        // Explicit static producer line and RETURN boundary. Libtest's later
        // returned-Error text is untrusted output, never the cause oracle.
        match inventory_failure_diagnostic(
            primary.as_ref().map(|_| ()).map_err(|code| *code), rollback, close, leases_zero,
        ) {
            Some((primary, rollback, close, leases)) => println!(
                "\nFVOCI_TURSO_INVENTORY_DIAGNOSTIC primary={} rollback={} close={} leases={}",
                primary.code(), rollback, close, leases,
            ),
            None => println!(
                "\nFVOCI_TURSO_INVENTORY_DIAGNOSTIC primary=UNKNOWN rollback=UNKNOWN close=UNKNOWN leases=UNKNOWN"
            ),
        }
        println!("FVOCI_TURSO_INVENTORY_RETURN");
    }
    // Both outcomes are retained; primary errors keep priority. A fresh state
    // observation never reconciles the old failed migration/finish receipt.
    inventory_settled_result(primary, rollback, close, leases_zero, admitted)
}

#[cfg(test)]
mod inventory_policy_tests {
    use super::*;

    fn args() -> Vec<String> {
        [
            "libtest",
            INVENTORY_TEST_NAME,
            "--ignored",
            "--exact",
            "--test-threads=1",
        ]
        .into_iter()
        .map(String::from)
        .collect()
    }

    fn flags() -> Vec<(&'static str, &'static str)> {
        vec![
            ("FVOCI_TEST_TURSO_MIGRATION_SELECTED", "1"),
            ("FVOCI_TEST_TURSO_PHASE", "inventory"),
            ("FVOCI_TEST_TURSO_DESTRUCTIVE", "false"),
            ("FVOCI_TEST_TURSO_ALLOW_DESTRUCTIVE", "false"),
        ]
    }

    #[test]
    fn inventory_requires_exact_selection_and_each_migration_gate() {
        let valid = flags();
        let lookup = |name: &str| {
            valid
                .iter()
                .find(|pair| pair.0 == name)
                .map(|pair| pair.1.to_owned())
        };
        inventory_selection(lookup, &args()).unwrap();
        let mut captured = args();
        captured.push("--nocapture".into());
        inventory_selection(lookup, &captured).unwrap();
        for index in 0..valid.len() {
            let mut missing = valid.clone();
            missing.remove(index);
            assert_eq!(
                inventory_selection(
                    |name| missing
                        .iter()
                        .find(|pair| pair.0 == name)
                        .map(|pair| pair.1.to_owned()),
                    &args()
                ),
                Err("EXPLICIT_INVENTORY_SELECTION_REQUIRED")
            );
            let mut wrong = valid.clone();
            wrong[index].1 = match index {
                1 => "migration",
                2 | 3 => "true",
                _ => "false",
            };
            assert!(inventory_selection(
                |name| wrong
                    .iter()
                    .find(|pair| pair.0 == name)
                    .map(|pair| pair.1.to_owned()),
                &args()
            )
            .is_err());
        }
        for index in 1..args().len() {
            let mut missing = args();
            missing.remove(index);
            assert!(inventory_selection(lookup, &missing).is_err());
            let mut duplicate = args();
            duplicate.push(duplicate[index].clone());
            assert!(inventory_selection(lookup, &duplicate).is_err());
        }
        let mut migration = args();
        migration[1] = MIGRATION_TEST_NAME.into();
        assert!(inventory_selection(lookup, &migration).is_err());
        let mut extra = args();
        extra.push("--include-ignored".into());
        assert!(inventory_selection(lookup, &extra).is_err());
    }

    #[test]
    fn inventory_rejects_migration_and_mixed_destructive_modes() {
        for phase in ["inventory", "migration", "connection", "unknown"] {
            for destructive in ["false", "true", "FALSE", "", "0"] {
                for allow in ["false", "true", "FALSE", "", "0"] {
                    let valid = flags();
                    let selected = inventory_selection(
                        |name| match name {
                            "FVOCI_TEST_TURSO_PHASE" => Some(phase.to_owned()),
                            "FVOCI_TEST_TURSO_DESTRUCTIVE" => Some(destructive.to_owned()),
                            "FVOCI_TEST_TURSO_ALLOW_DESTRUCTIVE" => Some(allow.to_owned()),
                            _ => valid
                                .iter()
                                .find(|pair| pair.0 == name)
                                .map(|pair| pair.1.to_owned()),
                        },
                        &args(),
                    );
                    if phase == "inventory" && destructive == "false" && allow == "false" {
                        assert_eq!(selected, Ok(()));
                    } else {
                        assert_eq!(selected, Err("EXPLICIT_INVENTORY_SELECTION_REQUIRED"));
                    }
                }
            }
        }
    }

    #[test]
    fn inventory_prefix_never_admits_unknown_or_ahead_ledger_count() {
        assert_eq!(inventory_prefix(0, None, 12), Ok(0));
        for count in 0..=12 {
            assert_eq!(inventory_prefix(1, Some(count), 12), Ok(count as usize));
        }
        for (exists, count) in [
            (0, Some(0)),
            (1, None),
            (2, Some(1)),
            (-1, None),
            (1, Some(-1)),
            (1, Some(13)),
            (1, Some(i64::MAX)),
        ] {
            assert!(inventory_prefix(exists, count, 12).is_err());
        }
        assert!(inventory_prefix(0, None, 11).is_err());
        assert!(inventory_prefix(1, Some(12), 13).is_err());
    }

    #[test]
    fn inventory_discloses_only_bounded_verified_shape_after_full_settlement() {
        let hash = "a".repeat(64);
        for prefix in 0..=12 {
            // Pure disclosure policy vectors, never fabricated remote receipts.
            let observed = Ok(TursoTargetInventory {
                prefix,
                schema_sha256: hash.clone(),
            });
            let expected = match prefix {
                0 => "BLANK",
                12 => "CURRENT",
                _ => "PREFIX",
            };
            assert_eq!(
                inventory_disclosure(&observed, Some(Ok(())), Ok(()), true),
                Some((expected, prefix, hash.as_str()))
            );
            for rollback in [None, Some(Ok(())), Some(Err("ROLLBACK_UNCONFIRMED"))] {
                for close in [Ok(()), Err("CLOSE_FAILED")] {
                    for leases in [false, true] {
                        let actual = inventory_disclosure(&observed, rollback, close, leases);
                        if rollback == Some(Ok(())) && close.is_ok() && leases {
                            assert_eq!(actual, Some((expected, prefix, hash.as_str())));
                        } else {
                            assert_eq!(actual, None);
                        }
                    }
                }
            }
        }
        for invalid in [
            "private://CANARY_TOKEN",
            "CANARY_TOKEN\nFAKE",
            "a",
            &"a".repeat(65),
            &"G".repeat(64),
        ] {
            let observed = Ok(TursoTargetInventory {
                prefix: 12,
                schema_sha256: invalid.into(),
            });
            assert_eq!(
                inventory_disclosure(&observed, Some(Ok(())), Ok(()), true),
                None
            );
        }
        let ahead = Ok(TursoTargetInventory {
            prefix: 13,
            schema_sha256: hash,
        });
        assert_eq!(
            inventory_disclosure(&ahead, Some(Ok(())), Ok(()), true),
            None
        );
        for error in [
            "INVENTORY_QUERY_FAILED",
            "INVENTORY_SCHEMA_REFUSED",
            "ROLLBACK_UNCONFIRMED",
            "private://CANARY_TOKEN",
        ] {
            assert_eq!(
                inventory_disclosure(&Err(error), Some(Ok(())), Ok(()), true),
                None
            );
        }
        assert_eq!(inventory_finish(None), "NOT_STARTED");
        assert_eq!(inventory_finish(Some(Ok(()))), "OK");
        assert_eq!(
            inventory_finish(Some(Err("private://CANARY_TOKEN"))),
            "FAILED"
        );
        let incomplete = super::super::migrate::TursoTestSchemaSnapshot {
            receipts: vec![],
            schema_sha256: "a".repeat(64),
        };
        assert!(TursoTargetInventory::from_snapshot(1, incomplete).is_err());
    }

    #[tokio::test]
    async fn inventory_wrong_backend_refuses_without_changing_actual_local_catalog() {
        let f = crate::db::attachment_preview::tests::Fixture::new().await;
        let before: Vec<(String, String, String, Option<String>)> =
            sqlx::query_as("SELECT type,name,tbl_name,sql FROM sqlite_schema ORDER BY type,name")
                .fetch_all(&f.pool)
                .await
                .unwrap();
        let (primary, rollback) = inventory_observe(&f.backend).await;
        assert!(matches!(primary, Err("WRONG_PRODUCT_BACKEND")));
        assert_eq!(rollback, None);
        let after: Vec<(String, String, String, Option<String>)> =
            sqlx::query_as("SELECT type,name,tbl_name,sql FROM sqlite_schema ORDER BY type,name")
                .fetch_all(&f.pool)
                .await
                .unwrap();
        assert_eq!(before, after);
        assert_eq!(
            sqlx::query_scalar::<_, i64>("PRAGMA foreign_keys")
                .fetch_one(&f.pool)
                .await
                .unwrap(),
            1
        );
        f.close().await;
    }

    #[test]
    fn inventory_diagnostics_close_the_current_primary_code_set() {
        for code in [
            "BEGIN_FAILED",
            "WRONG_PRODUCT_BACKEND",
            "WRONG_BACKEND",
            "FK_QUERY_FAILED",
            "FK_DECODE_FAILED",
            "FOREIGN_KEYS_NOT_ONE",
            "LITERAL_QUERY_FAILED",
            "LITERAL_DECODE_FAILED",
            "LITERAL_MISMATCH",
            "CURRENT_LINEAGE_CHANGED",
            "INVENTORY_QUERY_FAILED",
            "INVENTORY_DECODE_FAILED",
            "INVENTORY_PREFIX_REFUSED",
            "INVENTORY_SCHEMA_REFUSED",
            "INVENTORY_SNAPSHOT_MISMATCH",
            "INVENTORY_HASH_INVALID",
        ] {
            assert_eq!(
                InventoryPrimaryDiagnostic::from_result(Err(code))
                    .unwrap()
                    .code(),
                code
            );
        }
        assert_eq!(
            InventoryPrimaryDiagnostic::from_result(Ok(())),
            Some(InventoryPrimaryDiagnostic::Ok)
        );
        for code in [
            "OK",
            "CONNECT_FAILED",
            "PRODUCT_CONFIGURATION_FAILED",
            "DDL_FAILED",
            "COMMIT_UNCONFIRMED",
            "UNKNOWN",
        ] {
            assert_eq!(InventoryPrimaryDiagnostic::from_result(Err(code)), None);
        }
    }

    #[test]
    fn inventory_diagnostics_retain_original_rollback_close_and_lease_facts() {
        // Pure outcome vectors, never fabricated remote finish/close receipts.
        for rollback in [Ok(()), Err("ROLLBACK_UNCONFIRMED")] {
            for close in [Ok(()), Err("CLOSE_FAILED"), Err("LEASES_NOT_ZERO")] {
                for leases in [false, true] {
                    assert_eq!(
                        inventory_failure_diagnostic(
                            Err("INVENTORY_SCHEMA_REFUSED"),
                            Some(rollback),
                            close,
                            leases
                        ),
                        Some((
                            InventoryPrimaryDiagnostic::SchemaRefused,
                            if rollback.is_ok() {
                                "OK"
                            } else {
                                "ROLLBACK_UNCONFIRMED"
                            },
                            close.err().unwrap_or("OK"),
                            if leases { "ZERO" } else { "FAILED" }
                        )),
                    );
                }
            }
        }
        for code in ["BEGIN_FAILED", "WRONG_PRODUCT_BACKEND"] {
            assert!(inventory_failure_diagnostic(Err(code), None, Ok(()), true).is_some());
            assert_eq!(
                inventory_failure_diagnostic(Err(code), Some(Ok(())), Ok(()), true),
                None
            );
        }
        assert_eq!(
            inventory_failure_diagnostic(Err("INVENTORY_QUERY_FAILED"), None, Ok(()), true),
            None
        );
        assert_eq!(
            inventory_failure_diagnostic(Ok(()), None, Ok(()), true),
            None
        );
    }

    #[test]
    fn inventory_settlement_keeps_primary_then_rollback_then_close_then_lease_priority() {
        let healthy = || {
            Ok(TursoTargetInventory {
                prefix: 12,
                schema_sha256: "a".repeat(64),
            })
        };
        assert_eq!(
            inventory_settled_result(
                Err("INVENTORY_SCHEMA_REFUSED"),
                Some(Err("ROLLBACK_UNCONFIRMED")),
                Err("CLOSE_FAILED"),
                false,
                false
            ),
            Err("INVENTORY_SCHEMA_REFUSED")
        );
        assert_eq!(
            inventory_settled_result(Err("BEGIN_FAILED"), None, Err("CLOSE_FAILED"), false, false),
            Err("BEGIN_FAILED")
        );
        assert_eq!(
            inventory_settled_result(
                healthy(),
                Some(Err("ROLLBACK_UNCONFIRMED")),
                Err("CLOSE_FAILED"),
                false,
                false
            ),
            Err("ROLLBACK_UNCONFIRMED")
        );
        assert_eq!(
            inventory_settled_result(healthy(), None, Err("CLOSE_FAILED"), false, false),
            Err("ROLLBACK_NOT_STARTED")
        );
        assert_eq!(
            inventory_settled_result(healthy(), Some(Ok(())), Err("CLOSE_FAILED"), false, false),
            Err("CLOSE_FAILED")
        );
        assert_eq!(
            inventory_settled_result(healthy(), Some(Ok(())), Ok(()), false, false),
            Err("LEASES_NOT_ZERO")
        );
        assert_eq!(
            inventory_settled_result(healthy(), Some(Ok(())), Ok(()), true, false),
            Err("INVENTORY_DISCLOSURE_REFUSED")
        );
        assert_eq!(
            inventory_settled_result(healthy(), Some(Ok(())), Ok(()), true, true),
            Ok(())
        );
    }

    #[test]
    fn inventory_diagnostics_never_reflect_private_or_unknown_codes_or_invent_failure() {
        assert_eq!(
            inventory_failure_diagnostic(Ok(()), Some(Ok(())), Ok(()), true),
            None
        );
        for private in [
            "",
            "UNKNOWN",
            "schema_refused",
            "INVENTORY_SCHEMA_REFUSED_EXTRA",
            "INVENTORY_SCHEMA_REFUSED\nFAKE_PRIVATE_TOKEN",
            "libsql://FAKE_PRIVATE_TOKEN",
            "\rFAKE_PRIVATE_TOKEN",
            "\u{2028}FAKE_PRIVATE_TOKEN",
        ] {
            assert_eq!(InventoryPrimaryDiagnostic::from_result(Err(private)), None);
            assert_eq!(
                inventory_failure_diagnostic(Err(private), Some(Ok(())), Ok(()), true),
                None
            );
            assert_eq!(
                inventory_failure_diagnostic(
                    Err("INVENTORY_QUERY_FAILED"),
                    Some(Err(private)),
                    Ok(()),
                    true
                ),
                None
            );
            assert_eq!(
                inventory_failure_diagnostic(
                    Err("INVENTORY_QUERY_FAILED"),
                    Some(Ok(())),
                    Err(private),
                    true
                ),
                None
            );
        }
    }
}

// Separately selected disposable test-schema operation. Ordinary consumers
// never invoke this route or reset a nonblank schema on their own behalf.
const RESET_TEST_NAME: &str = "db::turso_test::turso_primary_disposable_prefix11_reset";

fn reset_selection(
    lookup: impl Fn(&str) -> Option<String>,
    args: &[String],
) -> Result<(), &'static str> {
    // The dedicated selector/exact body separates reset from migration. The
    // unchanged migration tuple admits ONLY reuse of its immutable snapshot.
    for (name, expected) in [
        ("FVOCI_TEST_TURSO_RESET_SELECTED", "1"),
        ("FVOCI_TEST_TURSO_MIGRATION_SELECTED", "1"),
        ("FVOCI_TEST_TURSO_PHASE", "migration"),
        ("FVOCI_TEST_TURSO_DESTRUCTIVE", "true"),
        ("FVOCI_TEST_TURSO_ALLOW_DESTRUCTIVE", "true"),
    ] {
        if lookup(name).as_deref() != Some(expected) {
            return Err("EXPLICIT_RESET_SELECTION_REQUIRED");
        }
    }
    for required in [RESET_TEST_NAME, "--ignored", "--exact", "--test-threads=1"] {
        if args
            .iter()
            .skip(1)
            .filter(|arg| arg.as_str() == required)
            .count()
            != 1
        {
            return Err("EXPLICIT_RESET_SELECTION_REQUIRED");
        }
    }
    if args.iter().skip(1).any(|arg| {
        !matches!(
            arg.as_str(),
            RESET_TEST_NAME | "--ignored" | "--exact" | "--test-threads=1" | "--nocapture"
        )
    }) {
        return Err("EXPLICIT_RESET_SELECTION_REQUIRED");
    }
    if args
        .iter()
        .skip(1)
        .filter(|arg| arg.as_str() == "--nocapture")
        .count()
        > 1
    {
        return Err("EXPLICIT_RESET_SELECTION_REQUIRED");
    }
    Ok(())
}

// Literal statements only, reviewed against the compiled schema. Remove
// triggers before implicit DROP deletes, then children before FK parents,
// and the ledger last. FK enforcement is NEVER disabled or deferred.
const RESET_DROP_STATEMENTS: [&str; 126] = [
    "DROP TRIGGER IF EXISTS \"api_tokens_scopes_insert\";",
    "DROP TRIGGER IF EXISTS \"api_tokens_scopes_update\";",
    "DROP TRIGGER IF EXISTS \"event_sequence_no_delete\";",
    "DROP TRIGGER IF EXISTS \"event_sequence_no_reset\";",
    "DROP TRIGGER IF EXISTS \"event_sequence_no_replace\";",
    "DROP TRIGGER IF EXISTS \"timer_segment_time_entry_purge\";",
    "DROP TRIGGER IF EXISTS \"task_timer_legacy_tracking_insert\";",
    "DROP TRIGGER IF EXISTS \"task_timer_legacy_tracking_close\";",
    "DROP TRIGGER IF EXISTS \"documents_path_insert\";",
    "DROP TRIGGER IF EXISTS \"documents_path_update\";",
    "DROP TRIGGER IF EXISTS \"wiki_create_commands_document_purge\";",
    "DROP TRIGGER IF EXISTS \"collab_fence_counter_no_delete\";",
    "DROP TRIGGER IF EXISTS \"collab_fence_counter_no_reset\";",
    "DROP TRIGGER IF EXISTS \"collab_fence_counter_no_replace\";",
    "DROP TRIGGER IF EXISTS \"body_save_commands_document_purge\";",
    "DROP TRIGGER IF EXISTS \"body_save_commands_task_purge\";",
    "DROP TRIGGER IF EXISTS \"attachment_object_cleanups_after_attachment_delete\";",
    "DROP TRIGGER IF EXISTS \"collection_items_scope_insert\";",
    "DROP TRIGGER IF EXISTS \"collection_items_scope_update\";",
    "DROP TRIGGER IF EXISTS \"collections_keep_scope\";",
    "DROP TRIGGER IF EXISTS \"documents_keep_collection_scope\";",
    "DROP TRIGGER IF EXISTS \"tasks_keep_collection_scope\";",
    "DROP TRIGGER IF EXISTS \"collection_fields_identity\";",
    "DROP TRIGGER IF EXISTS \"zotero_reference_document_purge\";",
    "DROP TRIGGER IF EXISTS \"personal_input_document_purge\";",
    "DROP TRIGGER IF EXISTS \"personal_input_task_purge\";",
    "DROP TRIGGER IF EXISTS \"personal_input_project_purge\";",
    "DROP TABLE IF EXISTS \"magic_tokens\";",
    "DROP TABLE IF EXISTS \"legal_documents\";",
    "DROP TABLE IF EXISTS \"user_consents\";",
    "DROP TABLE IF EXISTS \"user_mfa\";",
    "DROP TABLE IF EXISTS \"identity_links\";",
    "DROP TABLE IF EXISTS \"mfa_challenges\";",
    "DROP TABLE IF EXISTS \"oidc_states\";",
    "DROP TABLE IF EXISTS \"api_tokens\";",
    "DROP TABLE IF EXISTS \"group_members\";",
    "DROP TABLE IF EXISTS \"invitations\";",
    "DROP TABLE IF EXISTS \"workspace_holidays\";",
    "DROP TABLE IF EXISTS \"instance_settings\";",
    "DROP TABLE IF EXISTS \"instance_settings_meta\";",
    "DROP TABLE IF EXISTS \"workspace_oidc\";",
    "DROP TABLE IF EXISTS \"events\";",
    "DROP TABLE IF EXISTS \"audit_log\";",
    "DROP TABLE IF EXISTS \"outbox_consumers\";",
    "DROP TABLE IF EXISTS \"outbox_failures\";",
    "DROP TABLE IF EXISTS \"processed_events\";",
    "DROP TABLE IF EXISTS \"event_sequence\";",
    "DROP TABLE IF EXISTS \"project_members\";",
    "DROP TABLE IF EXISTS \"task_assignees\";",
    "DROP TABLE IF EXISTS \"task_labels\";",
    "DROP TABLE IF EXISTS \"task_dependencies\";",
    "DROP TABLE IF EXISTS \"task_activity\";",
    "DROP TABLE IF EXISTS \"task_states\";",
    "DROP TABLE IF EXISTS \"task_collab_updates\";",
    "DROP TABLE IF EXISTS \"task_collab_op_receipts\";",
    "DROP TABLE IF EXISTS \"task_timer_segments\";",
    "DROP TABLE IF EXISTS \"task_timer_legacy_open\";",
    "DROP TABLE IF EXISTS \"task_timer_commands\";",
    "DROP TABLE IF EXISTS \"task_timer_audit\";",
    "DROP TABLE IF EXISTS \"document_states\";",
    "DROP TABLE IF EXISTS \"document_collab_updates\";",
    "DROP TABLE IF EXISTS \"document_collab_op_receipts\";",
    "DROP TABLE IF EXISTS \"revisions\";",
    "DROP TABLE IF EXISTS \"document_members\";",
    "DROP TABLE IF EXISTS \"comments\";",
    "DROP TABLE IF EXISTS \"stars\";",
    "DROP TABLE IF EXISTS \"share_links\";",
    "DROP TABLE IF EXISTS \"document_tag_assignments\";",
    "DROP TABLE IF EXISTS \"task_origins\";",
    "DROP TABLE IF EXISTS \"templates\";",
    "DROP TABLE IF EXISTS \"wiki_create_commands\";",
    "DROP TABLE IF EXISTS \"collab_fence_counter\";",
    "DROP TABLE IF EXISTS \"collab_room_fences\";",
    "DROP TABLE IF EXISTS \"body_save_commands\";",
    "DROP TABLE IF EXISTS \"task_collab_room_fences\";",
    "DROP TABLE IF EXISTS \"attachment_text\";",
    "DROP TABLE IF EXISTS \"attachment_object_cleanups\";",
    "DROP TABLE IF EXISTS \"collection_values\";",
    "DROP TABLE IF EXISTS \"collection_choices\";",
    "DROP TABLE IF EXISTS \"collection_people\";",
    "DROP TABLE IF EXISTS \"collection_views\";",
    "DROP TABLE IF EXISTS \"views\";",
    "DROP TABLE IF EXISTS \"notifications\";",
    "DROP TABLE IF EXISTS \"notification_prefs\";",
    "DROP TABLE IF EXISTS \"ics_tokens\";",
    "DROP TABLE IF EXISTS \"instance_config\";",
    "DROP TABLE IF EXISTS \"push_deliveries\";",
    "DROP TABLE IF EXISTS \"webhook_deliveries\";",
    "DROP TABLE IF EXISTS \"github_installations\";",
    "DROP TABLE IF EXISTS \"github_install_states\";",
    "DROP TABLE IF EXISTS \"github_issue_links\";",
    "DROP TABLE IF EXISTS \"github_deliveries\";",
    "DROP TABLE IF EXISTS \"zotero_credentials\";",
    "DROP TABLE IF EXISTS \"zotero_memberships\";",
    "DROP TABLE IF EXISTS \"zotero_links\";",
    "DROP TABLE IF EXISTS \"import_deferred_events\";",
    "DROP TABLE IF EXISTS \"personal_input_commands\";",
    "DROP TABLE IF EXISTS \"personal_transfer_commands\";",
    "DROP TABLE IF EXISTS \"maintenance_job_claims\";",
    "DROP TABLE IF EXISTS \"memberships\";",
    "DROP TABLE IF EXISTS \"groups\";",
    "DROP TABLE IF EXISTS \"labels\";",
    "DROP TABLE IF EXISTS \"time_entries\";",
    "DROP TABLE IF EXISTS \"task_timer_runs\";",
    "DROP TABLE IF EXISTS \"document_tags\";",
    "DROP TABLE IF EXISTS \"attachments\";",
    "DROP TABLE IF EXISTS \"collection_items\";",
    "DROP TABLE IF EXISTS \"collection_options\";",
    "DROP TABLE IF EXISTS \"push_subscriptions\";",
    "DROP TABLE IF EXISTS \"webhooks\";",
    "DROP TABLE IF EXISTS \"zotero_references\";",
    "DROP TABLE IF EXISTS \"zotero_collections\";",
    "DROP TABLE IF EXISTS \"import_jobs\";",
    "DROP TABLE IF EXISTS \"sessions\";",
    "DROP TABLE IF EXISTS \"tasks\";",
    "DROP TABLE IF EXISTS \"documents\";",
    "DROP TABLE IF EXISTS \"collection_fields\";",
    "DROP TABLE IF EXISTS \"zotero_connectors\";",
    "DROP TABLE IF EXISTS \"statuses\";",
    "DROP TABLE IF EXISTS \"milestones\";",
    "DROP TABLE IF EXISTS \"collections\";",
    "DROP TABLE IF EXISTS \"workflows\";",
    "DROP TABLE IF EXISTS \"projects\";",
    "DROP TABLE IF EXISTS \"users\";",
    "DROP TABLE IF EXISTS \"workspaces\";",
    "DROP TABLE IF EXISTS \"schema_migrations\";",
];

#[derive(Debug, PartialEq, Eq)]
struct ResetOutcome {
    primary: &'static str,
    rollback: &'static str,
    commit: &'static str,
    blank: &'static str,
    steps: usize,
}

impl ResetOutcome {
    fn refused(primary: &'static str) -> Self {
        Self {
            primary,
            rollback: "NOT_STARTED",
            commit: "NOT_STARTED",
            blank: "NOT_RUN",
            steps: 0,
        }
    }
}

async fn reset_transaction(backend: &Backend) -> ResetOutcome {
    if !matches!(backend, Backend::LibsqlRemote(_)) {
        return ResetOutcome::refused("WRONG_PRODUCT_BACKEND");
    }
    let mut tx = match backend.begin_write().await {
        Ok(tx) => tx,
        Err(_) => return ResetOutcome::refused("BEGIN_FAILED"),
    };
    let mut steps = 0;
    let primary = async {
        same_stream_readback(&mut tx).await?;
        if super::migrate::compiled_sqlite_steps().len() != 12 {
            return Err("CURRENT_LINEAGE_CHANGED");
        }
        let family = remote_family(&mut tx)?;
        // Bound to the actually observed designated PREFIX11. Full ledger and
        // catalog validation is BEFORE any drop, on this same writer stream.
        super::migrate::turso_test_schema_in_writer(family, 11)
            .await
            .map_err(|_| "RESET_SCHEMA_REFUSED")?;
        for statement in RESET_DROP_STATEMENTS {
            family
                .execute(statement, &[])
                .await
                .map_err(|_| "RESET_DDL_FAILED")?;
            steps += 1;
        }
        same_stream_readback(&mut tx)
            .await
            .map_err(|_| "RESET_BLANK_IN_WRITER_FAILED")?;
        super::migrate::turso_test_schema_in_writer(remote_family(&mut tx)?, 0)
            .await
            .map_err(|_| "RESET_BLANK_IN_WRITER_FAILED")?;
        Ok::<(), &'static str>(())
    }
    .await;
    if let Err(primary) = primary {
        // Preserve primary cause; the result is only the original submitted
        // rollback's return, never fabricated remote settlement or durability.
        let rollback = if tx.rollback().await.is_ok() {
            "RETURNED_OK"
        } else {
            "UNCONFIRMED"
        };
        return ResetOutcome {
            primary,
            rollback,
            commit: "NOT_STARTED",
            blank: "NOT_RUN",
            steps,
        };
    }
    if tx.commit_with_cleanup().await.is_err() {
        // Maintained typed cleanup is awaited. Remote commit uncertainty NEVER
        // permits an alternate observer, rollback, retry or inferred success.
        return ResetOutcome {
            primary: "COMMIT_UNCONFIRMED",
            rollback: "NOT_STARTED",
            commit: "UNCONFIRMED",
            blank: "NOT_RUN",
            steps,
        };
    }
    // Backend::begin_write opens a NEW SDK connection/stream. This full blank
    // gate observes a new stream only after the original commit returned OK.
    let blank = migration_snapshot(backend, 0).await;
    ResetOutcome {
        primary: if blank.is_ok() {
            "OK"
        } else {
            "RESET_FRESH_BLANK_FAILED"
        },
        rollback: "NOT_STARTED",
        commit: "RETURNED_OK",
        blank: if blank.is_ok() { "CONFIRMED" } else { "FAILED" },
        steps,
    }
}

#[tokio::test]
#[ignore = "manual trusted-ref explicitly authorized disposable PREFIX11 reset only"]
async fn turso_primary_disposable_prefix11_reset() -> Result<(), &'static str> {
    let args: Vec<String> = std::env::args().collect();
    reset_selection(|name| std::env::var(name).ok(), &args)?;
    let settings = DatabaseSettings::from_env().map_err(|_| "PRODUCT_CONFIGURATION_FAILED")?;
    let DatabaseSettings::LibsqlRemote {
        primary_url,
        auth_token,
    } = settings
    else {
        return Err("WRONG_PRODUCT_BACKEND");
    };
    let (url, token) = configuration(primary_url, auth_token)?;
    let backend = Backend::LibsqlRemote(
        RemoteDatabase::connect(url, token, 1)
            .await
            .map_err(|_| "CONNECT_FAILED")?,
    );
    let outcome = reset_transaction(&backend).await;
    // Backend close ALWAYS awaits original local cleanup/drain, including
    // failure/quarantined commit; local drain is not a server Close ACK.
    let close = close_migration_owner(&backend).await;
    let leases_zero = backend
        .connection_stats()
        .is_ok_and(|stats| stats.size == 0);
    println!("FVOCI_TURSO_RESET_RECEIPT primary={} rollback={} commit={} blank={} steps={} close={} drain={} leases={}",
        outcome.primary, outcome.rollback, outcome.commit, outcome.blank, outcome.steps,
        if close.is_ok() { "OK" } else { "FAILED" },
        if close.is_ok() { "LOCAL_OK" } else { "UNCONFIRMED" },
        if leases_zero { "ZERO" } else { "FAILED" });
    if outcome.primary != "OK" || close.is_err() || !leases_zero {
        // Explicit static boundary: untrusted libtest returned Error follows.
        println!("\nFVOCI_TURSO_RESET_RETURN");
    }
    if outcome.primary != "OK" {
        return Err(outcome.primary);
    }
    close?;
    if !leases_zero {
        return Err("LEASES_NOT_ZERO");
    }
    Ok(())
}

#[cfg(test)]
mod reset_policy_tests {
    use super::*;

    fn flags() -> Vec<(&'static str, &'static str)> {
        vec![
            ("FVOCI_TEST_TURSO_RESET_SELECTED", "1"),
            ("FVOCI_TEST_TURSO_MIGRATION_SELECTED", "1"),
            ("FVOCI_TEST_TURSO_PHASE", "migration"),
            ("FVOCI_TEST_TURSO_DESTRUCTIVE", "true"),
            ("FVOCI_TEST_TURSO_ALLOW_DESTRUCTIVE", "true"),
        ]
    }

    fn args() -> Vec<String> {
        [
            "libtest",
            RESET_TEST_NAME,
            "--ignored",
            "--exact",
            "--test-threads=1",
        ]
        .into_iter()
        .map(String::from)
        .collect()
    }

    #[test]
    fn reset_requires_each_gate_and_exact_selected_body_before_connect() {
        let valid = flags();
        let lookup = |name: &str| {
            valid
                .iter()
                .find(|item| item.0 == name)
                .map(|item| item.1.to_owned())
        };
        assert_eq!(reset_selection(lookup, &args()), Ok(()));
        for index in 0..valid.len() {
            let mut missing = valid.clone();
            missing.remove(index);
            assert_eq!(
                reset_selection(
                    |name| missing
                        .iter()
                        .find(|item| item.0 == name)
                        .map(|item| item.1.to_owned()),
                    &args()
                ),
                Err("EXPLICIT_RESET_SELECTION_REQUIRED")
            );
            for wrong in ["false", "TRUE", "", "0", "inventory", "reset"] {
                let mut values = valid.clone();
                values[index].1 = wrong;
                assert_eq!(
                    reset_selection(
                        |name| values
                            .iter()
                            .find(|item| item.0 == name)
                            .map(|item| item.1.to_owned()),
                        &args()
                    ),
                    Err("EXPLICIT_RESET_SELECTION_REQUIRED")
                );
            }
        }
        for index in 1..args().len() {
            let mut missing = args();
            missing.remove(index);
            assert!(reset_selection(lookup, &missing).is_err());
            let mut duplicate = args();
            duplicate.push(duplicate[index].clone());
            assert!(reset_selection(lookup, &duplicate).is_err());
        }
        for body in [MIGRATION_TEST_NAME, INVENTORY_TEST_NAME, TEST_NAME] {
            let mut wrong = args();
            wrong[1] = body.to_owned();
            assert!(reset_selection(lookup, &wrong).is_err());
        }
        let mut extra = args();
        extra.push("--include-ignored".into());
        assert!(reset_selection(lookup, &extra).is_err());
        let mut captured = args();
        captured.push("--nocapture".into());
        assert_eq!(reset_selection(lookup, &captured), Ok(()));
        captured.push("--nocapture".into());
        assert!(reset_selection(lookup, &captured).is_err());
    }

    #[test]
    fn reset_plan_uses_only_literal_drops_with_ledger_last_and_no_fk_disable() {
        assert_eq!(RESET_DROP_STATEMENTS.len(), 126);
        assert_eq!(
            RESET_DROP_STATEMENTS.last(),
            Some(&"DROP TABLE IF EXISTS \"schema_migrations\";")
        );
        let mut names = std::collections::BTreeSet::new();
        for (index, statement) in RESET_DROP_STATEMENTS.iter().enumerate() {
            let prefix = if index < 27 {
                "DROP TRIGGER IF EXISTS \""
            } else {
                "DROP TABLE IF EXISTS \""
            };
            let name = statement
                .strip_prefix(prefix)
                .unwrap()
                .strip_suffix("\";")
                .unwrap();
            assert!(name
                .bytes()
                .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'_'));
            assert!(names.insert((index < 27, name)));
            assert!(!statement.contains("PRAGMA"));
        }
    }
    #[tokio::test]
    async fn reset_wrong_backend_preserves_actual_local_catalog_and_fk() {
        let f = crate::db::attachment_preview::tests::Fixture::new().await;
        let before: Vec<(String, String, String, Option<String>)> =
            sqlx::query_as("SELECT type,name,tbl_name,sql FROM sqlite_schema ORDER BY type,name")
                .fetch_all(&f.pool)
                .await
                .unwrap();
        assert_eq!(
            reset_transaction(&f.backend).await,
            ResetOutcome::refused("WRONG_PRODUCT_BACKEND")
        );
        let after: Vec<(String, String, String, Option<String>)> =
            sqlx::query_as("SELECT type,name,tbl_name,sql FROM sqlite_schema ORDER BY type,name")
                .fetch_all(&f.pool)
                .await
                .unwrap();
        assert_eq!(before, after);
        assert_eq!(
            sqlx::query_scalar::<_, i64>("PRAGMA foreign_keys")
                .fetch_one(&f.pool)
                .await
                .unwrap(),
            1
        );
        assert_eq!(
            sqlx::query_scalar::<_, i64>("SELECT 353")
                .fetch_one(&f.pool)
                .await
                .unwrap(),
            353
        );
        f.close().await;
    }

    async fn protocol_fixture() -> (
        crate::db::libsql_finish_fixture::LibsqlFinishFixture,
        std::path::PathBuf,
        Backend,
    ) {
        let sqld = std::path::PathBuf::from(
            std::env::var_os("FVOCI_TEST_SQLD")
                .expect("ROOT allocated pinned official sqld required; no skip"),
        );
        let root = std::env::temp_dir().join(format!("fvoci-reset-{}", uuid::Uuid::now_v7()));
        let driver = crate::db::libsql_finish_fixture::LibsqlFinishFixture::start(&sqld, &root)
            .await
            .unwrap();
        let database = driver.database().await.unwrap();
        let backend = Backend::LibsqlRemote(std::sync::Arc::new(RemoteDatabase::from_test_driver(
            database,
            std::num::NonZeroU32::new(1).unwrap(),
        )));
        (driver, root, backend)
    }

    async fn finish_fixture(
        driver: crate::db::libsql_finish_fixture::LibsqlFinishFixture,
        root: std::path::PathBuf,
    ) {
        let receipt = driver.finish().await.unwrap();
        assert!(receipt.sqld_reaped);
        std::fs::remove_dir_all(root).unwrap();
    }

    #[tokio::test]
    #[ignore = "ROOT allocated official sqld and explicit migration helper gates required"]
    async fn reset_known_populated_prefix11_reaches_blank_on_fresh_stream() {
        let (driver, root, backend) = protocol_fixture().await;
        super::super::migrate::turso_test_apply_prefix(&backend, 11)
            .await
            .unwrap();
        migration_populate(&backend, uuid::Uuid::now_v7())
            .await
            .unwrap();
        let outcome = reset_transaction(&backend).await;
        assert_eq!(
            outcome,
            ResetOutcome {
                primary: "OK",
                rollback: "NOT_STARTED",
                commit: "RETURNED_OK",
                blank: "CONFIRMED",
                steps: 126
            }
        );
        assert!(migration_snapshot(&backend, 0)
            .await
            .unwrap()
            .receipts
            .is_empty());
        close_migration_owner(&backend).await.unwrap();
        assert_eq!(backend.connection_stats().unwrap().size, 0);
        finish_fixture(driver, root).await;
    }

    #[tokio::test]
    #[ignore = "ROOT allocated official sqld and explicit migration helper gates required"]
    async fn reset_unexpected_catalog_refuses_without_drops_and_healthy_reset_progresses() {
        let (driver, root, backend) = protocol_fixture().await;
        super::super::migrate::turso_test_apply_prefix(&backend, 11)
            .await
            .unwrap();
        let workspace = uuid::Uuid::now_v7();
        migration_populate(&backend, workspace).await.unwrap();
        let before = migration_snapshot(&backend, 11).await.unwrap();
        let mut tx = backend.begin_write().await.unwrap();
        remote_family(&mut tx)
            .unwrap()
            .execute(
                "CREATE TABLE reset_unexpected_object (id INTEGER PRIMARY KEY) STRICT",
                &[],
            )
            .await
            .unwrap();
        tx.commit_with_cleanup().await.unwrap();
        let refused = reset_transaction(&backend).await;
        assert_eq!(refused.primary, "RESET_SCHEMA_REFUSED");
        assert_eq!(refused.steps, 0);
        assert_eq!(refused.commit, "NOT_STARTED");
        assert_eq!(refused.rollback, "RETURNED_OK");
        let mut tx = backend.begin_write().await.unwrap();
        same_stream_readback(&mut tx).await.unwrap();
        let rows = remote_family(&mut tx).unwrap().query("SELECT count(*) FROM sqlite_schema WHERE type='table' AND name='reset_unexpected_object'", &[]).await.unwrap();
        assert_eq!(rows[0].cell(0).unwrap(), Cell::Integer(1));
        let rows = remote_family(&mut tx)
            .unwrap()
            .query(
                "SELECT count(*) FROM workspaces WHERE id=?1",
                &[Cell::uuid(workspace)],
            )
            .await
            .unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].cell(0).unwrap(), Cell::Integer(1));
        let rows = remote_family(&mut tx)
            .unwrap()
            .query(
                "SELECT next_fence FROM collab_fence_counter WHERE id=1",
                &[],
            )
            .await
            .unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].cell(0).unwrap(), Cell::Integer(17));
        remote_family(&mut tx)
            .unwrap()
            .execute("DROP TABLE reset_unexpected_object", &[])
            .await
            .unwrap();
        tx.commit_with_cleanup().await.unwrap();
        assert_eq!(migration_snapshot(&backend, 11).await.unwrap(), before);
        assert_eq!(reset_transaction(&backend).await.primary, "OK");
        close_migration_owner(&backend).await.unwrap();
        finish_fixture(driver, root).await;
    }

    #[tokio::test]
    #[ignore = "ROOT allocated official sqld and explicit migration helper gates required"]
    async fn reset_lost_commit_reply_remains_unknown_without_fresh_blank_observer() {
        let (driver, root, backend) = protocol_fixture().await;
        super::super::migrate::turso_test_apply_prefix(&backend, 11)
            .await
            .unwrap();
        let gate = driver.arm_commit_response_loss();
        let reset = reset_transaction(&backend);
        tokio::pin!(reset);
        tokio::select! {
            outcome = &mut reset => panic!("reset returned before held actual commit response: {outcome:?}"),
            response = gate.wait_upstream_response() => response.unwrap(),
        }
        gate.release_lost_reply();
        let outcome = reset.await;
        assert_eq!(outcome.primary, "COMMIT_UNCONFIRMED");
        assert_eq!(outcome.commit, "UNCONFIRMED");
        assert_eq!(outcome.blank, "NOT_RUN");
        assert_eq!(outcome.rollback, "NOT_STARTED");
        assert_eq!(outcome.steps, 126);
        assert!(close_migration_owner(&backend).await.is_err());
        assert_eq!(backend.connection_stats().unwrap().size, 0);
        let exchanges = driver.exchanges();
        let commit = exchanges
            .iter()
            .rposition(|exchange| exchange.has_commit)
            .unwrap();
        assert!(exchanges[commit].reply_lost);
        assert_eq!(exchanges[commit].upstream_status, Some(200));
        // Local cleanup may Close. It must not execute a new blank SELECT or
        // ROLLBACK as though the hidden original finish reply were confirmed.
        for exchange in &exchanges[commit + 1..] {
            let request = serde_json::to_string(&exchange.request).unwrap();
            assert!(!request.contains("SELECT") && !request.contains("ROLLBACK"));
        }
        finish_fixture(driver, root).await;
    }

    // Diagnostic categories never participate in the original FK acceptance.
    // Unknown numeric/code values are not reflected, nor are SDK messages.
    #[derive(Clone, Copy, Debug, PartialEq, Eq)]
    struct FkControlObservation {
        kind: &'static str,
        primary: &'static str,
        extended: &'static str,
        hrana: &'static str,
    }

    impl FkControlObservation {
        const NOT_OBSERVED: Self = Self {
            kind: "NOT_OBSERVED",
            primary: "NOT_PRESENT",
            extended: "NOT_PRESENT",
            hrana: "NOT_PRESENT",
        };
    }

    fn fk_control_numeric(code: i32) -> &'static str {
        match code {
            19 => "19",
            787 => "787",
            _ => "UNKNOWN",
        }
    }

    fn fk_control_hrana(code: Option<&str>) -> &'static str {
        match code {
            None => "NOT_PRESENT",
            Some("SQLITE_CONSTRAINT_FOREIGNKEY") => "SQLITE_CONSTRAINT_FOREIGNKEY",
            Some("SQLITE_CONSTRAINT") => "SQLITE_CONSTRAINT",
            Some("SQLITE_CONSTRAINT_CHECK") => "SQLITE_CONSTRAINT_CHECK",
            Some("SQLITE_CONSTRAINT_DATATYPE") => "SQLITE_CONSTRAINT_DATATYPE",
            Some("SQLITE_CONSTRAINT_UNIQUE") => "SQLITE_CONSTRAINT_UNIQUE",
            Some("SQLITE_MISMATCH") => "SQLITE_MISMATCH",
            Some("SQLITE_ERROR") => "SQLITE_ERROR",
            Some(_) => "UNKNOWN",
        }
    }

    /// Filesystem lifetime of the local control run directory. The fixture only
    /// closes its process and ports; the caller owns the directory. It is removed
    /// only after a fully confirmed control (every receipt OK and the fixture
    /// transport reaped and closed); any failure or a live transport keeps it as
    /// evidence, so a residue after success is itself a failure.
    fn local_control_residue_policy(control_ok: bool, fixture_confirmed: bool) -> &'static str {
        if control_ok && fixture_confirmed {
            "REMOVE"
        } else {
            "KEEP"
        }
    }

    fn fk_control_observe(error: &sqlx::Error) -> FkControlObservation {
        let mut observation = FkControlObservation::NOT_OBSERVED;
        let sqlx::Error::AnyDriverError(original) = error else {
            observation.kind = "SQLX_OTHER";
            return observation;
        };
        match original.downcast_ref::<libsql::Error>() {
            None => observation.kind = "NON_LIBSQL",
            Some(libsql::Error::SqliteFailure(code, _)) => {
                observation.kind = "SQLITE";
                observation.primary = fk_control_numeric(*code);
            }
            Some(libsql::Error::RemoteSqliteFailure(primary, extended, _)) => {
                observation.kind = "REMOTE_SQLITE";
                observation.primary = fk_control_numeric(*primary);
                observation.extended = fk_control_numeric(*extended);
            }
            Some(error @ libsql::Error::Hrana(_)) => {
                observation.kind = "HRANA";
                observation.hrana = fk_control_hrana(error.hrana_error_code());
            }
            Some(_) => observation.kind = "LIBSQL_OTHER",
        }
        observation
    }

    #[test]
    fn fk_control_observation_refuses_unknown_codes_and_private_message_decoys() {
        assert_eq!(fk_control_numeric(19), "19");
        assert_eq!(fk_control_numeric(787), "787");
        for code in [i32::MIN, -1, 0, 1, 275, 788, i32::MAX] {
            assert_eq!(fk_control_numeric(code), "UNKNOWN");
        }
        for code in [
            "",
            "SQLITE_CONSTRAINT_FOREIGNKEY private suffix",
            "sqlite_constraint_foreignkey",
            "private-token https://private.invalid/秘密",
        ] {
            assert_eq!(fk_control_hrana(Some(code)), "UNKNOWN");
        }
        assert_eq!(fk_control_hrana(None), "NOT_PRESENT");
        for code in [
            "SQLITE_CONSTRAINT_FOREIGNKEY",
            "SQLITE_CONSTRAINT",
            "SQLITE_CONSTRAINT_CHECK",
            "SQLITE_CONSTRAINT_DATATYPE",
            "SQLITE_CONSTRAINT_UNIQUE",
            "SQLITE_MISMATCH",
            "SQLITE_ERROR",
        ] {
            assert_eq!(fk_control_hrana(Some(code)), code);
        }
        let generic = sqlx::Error::AnyDriverError(Box::new(libsql::Error::SqliteFailure(
            19,
            "SQLITE_CONSTRAINT_FOREIGNKEY private-token".into(),
        )));
        assert_eq!(genuine_remote_fk_failure(&generic), Ok(false));
        assert_eq!(fk_control_observe(&generic).primary, "19");
        assert_eq!(fk_control_observe(&generic).hrana, "NOT_PRESENT");
        for error in [
            sqlx::Error::AnyDriverError(Box::new(libsql::Error::Hrana(Box::new(
                std::io::Error::other("SQLITE_CONSTRAINT_FOREIGNKEY private-token"),
            )))),
            sqlx::Error::AnyDriverError(Box::new(libsql::Error::Hrana(Box::new(
                libsql::Error::SqliteFailure(787, "nested private-token".into()),
            )))),
        ] {
            assert_eq!(genuine_remote_fk_failure(&error), Ok(false));
            assert_eq!(fk_control_observe(&error).kind, "HRANA");
            assert_eq!(fk_control_observe(&error).primary, "NOT_PRESENT");
            assert_eq!(fk_control_observe(&error).hrana, "NOT_PRESENT");
        }
        let arbitrary = sqlx::Error::AnyDriverError(Box::new(std::io::Error::other(
            "SQLITE_CONSTRAINT_FOREIGNKEY private-token",
        )));
        assert_eq!(genuine_remote_fk_failure(&arbitrary), Ok(false));
        assert_eq!(fk_control_observe(&arbitrary).kind, "NON_LIBSQL");
        assert_eq!(
            fk_control_observe(&sqlx::Error::PoolTimedOut).kind,
            "SQLX_OTHER"
        );
        let remote = sqlx::Error::AnyDriverError(Box::new(libsql::Error::RemoteSqliteFailure(
            19,
            787,
            "private-token".into(),
        )));
        assert_eq!(genuine_remote_fk_failure(&remote), Ok(true));
        assert_eq!(
            fk_control_observe(&remote),
            FkControlObservation {
                kind: "REMOTE_SQLITE",
                primary: "19",
                extended: "787",
                hrana: "NOT_PRESENT",
            }
        );
        let unknown = sqlx::Error::AnyDriverError(Box::new(libsql::Error::RemoteSqliteFailure(
            i32::MAX,
            275,
            "SQLITE_CONSTRAINT_FOREIGNKEY private-token".into(),
        )));
        assert_eq!(genuine_remote_fk_failure(&unknown), Ok(false));
        assert_eq!(fk_control_observe(&unknown).primary, "UNKNOWN");
        assert_eq!(fk_control_observe(&unknown).extended, "UNKNOWN");
        assert!(!format!("{:?}", fk_control_observe(&unknown)).contains("private-token"));

        // The same-writer witness admission: no numeric, remote, arbitrary-box,
        // code-less Hrana or non-driver error is ever a primary-only Hrana
        // SQLITE_CONSTRAINT (a coded Hrana error is private to the SDK; the
        // pinned-sqld body exercises that path).
        for error in [
            generic,
            sqlx::Error::AnyDriverError(Box::new(libsql::Error::SqliteFailure(
                787,
                "SQLITE_CONSTRAINT".into(),
            ))),
            remote,
            unknown,
            arbitrary,
            sqlx::Error::AnyDriverError(Box::new(libsql::Error::Hrana(Box::new(
                std::io::Error::other("SQLITE_CONSTRAINT"),
            )))),
            sqlx::Error::PoolTimedOut,
        ] {
            assert!(!primary_only_hrana_constraint(&error));
        }

        // Run-directory lifetime: removal only after a fully confirmed control; a
        // failed control or a live/unconfirmed transport always keeps the evidence.
        assert_eq!(local_control_residue_policy(true, true), "REMOVE");
        for (control_ok, fixture_confirmed) in [(false, true), (true, false), (false, false)] {
            assert_eq!(
                local_control_residue_policy(control_ok, fixture_confirmed),
                "KEEP"
            );
        }

        // The pure witness mapping: exactly one fence row against the composite
        // tasks key through foreign key 0, and exactly that key mapping.
        fn check(table: &str, rowid: Cell, parent: &str, fkid: i64) -> Vec<Cell> {
            vec![
                Cell::text(table),
                rowid,
                Cell::text(parent),
                Cell::Integer(fkid),
            ]
        }
        fn key(id: i64, seq: i64, table: &str, from: &str, to: &str) -> Vec<Cell> {
            vec![
                Cell::Integer(id),
                Cell::Integer(seq),
                Cell::text(table),
                Cell::text(from),
                Cell::text(to),
                Cell::text("NO ACTION"),
                Cell::text("CASCADE"),
                Cell::text("NONE"),
            ]
        }
        let good_check = [check(FENCE_TABLE, Cell::Integer(9), "tasks", 0)];
        let good_list = [
            key(0, 0, "tasks", "workspace_id", "workspace_id"),
            key(0, 1, "tasks", "task_id", "id"),
        ];
        assert_eq!(fk_witness_mapping(9, &good_check, &good_list), Ok(()));
        assert_eq!(
            fk_witness_mapping(8, &good_check, &good_list),
            Err("WITNESS_MISMATCH")
        );
        for bad_check in [
            vec![],
            vec![
                check(FENCE_TABLE, Cell::Integer(9), "tasks", 0),
                check(FENCE_TABLE, Cell::Integer(10), "tasks", 0),
            ],
            vec![check("documents", Cell::Integer(9), "tasks", 0)],
            vec![check(FENCE_TABLE, Cell::Integer(9), "projects", 0)],
            vec![check(FENCE_TABLE, Cell::Integer(9), "tasks", 1)],
            vec![check(FENCE_TABLE, Cell::text("9"), "tasks", 0)],
            vec![check(FENCE_TABLE, Cell::Null, "tasks", 0)],
            vec![vec![
                Cell::text(FENCE_TABLE),
                Cell::Integer(9),
                Cell::text("tasks"),
            ]],
        ] {
            assert_eq!(
                fk_witness_mapping(9, &bad_check, &good_list),
                Err("WITNESS_MISMATCH")
            );
        }
        for bad_list in [
            vec![],
            vec![key(0, 0, "tasks", "workspace_id", "workspace_id")],
            vec![
                key(0, 0, "tasks", "task_id", "id"),
                key(0, 1, "tasks", "workspace_id", "workspace_id"),
            ],
            vec![
                key(0, 0, "tasks", "workspace_id", "workspace_id"),
                key(0, 1, "tasks", "task_id", "task_id"),
            ],
            vec![
                key(0, 0, "projects", "workspace_id", "workspace_id"),
                key(0, 1, "projects", "task_id", "id"),
            ],
            vec![
                key(1, 0, "tasks", "workspace_id", "workspace_id"),
                key(1, 1, "tasks", "task_id", "id"),
            ],
            vec![
                key(0, 0, "tasks", "workspace_id", "workspace_id"),
                key(0, 1, "tasks", "task_id", "id"),
                key(1, 0, "projects", "workspace_id", "workspace_id"),
            ],
            vec![
                vec![Cell::Integer(0), Cell::Integer(0), Cell::text("tasks")],
                good_list[1].clone(),
            ],
        ] {
            assert_eq!(
                fk_witness_mapping(9, &good_check, &bad_list),
                Err("WITNESS_MISMATCH")
            );
        }
    }

    #[tokio::test]
    #[ignore = "ROOT allocated pinned official sqld and explicit migration helper gates required"]
    async fn migration_fk_rollback_real_sqld_prefix11_control() -> Result<(), &'static str> {
        const NAME: &str =
            "db::turso_test::reset_policy_tests::migration_fk_rollback_real_sqld_prefix11_control";
        let args: Vec<String> = std::env::args().collect();
        if ![NAME, "--exact", "--ignored", "--test-threads=1"]
            .iter()
            .all(|required| args.iter().filter(|arg| arg.as_str() == *required).count() == 1)
        {
            return Err("EXPLICIT_LOCAL_FK_CONTROL_SELECTION_REQUIRED");
        }
        for (name, expected) in [
            ("FVOCI_TEST_TURSO_MIGRATION_SELECTED", "1"),
            ("FVOCI_TEST_TURSO_PHASE", "migration"),
            ("FVOCI_TEST_TURSO_DESTRUCTIVE", "true"),
            ("FVOCI_TEST_TURSO_ALLOW_DESTRUCTIVE", "true"),
        ] {
            if std::env::var(name).as_deref() != Ok(expected) {
                return Err("EXPLICIT_LOCAL_FK_CONTROL_SELECTION_REQUIRED");
            }
        }
        let sqld = std::path::PathBuf::from(
            std::env::var_os("FVOCI_TEST_SQLD").ok_or("PINNED_SQLD_REQUIRED")?,
        );
        let root = std::env::temp_dir().join(format!("fvoci-fk-control-{}", uuid::Uuid::now_v7()));
        let driver = crate::db::libsql_finish_fixture::LibsqlFinishFixture::start(&sqld, &root)
            .await
            .map_err(|_| "FIXTURE_START_FAILED")?;
        let database = match driver.database().await {
            Ok(database) => database,
            Err(_) => {
                driver
                    .finish()
                    .await
                    .map_err(|_| "FIXTURE_FINISH_UNCONFIRMED")?;
                return Err("FIXTURE_DATABASE_FAILED");
            }
        };
        let backend = Backend::LibsqlRemote(std::sync::Arc::new(RemoteDatabase::from_test_driver(
            database,
            std::num::NonZeroU32::MIN,
        )));
        let mut observation = FkControlObservation::NOT_OBSERVED;
        let mut strict_fk = "NOT_OBSERVED";
        let mut causal_witness = "NOT_RUN";
        let mut check_negative = "NOT_RUN";
        let mut pk_negative = "NOT_RUN";
        let mut rollback = "NOT_STARTED";
        let mut preserved = "NOT_RUN";
        let mut healthy = "NOT_RUN";
        let primary = async {
            super::super::migrate::turso_test_apply_prefix(&backend, 11)
                .await.map_err(|_| "PREFIX_APPLY_FAILED")?;
            let workspace = uuid::Uuid::now_v7();
            migration_populate(&backend, workspace).await?;
            let before = migration_snapshot(&backend, 11).await?;
            let steps = super::super::migrate::compiled_sqlite_steps();
            if steps.len() != 12 || steps[11].version != 12 {
                return Err("CURRENT_LINEAGE_CHANGED");
            }
            let mut tx = backend.begin_write().await.map_err(|_| "BEGIN_FAILED")?;
            let mut original_fk_error = None;
            let statement = async {
                same_stream_readback(&mut tx).await?;
                let family = remote_family(&mut tx)?;
                super::super::migrate::turso_test_schema_in_writer(family, 11)
                    .await.map_err(|_| "PREFIX_VALIDATION_FAILED")?;
                family.apply_migration_batch(steps[11].sql).await.map_err(|_| "DDL_FAILED")?;
                let cells = [Cell::uuid(workspace), Cell::uuid(uuid::Uuid::now_v7()), Cell::uuid(uuid::Uuid::now_v7())];
                migration_fk_baseline(family, &cells).await?;
                original_fk_error = family.execute(MIGRATION_UNKNOWN_TASK_FENCE_SQL, &cells).await.err();
                let error = original_fk_error.as_ref().ok_or("FK_FAILURE_MISSING")?;
                observation = fk_control_observe(error);
                // strict_fk reports the unchanged classifier only; a primary-only server
                // keeps REFUSED, and the separate causal witness is the only other proof.
                if genuine_remote_fk_failure(error)? {
                    strict_fk = "EXPECTED";
                    causal_witness = "NOT_NEEDED";
                    return Ok(());
                }
                strict_fk = "REFUSED";
                if !primary_only_hrana_constraint(error) {
                    causal_witness = "NOT_ADMITTED";
                    return Err("WRONG_FK_FAILURE");
                }
                match migration_fk_witness(family, &cells).await {
                    Ok(()) => {
                        causal_witness = "PROVEN";
                        Ok(())
                    }
                    Err(code) => {
                        causal_witness = code;
                        Err(code)
                    }
                }
            }.await;
            // Keep the exact typed statement error alive through original finish.
            let original_rollback = tx.rollback().await;
            drop(original_fk_error);
            rollback = if original_rollback.is_ok() { "RETURNED_OK" } else { "UNCONFIRMED" };
            original_rollback.map_err(|_| "ROLLBACK_UNCONFIRMED")?;

            // A rejected classification remains failure even when all preservation
            // and healthy writer controls pass. No observer follows uncertain finish.
            let preservation = async {
                if migration_snapshot(&backend, 11).await? != before {
                    return Err("FK_ROLLBACK_PREFIX_CHANGED");
                }
                migration_preserved_data(&backend, workspace, 11, 0).await
            }.await;
            preserved = if preservation.is_ok() { "OK" } else { "FAILED" };
            preservation?;

            // Real unrelated-constraint controls on one further writer, rollback only:
            // the identical canonical statement reports the same primary-only code when
            // a CHECK (15-byte owner_token) or the composite PRIMARY KEY (the same cells
            // inserted twice) fires, and the same-writer witness must refuse both.
            let mut tx = backend.begin_write().await.map_err(|_| "BEGIN_FAILED")?;
            let negatives = async {
                same_stream_readback(&mut tx).await?;
                let family = remote_family(&mut tx)?;
                super::super::migrate::turso_test_schema_in_writer(family, 11)
                    .await.map_err(|_| "PREFIX_VALIDATION_FAILED")?;
                let short = [Cell::uuid(workspace), Cell::uuid(uuid::Uuid::now_v7()), Cell::Blob(vec![0x15; 15])];
                migration_fk_baseline(family, &short).await?;
                check_negative = migration_fk_negative(family, &short).await;
                if check_negative != "REFUSED" { return Err("CHECK_NEGATIVE_NOT_REFUSED"); }
                family.execute("PRAGMA defer_foreign_keys=OFF", &[]).await.map_err(|_| "DEFER_PRAGMA_REFUSED")?;
                let twice = [Cell::uuid(workspace), Cell::uuid(uuid::Uuid::now_v7()), Cell::uuid(uuid::Uuid::now_v7())];
                migration_fk_baseline(family, &twice).await?;
                family.execute("PRAGMA defer_foreign_keys=ON", &[]).await.map_err(|_| "DEFER_PRAGMA_REFUSED")?;
                if family.execute(MIGRATION_UNKNOWN_TASK_FENCE_SQL, &twice).await.map_err(|_| "NEGATIVE_SETUP_FAILED")? != 1 {
                    return Err("NEGATIVE_SETUP_FAILED");
                }
                family.execute("PRAGMA defer_foreign_keys=OFF", &[]).await.map_err(|_| "DEFER_PRAGMA_REFUSED")?;
                pk_negative = migration_fk_negative(family, &twice).await;
                if pk_negative != "REFUSED" { return Err("PK_NEGATIVE_NOT_REFUSED"); }
                Ok(())
            }.await;
            let finish = tx.rollback().await;
            finish.map_err(|_| "ROLLBACK_UNCONFIRMED")?;
            negatives?;
            if migration_snapshot(&backend, 11).await? != before {
                return Err("NEGATIVE_ROLLBACK_PREFIX_CHANGED");
            }
            migration_preserved_data(&backend, workspace, 11, 0).await?;
            let mut tx = backend.begin_write().await.map_err(|_| "BEGIN_FAILED")?;
            healthy = "FAILED";
            let progress = async {
                same_stream_readback(&mut tx).await?;
                let family = remote_family(&mut tx)?;
                super::super::migrate::turso_test_schema_in_writer(family, 11)
                    .await.map_err(|_| "PREFIX_VALIDATION_FAILED")?;
                if family.execute("UPDATE collab_fence_counter SET next_fence=18 WHERE id=1 AND next_fence=17", &[])
                    .await.map_err(|_| "HEALTHY_WRITE_FAILED")? != 1 {
                    return Err("HEALTHY_WRITE_MISMATCH");
                }
                let rows = family.query("SELECT next_fence FROM collab_fence_counter WHERE id=1", &[])
                    .await.map_err(|_| "HEALTHY_READ_FAILED")?;
                if rows.len() != 1 || rows[0].cell(0).map_err(|_| "HEALTHY_DECODE_FAILED")? != Cell::Integer(18) {
                    return Err("HEALTHY_READ_MISMATCH");
                }
                Ok(())
            }.await;
            let finish = tx.rollback().await;
            finish.map_err(|_| "ROLLBACK_UNCONFIRMED")?;
            progress?;
            migration_preserved_data(&backend, workspace, 11, 0).await?;
            if migration_snapshot(&backend, 11).await? != before {
                return Err("HEALTHY_ROLLBACK_PREFIX_CHANGED");
            }
            healthy = "OK";
            statement
        }.await;
        let close = backend.close().await;
        let leases_zero = backend
            .connection_stats()
            .is_ok_and(|stats| stats.size == 0);
        let fixture = driver.finish().await;
        let fixture_confirmed = fixture.as_ref().is_ok_and(|receipt| {
            receipt.sqld_reaped
                && receipt.proxy_joined
                && receipt.upstream_closed
                && receipt.proxy_closed
        });
        // Run-directory lifetime: a fully confirmed control removes its own
        // directory (the fixture finish is process/port closure, not filesystem
        // cleanup); any failure or an unconfirmed transport keeps it as evidence.
        let control_ok = primary.is_ok() && close.is_ok() && leases_zero;
        let residue = match local_control_residue_policy(control_ok, fixture_confirmed) {
            "REMOVE" => match std::fs::remove_dir_all(&root) {
                Ok(()) if !root.exists() => "REMOVED",
                _ => "REMOVAL_FAILED",
            },
            _ => "KEPT",
        };
        // Only closed categories are printed, after cleanup; never print raw
        // exchanges or SDK error text.
        println!("FVOCI_LOCAL_MIGRATION_FK_CONTROL kind={} primary_code={} extended_code={} hrana_code={} strict_fk={} causal_witness={} check_negative={} pk_negative={} rollback={} preserved={} healthy={} close={} leases={} fixture={} residue={}",
            observation.kind, observation.primary, observation.extended, observation.hrana,
            strict_fk, causal_witness, check_negative, pk_negative, rollback, preserved, healthy,
            if close.is_ok() { "OK" } else { "UNCONFIRMED" },
            if leases_zero { "ZERO" } else { "FAILED" },
            if fixture_confirmed { "OK" } else { "UNCONFIRMED" },
            residue);
        primary?;
        if close.is_err() || !leases_zero || !fixture_confirmed {
            return Err("LOCAL_CONTROL_CLEANUP_UNCONFIRMED");
        }
        if residue != "REMOVED" {
            return Err("LOCAL_CONTROL_RESIDUE_NOT_REMOVED");
        }
        Ok(())
    }
}
