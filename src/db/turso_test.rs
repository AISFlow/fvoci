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
    primary?;
    close
}
