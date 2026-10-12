use super::backend::{Backend, OperationTx};
use super::codec::Cell;
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

pub async fn issue_password_reset_token_backend(
    backend: &Backend,
    user_id: Uuid,
    generation: i32,
    token_hash: &str,
    expires_at: DateTime<Utc>,
) -> Result<(), sqlx::Error> {
    if let Backend::Postgres(pool) = backend {
        return issue_password_reset_token(pool, user_id, generation, token_hash, expires_at).await;
    }
    issue_token_backend(
        backend,
        &MagicPayload {
            kind: MAGIC_KIND_PASSWORD_RESET.into(),
            user_id,
            generation,
            new_email: None,
        },
        token_hash,
        expires_at,
    )
    .await
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

pub async fn consume_magic_token_backend(
    backend: &Backend,
    token: &str,
) -> Result<Option<MagicPayload>, sqlx::Error> {
    if let Backend::Postgres(pool) = backend {
        return consume_magic_token(pool, token).await;
    }
    let mut tx = backend.begin_write().await?;
    tx.operation().set_system().await?;
    let OperationTx::SqliteFamily(family) = tx.operation() else {
        unreachable!("family magic consume")
    };
    family.require_writer()?;
    family.require_system_context()?;
    let rows=family.query("DELETE FROM magic_tokens WHERE token_hash=?1 AND expires_at>(unixepoch()*1000000+CAST(substr(strftime('%f','now'),4,3) AS INTEGER)*1000) RETURNING kind,user_id,generation,new_email",&[Cell::text(hash_token(token))]).await?;
    let payload = rows
        .first()
        .map(|r| {
            Ok::<_, sqlx::Error>(MagicPayload {
                kind: r.cell(0)?.string()?,
                user_id: r.cell(1)?.id()?,
                generation: r.cell(2)?.int32()?,
                new_email: r.cell(3)?.optional(Cell::string)?,
            })
        })
        .transpose()?;
    tx.commit()
        .await
        .map_err(|unknown| sqlx::Error::AnyDriverError(Box::new(unknown)))?;
    Ok(payload)
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

pub async fn complete_password_reset_backend(
    backend: &Backend,
    payload: &MagicPayload,
    password_hash: &str,
) -> Result<bool, sqlx::Error> {
    if let Backend::Postgres(pool) = backend {
        return complete_password_reset(pool, payload, password_hash).await;
    }
    if payload.kind != MAGIC_KIND_PASSWORD_RESET {
        return Ok(false);
    }
    let mut tx = backend.begin_write().await?;
    let OperationTx::SqliteFamily(family) = tx.operation() else {
        unreachable!("family password reset")
    };
    family.require_writer()?;
    let rows=family.query("SELECT auth_generation FROM users WHERE id=?1 AND deleted_at IS NULL AND suspended_at IS NULL",&[Cell::uuid(payload.user_id)]).await?;
    let generation = rows.first().map(|r| r.cell(0)?.int32()).transpose()?;
    if generation != Some(payload.generation) {
        tx.rollback().await?;
        return Ok(false);
    }
    let mut op = tx.operation();
    op.change_password_hash(payload.user_id, password_hash)
        .await?;
    op.revoke_auth_sessions(payload.user_id, None).await?;
    op.set_system().await?;
    op.append_event(crate::db::identity::EventAppend {
        id: Uuid::now_v7(),
        workspace_id: None,
        actor_user_id: Some(payload.user_id),
        verb: "auth.password_reset".into(),
        target_type: Some("user".into()),
        target_id: Some(payload.user_id),
        payload: serde_json::json!({"userId":payload.user_id.to_string()}),
    })
    .await?;
    tx.commit()
        .await
        .map_err(|unknown| sqlx::Error::AnyDriverError(Box::new(unknown)))?;
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

pub(crate) async fn issue_token_backend(
    backend: &Backend,
    payload: &MagicPayload,
    token_hash: &str,
    expires_at: DateTime<Utc>,
) -> Result<(), sqlx::Error> {
    let mut tx = backend.begin_write().await?;
    tx.operation().set_system().await?;
    tx.operation()
        .issue_magic_token(payload, token_hash, expires_at)
        .await?;
    tx.commit()
        .await
        .map_err(|unknown| sqlx::Error::AnyDriverError(Box::new(unknown)))
}

impl OperationTx<'_, '_> {
    pub(crate) async fn issue_magic_token(
        &mut self,
        payload: &MagicPayload,
        token_hash: &str,
        expires_at: DateTime<Utc>,
    ) -> Result<bool, sqlx::Error> {
        let Self::SqliteFamily(family) = self else {
            return Err(sqlx::Error::Protocol(
                "family magic issuance required".into(),
            ));
        };
        family.require_writer()?;
        family.require_system_context()?;
        let changed = family.execute("INSERT INTO magic_tokens (token_hash,kind,user_id,generation,expires_at,new_email) SELECT ?1,?2,?3,?4,?5,?6 WHERE EXISTS(SELECT 1 FROM users WHERE id=?3 AND auth_generation=?4 AND deleted_at IS NULL AND suspended_at IS NULL)",&[Cell::text(token_hash),Cell::text(&payload.kind),Cell::uuid(payload.user_id),Cell::Integer(i64::from(payload.generation)),Cell::Integer(expires_at.timestamp_micros()),Cell::optional_text(payload.new_email.as_deref())]).await?;
        Ok(changed == 1)
    }
}
