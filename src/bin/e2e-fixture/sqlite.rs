//! Test-only post-setup actor fixture for an explicitly owned SQLite run.
//! This never creates/migrates/resets a DB or supplies a product auth route.
use std::os::unix::fs::{MetadataExt, OpenOptionsExt};
use std::path::{Path, PathBuf};

use fvoci_server::auth::password::{hash_password, Keyring};
use fvoci_server::db::{backend::Backend, e2e_fixture, migrate, pool};
use serde::Deserialize;
use serde_json::json;
use sqlx::Connection;
use std::io::Read;
use uuid::Uuid;

#[derive(Debug, thiserror::Error)]
pub enum FixtureError {
    #[error("{0}")]
    Input(String),
    #[error(transparent)]
    Database(#[from] sqlx::Error),
    #[error("fixture cleanup failed after {original:?}: {cleanup}")]
    Cleanup {
        original: Option<Box<FixtureError>>,
        #[source]
        cleanup: sqlx::Error,
    },
    #[error("fixture receipt failed: {0}")]
    Receipt(#[from] std::io::Error),
    #[error("fixture receipt failed after {original:?}: {receipt}")]
    ReceiptAfterFinish {
        original: Option<Box<FixtureError>>,
        #[source]
        receipt: std::io::Error,
    },
}

fn required(name: &str) -> Result<String, FixtureError> {
    std::env::var(name)
        .ok()
        .filter(|value| !value.trim().is_empty())
        .ok_or_else(|| FixtureError::Input(format!("{name} is required")))
}

#[derive(Debug, PartialEq, Eq)]
enum Mode {
    Actor,
    Baseline,
    Observe,
    SerialObserve,
}

fn mode(args: &[String]) -> Result<Mode, FixtureError> {
    match args {
        [] => Ok(Mode::Actor),
        [value] => match value.as_str() {
            "baseline" => Ok(Mode::Baseline),
            "observe" => Ok(Mode::Observe),
            "serial-observe" => Ok(Mode::SerialObserve),
            _ => Err(FixtureError::Input("SQLite fixture mode invalid".into())),
        },
        _ => Err(FixtureError::Input("SQLite fixture mode invalid".into())),
    }
}

pub async fn run() -> Result<(), FixtureError> {
    // Reject unknown modes before reading credentials or opening the target.
    // An observer invocation must never fall through to actor INSERTs.
    let selected = mode(&std::env::args().skip(1).collect::<Vec<_>>())?;
    match selected {
        Mode::Actor => create_user().await,
        mode => observe(mode).await,
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct NativeInput {
    workspace_id: Uuid,
    document_ids: Vec<Uuid>,
}

fn observation_input(
    mode: Mode,
    bytes: &[u8],
) -> Result<e2e_fixture::OwnedSqliteObservation, FixtureError> {
    let invalid = || FixtureError::Input("SQLite observer input invalid".into());
    match mode {
        Mode::Baseline if bytes.is_empty() => Ok(e2e_fixture::OwnedSqliteObservation::Baseline),
        Mode::Observe if bytes.len() <= 16384 => {
            let input: NativeInput = serde_json::from_slice(bytes).map_err(|_| invalid())?;
            if input.document_ids.is_empty()
                || input.document_ids.len() > 100
                || input
                    .document_ids
                    .iter()
                    .collect::<std::collections::BTreeSet<_>>()
                    .len()
                    != input.document_ids.len()
            {
                return Err(invalid());
            }
            Ok(e2e_fixture::OwnedSqliteObservation::Native {
                workspace: input.workspace_id,
                ids: input.document_ids,
            })
        }
        Mode::SerialObserve if bytes.len() <= 4096 => serde_json::from_slice(bytes)
            .map(e2e_fixture::OwnedSqliteObservation::Serial)
            .map_err(|_| invalid()),
        _ => Err(invalid()),
    }
}

async fn observe(mode: Mode) -> Result<(), FixtureError> {
    if required("FVOCI_E2E_SQLITE_OBSERVER_SELECTED")? != "1" {
        return Err(FixtureError::Input(
            "SQLite observer allocation required".into(),
        ));
    }
    let mut bytes = Vec::new();
    std::io::stdin().take(16385).read_to_end(&mut bytes)?;
    let input = observation_input(mode, &bytes)?;
    let owned = OwnedFile::check(
        Path::new(&required("FVOCI_E2E_SQLITE_RUN_ROOT")?),
        Path::new(&required("FVOCI_E2E_SQLITE_PATH")?),
    )?;
    let pool = pool::connect_sqlite_app(&owned.path, 1).await?;
    let current = match migrate::assert_sqlite_schema_current(&Backend::Sqlite(pool.clone())).await
    {
        Ok(current) => current,
        Err(error) => {
            pool.close().await;
            return Err(FixtureError::Database(error));
        }
    };
    let mut connection = match pool.acquire().await {
        Ok(connection) => connection.detach(),
        Err(error) => {
            pool.close().await;
            return Err(FixtureError::Database(error));
        }
    };
    pool.close().await;
    // The detached connection owns every observation read, explicit rollback
    // and actual awaited native shutdown; no pool replacement proves close.
    let result = e2e_fixture::observe_owned_sqlite(&mut connection, &current, input).await;
    let close = connection.close().await;
    let mut value = result.as_ref().cloned().unwrap_or_else(|error| {
        json!({
            "originalFailure":"UI_SQLITE_OBSERVATION_FAILED",
            "nativeOutcome":e2e_fixture::failure_receipt(error),
        })
    });
    if result.is_ok() {
        value["nativeOutcome"] =
            json!({"operation":"confirmed","rollback":"confirmed","commit":"not-attempted"});
    }
    value["connectionClose"] = json!(if close.is_ok() { "confirmed" } else { "failed" });
    value["lifecycleDrain"] = json!(if close.is_ok() {
        "confirmed"
    } else {
        "unconfirmed"
    });
    value["leases"] = json!(0);
    value["serverCloseReceipt"] = json!("not-exposed-by-sdk");
    println!("{value}");
    match (result, close) {
        (Ok(_), Ok(())) => Ok(()),
        (Err(error), Ok(())) => Err(FixtureError::Database(error)),
        (original, Err(cleanup)) => Err(FixtureError::Cleanup {
            original: original
                .err()
                .map(|error| Box::new(FixtureError::Database(error))),
            cleanup,
        }),
    }
}

struct OwnedFile {
    root: PathBuf,
    path: PathBuf,
}
impl OwnedFile {
    fn check(root: &Path, path: &Path) -> Result<Self, FixtureError> {
        if !root.is_absolute() || !path.is_absolute() || root.parent().is_none() {
            return Err(FixtureError::Input(
                "fixture requires an explicit absolute isolated run root and existing DB file"
                    .into(),
            ));
        }
        let root_meta = std::fs::symlink_metadata(root)?;
        let file_meta = std::fs::symlink_metadata(path)?;
        let own_uid = std::fs::metadata("/proc/self")?.uid();
        if !root_meta.is_dir()
            || root_meta.uid() != own_uid
            || root_meta.mode() & 0o077 != 0
            || !file_meta.is_file()
            || file_meta.uid() != own_uid
            || file_meta.nlink() != 1
        {
            return Err(FixtureError::Input(
                "fixture root must be private/owned and its DB an owned regular single-link file"
                    .into(),
            ));
        }
        let root = root.canonicalize()?;
        let path = path.canonicalize()?;
        if path.parent() != Some(root.as_path()) {
            return Err(FixtureError::Input(
                "fixture DB must be directly inside the isolated run root".into(),
            ));
        }
        Ok(Self { root, path })
    }
}

pub async fn create_user() -> Result<(), FixtureError> {
    let owned = OwnedFile::check(
        Path::new(&required("FVOCI_E2E_SQLITE_RUN_ROOT")?),
        Path::new(&required("FVOCI_E2E_SQLITE_PATH")?),
    )?;
    let email = required("E2E_USER_EMAIL")?;
    let password = required("E2E_USER_PASSWORD")?;
    let given_name = required("E2E_USER_GIVEN_NAME")?;
    let family_name = std::env::var("E2E_USER_FAMILY_NAME").ok();
    let workspace_slug = std::env::var("E2E_WORKSPACE_SLUG").ok();
    let role = std::env::var("E2E_MEMBERSHIP_ROLE").unwrap_or_else(|_| "member".into());
    if !matches!(role.as_str(), "owner" | "admin" | "member" | "guest") {
        return Err(FixtureError::Input("E2E_MEMBERSHIP_ROLE is invalid".into()));
    }
    let keys = Keyring::parse(
        &required("PASSWORD_PEPPER_KEYS")?,
        &required("PASSWORD_PEPPER_ACTIVE_KEY_ID")?,
    )
    .map_err(FixtureError::Input)?;
    let hash = hash_password(&password, &keys)
        .await
        .map_err(FixtureError::Input)?;
    let pool = pool::connect_sqlite_app(&owned.path, 1).await?;
    // Use the exact current compiled schema, not an operator URL or new file.
    if let Err(error) = migrate::assert_sqlite_schema_current(&Backend::Sqlite(pool.clone())).await
    {
        pool.close().await;
        return Err(FixtureError::Database(error));
    }
    // Detach this actual max-one connection and close the pool. Afterwards
    // every fixture read/write/finish and the awaited native close belong to
    // this connection; no replacement can fabricate a shutdown receipt.
    let mut connection = match pool.acquire().await {
        Ok(connection) => connection.detach(),
        Err(error) => {
            pool.close().await;
            return Err(FixtureError::Database(error));
        }
    };
    pool.close().await;
    let user = Uuid::now_v7();
    let mut commit_state = "not-attempted";
    let result: Result<Option<Uuid>, FixtureError> = async {
        let runtime: (String, String, i64) = sqlx::query_as(
            "SELECT sqlite_version(),sqlite_source_id(),(SELECT foreign_keys FROM pragma_foreign_keys)")
            .fetch_one(&mut connection).await?;
        if runtime.0 != pool::SQLITE_VERSION || runtime.1 != pool::SQLITE_SOURCE_ID || runtime.2 != 1 {
            return Err(FixtureError::Input("fixture actual runtime/FK differs from supported pin".into()));
        }
        let mut tx = connection.begin_with("BEGIN IMMEDIATE").await?;
        let write: Result<Option<Uuid>, FixtureError> = async {
            let users: i64 = sqlx::query_scalar("SELECT count(*) FROM users WHERE deleted_at IS NULL")
                .fetch_one(&mut *tx).await?;
            if users == 0 {
                return Err(FixtureError::Input("fixture requires completed real Vue setup before adding an actor".into()));
            }
            let workspace = match workspace_slug {
                Some(slug) => {
                    let raw: Option<Vec<u8>> = sqlx::query_scalar("SELECT id FROM workspaces WHERE slug=?1 AND deleted_at IS NULL")
                        .bind(slug).fetch_optional(&mut *tx).await?;
                    let raw = raw.ok_or_else(|| FixtureError::Input("fixture target workspace missing".into()))?;
                    Some(Uuid::from_slice(&raw).map_err(|_| FixtureError::Input("fixture target UUID bytes corrupt".into()))?)
                }
                None => None,
            };
            sqlx::query("INSERT INTO users(id,email,password_hash,given_name,family_name) VALUES(?1,?2,?3,?4,?5)")
                .bind(user.as_bytes().as_slice()).bind(&email).bind(&hash).bind(&given_name).bind(family_name)
                .execute(&mut *tx).await?;
            if let Some(workspace) = workspace {
                sqlx::query("INSERT INTO memberships(workspace_id,user_id,role) VALUES(?1,?2,?3)")
                    .bind(workspace.as_bytes().as_slice()).bind(user.as_bytes().as_slice()).bind(role)
                    .execute(&mut *tx).await?;
            }
            Ok(workspace)
        }.await;
        match write {
            Ok(workspace) => {
                commit_state = "unknown";
                tx.commit().await?;
                commit_state = "confirmed";
                Ok(workspace)
            }
            Err(original) => match tx.rollback().await {
                Ok(()) => Err(original),
                Err(cleanup) => Err(FixtureError::Cleanup { original: Some(Box::new(original)), cleanup }),
            },
        }
    }.await;
    let close = connection.close().await;
    let receipt = serde_json::json!({
        "backend":"sqlite", "database":owned.path, "userId":user,
        "workspaceId":result.as_ref().ok().copied().flatten(),
        "commit":commit_state, "poolClosed":true,
        "connectionClose":if close.is_ok() { "confirmed" } else { "failed" },
        "operationSucceeded":result.is_ok(),
    });
    // Run-scoped, unique, create-new receipt contains no password/hash/key.
    let receipt_path = owned.root.join(format!("actor-{user}.json"));
    use std::io::Write;
    let result = match (result, close) {
        (Ok(workspace), Ok(())) => Ok(workspace),
        (Err(original), Ok(())) => Err(original),
        (original, Err(cleanup)) => Err(FixtureError::Cleanup {
            original: original.err().map(Box::new),
            cleanup,
        }),
    };
    let written = (|| -> std::io::Result<()> {
        let mut receipt_file = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o644)
            .open(receipt_path)?;
        receipt_file.write_all(receipt.to_string().as_bytes())?;
        receipt_file.sync_all()
    })();
    if let Err(receipt) = written {
        return Err(FixtureError::ReceiptAfterFinish {
            original: result.err().map(Box::new),
            receipt,
        });
    }
    result?;
    println!("{user}");
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::{symlink, PermissionsExt};

    #[test]
    fn sqlite_fixture_mode_never_falls_through_to_actor() {
        assert_eq!(mode(&[]).unwrap(), Mode::Actor);
        for (name, expected) in [
            ("baseline", Mode::Baseline),
            ("observe", Mode::Observe),
            ("serial-observe", Mode::SerialObserve),
        ] {
            assert_eq!(mode(&[name.into()]).unwrap(), expected);
            assert!(mode(&[name.into(), "extra".into()]).is_err());
        }
        for name in ["owner", "member", "observer", "", "unknown"] {
            assert!(mode(&[name.into()]).is_err());
        }
    }

    #[test]
    fn sqlite_observer_input_refuses_unknown_duplicate_and_unbounded_targets() {
        let workspace = Uuid::from_u128(1);
        let document = Uuid::from_u128(2);
        let valid = json!({"workspaceId":workspace,"documentIds":[document]}).to_string();
        assert!(observation_input(Mode::Observe, valid.as_bytes()).is_ok());
        for invalid in [
            json!({"workspaceId":workspace,"documentIds":[]}),
            json!({"workspaceId":workspace,"documentIds":[document,document]}),
            json!({"workspaceId":workspace,"documentIds":[document],"verified":true}),
            json!({"workspaceId":workspace,"documentIds":vec![document;101]}),
        ] {
            assert!(observation_input(Mode::Observe, invalid.to_string().as_bytes()).is_err());
        }
        assert!(observation_input(Mode::Observe, &vec![b' '; 16385]).is_err());
        assert!(observation_input(Mode::Baseline, b"{}").is_err());
        assert!(observation_input(Mode::Baseline, b"").is_ok());
        assert!(observation_input(
            Mode::SerialObserve,
            br#"{"revisionKeep":200,"revisionSnapshotIntervalHours":24}"#
        )
        .is_ok());
        for invalid in [
            br#"{"revisionKeep":0,"revisionSnapshotIntervalHours":24}"#.as_slice(),
            br#"{"revisionKeep":200,"revisionSnapshotIntervalHours":24,"verified":true}"#,
            br#"{"revisionKeep":200,"revisionKeep":300,"revisionSnapshotIntervalHours":24}"#,
        ] {
            assert!(observation_input(Mode::SerialObserve, invalid).is_err());
        }
    }

    #[test]
    fn sqlite_actor_fixture_path_requires_existing_private_owned_single_file() {
        let parent = std::env::temp_dir().join(format!("fvoci-actor-path-{}", Uuid::now_v7()));
        std::fs::create_dir(&parent).unwrap();
        let root = parent.join("run");
        std::fs::create_dir(&root).unwrap();
        std::fs::set_permissions(&root, std::fs::Permissions::from_mode(0o700)).unwrap();
        let file = root.join("app.db");
        std::fs::write(&file, []).unwrap();
        assert!(OwnedFile::check(&root, &file).is_ok());
        assert!(OwnedFile::check(&root, &root.join("missing.db")).is_err());
        assert!(OwnedFile::check(Path::new("/"), &file).is_err());
        assert!(OwnedFile::check(&root, Path::new("relative.db")).is_err());
        let outside = parent.join("outside.db");
        std::fs::write(&outside, []).unwrap();
        assert!(OwnedFile::check(&root, &outside).is_err());
        let alias = root.join("alias.db");
        symlink(&outside, &alias).unwrap();
        assert!(OwnedFile::check(&root, &alias).is_err());
        let linked = root.join("linked.db");
        std::fs::hard_link(&file, &linked).unwrap();
        assert!(OwnedFile::check(&root, &linked).is_err());
        std::fs::remove_file(linked).unwrap();
        std::fs::set_permissions(&root, std::fs::Permissions::from_mode(0o755)).unwrap();
        assert!(OwnedFile::check(&root, &file).is_err());
        std::fs::remove_dir_all(parent).unwrap();
    }
}
