use chrono::{DateTime, Duration, Utc};
use serde_json::{json, Value};
use sqlx::{PgPool, Postgres, Transaction};
use uuid::Uuid;

use super::backend::{Backend, OperationTx};
use super::codec::Cell;
use crate::auth::password::{hash_password, verify_password, Keyring};
use crate::auth::session::{as_text_scale, as_week_starts_on, SessionUser};
use crate::auth::token::SESSION_TTL_SECS;
use crate::db::quota::acquire_admission_lock;

const SESSION_SLIDE_THRESHOLD_SECS: i64 = 15 * 24 * 60 * 60;

/// Taken only through [`lock_instance_admin_changes`].
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
    count_users_backend(&Backend::Postgres(pool.clone())).await
}

pub async fn count_users_backend(backend: &Backend) -> Result<i64, sqlx::Error> {
    let mut tx = backend.begin_read().await?;
    let count = tx.operation().setup_user_count().await?;
    tx.rollback().await?;
    Ok(count)
}

pub async fn maybe_slide_session(
    pool: &PgPool,
    session_id: Uuid,
    expires_at: DateTime<Utc>,
) -> Result<DateTime<Utc>, sqlx::Error> {
    maybe_slide_session_backend(&Backend::Postgres(pool.clone()), session_id, expires_at).await
}

pub async fn maybe_slide_session_backend(
    backend: &Backend,
    session_id: Uuid,
    expires_at: DateTime<Utc>,
) -> Result<DateTime<Utc>, sqlx::Error> {
    let remaining = expires_at - Utc::now();
    if remaining.num_seconds() >= SESSION_SLIDE_THRESHOLD_SECS {
        return Ok(expires_at);
    }
    let new_expires = stored_now() + Duration::seconds(SESSION_TTL_SECS);
    let mut tx = backend.begin_write().await?;
    tx.operation()
        .slide_session(session_id, new_expires)
        .await?;
    tx.commit().await.map_err(|unknown| unknown.source)?;
    Ok(new_expires)
}

/// Generated DB instants deliberately start at storage precision. Requests
/// containing finer precision still fail the checked Cell::instant boundary.
pub(crate) fn stored_now() -> DateTime<Utc> {
    DateTime::from_timestamp_micros(Utc::now().timestamp_micros())
        .expect("current time in chrono range")
}

pub async fn find_live_session(
    pool: &PgPool,
    hash: &str,
) -> Result<Option<LiveSession>, sqlx::Error> {
    find_live_session_backend(&Backend::Postgres(pool.clone()), hash).await
}

pub async fn find_live_session_backend(
    backend: &Backend,
    hash: &str,
) -> Result<Option<LiveSession>, sqlx::Error> {
    if let Backend::Postgres(pool) = backend {
        return find_live_session_pg(pool, hash).await;
    }
    let mut tx = backend.begin_read().await?;
    let live = match tx.operation() {
        OperationTx::SqliteFamily(ref mut family) => {
            let rows=family.query("SELECT s.id,s.expires_at,u.id,u.email,u.given_name,u.family_name,u.text_scale,u.email_verified_at,u.is_instance_admin,u.locale,u.timezone,u.week_starts_on,(u.password_hash IS NOT NULL) FROM sessions s JOIN users u ON u.id=s.user_id WHERE s.token_hash=?1 AND s.revoked_at IS NULL AND s.expires_at>(unixepoch()*1000000+CAST(substr(strftime('%f','now'),4,3) AS INTEGER)*1000) AND u.deleted_at IS NULL AND u.suspended_at IS NULL LIMIT 1", &[Cell::text(hash)]).await?;
            rows.first()
                .map(|r| {
                    Ok::<LiveSession, sqlx::Error>(LiveSession {
                        session_id: r.cell(0)?.id()?,
                        expires_at: r.cell(1)?.datetime()?,
                        user_id: r.cell(2)?.id()?,
                        email: r.cell(3)?.string()?,
                        given_name: r.cell(4)?.string()?,
                        family_name: r.cell(5)?.optional(Cell::string)?,
                        text_scale: i16::try_from(r.cell(6)?.integer()?).map_err(|_| {
                            sqlx::Error::Protocol("stored text_scale exceeds i16".into())
                        })?,
                        email_verified_at: r.cell(7)?.optional(Cell::datetime)?,
                        is_instance_admin: r.cell(8)?.boolean()?,
                        locale: r.cell(9)?.string()?,
                        timezone: r.cell(10)?.string()?,
                        week_starts_on: i32::try_from(r.cell(11)?.integer()?).map_err(|_| {
                            sqlx::Error::Protocol("stored week_starts_on exceeds i32".into())
                        })?,
                        has_password: r.cell(12)?.boolean()?,
                    })
                })
                .transpose()?
        }
        OperationTx::Postgres(_) => {
            return Err(sqlx::Error::Protocol(
                "family session read has unexpected transaction kind".into(),
            ));
        }
    };
    tx.rollback().await?;
    Ok(live)
}

async fn find_live_session_pg(
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
    setup_first_owner_backend(&Backend::Postgres(pool.clone()), input).await
}

/// One setup policy and commit owner for all selected backends. SQLite's
/// writer reservation is already held before the first setup/admission read.
pub async fn setup_first_owner_backend(
    backend: &Backend,
    input: SetupFirstOwnerInput,
) -> Result<SetupFirstOwnerResult, sqlx::Error> {
    let mut tx = backend.begin_write().await?;
    let mut operation = tx.operation();
    operation.lock_instance_admin_changes().await?;
    if operation.setup_user_count().await? > 0 {
        tx.rollback().await?;
        return Ok(SetupFirstOwnerResult::Closed);
    }
    operation.set_tenant(input.workspace_id).await?;
    // This check also provides the SQLite-family SlugTaken outcome without
    // parsing driver error strings. It is protected by the setup writer lock.
    if operation.setup_slug_exists(&input.workspace_slug).await? {
        tx.rollback().await?;
        return Ok(SetupFirstOwnerResult::SlugTaken);
    }
    if let Err(err) = insert_setup_rows(&mut operation, &input).await {
        let slug_taken = err
            .as_database_error()
            .is_some_and(|err| err.constraint() == Some("workspaces_slug_unique"));
        tx.rollback().await?;
        if slug_taken {
            return Ok(SetupFirstOwnerResult::SlugTaken);
        }
        return Err(err);
    }
    tx.commit().await.map_err(|unknown| unknown.source)?;
    Ok(SetupFirstOwnerResult::Created)
}

async fn insert_setup_rows(
    tx: &mut OperationTx<'_, '_>,
    input: &SetupFirstOwnerInput,
) -> Result<(), sqlx::Error> {
    tx.insert_setup_user(input).await?;
    tx.insert_setup_workspace(input).await?;
    tx.insert_setup_membership(input).await?;
    tx.create_session(
        input.session_id,
        input.user_id,
        &input.session_token_hash,
        input.session_expires_at,
    )
    .await?;
    let payload = json!({"userId":input.user_id.to_string(),"workspaceId":input.workspace_id.to_string(),"email":input.email});
    tx.append_event(EventAppend {
        id: input.event_id,
        workspace_id: Some(input.workspace_id),
        actor_user_id: Some(input.user_id),
        verb: "instance.setup".into(),
        target_type: Some("workspace".into()),
        target_id: Some(input.workspace_id),
        payload: payload.clone(),
    })
    .await?;
    tx.append_audit(AuditAppend {
        id: input.audit_id,
        workspace_id: Some(input.workspace_id),
        actor_user_id: Some(input.user_id),
        verb: "instance.setup".into(),
        target_type: Some("workspace".into()),
        target_id: Some(input.workspace_id),
        payload,
        ip: input.ip.clone(),
    })
    .await
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

/// Lock prologue shared by first-owner setup, `admin::patch_instance_user` and
/// the account lifecycle (`account::lock_account_for`): the admission lock,
/// then the instance-admin lock, both transaction-scoped. Callers that act on
/// existing users (`patch_instance_user`, `lock_account_for`) then take
/// `lock_membership_users` and [`lock_sign_in`]; first-owner setup takes no
/// further lock. One order in every caller keeps these transactions from
/// deadlocking on each other.
pub(crate) async fn lock_instance_admin_changes(
    tx: &mut Transaction<'_, Postgres>,
) -> Result<(), sqlx::Error> {
    acquire_admission_lock(tx).await?;
    sqlx::query("SELECT pg_advisory_xact_lock($1)")
        .bind(INSTANCE_ADMIN_LOCK_KEY)
        .execute(&mut **tx)
        .await?;
    Ok(())
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

pub async fn revoke_session(
    pool: &PgPool,
    token_hash: &str,
    actor_user_id: Option<Uuid>,
) -> Result<(), sqlx::Error> {
    revoke_session_with_push(pool, token_hash, actor_user_id, None).await
}

/// Logout: revokes the session and, in the same transaction, disconnects this
/// browser's Web Push subscription for the session's user (rows registered by
/// this session plus `push_endpoint` reported by the browser).
pub async fn revoke_session_with_push(
    pool: &PgPool,
    token_hash: &str,
    actor_user_id: Option<Uuid>,
    push_endpoint: Option<&str>,
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
        Some(session_id)
    } else {
        None
    };

    if let Some(session_id) = revoked {
        sqlx::query("SELECT set_config('app.system_ctx', 'on', true)")
            .execute(&mut *tx)
            .await?;
        if let Some(user_id) = session.and_then(|s| s.0) {
            crate::push::disconnect_browser(&mut tx, user_id, session_id, push_endpoint).await?;
        }

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

/// Preserve the PostgreSQL sign-in lock/RLS path; SQLite-family writers use
/// the existing exclusive writer reservation for the entire logout operation.
pub async fn revoke_session_with_push_backend(
    backend: &Backend,
    token_hash: &str,
    actor_user_id: Option<Uuid>,
    push_endpoint: Option<&str>,
) -> Result<(), sqlx::Error> {
    if let Backend::Postgres(pool) = backend {
        return revoke_session_with_push(pool, token_hash, actor_user_id, push_endpoint).await;
    }
    let mut tx = backend.begin_write().await?;
    let mut operation = tx.operation();
    let session = match &mut operation {
        OperationTx::SqliteFamily(family) => {
            family.require_writer()?;
            let rows = family.query(
                "SELECT id,user_id FROM sessions WHERE token_hash=?1 AND revoked_at IS NULL AND expires_at>(unixepoch()*1000000+CAST(substr(strftime('%f','now'),4,3) AS INTEGER)*1000)",
                &[Cell::text(token_hash)],
            ).await?;
            rows.first()
                .map(|row| Ok::<_, sqlx::Error>((row.cell(0)?.id()?, row.cell(1)?.id()?)))
                .transpose()?
        }
        OperationTx::Postgres(_) => {
            return Err(sqlx::Error::Protocol(
                "family session revoke has unexpected transaction kind".into(),
            ));
        }
    };
    if let Some((session_id, user_id)) = session {
        if let OperationTx::SqliteFamily(family) = &mut operation {
            family
                .execute(
                    "UPDATE sessions SET revoked_at=?2 WHERE id=?1",
                    &[Cell::uuid(session_id), Cell::instant(stored_now())?],
                )
                .await?;
            family.execute(
                "DELETE FROM push_subscriptions WHERE user_id=?1 AND (session_id=?2 OR endpoint=?3)",
                &[Cell::uuid(user_id), Cell::uuid(session_id), Cell::optional_text(push_endpoint)],
            ).await?;
            family.replace_system_context(true);
        }
        operation
            .append_event(EventAppend {
                id: Uuid::now_v7(),
                workspace_id: None,
                actor_user_id,
                verb: "auth.logout".to_string(),
                target_type: None,
                target_id: None,
                payload: json!({}),
            })
            .await?;
    }
    tx.commit().await.map_err(|unknown| unknown.source)?;
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

pub async fn authenticate_password(
    pool: &PgPool,
    email: &str,
    password: &str,
    ring: &Keyring,
) -> Result<Option<Uuid>, sqlx::Error> {
    authenticate_password_backend(&Backend::Postgres(pool.clone()), email, password, ring).await
}

pub async fn authenticate_password_backend(
    backend: &Backend,
    email: &str,
    password: &str,
    ring: &Keyring,
) -> Result<Option<Uuid>, sqlx::Error> {
    let mut tx = backend.begin_read().await?;
    let user = tx.operation().password_user_by_email(email).await?;
    tx.rollback().await?;
    let (user_id, suspended) = match user {
        Some(row) => row,
        None => {
            let _ = verify_password(None, password, ring).await;
            return Ok(None);
        }
    };
    let mut tx = backend.begin_read().await?;
    let stored = tx.operation().password_hash(user_id).await?;
    tx.rollback().await?;
    let verified = verify_password(stored.as_deref(), password, ring).await;
    if !verified.ok || suspended.is_some() {
        return Ok(None);
    }
    if verified.needs_pepper_rotation {
        if let Some(old_hash) = stored.as_deref() {
            if let Ok(new_hash) = hash_password(password, ring).await {
                // Preserve the existing optional CAS rehash policy. Successful
                // authentication does not depend on opportunistic rotation.
                let _ = rehash_password_backend(backend, user_id, &new_hash, old_hash).await;
            }
        }
    }
    Ok(Some(user_id))
}

async fn rehash_password_backend(
    backend: &Backend,
    user: Uuid,
    new_hash: &str,
    old_hash: &str,
) -> Result<(), sqlx::Error> {
    let mut tx = backend.begin_write().await?;
    tx.operation()
        .rehash_password(user, new_hash, old_hash)
        .await?;
    tx.commit().await.map_err(|unknown| unknown.source)
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

impl OperationTx<'_, '_> {
    async fn lock_instance_admin_changes(&mut self) -> Result<(), sqlx::Error> {
        match self {
            Self::Postgres(tx) => lock_instance_admin_changes(tx).await,
            Self::SqliteFamily(tx) => tx.require_writer(),
        }
    }
    async fn password_user_by_email(
        &mut self,
        email: &str,
    ) -> Result<Option<(Uuid, Option<DateTime<Utc>>)>, sqlx::Error> {
        match self {
            Self::Postgres(tx) => {
                sqlx::query_as(
                    "SELECT id,suspended_at FROM fvoci.users WHERE email=$1 AND deleted_at IS NULL",
                )
                .bind(email)
                .fetch_optional(&mut ***tx)
                .await
            }
            Self::SqliteFamily(tx) => {
                let rows=tx.query("SELECT id,suspended_at FROM users WHERE email=?1 AND deleted_at IS NULL LIMIT 1", &[Cell::text(email)]).await?;
                rows.first()
                    .map(|r| {
                        Ok::<_, sqlx::Error>((
                            r.cell(0)?.id()?,
                            r.cell(1)?.optional(Cell::datetime)?,
                        ))
                    })
                    .transpose()
            }
        }
    }
    async fn password_hash(&mut self, user: Uuid) -> Result<Option<String>, sqlx::Error> {
        match self {
            Self::Postgres(tx) => {
                sqlx::query_scalar("SELECT fvoci.app_user_password_hash($1)")
                    .bind(user)
                    .fetch_one(&mut ***tx)
                    .await
            }
            Self::SqliteFamily(tx) => {
                let rows=tx.query("SELECT password_hash FROM users WHERE id=?1 AND deleted_at IS NULL LIMIT 1", &[Cell::uuid(user)]).await?;
                match rows.first() {
                    Some(r) => r.cell(0)?.optional(Cell::string),
                    None => Ok(None),
                }
            }
        }
    }
    async fn rehash_password(
        &mut self,
        user: Uuid,
        new_hash: &str,
        old_hash: &str,
    ) -> Result<(), sqlx::Error> {
        match self {
            Self::Postgres(tx) => {
                sqlx::query("SELECT fvoci.app_user_rehash_password_hash($1,$2,$3)")
                    .bind(user)
                    .bind(new_hash)
                    .bind(old_hash)
                    .execute(&mut ***tx)
                    .await?;
            }
            Self::SqliteFamily(tx) => {
                tx.require_writer()?;
                tx.execute("UPDATE users SET password_hash=?2,updated_at=(unixepoch()*1000000+CAST(substr(strftime('%f','now'),4,3) AS INTEGER)*1000) WHERE id=?1 AND deleted_at IS NULL AND password_hash=?3", &[Cell::uuid(user),Cell::text(new_hash),Cell::text(old_hash)]).await?;
            }
        }
        Ok(())
    }
    pub(crate) async fn lock_sign_in(&mut self, user: Uuid) -> Result<(), sqlx::Error> {
        match self {
            Self::Postgres(tx) => lock_sign_in(tx, user).await,
            Self::SqliteFamily(tx) => tx.require_writer(),
        }
    }
    async fn slide_session(&mut self, id: Uuid, expires: DateTime<Utc>) -> Result<(), sqlx::Error> {
        match self {
            Self::Postgres(tx) => {
                sqlx::query("UPDATE fvoci.sessions SET expires_at=$2,updated_at=now() WHERE id=$1 AND revoked_at IS NULL").bind(id).bind(expires).execute(&mut ***tx).await?;
            }
            Self::SqliteFamily(tx) => {
                tx.require_writer()?;
                tx.execute("UPDATE sessions SET expires_at=?2,updated_at=(unixepoch()*1000000+CAST(substr(strftime('%f','now'),4,3) AS INTEGER)*1000) WHERE id=?1 AND revoked_at IS NULL",&[Cell::uuid(id),Cell::instant(expires)?]).await?;
            }
        }
        Ok(())
    }
    async fn setup_user_count(&mut self) -> Result<i64, sqlx::Error> {
        match self {
            Self::Postgres(tx) => {
                sqlx::query_scalar("SELECT count(*) FROM fvoci.users")
                    .fetch_one(&mut ***tx)
                    .await
            }
            Self::SqliteFamily(tx) => tx
                .query("SELECT count(*) FROM users", &[])
                .await?
                .first()
                .ok_or(sqlx::Error::RowNotFound)?
                .cell(0)?
                .integer(),
        }
    }
    async fn setup_slug_exists(&mut self, slug: &str) -> Result<bool, sqlx::Error> {
        match self {
            Self::Postgres(tx) => {
                sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM fvoci.workspaces WHERE slug=$1)")
                    .bind(slug)
                    .fetch_one(&mut ***tx)
                    .await
            }
            Self::SqliteFamily(tx) => tx
                .query(
                    "SELECT EXISTS(SELECT 1 FROM workspaces WHERE slug=?1)",
                    &[Cell::text(slug)],
                )
                .await?
                .first()
                .ok_or(sqlx::Error::RowNotFound)?
                .cell(0)?
                .boolean(),
        }
    }
    async fn insert_setup_user(&mut self, input: &SetupFirstOwnerInput) -> Result<(), sqlx::Error> {
        match self {
            Self::Postgres(tx) => {
                sqlx::query("INSERT INTO fvoci.users (id,email,password_hash,given_name,family_name,is_instance_admin,locale,timezone,week_starts_on,text_scale) VALUES ($1,$2,$3,$4,$5,true,$6,$7,$8,$9)")
                .bind(input.user_id).bind(&input.email).bind(&input.password_hash).bind(&input.given_name).bind(&input.family_name).bind(&input.locale).bind(&input.timezone).bind(input.week_starts_on).bind(input.text_scale).execute(&mut ***tx).await?;
            }
            Self::SqliteFamily(tx) => {
                tx.require_writer()?;
                tx.require_tenant(input.workspace_id)?;
                tx.execute("INSERT INTO users (id,email,password_hash,given_name,family_name,is_instance_admin,locale,timezone,week_starts_on,text_scale) VALUES (?1,?2,?3,?4,?5,1,?6,?7,?8,?9)",
                    &[Cell::uuid(input.user_id), Cell::text(&input.email), Cell::text(&input.password_hash), Cell::text(&input.given_name), Cell::optional_text(input.family_name.as_deref()), Cell::text(&input.locale), Cell::text(&input.timezone), Cell::Integer(i64::from(input.week_starts_on)), Cell::Integer(i64::from(input.text_scale))]).await?;
            }
        }
        Ok(())
    }
    async fn insert_setup_workspace(
        &mut self,
        input: &SetupFirstOwnerInput,
    ) -> Result<(), sqlx::Error> {
        match self {
            Self::Postgres(tx) => {
                sqlx::query("INSERT INTO fvoci.workspaces (id,slug,name) VALUES ($1,$2,$3)")
                    .bind(input.workspace_id)
                    .bind(&input.workspace_slug)
                    .bind(&input.workspace_name)
                    .execute(&mut ***tx)
                    .await?;
            }
            Self::SqliteFamily(tx) => {
                tx.require_writer()?;
                tx.require_tenant(input.workspace_id)?;
                tx.execute(
                    "INSERT INTO workspaces (id,slug,name) VALUES (?1,?2,?3)",
                    &[
                        Cell::uuid(input.workspace_id),
                        Cell::text(&input.workspace_slug),
                        Cell::text(&input.workspace_name),
                    ],
                )
                .await?;
            }
        }
        Ok(())
    }
    async fn insert_setup_membership(
        &mut self,
        input: &SetupFirstOwnerInput,
    ) -> Result<(), sqlx::Error> {
        match self {
            Self::Postgres(tx) => {
                sqlx::query("INSERT INTO fvoci.memberships (workspace_id,user_id,role) VALUES ($1,$2,'owner')").bind(input.workspace_id).bind(input.user_id).execute(&mut ***tx).await?;
            }
            Self::SqliteFamily(tx) => {
                tx.require_writer()?;
                tx.require_tenant(input.workspace_id)?;
                tx.execute(
                    "INSERT INTO memberships (workspace_id,user_id,role) VALUES (?1,?2,'owner')",
                    &[Cell::uuid(input.workspace_id), Cell::uuid(input.user_id)],
                )
                .await?;
            }
        }
        Ok(())
    }
    pub(crate) async fn create_session(
        &mut self,
        session: Uuid,
        user: Uuid,
        hash: &str,
        expires: DateTime<Utc>,
    ) -> Result<(), sqlx::Error> {
        match self {
            Self::Postgres(tx) => create_session(tx, session, user, hash, expires).await,
            Self::SqliteFamily(tx) => {
                tx.require_writer()?;
                tx.execute(
                    "INSERT INTO sessions (id,user_id,token_hash,expires_at) VALUES (?1,?2,?3,?4)",
                    &[
                        Cell::uuid(session),
                        Cell::uuid(user),
                        Cell::text(hash),
                        Cell::instant(expires)?,
                    ],
                )
                .await?;
                Ok(())
            }
        }
    }
    pub(crate) async fn append_event(&mut self, row: EventAppend) -> Result<(), sqlx::Error> {
        self.append_event_channel(row, "web").await
    }
    pub(crate) async fn append_event_channel(
        &mut self,
        row: EventAppend,
        channel: &str,
    ) -> Result<(), sqlx::Error> {
        match self {
            Self::Postgres(tx) => append_event_channel(tx, row, channel).await,
            Self::SqliteFamily(tx) => {
                tx.require_writer()?;
                if let Some(workspace) = row.workspace_id {
                    tx.require_tenant(workspace)?;
                } else {
                    tx.require_system_context()?;
                }
                let seq = allocate_family_event_sequence(tx).await?;
                tx.execute("INSERT INTO events (id,seq,workspace_id,actor_user_id,verb,target_type,target_id,payload,channel) VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9)", &[Cell::uuid(row.id),Cell::Integer(seq),Cell::optional_uuid(row.workspace_id),Cell::optional_uuid(row.actor_user_id),Cell::text(row.verb),Cell::optional_text(row.target_type.as_deref()),Cell::optional_uuid(row.target_id),Cell::json(&row.payload)?,Cell::text(channel)]).await?;
                Ok(())
            }
        }
    }
    pub(crate) async fn append_audit(&mut self, row: AuditAppend) -> Result<(), sqlx::Error> {
        match self {
            Self::Postgres(tx) => append_audit(tx, row).await,
            Self::SqliteFamily(tx) => {
                tx.require_writer()?;
                if let Some(workspace) = row.workspace_id {
                    tx.require_tenant(workspace)?;
                } else {
                    tx.require_system_context()?;
                }
                tx.execute("INSERT INTO audit_log (id,workspace_id,actor_user_id,verb,target_type,target_id,payload,ip) VALUES (?1,?2,?3,?4,?5,?6,?7,?8)", &[Cell::uuid(row.id),Cell::optional_uuid(row.workspace_id),Cell::optional_uuid(row.actor_user_id),Cell::text(row.verb),Cell::optional_text(row.target_type.as_deref()),Cell::optional_uuid(row.target_id),Cell::json(&row.payload)?,Cell::optional_text(row.ip.as_deref())]).await?;
                Ok(())
            }
        }
    }
}

/// SQLite-family visible event order is allocated only inside the owning
/// serialized writer transaction. Rolled-back values never reach a reader;
/// the counter survives retention/purge, including an empty events table.
async fn allocate_family_event_sequence(
    tx: &mut super::backend::FamilyTx,
) -> Result<i64, sqlx::Error> {
    tx.require_writer()?;
    let rows = tx
        .query("SELECT last_seq FROM event_sequence WHERE id=1", &[])
        .await?;
    let current = rows
        .first()
        .ok_or(sqlx::Error::RowNotFound)?
        .cell(0)?
        .integer()?;
    let next = current
        .checked_add(1)
        .filter(|next| current >= 0 && *next > 0)
        .ok_or_else(|| sqlx::Error::Protocol("event sequence is invalid or exhausted".into()))?;
    let changed=tx.execute("UPDATE event_sequence SET last_seq=?2 WHERE id=1 AND typeof(last_seq)='integer' AND last_seq=?1 AND last_seq<9223372036854775807", &[Cell::Integer(current),Cell::Integer(next)]).await?;
    if changed != 1 {
        return Err(sqlx::Error::Protocol(
            "event sequence allocation rejected".into(),
        ));
    }
    Ok(next)
}
