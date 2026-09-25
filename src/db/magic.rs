use chrono::{DateTime, Utc};
use sqlx::{PgPool, Postgres, Transaction};
use uuid::Uuid;

use crate::auth::token::hash_token;
use crate::db::context::set_system;
use crate::mail::MAGIC_TTL_SECS;

pub const MAGIC_KIND_PASSWORD_RESET: &str = "password_reset";
pub const MAGIC_KIND_LOGIN: &str = "login";
pub const MAGIC_KIND_EMAIL_CHANGE: &str = "email_change";

#[derive(Debug, Clone)]
pub struct MagicPayload {
    pub kind: String,
    pub user_id: Uuid,
    pub generation: i32,
    /// Present only for `email_change`.
    pub new_email: Option<String>,
}

pub async fn issue_password_reset_token(
    pool: &PgPool,
    user_id: Uuid,
    generation: i32,
    token_hash: &str,
    expires_at: DateTime<Utc>,
) -> Result<(), sqlx::Error> {
    let mut tx = pool.begin().await?;
    set_system(&mut tx).await?;
    sqlx::query("SELECT fvoci.app_magic_issue($1, $2, $3, $4, $5)")
        .bind(token_hash)
        .bind(MAGIC_KIND_PASSWORD_RESET)
        .bind(user_id)
        .bind(generation)
        .bind(expires_at)
        .execute(&mut *tx)
        .await?;
    tx.commit().await?;
    Ok(())
}

pub async fn consume_magic_token(
    pool: &PgPool,
    token: &str,
) -> Result<Option<MagicPayload>, sqlx::Error> {
    let hash = hash_token(token);
    let mut tx = pool.begin().await?;
    set_system(&mut tx).await?;
    // Source GETDEL: any kind is consumed; the caller checks the kind.
    let row: Option<(String, Uuid, i32, Option<String>)> = sqlx::query_as(
        "SELECT kind, user_id, generation, new_email FROM fvoci.app_magic_consume_payload($1)",
    )
    .bind(&hash)
    .fetch_optional(&mut *tx)
    .await?;
    tx.commit().await?;
    Ok(
        row.map(|(kind, user_id, generation, new_email)| MagicPayload {
            kind,
            user_id,
            generation,
            new_email,
        }),
    )
}

pub fn magic_expires_at(now: DateTime<Utc>) -> DateTime<Utc> {
    now + chrono::Duration::seconds(MAGIC_TTL_SECS)
}

pub async fn complete_password_reset(
    pool: &PgPool,
    payload: &MagicPayload,
    password_hash: &str,
) -> Result<bool, sqlx::Error> {
    if payload.kind != MAGIC_KIND_PASSWORD_RESET {
        return Ok(false);
    }
    let mut tx = pool.begin().await?;
    crate::db::identity::lock_sign_in(&mut tx, payload.user_id).await?;
    let Some((generation, suspended_at)) =
        load_live_user_for_reset(&mut tx, payload.user_id).await?
    else {
        tx.commit().await?;
        return Ok(false);
    };
    if suspended_at.is_some() || generation != payload.generation {
        tx.commit().await?;
        return Ok(false);
    }
    crate::db::identity::set_password_hash(&mut tx, payload.user_id, password_hash).await?;
    crate::db::identity::revoke_all_sessions_for_user(&mut tx, payload.user_id).await?;
    sqlx::query("SELECT set_config('app.system_ctx', 'on', true)")
        .execute(&mut *tx)
        .await?;
    crate::db::identity::append_event(
        &mut tx,
        crate::db::identity::EventAppend {
            id: Uuid::now_v7(),
            workspace_id: None,
            actor_user_id: Some(payload.user_id),
            verb: "auth.password_reset".to_string(),
            target_type: Some("user".to_string()),
            target_id: Some(payload.user_id),
            payload: serde_json::json!({ "userId": payload.user_id.to_string() }),
        },
    )
    .await?;
    tx.commit().await?;
    Ok(true)
}

pub async fn load_live_user_for_reset(
    tx: &mut Transaction<'_, Postgres>,
    user_id: Uuid,
) -> Result<Option<(i32, Option<chrono::DateTime<Utc>>)>, sqlx::Error> {
    sqlx::query_as(
        r#"
        SELECT auth_generation, suspended_at
        FROM fvoci.users
        WHERE id = $1 AND deleted_at IS NULL
        "#,
    )
    .bind(user_id)
    .fetch_optional(&mut **tx)
    .await
}
