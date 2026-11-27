//! Pre-migrated SQLite file template for tests.
//!
//! Test suites open hundreds of fresh file-backed SQLite stores via [`Store::connect`].
//! Running every migration under `SqliteSynchronous::Full` on each empty file repeats tens
//! of thousands of `fsync` calls across a run. Building one migrated, checkpointed SQLite
//! file per process and copying its bytes before [`Store::open`] when the target path does
//! not yet exist avoids repeating the schema DDL while keeping [`Store::migrate`]'s
//! `_sqlx_migrations` checksum and `SchemaState::Current` verification intact on every open.

use std::ffi::OsString;
use std::path::{Path, PathBuf};

use tokio::sync::OnceCell;

use crate::error::StoreError;
use crate::{DbType, Pool, Store};

static TEMPLATE: OnceCell<Vec<u8>> = OnceCell::const_new();

#[derive(Debug)]
struct TempFiles(PathBuf);

impl Drop for TempFiles {
    fn drop(&mut self) {
        for suffix in ["", "-wal", "-shm"] {
            let _ = std::fs::remove_file(with_suffix(&self.0, suffix));
        }
    }
}

fn with_suffix(path: &Path, suffix: &str) -> PathBuf {
    let mut os: OsString = path.as_os_str().to_os_string();
    os.push(suffix);
    PathBuf::from(os)
}

fn sidecar_exists(path: &Path) -> bool {
    ["-wal", "-shm"]
        .into_iter()
        .any(|suffix| with_suffix(path, suffix).exists())
}

async fn build_template() -> Result<Vec<u8>, StoreError> {
    let temp = TempFiles(std::env::temp_dir().join(format!(
        "fvoci-sqlite-seed-{}-{}.db",
        std::process::id(),
        fvoci_model::new_id(),
    )));
    let Some(connection) = temp.0.to_str() else {
        return Err(StoreError::Config(
            "temporary directory path is not valid UTF-8".to_owned(),
        ));
    };
    let store = Store::open(DbType::Sqlite, connection).await?;
    store.migrate().await?;
    if let Pool::Sqlite(pool) = &store.inner.pool {
        sqlx::query("PRAGMA wal_checkpoint(TRUNCATE)")
            .execute(pool)
            .await?;
    }
    store.close().await;
    std::fs::read(&temp.0)
        .map_err(|err| StoreError::Config(format!("reading SQLite seed template: {err}")))
}

fn write_fresh(path: &Path, bytes: &[u8]) {
    let staging = with_suffix(path, &format!(".seed-{}", fvoci_model::new_id()));
    if std::fs::write(&staging, bytes).is_ok() {
        let _ = std::fs::hard_link(&staging, path);
        let _ = std::fs::remove_file(&staging);
    }
}

/// Copies a migrated SQLite file to `connection`'s target path when `connection` names a
/// non-existent file inside an existing directory, leaving [`Store::open`] and
/// [`Store::migrate`] to open and verify it as usual.
pub(crate) async fn seed_if_fresh(connection: &str) {
    let Ok(settings) = crate::connect::sqlite_settings(connection) else {
        return;
    };
    if settings.in_memory {
        return;
    }
    let path = settings.options.get_filename();
    if path.exists() || sidecar_exists(path) || !path.parent().is_some_and(Path::is_dir) {
        return;
    }
    if let Ok(bytes) = TEMPLATE.get_or_try_init(build_template).await {
        write_fresh(path, bytes);
    }
}
