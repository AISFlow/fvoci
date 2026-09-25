use chrono::{DateTime, Duration, Utc};
use serde_json::json;
use sqlx::{PgPool, Postgres, Transaction};
use uuid::Uuid;

use crate::auth::scopes::{parse_api_token_scope, ApiTokenScope};
use crate::auth::session::{as_text_scale, as_week_starts_on, SessionUser};
use crate::auth::token::new_token;
use crate::db::context::{restore_system, session_is_live, set_self_user, set_system, set_tenant};
use crate::db::identity::{append_audit, append_event, AuditAppend, EventAppend};
use crate::db::workspace::{membership_role, WorkspaceRole};

pub const API_TOKEN_NAME_MAX: usize = 100;
pub const API_TOKEN_DEFAULT_TTL: Duration = Duration::days(90);
pub const API_TOKEN_CREATE_LIMIT: u32 = 20;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ApiTokenDbError {
    InvalidInput,
    Forbidden,
    NotFound,
}

#[derive(Debug, Clone)]
pub struct ApiTokenRecord {
    pub id: Uuid,
    pub workspace_id: Uuid,
    pub user_id: Option<Uuid>,
    pub name: String,
    pub scopes: Vec<ApiTokenScope>,
    pub expires_at: Option<DateTime<Utc>>,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Clone)]
pub struct ApiTokenCreated {
    pub record: ApiTokenRecord,
    pub token: String,
}

#[derive(Debug, Clone)]
pub struct ApiTokenSession {
    pub user: SessionUser,
    pub token_id: Uuid,
    pub workspace_id: Uuid,
    pub scopes: Vec<ApiTokenScope>,
}

#[derive(Debug, Clone)]
pub struct CreateApiTokenInput<'a> {
    pub name: &'a str,
    pub scopes: &'a [ApiTokenScope],
    pub unlimited: bool,
    pub service: bool,
}

type TokenRow = (
    Uuid,
    Uuid,
    Option<Uuid>,
    String,
    Vec<String>,
    Option<DateTime<Utc>>,
    DateTime<Utc>,
);

fn map_row(row: TokenRow) -> Result<ApiTokenRecord, ApiTokenDbError> {
    let (id, workspace_id, user_id, name, scopes, expires_at, created_at) = row;
    let mut parsed = Vec::with_capacity(scopes.len());
    for scope in scopes {
        parsed.push(parse_api_token_scope(&scope).ok_or(ApiTokenDbError::InvalidInput)?);
    }
    if parsed.is_empty() {
        return Err(ApiTokenDbError::InvalidInput);
    }
    Ok(ApiTokenRecord {
        id,
        workspace_id,
        user_id,
        name,
        scopes: parsed,
        expires_at,
        created_at,
    })
}

fn normalize_name(name: &str) -> Result<String, ApiTokenDbError> {
    let trimmed = name.trim();
    if trimmed.is_empty() || trimmed.chars().count() > API_TOKEN_NAME_MAX {
        return Err(ApiTokenDbError::InvalidInput);
    }
    Ok(trimmed.to_string())
}

async fn require_manage(
    tx: &mut Transaction<'_, Postgres>,
    workspace_id: Uuid,
    actor_user_id: Uuid,
) -> Result<Result<(), ApiTokenDbError>, sqlx::Error> {
    let role = membership_role(tx, workspace_id, actor_user_id).await?;
    if role
        .map(|r| r.at_least(WorkspaceRole::Admin))
        .unwrap_or(false)
    {
        Ok(Ok(()))
    } else {
        Ok(Err(ApiTokenDbError::Forbidden))
    }
}

async fn actor_is_active(
    tx: &mut Transaction<'_, Postgres>,
    actor_user_id: Uuid,
) -> Result<bool, sqlx::Error> {
    let live: Option<(bool,)> = sqlx::query_as(
        r#"
        SELECT (deleted_at IS NULL AND suspended_at IS NULL)
        FROM fvoci.users
        WHERE id = $1
        FOR UPDATE
        "#,
    )
    .bind(actor_user_id)
    .fetch_optional(&mut **tx)
    .await?;
    Ok(live.map(|(v,)| v).unwrap_or(false))
}

pub(crate) async fn remove_by_workspace(
    tx: &mut Transaction<'_, Postgres>,
    workspace_id: Uuid,
) -> Result<u64, sqlx::Error> {
    let result = sqlx::query("DELETE FROM fvoci.api_tokens WHERE workspace_id = $1")
        .bind(workspace_id)
        .execute(&mut **tx)
        .await?;
    Ok(result.rows_affected())
}

pub async fn create_api_token(
    pool: &PgPool,
    workspace_id: Uuid,
    actor_user_id: Uuid,
    credential_id: Uuid,
    input: CreateApiTokenInput<'_>,
    ip: Option<&str>,
) -> Result<Result<ApiTokenCreated, ApiTokenDbError>, sqlx::Error> {
    let name = match normalize_name(input.name) {
        Ok(name) => name,
        Err(err) => return Ok(Err(err)),
    };
    if input.scopes.is_empty() {
        return Ok(Err(ApiTokenDbError::InvalidInput));
    }
    let scope_values: Vec<String> = input
        .scopes
        .iter()
        .map(|s| s.as_str().to_string())
        .collect();
    let token = new_token();
    let id = Uuid::now_v7();
    let expires_at = if input.unlimited {
        None
    } else {
        Some(Utc::now() + API_TOKEN_DEFAULT_TTL)
    };
    let user_id = if input.service {
        None
    } else {
        Some(actor_user_id)
    };

    let mut tx = pool.begin().await?;
    set_tenant(&mut tx, workspace_id).await?;
    set_self_user(&mut tx, actor_user_id).await?;
    if !actor_is_active(&mut tx, actor_user_id).await? {
        tx.rollback().await?;
        return Ok(Err(ApiTokenDbError::Forbidden));
    }
    if !session_is_live(&mut tx, actor_user_id, credential_id).await? {
        tx.rollback().await?;
        return Ok(Err(ApiTokenDbError::Forbidden));
    }
    if let Err(err) = require_manage(&mut tx, workspace_id, actor_user_id).await? {
        tx.rollback().await?;
        return Ok(Err(err));
    }

    sqlx::query(
        r#"
        INSERT INTO fvoci.api_tokens (
            id, workspace_id, user_id, token_hash, name, scopes, expires_at
        ) VALUES ($1, $2, $3, $4, $5, $6, $7)
        "#,
    )
    .bind(id)
    .bind(workspace_id)
    .bind(user_id)
    .bind(&token.hash)
    .bind(&name)
    .bind(&scope_values)
    .bind(expires_at)
    .execute(&mut *tx)
    .await?;

    let row = sqlx::query_as::<_, TokenRow>(
        r#"
        SELECT id, workspace_id, user_id, name, scopes, expires_at, created_at
        FROM fvoci.api_tokens
        WHERE workspace_id = $1 AND id = $2
        "#,
    )
    .bind(workspace_id)
    .bind(id)
    .fetch_optional(&mut *tx)
    .await?;
    let Some(row) = row else {
        tx.rollback().await?;
        return Err(sqlx::Error::Protocol(
            "createApiToken: row missing after insert".into(),
        ));
    };
    let record = match map_row(row) {
        Ok(record) => record,
        Err(err) => {
            tx.rollback().await?;
            return Ok(Err(err));
        }
    };

    append_event(
        &mut tx,
        EventAppend {
            id: Uuid::now_v7(),
            workspace_id: Some(workspace_id),
            actor_user_id: Some(actor_user_id),
            verb: "api_token.created".to_string(),
            target_type: Some("api_token".to_string()),
            target_id: Some(id),
            payload: json!({
                "name": name,
                "scopes": scope_values,
                "service": input.service,
                "unlimited": input.unlimited,
            }),
        },
    )
    .await?;
    append_audit(
        &mut tx,
        AuditAppend {
            id: Uuid::now_v7(),
            workspace_id: Some(workspace_id),
            actor_user_id: Some(actor_user_id),
            verb: "api_token.created".to_string(),
            target_type: Some("api_token".to_string()),
            target_id: Some(id),
            payload: json!({
                "name": name,
                "scopes": scope_values,
                "service": input.service,
                "unlimited": input.unlimited,
            }),
            ip: ip.map(str::to_string),
        },
    )
    .await?;
    tx.commit().await?;
    Ok(Ok(ApiTokenCreated {
        record,
        token: token.token,
    }))
}

pub async fn list_api_tokens(
    pool: &PgPool,
    workspace_id: Uuid,
    actor_user_id: Uuid,
    credential_id: Uuid,
) -> Result<Result<Vec<ApiTokenRecord>, ApiTokenDbError>, sqlx::Error> {
    let mut tx = pool.begin().await?;
    set_tenant(&mut tx, workspace_id).await?;
    if !session_is_live(&mut tx, actor_user_id, credential_id).await? {
        tx.rollback().await?;
        return Ok(Err(ApiTokenDbError::Forbidden));
    }
    if let Err(err) = require_manage(&mut tx, workspace_id, actor_user_id).await? {
        tx.rollback().await?;
        return Ok(Err(err));
    }
    let rows = sqlx::query_as::<_, TokenRow>(
        r#"
        SELECT id, workspace_id, user_id, name, scopes, expires_at, created_at
        FROM fvoci.api_tokens
        WHERE workspace_id = $1
        ORDER BY created_at DESC, id DESC
        "#,
    )
    .bind(workspace_id)
    .fetch_all(&mut *tx)
    .await?;
    tx.commit().await?;
    let mut items = Vec::with_capacity(rows.len());
    for row in rows {
        match map_row(row) {
            Ok(item) => items.push(item),
            Err(err) => return Ok(Err(err)),
        }
    }
    Ok(Ok(items))
}

pub async fn revoke_api_token(
    pool: &PgPool,
    workspace_id: Uuid,
    actor_user_id: Uuid,
    credential_id: Uuid,
    id: Uuid,
    ip: Option<&str>,
) -> Result<Result<(), ApiTokenDbError>, sqlx::Error> {
    let mut tx = pool.begin().await?;
    set_tenant(&mut tx, workspace_id).await?;
    if !session_is_live(&mut tx, actor_user_id, credential_id).await? {
        tx.rollback().await?;
        return Ok(Err(ApiTokenDbError::Forbidden));
    }
    if let Err(err) = require_manage(&mut tx, workspace_id, actor_user_id).await? {
        tx.rollback().await?;
        return Ok(Err(err));
    }
    let row: Option<(Uuid, String)> = sqlx::query_as(
        r#"
        SELECT id, name
        FROM fvoci.api_tokens
        WHERE workspace_id = $1 AND id = $2
        FOR UPDATE
        "#,
    )
    .bind(workspace_id)
    .bind(id)
    .fetch_optional(&mut *tx)
    .await?;
    let Some((_, name)) = row else {
        tx.rollback().await?;
        return Ok(Err(ApiTokenDbError::NotFound));
    };
    sqlx::query("DELETE FROM fvoci.api_tokens WHERE workspace_id = $1 AND id = $2")
        .bind(workspace_id)
        .bind(id)
        .execute(&mut *tx)
        .await?;
    append_event(
        &mut tx,
        EventAppend {
            id: Uuid::now_v7(),
            workspace_id: Some(workspace_id),
            actor_user_id: Some(actor_user_id),
            verb: "api_token.revoked".to_string(),
            target_type: Some("api_token".to_string()),
            target_id: Some(id),
            payload: json!({ "name": name }),
        },
    )
    .await?;
    append_audit(
        &mut tx,
        AuditAppend {
            id: Uuid::now_v7(),
            workspace_id: Some(workspace_id),
            actor_user_id: Some(actor_user_id),
            verb: "api_token.revoked".to_string(),
            target_type: Some("api_token".to_string()),
            target_id: Some(id),
            payload: json!({ "name": name }),
            ip: ip.map(str::to_string),
        },
    )
    .await?;
    tx.commit().await?;
    Ok(Ok(()))
}

pub async fn list_user_api_tokens(
    pool: &PgPool,
    user_id: Uuid,
    credential_id: Uuid,
) -> Result<Result<Vec<ApiTokenRecord>, ApiTokenDbError>, sqlx::Error> {
    let mut tx = pool.begin().await?;
    if !session_is_live(&mut tx, user_id, credential_id).await? {
        tx.rollback().await?;
        return Ok(Err(ApiTokenDbError::Forbidden));
    }
    let previous = set_system(&mut tx).await?;
    let rows = sqlx::query_as::<_, TokenRow>(
        r#"
        SELECT id, workspace_id, user_id, name, scopes, expires_at, created_at
        FROM fvoci.api_tokens
        WHERE user_id = $1
        ORDER BY created_at DESC, id DESC
        "#,
    )
    .bind(user_id)
    .fetch_all(&mut *tx)
    .await?;
    restore_system(&mut tx, &previous).await?;
    tx.commit().await?;
    let mut items = Vec::with_capacity(rows.len());
    for row in rows {
        if let Ok(item) = map_row(row) {
            items.push(item);
        }
    }
    Ok(Ok(items))
}

pub async fn revoke_user_api_token(
    pool: &PgPool,
    user_id: Uuid,
    credential_id: Uuid,
    id: Uuid,
    ip: Option<&str>,
) -> Result<Result<(), ApiTokenDbError>, sqlx::Error> {
    let mut tx = pool.begin().await?;
    if !session_is_live(&mut tx, user_id, credential_id).await? {
        tx.rollback().await?;
        return Ok(Err(ApiTokenDbError::Forbidden));
    }
    let previous = set_system(&mut tx).await?;
    let row: Option<(Uuid, Uuid, String)> = sqlx::query_as(
        r#"
        SELECT id, workspace_id, name
        FROM fvoci.api_tokens
        WHERE id = $1 AND user_id = $2
        FOR UPDATE
        "#,
    )
    .bind(id)
    .bind(user_id)
    .fetch_optional(&mut *tx)
    .await?;
    let Some((_, workspace_id, name)) = row else {
        restore_system(&mut tx, &previous).await?;
        tx.rollback().await?;
        return Ok(Err(ApiTokenDbError::NotFound));
    };
    sqlx::query("DELETE FROM fvoci.api_tokens WHERE id = $1 AND user_id = $2")
        .bind(id)
        .bind(user_id)
        .execute(&mut *tx)
        .await?;
    append_event(
        &mut tx,
        EventAppend {
            id: Uuid::now_v7(),
            workspace_id: Some(workspace_id),
            actor_user_id: Some(user_id),
            verb: "api_token.revoked".to_string(),
            target_type: Some("api_token".to_string()),
            target_id: Some(id),
            payload: json!({ "name": name }),
        },
    )
    .await?;
    append_audit(
        &mut tx,
        AuditAppend {
            id: Uuid::now_v7(),
            workspace_id: Some(workspace_id),
            actor_user_id: Some(user_id),
            verb: "api_token.revoked".to_string(),
            target_type: Some("api_token".to_string()),
            target_id: Some(id),
            payload: json!({ "name": name }),
            ip: ip.map(str::to_string),
        },
    )
    .await?;
    restore_system(&mut tx, &previous).await?;
    tx.commit().await?;
    Ok(Ok(()))
}

pub async fn resolve_api_token_session(
    pool: &PgPool,
    token_hash: &str,
) -> Result<Option<ApiTokenSession>, sqlx::Error> {
    let mut tx = pool.begin().await?;
    let previous = set_system(&mut tx).await?;
    let row = sqlx::query_as::<
        _,
        (
            Uuid,
            Uuid,
            Option<Uuid>,
            Vec<String>,
            Option<DateTime<Utc>>,
            Uuid,
            String,
            String,
            Option<String>,
            i16,
            Option<DateTime<Utc>>,
            bool,
            String,
            String,
            i32,
            bool,
        ),
    >(
        r#"
        SELECT
            t.id,
            t.workspace_id,
            t.user_id,
            t.scopes,
            t.expires_at,
            u.id,
            u.email,
            u.given_name,
            u.family_name,
            u.text_scale,
            u.email_verified_at,
            u.is_instance_admin,
            u.locale,
            u.timezone,
            u.week_starts_on,
            (fvoci.app_user_password_hash(u.id) IS NOT NULL)
        FROM fvoci.api_tokens t
        INNER JOIN fvoci.users u ON u.id = t.user_id
        WHERE t.token_hash = $1
        "#,
    )
    .bind(token_hash)
    .fetch_optional(&mut *tx)
    .await?;

    let Some(row) = row else {
        restore_system(&mut tx, &previous).await?;
        tx.commit().await?;
        return Ok(None);
    };
    let (
        token_id,
        workspace_id,
        user_id,
        scopes,
        expires_at,
        resolved_user_id,
        email,
        given_name,
        family_name,
        text_scale,
        email_verified_at,
        is_instance_admin,
        locale,
        timezone,
        week_starts_on,
        has_password,
    ) = row;
    if user_id.is_none() {
        restore_system(&mut tx, &previous).await?;
        tx.commit().await?;
        return Ok(None);
    }
    if expires_at.is_some_and(|at| at <= Utc::now()) {
        restore_system(&mut tx, &previous).await?;
        tx.commit().await?;
        return Ok(None);
    }
    let inactive: Option<bool> = sqlx::query_scalar(
        "SELECT (deleted_at IS NOT NULL OR suspended_at IS NOT NULL) FROM fvoci.users WHERE id = $1",
    )
    .bind(resolved_user_id)
    .fetch_optional(&mut *tx)
    .await?;
    if inactive.unwrap_or(true) {
        restore_system(&mut tx, &previous).await?;
        tx.commit().await?;
        return Ok(None);
    }

    let mut parsed_scopes = Vec::new();
    for scope in scopes {
        if let Some(parsed) = parse_api_token_scope(&scope) {
            parsed_scopes.push(parsed);
        }
    }
    if parsed_scopes.is_empty() {
        restore_system(&mut tx, &previous).await?;
        tx.commit().await?;
        return Ok(None);
    }

    sqlx::query("UPDATE fvoci.api_tokens SET last_used_at = clock_timestamp() WHERE id = $1")
        .bind(token_id)
        .execute(&mut *tx)
        .await?;
    restore_system(&mut tx, &previous).await?;
    tx.commit().await?;

    Ok(Some(ApiTokenSession {
        token_id,
        workspace_id,
        scopes: parsed_scopes,
        user: SessionUser {
            user_id: resolved_user_id.to_string(),
            email,
            given_name,
            family_name,
            text_scale: as_text_scale(text_scale),
            session_id: token_id.to_string(),
            email_verified_at,
            has_password,
            is_instance_admin,
            locale: if locale.is_empty() {
                "ko".to_string()
            } else {
                locale
            },
            timezone: if timezone.is_empty() {
                "Asia/Seoul".to_string()
            } else {
                timezone
            },
            week_starts_on: as_week_starts_on(week_starts_on),
        },
    }))
}
