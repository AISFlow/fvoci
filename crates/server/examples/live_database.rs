//! A fresh PostgreSQL schema for the real-server browser tests, held until stdin closes.

use std::io::{Read as _, Write as _};
use std::path::PathBuf;

use anyhow::{Context as _, ensure};
use fvoci_store::DbType;
use fvoci_store::testing::{TestDatabase, db_type_of_the_tests};

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    ensure!(
        db_type_of_the_tests() == DbType::Postgres,
        "the organizations browser tests require PostgreSQL"
    );
    let directory = PathBuf::from(std::env::args().nth(1).context("missing data directory")?);
    // TestDatabase rejects superusers and BYPASSRLS roles before making the schema.
    let database = TestDatabase::create(&directory).await;
    let result = hold(&directory, &database).await;
    database.remove().await;
    result
}

async fn hold(directory: &std::path::Path, database: &TestDatabase) -> anyhow::Result<()> {
    std::fs::write(
        directory.join("database.json"),
        serde_json::to_vec(
            &serde_json::json!({"database_connection": database.database_connection()}),
        )?,
    )?;
    {
        let mut stdout = std::io::stdout().lock();
        writeln!(stdout, "database: ready")?;
        stdout.flush()?;
    }
    tokio::task::spawn_blocking(|| {
        let mut ignored = Vec::new();
        std::io::stdin().read_to_end(&mut ignored)
    })
    .await??;
    Ok(())
}
