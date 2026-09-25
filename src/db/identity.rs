use chrono::{DateTime, Duration, Utc};
use serde_json::{json, Value};
use sqlx::{PgPool, Postgres, Transaction};
use uuid::Uuid;

use crate::auth::password::{hash_password, verify_password, Keyring};
use crate::auth::session::{as_text_scale, as_week_starts_on, SessionUser};
use crate::auth::token::{new_token, SESSION_TTL_SECS};
use crate::db::quota::acquire_admission_lock;

const SESSION_SLIDE_THRESHOLD_SECS: i64 = 15 * 24 * 60 * 60;

pub(crate) const INSTANCE_ADMIN_LOCK_KEY: i64 = 847_291_003_551;

pub(crate) struct EventAppend {
    pub id: Uuid,
    pub workspace_id: Option<Uuid>,
    pub actor_user_id: Option<Uuid>,
    pub verb: String,
    pub target_type: Option<String>,
    pub target_id: Option<Uuid>,
    pub payload: Value,
}

pub(crate) struct AuditAppend {
    pub id: Uuid,
    pub workspace_id: Option<Uuid>,
    pub actor_user_id: Option<Uuid>,
    pub verb: String,
    pub target_type: Option<String>,
    pub target_id: Option<Uuid>,
    pub payload: Value,
    pub ip: Option<String>,
}

pub struct SetupSessionParams {
    pub email: String,
    pub password_hash: String,
    pub given_name: String,
    pub family_name: Option<String>,
    pub workspace_slug: String,
    pub workspace_name: String,
    pub token_hash: String,
    pub expires_at: DateTime<Utc>,
    pub client_ip: Option<String>,
}

pub struct LiveSession {
    pub session_id: Uuid,
    pub expires_at: DateTime<Utc>,
    pub has_password: bool,
    pub user_id: Uuid,
    pub email: String,
    pub given_name: String,
    pub family_name: Option<String>,
    pub text_scale: i16,
    pub email_verified_at: Option<DateTime<Utc>>,
    pub is_instance_admin: bool,
    pub locale: String,
    pub timezone: String,
    pub week_starts_on: i32,
}

pub struct SetupResult {
    pub user_id: Uuid,
    pub workspace_id: Uuid,
    pub token: String,
}

pub enum SetupFirstOwnerResult {
    Created,
    Closed,
    SlugTaken,
}

pub struct SetupFirstOwnerInput {
    pub user_id: Uuid,
    pub email: String,
    pub password_hash: String,
    pub given_name: String,
    pub family_name: Option<String>,
    pub locale: String,
    pub timezone: String,
    pub week_starts_on: i32,
    pub text_scale: i16,
    pub workspace_id: Uuid,
    pub workspace_slug: String,
    pub workspace_name: String,
    pub session_id: Uuid,
    pub session_token_hash: String,
    pub session_expires_at: DateTime<Utc>,
    pub event_id: Uuid,
    pub audit_id: Uuid,
    pub ip: Option<String>,
}

pub async fn count_users(pool: &PgPool) -> Result<i64, sqlx::Error> {
    let row: (i64,) = sqlx::query_as("SELECT count(*) FROM fvoci.users")
        .fetch_one(pool)
        .await?;
    Ok(row.0)
}

pub async fn maybe_slide_session(
    pool: &PgPool,
    session_id: Uuid,
    expires_at: DateTime<Utc>,
) -> Result<DateTime<Utc>, sqlx::Error> {
    let remaining = expires_at - Utc::now();
    if remaining.num_seconds() < SESSION_SLIDE_THRESHOLD_SECS {
        let new_expires = Utc::now() + Duration::seconds(SESSION_TTL_SECS);
        sqlx::query(
            "UPDATE fvoci.sessions SET expires_at = $2, updated_at = now() WHERE id = $1 AND revoked_at IS NULL",
        )
        .bind(session_id)
        .bind(new_expires)
        .execute(pool)
        .await?;
        return Ok(new_expires);
    }
    Ok(expires_at)
}

pub async fn find_live_session(
    pool: &PgPool,
    token_hash: &str,
) -> Result<Option<LiveSession>, sqlx::Error> {
    let row = sqlx::query_as::<
        _,
        (
            Uuid,
            DateTime<Utc>,
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
            s.id,
            s.expires_at,
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
        FROM fvoci.app_session_by_token_hash($1) s
        INNER JOIN fvoci.users u ON u.id = s.user_id
        WHERE u.deleted_at IS NULL AND u.suspended_at IS NULL
        "#,
    )
    .bind(token_hash)
    .fetch_optional(pool)
    .await?;

    Ok(row.map(
        |(
            session_id,
            expires_at,
            user_id,
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
        )| {
            LiveSession {
                session_id,
                expires_at,
                has_password,
                user_id,
                email,
                given_name,
                family_name,
                text_scale,
                email_verified_at,
                is_instance_admin,
                locale,
                timezone,
                week_starts_on,
            }
        },
    ))
}

pub fn live_to_session_user(live: &LiveSession) -> SessionUser {
    SessionUser {
        user_id: live.user_id.to_string(),
        email: live.email.clone(),
        given_name: live.given_name.clone(),
        family_name: live.family_name.clone(),
        text_scale: as_text_scale(live.text_scale),
        session_id: live.session_id.to_string(),
        email_verified_at: live.email_verified_at,
        has_password: live.has_password,
        is_instance_admin: live.is_instance_admin,
        locale: if live.locale.is_empty() {
            "ko".to_string()
        } else {
            live.locale.clone()
        },
        timezone: if live.timezone.is_empty() {
            "Asia/Seoul".to_string()
        } else {
            live.timezone.clone()
        },
        week_starts_on: as_week_starts_on(live.week_starts_on),
    }
}

pub async fn setup_first_owner(
    pool: &PgPool,
    input: SetupFirstOwnerInput,
) -> Result<SetupFirstOwnerResult, sqlx::Error> {
    let mut tx = pool.begin().await?;
    acquire_admission_lock(&mut tx).await?;
    sqlx::query("SELECT pg_advisory_xact_lock($1)")
        .bind(INSTANCE_ADMIN_LOCK_KEY)
        .execute(&mut *tx)
        .await?;

    let existing: (i64,) = sqlx::query_as("SELECT count(*) FROM fvoci.users")
        .fetch_one(&mut *tx)
        .await?;
    if existing.0 > 0 {
        return Ok(SetupFirstOwnerResult::Closed);
    }

    sqlx::query("SELECT set_config('app.tenant_id', $1, true)")
        .bind(input.workspace_id.to_string())
        .execute(&mut *tx)
        .await?;

    if let Err(err) = insert_setup_rows(&mut tx, &input).await {
        if let Some(db_err) = err.as_database_error() {
            if db_err.constraint() == Some("workspaces_slug_unique") {
                return Ok(SetupFirstOwnerResult::SlugTaken);
            }
        }
        return Err(err);
    }

    tx.commit().await?;
    Ok(SetupFirstOwnerResult::Created)
}

async fn insert_setup_rows(
    tx: &mut Transaction<'_, Postgres>,
    input: &SetupFirstOwnerInput,
) -> Result<(), sqlx::Error> {
    sqlx::query(
        r#"
        INSERT INTO fvoci.users (
            id, email, password_hash, given_name, family_name, is_instance_admin,
            locale, timezone, week_starts_on, text_scale
        ) VALUES ($1, $2, $3, $4, $5, true, $6, $7, $8, $9)
        "#,
    )
    .bind(input.user_id)
    .bind(&input.email)
    .bind(&input.password_hash)
    .bind(&input.given_name)
    .bind(&input.family_name)
    .bind(&input.locale)
    .bind(&input.timezone)
    .bind(input.week_starts_on)
    .bind(input.text_scale)
    .execute(&mut **tx)
    .await?;

    sqlx::query("INSERT INTO fvoci.workspaces (id, slug, name) VALUES ($1, $2, $3)")
        .bind(input.workspace_id)
        .bind(&input.workspace_slug)
        .bind(&input.workspace_name)
        .execute(&mut **tx)
        .await?;

    sqlx::query(
        "INSERT INTO fvoci.memberships (workspace_id, user_id, role) VALUES ($1, $2, 'owner')",
    )
    .bind(input.workspace_id)
    .bind(input.user_id)
    .execute(&mut **tx)
    .await?;

    sqlx::query(
        r#"
        INSERT INTO fvoci.sessions (id, user_id, token_hash, expires_at)
        VALUES ($1, $2, $3, $4)
        "#,
    )
    .bind(input.session_id)
    .bind(input.user_id)
    .bind(&input.session_token_hash)
    .bind(input.session_expires_at)
    .execute(&mut **tx)
    .await?;

    let payload = json!({
        "userId": input.user_id.to_string(),
        "workspaceId": input.workspace_id.to_string(),
        "email": input.email,
    });

    append_event(
        tx,
        EventAppend {
            id: input.event_id,
            workspace_id: Some(input.workspace_id),
            actor_user_id: Some(input.user_id),
            verb: "instance.setup".to_string(),
            target_type: Some("workspace".to_string()),
            target_id: Some(input.workspace_id),
            payload: payload.clone(),
        },
    )
    .await?;

    append_audit(
        tx,
        AuditAppend {
            id: input.audit_id,
            workspace_id: Some(input.workspace_id),
            actor_user_id: Some(input.user_id),
            verb: "instance.setup".to_string(),
            target_type: Some("workspace".to_string()),
            target_id: Some(input.workspace_id),
            payload,
            ip: input.ip.clone(),
        },
    )
    .await?;

    Ok(())
}

pub async fn password_hash_by_id(
    pool: &PgPool,
    user_id: Uuid,
) -> Result<Option<String>, sqlx::Error> {
    let row: Option<(Option<String>,)> = sqlx::query_as("SELECT fvoci.app_user_password_hash($1)")
        .bind(user_id)
        .fetch_optional(pool)
        .await?;
    Ok(row.and_then(|r| r.0))
}

pub async fn find_user_id_by_email(
    pool: &PgPool,
    email: &str,
) -> Result<Option<(Uuid, Option<DateTime<Utc>>)>, sqlx::Error> {
    let row = sqlx::query_as::<_, (Uuid, Option<DateTime<Utc>>)>(
        "SELECT id, suspended_at FROM fvoci.users WHERE email = $1 AND deleted_at IS NULL",
    )
    .bind(email)
    .fetch_optional(pool)
    .await?;
    Ok(row)
}

pub async fn lock_sign_in(
    tx: &mut Transaction<'_, Postgres>,
    user_id: Uuid,
) -> Result<(), sqlx::Error> {
    sqlx::query("SELECT id FROM fvoci.users WHERE id = $1 FOR UPDATE")
        .bind(user_id)
        .execute(&mut **tx)
        .await?;
    Ok(())
}

pub async fn create_session(
    tx: &mut Transaction<'_, Postgres>,
    session_id: Uuid,
    user_id: Uuid,
    token_hash: &str,
    expires_at: DateTime<Utc>,
) -> Result<(), sqlx::Error> {
    sqlx::query(
        "INSERT INTO fvoci.sessions (id, user_id, token_hash, expires_at) VALUES ($1, $2, $3, $4)",
    )
    .bind(session_id)
    .bind(user_id)
    .bind(token_hash)
    .bind(expires_at)
    .execute(&mut **tx)
    .await?;
    Ok(())
}

pub async fn append_auth_login_event(
    tx: &mut Transaction<'_, Postgres>,
    event_id: Uuid,
    user_id: Uuid,
) -> Result<(), sqlx::Error> {
    sqlx::query("SELECT set_config('app.system_ctx', 'on', true)")
        .execute(&mut **tx)
        .await?;
    append_event(
        tx,
        EventAppend {
            id: event_id,
            workspace_id: None,
            actor_user_id: Some(user_id),
            verb: "auth.login".to_string(),
            target_type: Some("user".to_string()),
            target_id: Some(user_id),
            payload: json!({"userId": user_id.to_string(), "method": "password"}),
        },
    )
    .await?;
    Ok(())
}

pub async fn revoke_session(
    pool: &PgPool,
    token_hash: &str,
    actor_user_id: Option<Uuid>,
) -> Result<(), sqlx::Error> {
    let mut tx = pool.begin().await?;
    let session = sqlx::query_as::<_, (Option<Uuid>,)>(
        "SELECT user_id FROM fvoci.app_session_by_token_hash($1)",
    )
    .bind(token_hash)
    .fetch_optional(&mut *tx)
    .await?;

    let lock_id = session.and_then(|s| s.0).or(actor_user_id);
    if let Some(user_id) = lock_id {
        lock_sign_in(&mut tx, user_id).await?;
    }

    let revoked = if let Some((session_id,)) =
        sqlx::query_as::<_, (Uuid,)>("SELECT id FROM fvoci.app_session_by_token_hash($1)")
            .bind(token_hash)
            .fetch_optional(&mut *tx)
            .await?
    {
        sqlx::query("UPDATE fvoci.sessions SET revoked_at = now() WHERE id = $1")
            .bind(session_id)
            .execute(&mut *tx)
            .await?;
        true
    } else {
        false
    };

    if revoked {
        sqlx::query("SELECT set_config('app.system_ctx', 'on', true)")
            .execute(&mut *tx)
            .await?;

        let event_id = Uuid::now_v7();
        append_event(
            &mut tx,
            EventAppend {
                id: event_id,
                workspace_id: None,
                actor_user_id,
                verb: "auth.logout".to_string(),
                target_type: None,
                target_id: None,
                payload: json!({}),
            },
        )
        .await?;
    }

    tx.commit().await?;
    Ok(())
}

pub enum FamilyNamePatch {
    Preserve,
    Clear,
    Set(String),
}

pub struct ProfilePatch {
    pub given_name: String,
    pub family_name: FamilyNamePatch,
    pub locale: Option<String>,
    pub timezone: Option<String>,
    pub week_starts_on: Option<i32>,
    pub text_scale: Option<i16>,
}

pub async fn update_profile(
    pool: &PgPool,
    user_id: Uuid,
    session_id: Uuid,
    patch: ProfilePatch,
) -> Result<bool, sqlx::Error> {
    let mut tx = pool.begin().await?;

    let locked = sqlx::query_as::<_, (Uuid,)>(
        r#"
        SELECT u.id
        FROM fvoci.users u
        INNER JOIN fvoci.sessions s ON s.id = $2 AND s.user_id = u.id
        WHERE u.id = $1
        FOR UPDATE OF u, s
        "#,
    )
    .bind(user_id)
    .bind(session_id)
    .fetch_optional(&mut *tx)
    .await?;

    if locked.is_none() {
        tx.rollback().await?;
        return Ok(false);
    }

    let still_live: Option<(bool,)> = sqlx::query_as(
        r#"
        SELECT (
            s.revoked_at IS NULL
            AND s.expires_at > clock_timestamp()
            AND u.deleted_at IS NULL
            AND u.suspended_at IS NULL
        )
        FROM fvoci.users u
        INNER JOIN fvoci.sessions s ON s.id = $2 AND s.user_id = u.id
        WHERE u.id = $1
        "#,
    )
    .bind(user_id)
    .bind(session_id)
    .fetch_optional(&mut *tx)
    .await?;

    if !still_live.map(|(live,)| live).unwrap_or(false) {
        tx.rollback().await?;
        return Ok(false);
    }

    match &patch.family_name {
        FamilyNamePatch::Set(family_name) => {
            sqlx::query(
                "UPDATE fvoci.users SET given_name = $2, family_name = $3, updated_at = now() WHERE id = $1",
            )
            .bind(user_id)
            .bind(&patch.given_name)
            .bind(family_name)
            .execute(&mut *tx)
            .await?;
        }
        FamilyNamePatch::Clear => {
            sqlx::query(
                "UPDATE fvoci.users SET given_name = $2, family_name = NULL, updated_at = now() WHERE id = $1",
            )
            .bind(user_id)
            .bind(&patch.given_name)
            .execute(&mut *tx)
            .await?;
        }
        FamilyNamePatch::Preserve => {
            sqlx::query("UPDATE fvoci.users SET given_name = $2, updated_at = now() WHERE id = $1")
                .bind(user_id)
                .bind(&patch.given_name)
                .execute(&mut *tx)
                .await?;
        }
    }

    if patch.locale.is_some()
        || patch.timezone.is_some()
        || patch.week_starts_on.is_some()
        || patch.text_scale.is_some()
    {
        sqlx::query(
            r#"
            UPDATE fvoci.users SET
                locale = COALESCE($2, locale),
                timezone = COALESCE($3, timezone),
                week_starts_on = COALESCE($4, week_starts_on),
                text_scale = COALESCE($5, text_scale),
                updated_at = now()
            WHERE id = $1
            "#,
        )
        .bind(user_id)
        .bind(patch.locale.as_deref())
        .bind(patch.timezone.as_deref())
        .bind(patch.week_starts_on)
        .bind(patch.text_scale)
        .execute(&mut *tx)
        .await?;
    }

    sqlx::query("SELECT set_config('app.system_ctx', 'on', true)")
        .execute(&mut *tx)
        .await?;

    let event_id = Uuid::now_v7();
    let mut payload = serde_json::Map::new();
    payload.insert("userId".to_string(), json!(user_id.to_string()));
    payload.insert("givenName".to_string(), json!(patch.given_name));
    match &patch.family_name {
        FamilyNamePatch::Preserve => {}
        FamilyNamePatch::Clear => {
            payload.insert("familyName".to_string(), Value::Null);
        }
        FamilyNamePatch::Set(value) => {
            payload.insert("familyName".to_string(), Value::String(value.clone()));
        }
    }
    let payload = Value::Object(payload);
    append_event(
        &mut tx,
        EventAppend {
            id: event_id,
            workspace_id: None,
            actor_user_id: Some(user_id),
            verb: "user.name_updated".to_string(),
            target_type: Some("user".to_string()),
            target_id: Some(user_id),
            payload: payload.clone(),
        },
    )
    .await?;

    let audit_id = Uuid::now_v7();
    append_audit(
        &mut tx,
        AuditAppend {
            id: audit_id,
            workspace_id: None,
            actor_user_id: Some(user_id),
            verb: "user.name_updated".to_string(),
            target_type: Some("user".to_string()),
            target_id: Some(user_id),
            payload,
            ip: None,
        },
    )
    .await?;

    tx.commit().await?;
    Ok(true)
}

pub async fn set_password_hash(
    tx: &mut Transaction<'_, Postgres>,
    user_id: Uuid,
    password_hash: &str,
) -> Result<(), sqlx::Error> {
    sqlx::query("SELECT fvoci.app_user_set_password_hash($1, $2)")
        .bind(user_id)
        .bind(password_hash)
        .execute(&mut **tx)
        .await?;
    Ok(())
}

pub async fn revoke_all_sessions_for_user(
    tx: &mut Transaction<'_, Postgres>,
    user_id: Uuid,
) -> Result<u64, sqlx::Error> {
    let result = sqlx::query(
        r#"
        UPDATE fvoci.sessions
        SET revoked_at = now(), updated_at = now()
        WHERE user_id = $1 AND revoked_at IS NULL
        "#,
    )
    .bind(user_id)
    .execute(&mut **tx)
    .await?;
    Ok(result.rows_affected())
}

pub async fn find_reset_user_by_email(
    pool: &PgPool,
    email: &str,
) -> Result<Option<(Uuid, i32, Option<DateTime<Utc>>)>, sqlx::Error> {
    sqlx::query_as(
        r#"
        SELECT id, auth_generation, suspended_at
        FROM fvoci.users
        WHERE email = $1 AND deleted_at IS NULL
        "#,
    )
    .bind(email)
    .fetch_optional(pool)
    .await
}

pub async fn rehash_password_if_unchanged(
    pool: &PgPool,
    user_id: Uuid,
    new_hash: &str,
    expected_hash: &str,
) -> Result<(), sqlx::Error> {
    sqlx::query("SELECT fvoci.app_user_rehash_password_hash($1, $2, $3)")
        .bind(user_id)
        .bind(new_hash)
        .bind(expected_hash)
        .execute(pool)
        .await?;
    Ok(())
}

pub async fn issue_session(pool: &PgPool, user_id: Uuid) -> Result<Option<String>, sqlx::Error> {
    let mut tx = pool.begin().await?;
    lock_sign_in(&mut tx, user_id).await?;

    // Recheck under the row lock: a withdraw (deleted_at) or suspension that
    // committed after the password check must not receive a new session.
    let suspended: Option<(Option<DateTime<Utc>>,)> =
        sqlx::query_as("SELECT suspended_at FROM fvoci.users WHERE id = $1 AND deleted_at IS NULL")
            .bind(user_id)
            .fetch_optional(&mut *tx)
            .await?;
    if suspended.map(|s| s.0.is_some()).unwrap_or(true) {
        tx.rollback().await?;
        return Ok(None);
    }

    let token = new_token();
    let expires_at = Utc::now() + Duration::seconds(SESSION_TTL_SECS);
    create_session(&mut tx, Uuid::now_v7(), user_id, &token.hash, expires_at).await?;
    append_auth_login_event(&mut tx, Uuid::now_v7(), user_id).await?;
    tx.commit().await?;
    Ok(Some(token.token))
}

pub async fn authenticate_password(
    pool: &PgPool,
    email: &str,
    password: &str,
    ring: &Keyring,
) -> Result<Option<Uuid>, sqlx::Error> {
    let user = find_user_id_by_email(pool, email).await?;
    let (user_id, suspended) = match user {
        Some(row) => row,
        None => {
            let _ = verify_password(None, password, ring).await;
            return Ok(None);
        }
    };

    let stored = password_hash_by_id(pool, user_id).await?;
    let verified = verify_password(stored.as_deref(), password, ring).await;
    if !verified.ok || suspended.is_some() {
        return Ok(None);
    }

    if verified.needs_pepper_rotation {
        if let Some(old_hash) = stored.as_deref() {
            if let Ok(new_hash) = hash_password(password, ring).await {
                let _ = rehash_password_if_unchanged(pool, user_id, &new_hash, old_hash).await;
            }
        }
    }

    Ok(Some(user_id))
}

pub(crate) async fn append_event(
    tx: &mut Transaction<'_, Postgres>,
    row: EventAppend,
) -> Result<(), sqlx::Error> {
    append_event_channel(tx, row, "web").await
}

/// `append_event` with an explicit channel (`webhook` for inbound GitHub).
pub(crate) async fn append_event_channel(
    tx: &mut Transaction<'_, Postgres>,
    row: EventAppend,
    channel: &str,
) -> Result<(), sqlx::Error> {
    sqlx::query(
        r#"
        INSERT INTO fvoci.events (
            id, workspace_id, actor_user_id, verb, target_type, target_id, payload, channel
        ) VALUES ($1, $2, $3, $4, $5, $6, $7, $8)
        "#,
    )
    .bind(row.id)
    .bind(row.workspace_id)
    .bind(row.actor_user_id)
    .bind(&row.verb)
    .bind(row.target_type.as_deref())
    .bind(row.target_id)
    .bind(row.payload)
    .bind(channel)
    .execute(&mut **tx)
    .await?;
    Ok(())
}

pub(crate) async fn append_audit(
    tx: &mut Transaction<'_, Postgres>,
    row: AuditAppend,
) -> Result<(), sqlx::Error> {
    sqlx::query(
        r#"
        INSERT INTO fvoci.audit_log (
            id, workspace_id, actor_user_id, verb, target_type, target_id, payload, ip
        ) VALUES ($1, $2, $3, $4, $5, $6, $7, $8::inet)
        "#,
    )
    .bind(row.id)
    .bind(row.workspace_id)
    .bind(row.actor_user_id)
    .bind(&row.verb)
    .bind(row.target_type.as_deref())
    .bind(row.target_id)
    .bind(row.payload)
    .bind(row.ip.as_deref())
    .execute(&mut **tx)
    .await?;
    Ok(())
}

pub fn new_setup_input(params: SetupSessionParams) -> SetupFirstOwnerInput {
    let user_id = Uuid::now_v7();
    let workspace_id = Uuid::now_v7();
    SetupFirstOwnerInput {
        user_id,
        email: params.email,
        password_hash: params.password_hash,
        given_name: params.given_name,
        family_name: params.family_name,
        locale: "ko".to_string(),
        timezone: "Asia/Seoul".to_string(),
        week_starts_on: 1,
        text_scale: 16,
        workspace_id,
        workspace_slug: params.workspace_slug,
        workspace_name: params.workspace_name,
        session_id: Uuid::now_v7(),
        session_token_hash: params.token_hash,
        session_expires_at: params.expires_at,
        event_id: Uuid::now_v7(),
        audit_id: Uuid::now_v7(),
        ip: params.client_ip,
    }
}
