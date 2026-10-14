//! The allocated observer's fixed, credential-free stdout contract.
use fvoci_server::db::{
    backend::{Backend, RemoteDatabase},
    keycloak_fixture::{observe, Observation},
};
use serde_json::json;
use sqlx::{
    postgres::PgPoolOptions,
    sqlite::{SqliteConnectOptions, SqlitePoolOptions},
};
use std::io::Read;

fn required(key: &str) -> Result<String, &'static str> {
    std::env::var(key)
        .ok()
        .filter(|v| !v.is_empty())
        .ok_or("KEYCLOAK_OBSERVER_INPUT")
}

pub async fn run() -> Result<(), &'static str> {
    if required("FVOCI_KC_OBSERVER_SELECTED")? != "1" || std::env::args().len() != 2 {
        return Err("KEYCLOAK_OBSERVER_ALLOCATION_REQUIRED");
    }
    let mut bytes = Vec::new();
    std::io::stdin()
        .take(4097)
        .read_to_end(&mut bytes)
        .map_err(|_| "KEYCLOAK_OBSERVER_INPUT")?;
    if bytes.len() > 4096 {
        return Err("KEYCLOAK_OBSERVER_INPUT");
    }
    let input: Observation =
        serde_json::from_slice(&bytes).map_err(|_| "KEYCLOAK_OBSERVER_INPUT")?;
    input.validate().map_err(|_| "KEYCLOAK_OBSERVER_INPUT")?;
    let backend = match required("E2E_DATABASE_BACKEND")?.as_str() {
        "postgres" => Backend::Postgres(
            PgPoolOptions::new()
                .max_connections(1)
                .connect(&required("FVOCI_KC_OBSERVER_DATABASE_URL")?)
                .await
                .map_err(|_| "KEYCLOAK_OBSERVER_CONNECT")?,
        ),
        "sqlite" => Backend::Sqlite(
            SqlitePoolOptions::new()
                .max_connections(1)
                .connect_with(
                    SqliteConnectOptions::new()
                        .filename(required("FVOCI_SQLITE_PATH")?)
                        .read_only(true),
                )
                .await
                .map_err(|_| "KEYCLOAK_OBSERVER_CONNECT")?,
        ),
        "libsql-remote" => Backend::LibsqlRemote(
            RemoteDatabase::connect(
                required("FVOCI_LIBSQL_URL")?,
                required("FVOCI_LIBSQL_AUTH_TOKEN")?,
                1,
            )
            .await
            .map_err(|_| "KEYCLOAK_OBSERVER_CONNECT")?,
        ),
        _ => return Err("KEYCLOAK_OBSERVER_INPUT"),
    };
    let result = observe(&backend, &input).await;
    let closed = backend.close().await;
    let drained = closed.is_ok() && backend.connection_stats().is_ok_and(|s| s.size == 0);
    if result.is_err() || !drained {
        println!(
            "{}",
            json!({"value":null,"rollback":"unconfirmed","lifecycleDrain":if drained {"confirmed"} else {"unconfirmed"},"leases":backend.connection_stats().ok().map(|s|s.size)})
        );
        return Err("KEYCLOAK_OBSERVER_READ_OR_FINISH");
    }
    println!(
        "{}",
        json!({"value":result.unwrap(),"rollback":"confirmed","lifecycleDrain":"confirmed","leases":0})
    );
    Ok(())
}
