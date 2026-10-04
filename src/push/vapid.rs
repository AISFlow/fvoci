use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use base64::Engine;
use sqlx::PgPool;
use uuid::Uuid;
use web_push_native::jwt_simple::algorithms::{ECDSAP256PublicKeyLike, ES256KeyPair};

use crate::auth::password::Keyring;
use crate::db::backend::Backend;
use crate::db::identity::{AuditAppend, EventAppend};
use crate::secret_box::{self, SecretBoxError};

pub const VAPID_CONTEXT: &str = "vapid:1";
const VAPID_ROTATED_VERB: &str = "instance.vapid_rotated";

pub fn vapid_context() -> &'static str {
    VAPID_CONTEXT
}

#[derive(Debug, thiserror::Error)]
pub enum VapidKeysError {
    #[error("encryption keyring unavailable")]
    KeyringUnavailable,
    #[error("vapid key material invalid")]
    InvalidMaterial,
    #[error("database error: {0}")]
    Db(#[from] sqlx::Error),
    #[error("secret box error: {0}")]
    Secret(#[from] SecretBoxError),
}

/// Read per request: `/instance` must show a rotated key without a restart.
pub async fn load_vapid_public_key(pool: &PgPool) -> Result<Option<String>, sqlx::Error> {
    sqlx::query_scalar("SELECT fvoci.app_vapid_public_key()")
        .fetch_one(pool)
        .await
}

/// The same current public key read for a selected backend. Private material
/// is never loaded by the anonymous instance projection.
pub async fn load_vapid_public_key_backend(
    backend: &crate::db::backend::Backend,
) -> Result<Option<String>, sqlx::Error> {
    if let Backend::Postgres(pool) = backend {
        return load_vapid_public_key(pool).await;
    }
    let mut tx = backend.begin_read().await?;
    let result = tx.operation().read_vapid_public_key().await;
    tx.rollback().await?;
    result
}

pub async fn load_vapid_key_pair(
    pool: &PgPool,
    keys: Option<&Keyring>,
) -> Result<Option<ES256KeyPair>, VapidKeysError> {
    load_vapid_key_pair_backend(&Backend::Postgres(pool.clone()), keys).await
}

pub async fn load_vapid_key_pair_backend(
    backend: &Backend,
    keys: Option<&Keyring>,
) -> Result<Option<ES256KeyPair>, VapidKeysError> {
    let Some(keys) = keys else {
        return Ok(None);
    };
    // Preserve PostgreSQL's existing private-reader transaction mode.
    let mut tx = match backend {
        Backend::Postgres(_) => backend.begin_write().await?,
        _ => backend.begin_read().await?,
    };
    tx.operation().set_system().await?;
    let sealed = tx.operation().read_vapid_sealed_private().await?;
    tx.commit().await.map_err(|err| err.source)?;
    let Some(sealed) = sealed else {
        return Ok(None);
    };
    let plaintext = secret_box::open(keys, &sealed, VAPID_CONTEXT)?;
    let der = URL_SAFE_NO_PAD
        .decode(plaintext.trim())
        .map_err(|_| VapidKeysError::InvalidMaterial)?;
    let pair = ES256KeyPair::from_der(&der).map_err(|_| VapidKeysError::InvalidMaterial)?;
    Ok(Some(pair))
}

fn encode_key_material(pair: &ES256KeyPair) -> Result<(String, String), VapidKeysError> {
    let public = pair.public_key();
    let public_bytes = public.public_key().to_bytes_uncompressed();
    let public_b64 = URL_SAFE_NO_PAD.encode(public_bytes);
    let der = pair.to_der().map_err(|_| VapidKeysError::InvalidMaterial)?;
    let private_b64 = URL_SAFE_NO_PAD.encode(der);
    Ok((public_b64, private_b64))
}

/// Boot-time keypair bootstrap (source `ensureVapidKeys`). First writer wins:
/// `app_init_vapid` stores the pair only while the private column is null, so
/// replicas booting together converge on one keypair. The app role cannot read
/// or lock the private column directly.
pub async fn ensure_vapid_keys(
    pool: &PgPool,
    keys: Option<&Keyring>,
) -> Result<(), VapidKeysError> {
    ensure_vapid_keys_backend(&Backend::Postgres(pool.clone()), keys).await
}

/// Selected-backend startup follows the same key-dependent warning contract:
/// no keyring fails before opening a connection or generating key material.
pub async fn ensure_vapid_keys_backend(
    backend: &Backend,
    keys: Option<&Keyring>,
) -> Result<(), VapidKeysError> {
    let Some(keys) = keys else {
        return Err(VapidKeysError::KeyringUnavailable);
    };
    if load_vapid_public_key_backend(backend).await?.is_some() {
        return Ok(());
    }
    let pair = ES256KeyPair::generate();
    let (public_b64, private_b64) = encode_key_material(&pair)?;
    let sealed = secret_box::seal(keys, &private_b64, VAPID_CONTEXT)?;
    let mut tx = backend.begin_write().await?;
    tx.operation().set_system().await?;
    let stored = tx
        .operation()
        .init_vapid_if_absent(&public_b64, &sealed)
        .await?;
    // The losing initializer must observe the winner, rather than claiming
    // success when the singleton disappeared or lacks a public projection.
    if !stored && tx.operation().read_vapid_public_key().await?.is_none() {
        return Err(VapidKeysError::InvalidMaterial);
    }
    tx.commit().await.map_err(|err| err.source)?;
    if stored {
        tracing::info!(event = "vapid.initialized");
    }
    Ok(())
}

pub struct RotateVapidOutcome {
    pub public_key: String,
    pub revoked_subscriptions: u64,
}

/// `fvoci secrets rotate-vapid`: new keypair, every subscription deleted (they
/// are bound to the old public key and push services answer 401/403, which the
/// sender does not clean up) and the `instance.vapid_rotated` event + audit row,
/// all in one system transaction.
pub async fn rotate_vapid_keys(
    pool: &PgPool,
    keys: &Keyring,
) -> Result<RotateVapidOutcome, VapidKeysError> {
    rotate_vapid_keys_backend(&Backend::Postgres(pool.clone()), keys).await
}

/// Trusted operator rotation; system context and the family writer reservation
/// cover the key replacement, all subscription cleanup, event and audit.
pub async fn rotate_vapid_keys_backend(
    backend: &Backend,
    keys: &Keyring,
) -> Result<RotateVapidOutcome, VapidKeysError> {
    let pair = ES256KeyPair::generate();
    let (public_b64, private_b64) = encode_key_material(&pair)?;
    let sealed = secret_box::seal(keys, &private_b64, VAPID_CONTEXT)?;

    let mut tx = backend.begin_write().await?;
    let mut operation = tx.operation();
    operation.set_system().await?;
    operation.replace_vapid_pair(&public_b64, &sealed).await?;
    let revoked = operation.revoke_vapid_subscriptions().await?;
    let payload = serde_json::json!({ "revokedSubscriptions": revoked });
    operation
        .append_event_channel(
            EventAppend {
                id: Uuid::now_v7(),
                workspace_id: None,
                actor_user_id: None,
                verb: VAPID_ROTATED_VERB.to_string(),
                target_type: None,
                target_id: None,
                payload: payload.clone(),
            },
            "system",
        )
        .await?;
    operation
        .append_audit(AuditAppend {
            id: Uuid::now_v7(),
            workspace_id: None,
            actor_user_id: None,
            verb: VAPID_ROTATED_VERB.to_string(),
            target_type: None,
            target_id: None,
            payload,
            ip: None,
        })
        .await?;
    tx.commit().await.map_err(|err| err.source)?;
    Ok(RotateVapidOutcome {
        public_key: public_b64,
        revoked_subscriptions: revoked,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn generated_material_round_trips_through_der() {
        let pair = ES256KeyPair::generate();
        let (public_b64, private_b64) = encode_key_material(&pair).expect("encode");
        assert_eq!(public_b64.len(), 87);
        assert!(!private_b64.is_empty());
        let der = URL_SAFE_NO_PAD.decode(private_b64).expect("decode");
        let loaded = ES256KeyPair::from_der(&der).expect("load");
        let (public2, _) = encode_key_material(&loaded).expect("re-encode");
        assert_eq!(public_b64, public2);
    }
    struct Fixture {
        directory: std::path::PathBuf,
        path: std::path::PathBuf,
        pool: sqlx::SqlitePool,
    }

    impl Fixture {
        async fn new() -> Self {
            let directory = std::env::temp_dir().join(format!("fvoci-w2-vapid-{}", Uuid::now_v7()));
            std::fs::create_dir(&directory).unwrap();
            let path = directory.join("vapid.sqlite");
            let pool = crate::db::pool::connect_sqlite_prepare(&path)
                .await
                .unwrap();
            // Entire fixed current DDL, executed by SQLx's existing raw_sql
            // implementation. This is a storage fixture, not a schema runner.
            let mut tx = pool.begin_with("BEGIN IMMEDIATE").await.unwrap();
            for ddl in [
                include_str!("../../migrations/sqlite/001_current_schema.sql"),
                include_str!("../../migrations/sqlite/002_wiki_create_commands.sql"),
                include_str!("../../migrations/sqlite/003_collab_room_fences.sql"),
            ] {
                sqlx::raw_sql(ddl).execute(&mut *tx).await.unwrap();
            }
            tx.commit().await.unwrap();
            Self {
                directory,
                path,
                pool,
            }
        }

        fn backend(&self) -> Backend {
            Backend::Sqlite(self.pool.clone())
        }

        async fn reopen(&mut self) {
            self.pool.close().await;
            self.pool = crate::db::pool::connect_sqlite_app(&self.path, 4)
                .await
                .unwrap();
        }

        async fn finish(self) {
            self.pool.close().await;
        }
    }

    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.directory);
        }
    }

    fn fixture_keys() -> Keyring {
        Keyring::parse_named(
            &format!(r#"{{"fixture":"{}"}}"#, "1".repeat(64)),
            "fixture",
            "ENCRYPTION_KEYS",
        )
        .unwrap()
    }

    async fn public(backend: &Backend) -> String {
        load_vapid_public_key_backend(backend)
            .await
            .unwrap()
            .unwrap()
    }

    async fn sealed(fixture: &Fixture) -> String {
        sqlx::query_scalar("SELECT vapid_private_key FROM instance_config WHERE id=1")
            .fetch_one(&fixture.pool)
            .await
            .unwrap()
    }

    async fn assert_pair(backend: &Backend, keys: &Keyring, expected: &str) {
        let pair = load_vapid_key_pair_backend(backend, Some(keys))
            .await
            .unwrap()
            .unwrap();
        let (actual, _) = encode_key_material(&pair).unwrap();
        assert!(
            actual == expected,
            "loaded private pair must match public projection"
        );
    }

    #[tokio::test]
    async fn absent_keyring_does_not_open_database() {
        let pool = sqlx::sqlite::SqlitePoolOptions::new()
            .connect_lazy("sqlite::memory:")
            .unwrap();
        pool.close().await;
        let backend = Backend::Sqlite(pool);
        assert!(matches!(
            ensure_vapid_keys_backend(&backend, None).await,
            Err(VapidKeysError::KeyringUnavailable)
        ));
        assert!(load_vapid_key_pair_backend(&backend, None)
            .await
            .unwrap()
            .is_none());
    }

    #[tokio::test]
    async fn bootstrap_sealed_roundtrip_idempotence_and_restart() {
        let mut fixture = Fixture::new().await;
        let keys = fixture_keys();
        assert!(load_vapid_public_key_backend(&fixture.backend())
            .await
            .unwrap()
            .is_none());
        assert!(load_vapid_key_pair_backend(&fixture.backend(), Some(&keys))
            .await
            .unwrap()
            .is_none());
        ensure_vapid_keys_backend(&fixture.backend(), Some(&keys))
            .await
            .unwrap();
        let expected = public(&fixture.backend()).await;
        assert_eq!(expected.len(), 87);
        assert_eq!(URL_SAFE_NO_PAD.decode(&expected).unwrap()[0], 4);
        let original_sealed = sealed(&fixture).await;
        assert!(original_sealed.starts_with("enc:v2:fixture:"));
        assert_pair(&fixture.backend(), &keys, &expected).await;
        ensure_vapid_keys_backend(&fixture.backend(), Some(&keys))
            .await
            .unwrap();
        assert!(sealed(&fixture).await == original_sealed);
        fixture.reopen().await;
        ensure_vapid_keys_backend(&fixture.backend(), Some(&keys))
            .await
            .unwrap();
        assert!(public(&fixture.backend()).await == expected);
        assert!(sealed(&fixture).await == original_sealed);
        assert_pair(&fixture.backend(), &keys, &expected).await;
        fixture.finish().await;
    }

    #[tokio::test]
    async fn competing_initializers_reread_winner_without_replacement() {
        let mut fixture = Fixture::new().await;
        fixture.reopen().await;
        let barrier = std::sync::Arc::new(tokio::sync::Barrier::new(2));
        let initialize = |backend: Backend, barrier: std::sync::Arc<tokio::sync::Barrier>| async move {
            assert!(load_vapid_public_key_backend(&backend)
                .await
                .unwrap()
                .is_none());
            let pair = ES256KeyPair::generate();
            let (candidate, private) = encode_key_material(&pair).unwrap();
            let sealed = secret_box::seal(&fixture_keys(), &private, VAPID_CONTEXT).unwrap();
            barrier.wait().await;
            let mut tx = backend.begin_write().await.unwrap();
            tx.operation().set_system().await.unwrap();
            let stored = tx
                .operation()
                .init_vapid_if_absent(&candidate, &sealed)
                .await
                .unwrap();
            let observed = tx
                .operation()
                .read_vapid_public_key()
                .await
                .unwrap()
                .unwrap();
            tx.commit().await.unwrap();
            (stored, candidate, observed)
        };
        let (a, b) = tokio::join!(
            initialize(fixture.backend(), barrier.clone()),
            initialize(fixture.backend(), barrier)
        );
        assert_ne!(a.0, b.0, "exactly one candidate wins");
        let winner = if a.0 { &a.1 } else { &b.1 };
        assert!(a.2 == *winner && b.2 == *winner);
        ensure_vapid_keys_backend(&fixture.backend(), Some(&fixture_keys()))
            .await
            .unwrap();
        assert!(public(&fixture.backend()).await == *winner);
        assert_pair(&fixture.backend(), &fixture_keys(), winner).await;
        fixture.finish().await;
    }

    #[tokio::test]
    async fn public_projection_never_opens_damaged_private_material() {
        let fixture = Fixture::new().await;
        ensure_vapid_keys_backend(&fixture.backend(), Some(&fixture_keys()))
            .await
            .unwrap();
        let expected = public(&fixture.backend()).await;
        sqlx::query("UPDATE instance_config SET vapid_private_key='enc:v2:damaged'")
            .execute(&fixture.pool)
            .await
            .unwrap();
        assert!(public(&fixture.backend()).await == expected);
        assert!(matches!(
            load_vapid_key_pair_backend(&fixture.backend(), Some(&fixture_keys())).await,
            Err(VapidKeysError::Secret(_))
        ));
        fixture.finish().await;
    }

    #[tokio::test]
    async fn sealed_context_and_der_validation_fail_closed() {
        let fixture = Fixture::new().await;
        let keys = fixture_keys();
        for (plaintext, context, invalid_material) in [
            ("invalid base64!", VAPID_CONTEXT, true),
            ("AA", VAPID_CONTEXT, true),
            ("AA", "other-context", false),
        ] {
            let sealed = secret_box::seal(&keys, plaintext, context).unwrap();
            sqlx::query("UPDATE instance_config SET vapid_private_key=?1")
                .bind(sealed)
                .execute(&fixture.pool)
                .await
                .unwrap();
            let result = load_vapid_key_pair_backend(&fixture.backend(), Some(&keys)).await;
            if invalid_material {
                assert!(matches!(result, Err(VapidKeysError::InvalidMaterial)));
            } else {
                assert!(matches!(result, Err(VapidKeysError::Secret(_))));
            }
        }
        fixture.finish().await;
    }

    #[tokio::test]
    async fn private_and_mutations_require_current_system_context_and_writer() {
        let fixture = Fixture::new().await;
        let keys = fixture_keys();
        ensure_vapid_keys_backend(&fixture.backend(), Some(&keys))
            .await
            .unwrap();
        let before = sealed(&fixture).await;
        let backend = fixture.backend();
        let mut tx = backend.begin_write().await.unwrap();
        assert!(tx.operation().read_vapid_sealed_private().await.is_err());
        assert!(tx
            .operation()
            .init_vapid_if_absent("x", "enc:v2:fixture:x")
            .await
            .is_err());
        assert!(tx
            .operation()
            .replace_vapid_pair("x", "enc:v2:fixture:x")
            .await
            .is_err());
        assert!(tx.operation().revoke_vapid_subscriptions().await.is_err());
        let previous = tx.operation().set_system().await.unwrap();
        assert!(tx
            .operation()
            .read_vapid_sealed_private()
            .await
            .unwrap()
            .is_some());
        tx.operation().restore_system(previous).await.unwrap();
        assert!(tx.operation().read_vapid_sealed_private().await.is_err());
        tx.rollback().await.unwrap();
        let mut read = backend.begin_read().await.unwrap();
        assert!(
            read.operation().read_vapid_sealed_private().await.is_err(),
            "context must not leak into reused connection"
        );
        read.operation().set_system().await.unwrap();
        assert!(read
            .operation()
            .init_vapid_if_absent("x", "enc:v2:fixture:x")
            .await
            .is_err());
        assert!(read
            .operation()
            .replace_vapid_pair("x", "enc:v2:fixture:x")
            .await
            .is_err());
        assert!(read.operation().revoke_vapid_subscriptions().await.is_err());
        read.rollback().await.unwrap();
        assert!(sealed(&fixture).await == before);
        fixture.finish().await;
    }

    #[tokio::test]
    async fn absent_singleton_is_not_reported_as_initialized() {
        let fixture = Fixture::new().await;
        sqlx::query("DELETE FROM instance_config")
            .execute(&fixture.pool)
            .await
            .unwrap();
        assert!(matches!(
            ensure_vapid_keys_backend(&fixture.backend(), Some(&fixture_keys())).await,
            Err(VapidKeysError::InvalidMaterial)
        ));
        assert!(
            rotate_vapid_keys_backend(&fixture.backend(), &fixture_keys())
                .await
                .is_err()
        );
        // A private singleton without its public projection is also not a
        // successful losing initialization, and must not replace private data.
        sqlx::query(
            "INSERT INTO instance_config(id,vapid_private_key) VALUES (1,'enc:v2:damaged')",
        )
        .execute(&fixture.pool)
        .await
        .unwrap();
        assert!(matches!(
            ensure_vapid_keys_backend(&fixture.backend(), Some(&fixture_keys())).await,
            Err(VapidKeysError::InvalidMaterial)
        ));
        assert!(sealed(&fixture).await == "enc:v2:damaged");
        fixture.finish().await;
    }

    async fn seed_subscriptions(fixture: &Fixture) {
        // Two accounts and workspaces; rotation cleanup is instance-wide.
        for index in 0..2 {
            let user = Uuid::now_v7();
            let session = Uuid::now_v7();
            let workspace = Uuid::now_v7();
            let subscription = Uuid::now_v7();
            sqlx::query("INSERT INTO users(id,email,given_name) VALUES (?1,?2,'Fixture')")
                .bind(user.as_bytes().as_slice())
                .bind(format!("vapid-{index}@example.test"))
                .execute(&fixture.pool)
                .await
                .unwrap();
            sqlx::query("INSERT INTO sessions(id,user_id,token_hash,expires_at) VALUES (?1,?2,?3,9223372036854775807)")
                .bind(session.as_bytes().as_slice()).bind(user.as_bytes().as_slice()).bind(format!("fixture-{index}"))
                .execute(&fixture.pool).await.unwrap();
            sqlx::query("INSERT INTO workspaces(id,slug,name) VALUES (?1,?2,'Fixture')")
                .bind(workspace.as_bytes().as_slice())
                .bind(format!("vapid-{index}"))
                .execute(&fixture.pool)
                .await
                .unwrap();
            sqlx::query("INSERT INTO push_subscriptions(id,user_id,session_id,endpoint,p256dh,auth) VALUES (?1,?2,?3,?4,?5,?6)")
                .bind(subscription.as_bytes().as_slice()).bind(user.as_bytes().as_slice()).bind(session.as_bytes().as_slice())
                .bind(format!("https://push.example.test/{index}")).bind("x".repeat(87)).bind("x".repeat(22))
                .execute(&fixture.pool).await.unwrap();
            sqlx::query("INSERT INTO push_deliveries(id,event_id,workspace_id,user_id,subscription_id) VALUES (?1,?2,?3,?4,?5)")
                .bind(Uuid::now_v7().as_bytes().as_slice()).bind(Uuid::now_v7().as_bytes().as_slice())
                .bind(workspace.as_bytes().as_slice()).bind(user.as_bytes().as_slice()).bind(subscription.as_bytes().as_slice())
                .execute(&fixture.pool).await.unwrap();
        }
    }

    #[tokio::test]
    async fn rotation_cleanup_event_and_audit_commit_or_rollback_together() {
        let fixture = Fixture::new().await;
        let keys = fixture_keys();
        ensure_vapid_keys_backend(&fixture.backend(), Some(&keys))
            .await
            .unwrap();
        let original_public = public(&fixture.backend()).await;
        let original_sealed = sealed(&fixture).await;
        seed_subscriptions(&fixture).await;
        for table in ["events", "audit_log"] {
            // Test-only failure injection, never exposed by a product route.
            sqlx::raw_sql(&format!("CREATE TRIGGER reject_rotation BEFORE INSERT ON {table} WHEN NEW.verb='instance.vapid_rotated' BEGIN SELECT RAISE(ABORT,'injected rotation failure'); END;"))
                .execute(&fixture.pool).await.unwrap();
            assert!(rotate_vapid_keys_backend(&fixture.backend(), &keys)
                .await
                .is_err());
            assert!(public(&fixture.backend()).await == original_public);
            assert!(sealed(&fixture).await == original_sealed);
            for (query, expected) in [
                ("SELECT count(*) FROM push_subscriptions", 2_i64),
                ("SELECT count(*) FROM push_deliveries", 2),
                ("SELECT count(*) FROM events", 0),
                ("SELECT count(*) FROM audit_log", 0),
                ("SELECT last_seq FROM event_sequence WHERE id=1", 0),
            ] {
                let actual: i64 = sqlx::query_scalar(query)
                    .fetch_one(&fixture.pool)
                    .await
                    .unwrap();
                assert_eq!(actual, expected);
            }
            sqlx::query("DROP TRIGGER reject_rotation")
                .execute(&fixture.pool)
                .await
                .unwrap();
        }
        let rotated = rotate_vapid_keys_backend(&fixture.backend(), &keys)
            .await
            .unwrap();
        assert_eq!(rotated.revoked_subscriptions, 2);
        assert!(rotated.public_key != original_public);
        assert!(public(&fixture.backend()).await == rotated.public_key);
        assert_pair(&fixture.backend(), &keys, &rotated.public_key).await;
        for query in [
            "SELECT count(*) FROM push_subscriptions",
            "SELECT count(*) FROM push_deliveries",
        ] {
            assert_eq!(
                sqlx::query_scalar::<_, i64>(query)
                    .fetch_one(&fixture.pool)
                    .await
                    .unwrap(),
                0
            );
        }
        for query in [
            "SELECT count(*) FROM events",
            "SELECT count(*) FROM audit_log",
        ] {
            assert_eq!(
                sqlx::query_scalar::<_, i64>(query)
                    .fetch_one(&fixture.pool)
                    .await
                    .unwrap(),
                1
            );
        }
        for query in [
            "SELECT count(*) FROM users",
            "SELECT count(*) FROM sessions",
            "SELECT count(*) FROM workspaces",
        ] {
            assert_eq!(sqlx::query_scalar::<_, i64>(query).fetch_one(&fixture.pool).await.unwrap(), 2,
                "rotation removes subscriptions and deliveries, preserving accounts/sessions/workspaces");
        }
        let event: (Option<Vec<u8>>, Option<Vec<u8>>, String, String, i64) = sqlx::query_as(
            "SELECT workspace_id,actor_user_id,channel,payload,seq FROM events WHERE verb='instance.vapid_rotated'")
            .fetch_one(&fixture.pool).await.unwrap();
        assert!(event.0.is_none() && event.1.is_none());
        assert_eq!(event.2, "system");
        assert_eq!(
            serde_json::from_str::<serde_json::Value>(&event.3).unwrap(),
            serde_json::json!({"revokedSubscriptions":2})
        );
        assert_eq!(event.4, 1);
        let audit: (Option<Vec<u8>>, Option<Vec<u8>>, String) = sqlx::query_as(
            "SELECT workspace_id,actor_user_id,payload FROM audit_log WHERE verb='instance.vapid_rotated'")
            .fetch_one(&fixture.pool).await.unwrap();
        assert!(audit.0.is_none() && audit.1.is_none());
        assert_eq!(
            serde_json::from_str::<serde_json::Value>(&audit.2).unwrap(),
            serde_json::json!({"revokedSubscriptions":2})
        );
        fixture.finish().await;
    }
}
