//! Explicit manual primary consumers, never part of automatic DB suites.
//! Connection is read-only; migration requires both explicit destructive gates.
//! No synthetic transport, reset, retry, or normal startup unlock is used.
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

async fn migration_fk_rollback(
    backend: &Backend,
    workspace: uuid::Uuid,
) -> Result<(), &'static str> {
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
        family.apply_migration_batch(steps[11].sql).await.map_err(|_| "DDL_FAILED")?;
        original_fk_error = family.execute(
            "INSERT INTO task_collab_room_fences(workspace_id,task_id,owner_token,fence,expires_at) VALUES(?1,?2,?3,17,1)",
            &[Cell::uuid(workspace), Cell::uuid(uuid::Uuid::now_v7()), Cell::uuid(uuid::Uuid::now_v7())],
        ).await.err();
        let error = original_fk_error.as_ref().ok_or("FK_FAILURE_MISSING")?;
        if !genuine_remote_fk_failure(error)? { return Err("WRONG_FK_FAILURE"); }
        Ok(())
    }.await;
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
        migration_fk_rollback(&backend, workspace).await?;
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
    println!(
        "FVOCI_TURSO_MIGRATION_RECEIPT primary={} prefix={} fk_rollback={} current={} restart={} close={} leases={}",
        if ok { "OK" } else { "FAILED" },
        if ok { "OK" } else { "NOT_CONFIRMED" },
        if ok { "OK" } else { "NOT_CONFIRMED" },
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
    // Same four maintained migration-helper gates; inventory performs no DDL
    // but reserves a writer, so it cannot use the connection-only admission.
    for (name, expected) in [
        ("FVOCI_TEST_TURSO_MIGRATION_SELECTED", "1"),
        ("FVOCI_TEST_TURSO_PHASE", "migration"),
        ("FVOCI_TEST_TURSO_DESTRUCTIVE", "true"),
        ("FVOCI_TEST_TURSO_ALLOW_DESTRUCTIVE", "true"),
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

fn inventory_disclosure(
    primary: &Result<TursoTargetInventory, &'static str>,
    rollback: Option<Result<(), &'static str>>,
    close: Result<(), &'static str>,
    leases_zero: bool,
) -> Option<(&'static str, u8, &str)> {
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
    // Both outcomes are retained; primary errors keep priority. A fresh state
    // observation never reconciles the old failed migration/finish receipt.
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
            ("FVOCI_TEST_TURSO_PHASE", "migration"),
            ("FVOCI_TEST_TURSO_DESTRUCTIVE", "true"),
            ("FVOCI_TEST_TURSO_ALLOW_DESTRUCTIVE", "true"),
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
            wrong[index].1 = if index == 1 { "connection" } else { "false" };
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
}
