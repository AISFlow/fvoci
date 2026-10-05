//! Explicit manual connection probe, never part of automatic DB suites.
//! No schema/data mutation, migration, retry, or synthetic transport is used.
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
            println!("FVOCI_TURSO_RECEIPT primary=CONNECT_FAILED rollback=NOT_STARTED close=NOT_STARTED leases=NOT_OBSERVED");
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
