use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use base64::Engine;
use sqlx::{PgPool, Postgres, Transaction};
use uuid::Uuid;
use web_push_native::jwt_simple::algorithms::{ECDSAP256PublicKeyLike, ES256KeyPair};

use crate::auth::password::Keyring;
use crate::db::context::set_system;
use crate::db::identity::{append_audit, append_event_channel, AuditAppend, EventAppend};
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
    use crate::db::backend::{Backend, DbTransaction};
    if let Backend::Postgres(pool) = backend {
        return load_vapid_public_key(pool).await;
    }
    let mut tx = backend.begin_read().await?;
    let result = async {
        tx.operation().set_system().await?;
        let DbTransaction::SqliteFamily(family) = &mut tx else {
            unreachable!()
        };
        let rows = family
            .query(
                "SELECT vapid_public_key FROM instance_config WHERE id=1",
                &[],
            )
            .await?;
        rows.first()
            .map(|row| row.cell(0)?.optional(|cell| cell.string()))
            .transpose()
            .map(Option::flatten)
    }
    .await;
    tx.rollback().await?;
    result
}

pub async fn load_vapid_key_pair(
    pool: &PgPool,
    keys: Option<&Keyring>,
) -> Result<Option<ES256KeyPair>, VapidKeysError> {
    let Some(keys) = keys else {
        return Ok(None);
    };
    let mut tx = pool.begin().await?;
    set_system(&mut tx).await?;
    let sealed: Option<String> = sqlx::query_scalar("SELECT fvoci.app_vapid_private_key()")
        .fetch_one(&mut *tx)
        .await?;
    tx.commit().await?;
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

async fn set_vapid_tx(
    tx: &mut Transaction<'_, Postgres>,
    public: &str,
    private_sealed: &str,
) -> Result<(), sqlx::Error> {
    sqlx::query("SELECT fvoci.app_set_vapid($1, $2)")
        .bind(public)
        .bind(private_sealed)
        .execute(&mut **tx)
        .await?;
    Ok(())
}

/// Boot-time keypair bootstrap (source `ensureVapidKeys`). First writer wins:
/// `app_init_vapid` stores the pair only while the private column is null, so
/// replicas booting together converge on one keypair. The app role cannot read
/// or lock the private column directly.
pub async fn ensure_vapid_keys(
    pool: &PgPool,
    keys: Option<&Keyring>,
) -> Result<(), VapidKeysError> {
    let Some(keys) = keys else {
        return Err(VapidKeysError::KeyringUnavailable);
    };
    if load_vapid_public_key(pool).await?.is_some() {
        return Ok(());
    }
    let pair = ES256KeyPair::generate();
    let (public_b64, private_b64) = encode_key_material(&pair)?;
    let sealed = secret_box::seal(keys, &private_b64, VAPID_CONTEXT)?;
    let mut tx = pool.begin().await?;
    set_system(&mut tx).await?;
    let stored: bool = sqlx::query_scalar("SELECT fvoci.app_init_vapid($1, $2)")
        .bind(&public_b64)
        .bind(&sealed)
        .fetch_one(&mut *tx)
        .await?;
    tx.commit().await?;
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
    let pair = ES256KeyPair::generate();
    let (public_b64, private_b64) = encode_key_material(&pair)?;
    let sealed = secret_box::seal(keys, &private_b64, VAPID_CONTEXT)?;

    let mut tx = pool.begin().await?;
    set_system(&mut tx).await?;
    set_vapid_tx(&mut tx, &public_b64, &sealed).await?;
    let revoked: u64 = sqlx::query("DELETE FROM fvoci.push_subscriptions")
        .execute(&mut *tx)
        .await?
        .rows_affected();
    let payload = serde_json::json!({ "revokedSubscriptions": revoked });
    append_event_channel(
        &mut tx,
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
    append_audit(
        &mut tx,
        AuditAppend {
            id: Uuid::now_v7(),
            workspace_id: None,
            actor_user_id: None,
            verb: VAPID_ROTATED_VERB.to_string(),
            target_type: None,
            target_id: None,
            payload,
            ip: None,
        },
    )
    .await?;
    tx.commit().await?;
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
}
