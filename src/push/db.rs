//! `push_subscriptions`: user-owned rows (no workspace, no RLS), read and
//! written in the system context only.
//!
//! Uniqueness is `(user_id, endpoint)` (source schema): two accounts used in
//! the same browser profile hold separate rows for the same endpoint. Delivery
//! cleanup ([`remove_by_endpoint`]) is global by endpoint string, as in the
//! source, because a 404/410 means the endpoint itself is gone.

use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use base64::Engine;
use sqlx::{PgPool, Postgres, Transaction};
use uuid::Uuid;

use crate::db::context::{lock_membership_users, session_is_live, set_system, set_tenant};
use crate::db::workspace::{membership_role, WorkspaceRole};

/// Source `PUSH_SUBSCRIPTIONS_PER_USER`; also the per-user send fan-out bound.
pub const PUSH_SUBSCRIPTIONS_PER_USER: i64 = 20;

#[derive(Debug, Clone)]
pub struct PushSubscriptionRow {
    pub endpoint: String,
    pub p256dh: String,
    pub auth: String,
}

/// Source `pushKey(bytes)`: unpadded base64url of the exact length, optional
/// `=` padding accepted and stripped. The value must also decode to exactly
/// `bytes` bytes, so a stored row always encrypts.
pub fn normalize_subscription_key(value: &str, bytes: usize) -> Option<String> {
    let unpadded = value
        .strip_suffix("==")
        .or_else(|| value.strip_suffix('='))
        .unwrap_or(value);
    let decoded = URL_SAFE_NO_PAD.decode(unpadded).ok()?;
    (unpadded.len() == (bytes * 8).div_ceil(6) && decoded.len() == bytes)
        .then(|| unpadded.to_string())
}

/// `p256dh`: 65-byte uncompressed P-256 point; `auth`: 16-byte secret.
pub fn validate_subscription_keys(p256dh: &str, auth: &str) -> bool {
    normalize_subscription_key(p256dh, 65).is_some()
        && normalize_subscription_key(auth, 16).is_some()
}

/// Stores the caller's subscription after re-checking, in the same
/// transaction, that the session is live and the user is a member (guest+)
/// of `workspace_id`. The membership advisory lock orders this against a
/// concurrent removal and serializes the per-user cap. `Ok(false)`: not a
/// member / workspace gone (the route answers 404).
pub async fn register_subscription(
    pool: &PgPool,
    workspace_id: Uuid,
    user_id: Uuid,
    session_id: Uuid,
    subscription: &PushSubscriptionRow,
) -> Result<bool, sqlx::Error> {
    let mut tx = pool.begin().await?;
    set_tenant(&mut tx, workspace_id).await?;
    lock_membership_users(&mut tx, &[user_id]).await?;
    let member = session_is_live(&mut tx, user_id, session_id).await?
        && membership_role(&mut tx, workspace_id, user_id)
            .await?
            .is_some_and(|role| role.at_least(WorkspaceRole::Guest))
        && sqlx::query_scalar::<_, bool>(
            "SELECT EXISTS (SELECT 1 FROM fvoci.workspaces WHERE id = $1 AND deleted_at IS NULL)",
        )
        .bind(workspace_id)
        .fetch_one(&mut *tx)
        .await?;
    if !member {
        tx.rollback().await?;
        return Ok(false);
    }
    set_system(&mut tx).await?;
    upsert_subscription(
        &mut tx,
        user_id,
        Some(session_id),
        &subscription.endpoint,
        &subscription.p256dh,
        &subscription.auth,
    )
    .await?;
    tx.commit().await?;
    Ok(true)
}

/// Upsert on `(user_id, endpoint)` bound to the registering session, then
/// drop the user's least recently updated rows beyond
/// [`PUSH_SUBSCRIPTIONS_PER_USER`].
pub async fn upsert_subscription(
    tx: &mut Transaction<'_, Postgres>,
    user_id: Uuid,
    session_id: Option<Uuid>,
    endpoint: &str,
    p256dh: &str,
    auth: &str,
) -> Result<(), sqlx::Error> {
    sqlx::query(
        r#"
        INSERT INTO fvoci.push_subscriptions (user_id, endpoint, p256dh, auth, session_id)
        VALUES ($1, $2, $3, $4, $5)
        ON CONFLICT (user_id, endpoint) DO UPDATE
        SET p256dh = EXCLUDED.p256dh, auth = EXCLUDED.auth,
            session_id = EXCLUDED.session_id, updated_at = clock_timestamp()
        "#,
    )
    .bind(user_id)
    .bind(endpoint)
    .bind(p256dh)
    .bind(auth)
    .bind(session_id)
    .execute(&mut **tx)
    .await?;
    sqlx::query(
        r#"
        DELETE FROM fvoci.push_subscriptions
        WHERE user_id = $1
          AND id NOT IN (
              SELECT id FROM fvoci.push_subscriptions
              WHERE user_id = $1
              ORDER BY updated_at DESC, id DESC
              LIMIT $2
          )
        "#,
    )
    .bind(user_id)
    .bind(PUSH_SUBSCRIPTIONS_PER_USER)
    .execute(&mut **tx)
    .await?;
    Ok(())
}

/// Logout disconnect: removes the logged-out user's rows for this browser,
/// that is rows registered by the ending session plus the endpoint the
/// browser reported. Keyed by the user, so another account's row for the same
/// endpoint and this user's other devices stay. Runs in the logout
/// transaction; a send holding the row lock finishes first, and no send starts
/// after the logout commits (the sender re-reads the row before each POST).
pub async fn disconnect_browser(
    tx: &mut Transaction<'_, Postgres>,
    user_id: Uuid,
    session_id: Uuid,
    endpoint: Option<&str>,
) -> Result<u64, sqlx::Error> {
    Ok(sqlx::query(
        r#"
        DELETE FROM fvoci.push_subscriptions
        WHERE user_id = $1 AND (session_id = $2 OR endpoint = $3)
        "#,
    )
    .bind(user_id)
    .bind(session_id)
    .bind(endpoint)
    .execute(&mut **tx)
    .await?
    .rows_affected())
}

pub async fn remove_by_endpoint(
    tx: &mut Transaction<'_, Postgres>,
    endpoint: &str,
) -> Result<u64, sqlx::Error> {
    Ok(
        sqlx::query("DELETE FROM fvoci.push_subscriptions WHERE endpoint = $1")
            .bind(endpoint)
            .execute(&mut **tx)
            .await?
            .rows_affected(),
    )
}

pub async fn list_for_user(
    tx: &mut Transaction<'_, Postgres>,
    user_id: Uuid,
) -> Result<Vec<PushSubscriptionRow>, sqlx::Error> {
    let rows: Vec<(String, String, String)> = sqlx::query_as(
        "SELECT endpoint, p256dh, auth FROM fvoci.push_subscriptions WHERE user_id = $1 ORDER BY id",
    )
    .bind(user_id)
    .fetch_all(&mut **tx)
    .await?;
    Ok(rows
        .into_iter()
        .map(|(endpoint, p256dh, auth)| PushSubscriptionRow {
            endpoint,
            p256dh,
            auth,
        })
        .collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn key_lengths_and_encoding_are_locked() {
        let p256dh = format!("B{}", "A".repeat(86));
        let auth = "A".repeat(22);
        assert!(validate_subscription_keys(&p256dh, &auth));
        assert_eq!(
            normalize_subscription_key(&format!("{auth}=="), 16),
            Some(auth.clone())
        );
        assert_eq!(
            normalize_subscription_key(&format!("{p256dh}="), 65),
            Some(p256dh.clone())
        );
        assert!(!validate_subscription_keys(&p256dh, &"A".repeat(21)));
        assert!(!validate_subscription_keys(&p256dh, &"A".repeat(23)));
        assert!(!validate_subscription_keys(
            &p256dh.replace('A', "+"),
            &auth
        ));
        // 22 chars carry 4 spare bits; non-zero spare bits are not a 16-byte value.
        assert!(!validate_subscription_keys(
            &p256dh,
            &format!("{}v", "A".repeat(21))
        ));
    }
}
