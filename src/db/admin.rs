//! Instance administration (source `apps/server/src/domains/admin`,
//! `packages/core/src/{audit,legal}.ts` patchInstanceUser).
//!
//! Every operation rechecks the actor's instance-admin status inside its own
//! transaction; the HTTP layer never decides authorization from the session
//! alone. Reads of cross-tenant tables switch to the system context only after
//! that check.

use chrono::{DateTime, SecondsFormat, Utc};
use serde_json::{json, Value};
use sqlx::{PgPool, Postgres, Transaction};
use uuid::Uuid;

use crate::db::context::{lock_membership_users, set_self_user, set_system, set_tenant};
use crate::db::identity::{
    append_audit, append_event, lock_sign_in, revoke_all_sessions_for_user, AuditAppend,
    EventAppend, INSTANCE_ADMIN_LOCK_KEY,
};
use crate::db::quota::{acquire_admission_lock, require_new_instance_billable_user, QuotaError};

/// Source `requireInstanceAdmin`: a live (not deleted, not suspended) user
/// with the flag. `FOR SHARE` holds the row so a concurrent demotion or
/// suspension (which updates it) orders strictly before or after this
/// transaction.
pub(crate) async fn require_live_instance_admin(
    tx: &mut Transaction<'_, Postgres>,
    user_id: Uuid,
) -> Result<bool, sqlx::Error> {
    let row: Option<(Uuid,)> = sqlx::query_as(
        r#"
        SELECT id FROM fvoci.users
        WHERE id = $1 AND is_instance_admin AND deleted_at IS NULL AND suspended_at IS NULL
        FOR SHARE
        "#,
    )
    .bind(user_id)
    .fetch_optional(&mut **tx)
    .await?;
    Ok(row.is_some())
}

pub(crate) struct InstanceChange<'a> {
    pub actor_user_id: Uuid,
    pub verb: &'a str,
    pub target: Option<(&'a str, Uuid)>,
    pub payload: Value,
    pub ip: Option<&'a str>,
}

/// Instance-level event + audit row in the caller's transaction (source
/// `recordEvent` with an audit verb and no workspace).
pub(crate) async fn record_instance_change(
    tx: &mut Transaction<'_, Postgres>,
    change: InstanceChange<'_>,
) -> Result<(), sqlx::Error> {
    set_system(tx).await?;
    append_event(
        tx,
        EventAppend {
            id: Uuid::now_v7(),
            workspace_id: None,
            actor_user_id: Some(change.actor_user_id),
            verb: change.verb.to_string(),
            target_type: change.target.map(|(t, _)| t.to_string()),
            target_id: change.target.map(|(_, id)| id),
            payload: change.payload.clone(),
        },
    )
    .await?;
    append_audit(
        tx,
        AuditAppend {
            id: Uuid::now_v7(),
            workspace_id: None,
            actor_user_id: Some(change.actor_user_id),
            verb: change.verb.to_string(),
            target_type: change.target.map(|(t, _)| t.to_string()),
            target_id: change.target.map(|(_, id)| id),
            payload: change.payload,
            ip: change.ip.map(str::to_string),
        },
    )
    .await
}

/// Starts a transaction and fails closed unless `actor` is a live instance
/// admin. Returns `None` for "not an admin" (the routes answer 404).
async fn admin_tx(
    pool: &PgPool,
    actor: Uuid,
) -> Result<Option<Transaction<'static, Postgres>>, sqlx::Error> {
    let mut tx = pool.begin().await?;
    if !require_live_instance_admin(&mut tx, actor).await? {
        tx.rollback().await?;
        return Ok(None);
    }
    Ok(Some(tx))
}

// ---------------------------------------------------------------- audit

#[derive(Debug, Clone)]
pub struct AuditRow {
    pub id: Uuid,
    pub actor_user_id: Option<Uuid>,
    pub workspace_id: Option<Uuid>,
    pub verb: String,
    pub target_type: Option<String>,
    pub target_id: Option<Uuid>,
    pub payload: Value,
    pub ip: Option<String>,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Copy)]
pub struct AuditCursor {
    pub created_at: DateTime<Utc>,
    pub id: Uuid,
}

pub struct AuditPage {
    pub items: Vec<AuditRow>,
    pub next_cursor: Option<String>,
}

/// Opaque base64url JSON `{ca, id}` (source `auditCursor`). The timestamp keeps
/// microseconds: a millisecond cursor would skip rows committed inside the same
/// millisecond as the page's last row.
pub fn encode_audit_cursor(cursor: AuditCursor) -> String {
    use base64::Engine;
    let payload = json!({
        "ca": cursor.created_at.to_rfc3339_opts(SecondsFormat::Micros, true),
        "id": cursor.id.to_string(),
    });
    base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(payload.to_string())
}

pub fn decode_audit_cursor(raw: &str) -> Option<AuditCursor> {
    use base64::Engine;
    if raw.len() > 1024 {
        return None;
    }
    let bytes = base64::engine::general_purpose::URL_SAFE_NO_PAD
        .decode(raw.trim_end_matches('='))
        .ok()?;
    let value: Value = serde_json::from_slice(&bytes).ok()?;
    let object = value.as_object()?;
    if object.len() != 2 {
        return None;
    }
    let created_at = DateTime::parse_from_rfc3339(object.get("ca")?.as_str()?)
        .ok()?
        .with_timezone(&Utc);
    let id = Uuid::parse_str(object.get("id")?.as_str()?).ok()?;
    Some(AuditCursor { created_at, id })
}

type AuditScan = (
    Uuid,
    Option<Uuid>,
    Option<Uuid>,
    String,
    Option<String>,
    Option<Uuid>,
    Value,
    Option<String>,
    DateTime<Utc>,
);

/// Instance audit log, newest first, keyset `(created_at, id)`.
pub async fn list_audit(
    pool: &PgPool,
    actor: Uuid,
    cursor: Option<AuditCursor>,
    limit: i64,
) -> Result<Option<AuditPage>, sqlx::Error> {
    let Some(mut tx) = admin_tx(pool, actor).await? else {
        return Ok(None);
    };
    set_system(&mut tx).await?;
    let rows: Vec<AuditScan> = sqlx::query_as(
        r#"
        SELECT id, actor_user_id, workspace_id, verb, target_type, target_id, payload,
               host(ip), created_at
        FROM fvoci.audit_log
        WHERE $1::timestamptz IS NULL OR (created_at, id) < ($1::timestamptz, $2::uuid)
        ORDER BY created_at DESC, id DESC
        LIMIT $3
        "#,
    )
    .bind(cursor.map(|c| c.created_at))
    .bind(cursor.map(|c| c.id))
    .bind(limit + 1)
    .fetch_all(&mut *tx)
    .await?;
    tx.commit().await?;
    let has_more = rows.len() as i64 > limit;
    let items: Vec<AuditRow> = rows
        .into_iter()
        .take(limit as usize)
        .map(
            |(
                id,
                actor_user_id,
                workspace_id,
                verb,
                target_type,
                target_id,
                payload,
                ip,
                created_at,
            )| {
                AuditRow {
                    id,
                    actor_user_id,
                    workspace_id,
                    verb,
                    target_type,
                    target_id,
                    payload,
                    ip,
                    created_at,
                }
            },
        )
        .collect();
    let next_cursor = if has_more {
        items.last().map(|last| {
            encode_audit_cursor(AuditCursor {
                created_at: last.created_at,
                id: last.id,
            })
        })
    } else {
        None
    };
    Ok(Some(AuditPage { items, next_cursor }))
}

// ---------------------------------------------------------------- directory

#[derive(Debug, Clone)]
pub struct AdminUserRow {
    pub id: Uuid,
    pub email: String,
    pub given_name: String,
    pub family_name: Option<String>,
    pub instance_admin: bool,
    pub suspended_at: Option<DateTime<Utc>>,
    pub deleted_at: Option<DateTime<Utc>>,
    pub created_at: DateTime<Utc>,
}

/// Every user not yet anonymized, oldest first (source `users.list`).
pub async fn list_users(
    pool: &PgPool,
    actor: Uuid,
) -> Result<Option<Vec<AdminUserRow>>, sqlx::Error> {
    let Some(mut tx) = admin_tx(pool, actor).await? else {
        return Ok(None);
    };
    let rows = list_users_in(&mut tx).await?;
    tx.commit().await?;
    Ok(Some(rows))
}

type UserScan = (
    Uuid,
    String,
    String,
    Option<String>,
    bool,
    Option<DateTime<Utc>>,
    Option<DateTime<Utc>>,
    DateTime<Utc>,
);

async fn list_users_in(
    tx: &mut Transaction<'_, Postgres>,
) -> Result<Vec<AdminUserRow>, sqlx::Error> {
    let rows: Vec<UserScan> = sqlx::query_as(
        r#"
        SELECT id, email, given_name, family_name, is_instance_admin, suspended_at,
               deleted_at, created_at
        FROM fvoci.users
        WHERE anonymized_at IS NULL
        ORDER BY created_at, id
        "#,
    )
    .fetch_all(&mut **tx)
    .await?;
    Ok(rows
        .into_iter()
        .map(
            |(
                id,
                email,
                given_name,
                family_name,
                instance_admin,
                suspended_at,
                deleted_at,
                created_at,
            )| {
                AdminUserRow {
                    id,
                    email,
                    given_name,
                    family_name,
                    instance_admin,
                    suspended_at,
                    deleted_at,
                    created_at,
                }
            },
        )
        .collect())
}

#[derive(Debug, Clone)]
pub struct AdminWorkspaceRow {
    pub id: Uuid,
    pub slug: String,
    pub name: String,
    pub created_at: DateTime<Utc>,
}

async fn list_workspaces_in(
    tx: &mut Transaction<'_, Postgres>,
) -> Result<Vec<AdminWorkspaceRow>, sqlx::Error> {
    set_system(tx).await?;
    let rows: Vec<(Uuid, String, String, DateTime<Utc>)> = sqlx::query_as(
        r#"
        SELECT id, slug, name, created_at FROM fvoci.workspaces
        WHERE deleted_at IS NULL
        ORDER BY created_at, id
        "#,
    )
    .fetch_all(&mut **tx)
    .await?;
    Ok(rows
        .into_iter()
        .map(|(id, slug, name, created_at)| AdminWorkspaceRow {
            id,
            slug,
            name,
            created_at,
        })
        .collect())
}

pub async fn list_workspaces(
    pool: &PgPool,
    actor: Uuid,
) -> Result<Option<Vec<AdminWorkspaceRow>>, sqlx::Error> {
    let Some(mut tx) = admin_tx(pool, actor).await? else {
        return Ok(None);
    };
    let rows = list_workspaces_in(&mut tx).await?;
    tx.commit().await?;
    Ok(Some(rows))
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct InstanceDirectory {
    pub users: i64,
    pub workspaces: i64,
    pub documents: i64,
    pub tasks: i64,
}

/// Source `instanceDirectory`: listed users and live workspaces, live
/// documents and all tasks of those workspaces, counted per tenant context.
pub async fn instance_directory(
    pool: &PgPool,
    actor: Uuid,
) -> Result<Option<InstanceDirectory>, sqlx::Error> {
    let Some(mut tx) = admin_tx(pool, actor).await? else {
        return Ok(None);
    };
    let users = list_users_in(&mut tx).await?.len() as i64;
    let workspaces = list_workspaces_in(&mut tx).await?;
    sqlx::query("SELECT set_config('app.system_ctx', '', true)")
        .execute(&mut *tx)
        .await?;
    let mut documents = 0i64;
    let mut tasks = 0i64;
    for workspace in &workspaces {
        set_tenant(&mut tx, workspace.id).await?;
        let (d, t): (i64, i64) = sqlx::query_as(
            r#"
            SELECT
                (SELECT count(*) FROM fvoci.documents WHERE workspace_id = $1 AND deleted_at IS NULL),
                (SELECT count(*) FROM fvoci.tasks WHERE workspace_id = $1)
            "#,
        )
        .bind(workspace.id)
        .fetch_one(&mut *tx)
        .await?;
        documents += d;
        tasks += t;
    }
    tx.commit().await?;
    Ok(Some(InstanceDirectory {
        users,
        workspaces: workspaces.len() as i64,
        documents,
        tasks,
    }))
}

// ---------------------------------------------------------------- user patch

#[derive(Debug, Clone, Copy, Default)]
pub struct InstanceUserPatch {
    pub instance_admin: Option<bool>,
    pub suspended: Option<bool>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PatchUserOutcome {
    Ok { suspended_at: Option<DateTime<Utc>> },
    NotFound,
    LastInstanceAdmin,
    SelfSuspension,
    SeatLimit,
}

async fn count_live_instance_admins(
    tx: &mut Transaction<'_, Postgres>,
) -> Result<i64, sqlx::Error> {
    sqlx::query_scalar(
        r#"
        SELECT count(*) FROM fvoci.users
        WHERE is_instance_admin AND deleted_at IS NULL AND suspended_at IS NULL
        "#,
    )
    .fetch_one(&mut **tx)
    .await
}

/// Source `patchInstanceUser`. Lock order matches the source and the account
/// lifecycle: admission lock -> instance-admin set lock -> membership locks
/// -> the target's sign-in row. The admin check, the last-admin rule and both
/// flag writes happen under those locks; suspension revokes every session and
/// API token of the target in the same transaction as its audit rows.
pub async fn patch_instance_user(
    pool: &PgPool,
    license: &crate::license::Entitlements,
    actor: Uuid,
    target: Uuid,
    patch: InstanceUserPatch,
    ip: Option<&str>,
) -> Result<Option<PatchUserOutcome>, sqlx::Error> {
    let mut tx = pool.begin().await?;
    acquire_admission_lock(&mut tx).await?;
    sqlx::query("SELECT pg_advisory_xact_lock($1)")
        .bind(INSTANCE_ADMIN_LOCK_KEY)
        .execute(&mut *tx)
        .await?;
    lock_membership_users(&mut tx, &[actor, target]).await?;
    lock_sign_in(&mut tx, target).await?;
    let current: Option<(bool, Option<DateTime<Utc>>)> = sqlx::query_as(
        "SELECT is_instance_admin, suspended_at FROM fvoci.users WHERE id = $1 AND deleted_at IS NULL",
    )
    .bind(target)
    .fetch_optional(&mut *tx)
    .await?;
    if !require_live_instance_admin(&mut tx, actor).await? {
        tx.rollback().await?;
        return Ok(None);
    }
    let Some((is_admin, suspended_at)) = current else {
        tx.rollback().await?;
        return Ok(Some(PatchUserOutcome::NotFound));
    };
    let outcome = patch_locked(
        &mut tx,
        actor,
        target,
        patch,
        is_admin,
        suspended_at,
        ip,
        license,
    )
    .await?;
    match outcome {
        PatchUserOutcome::Ok { .. } => tx.commit().await?,
        _ => tx.rollback().await?,
    }
    Ok(Some(outcome))
}

async fn patch_locked(
    tx: &mut Transaction<'_, Postgres>,
    actor: Uuid,
    target: Uuid,
    patch: InstanceUserPatch,
    is_admin: bool,
    suspended_at: Option<DateTime<Utc>>,
    ip: Option<&str>,
    license: &crate::license::Entitlements,
) -> Result<PatchUserOutcome, sqlx::Error> {
    if patch.instance_admin == Some(false) && is_admin && count_live_instance_admins(tx).await? <= 1
    {
        return Ok(PatchUserOutcome::LastInstanceAdmin);
    }
    let suspends = patch.suspended == Some(true) && suspended_at.is_none();
    if suspends {
        let last = is_admin && count_live_instance_admins(tx).await? <= 1;
        if last {
            return Ok(PatchUserOutcome::LastInstanceAdmin);
        }
        if target == actor {
            return Ok(PatchUserOutcome::SelfSuspension);
        }
    }
    if patch.instance_admin == Some(true) && !is_admin {
        if let Err(QuotaError::SeatLimit | QuotaError::GuestLimit) =
            require_new_instance_billable_user(tx, Some(target), license).await?
        {
            return Ok(PatchUserOutcome::SeatLimit);
        }
    }
    // The definers check app.self_user_id is a live admin and keep >= 1 admin.
    set_self_user(tx, actor).await?;
    if let Some(value) = patch.instance_admin.filter(|v| *v != is_admin) {
        sqlx::query_scalar::<_, i32>("SELECT fvoci.app_admin_set_instance_admin($1, $2)")
            .bind(target)
            .bind(value)
            .fetch_one(&mut **tx)
            .await?;
        record_instance_change(
            tx,
            InstanceChange {
                actor_user_id: actor,
                verb: "admin.instance_admin_set",
                target: Some(("user", target)),
                payload: json!({ "targetId": target.to_string(), "granted": value }),
                ip,
            },
        )
        .await?;
    }
    let restores = patch.suspended == Some(false) && suspended_at.is_some();
    if suspends || restores {
        sqlx::query_scalar::<_, i32>("SELECT fvoci.app_admin_set_suspended($1, $2)")
            .bind(target)
            .bind(suspends)
            .fetch_one(&mut **tx)
            .await?;
        if suspends {
            revoke_all_sessions_for_user(tx, target).await?;
            set_system(tx).await?;
            sqlx::query("DELETE FROM fvoci.api_tokens WHERE user_id = $1")
                .bind(target)
                .execute(&mut **tx)
                .await?;
        }
        record_instance_change(
            tx,
            InstanceChange {
                actor_user_id: actor,
                verb: "admin.user_suspended_set",
                target: Some(("user", target)),
                payload: json!({ "targetId": target.to_string(), "suspended": suspends }),
                ip,
            },
        )
        .await?;
    }
    let suspended_at: Option<DateTime<Utc>> =
        sqlx::query_scalar("SELECT suspended_at FROM fvoci.users WHERE id = $1")
            .bind(target)
            .fetch_one(&mut **tx)
            .await?;
    Ok(PatchUserOutcome::Ok { suspended_at })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn audit_cursor_round_trips_microseconds_and_rejects_extra_keys() {
        let created_at = DateTime::parse_from_rfc3339("2026-09-26T01:02:03.123456Z")
            .unwrap()
            .with_timezone(&Utc);
        let id = Uuid::now_v7();
        let raw = encode_audit_cursor(AuditCursor { created_at, id });
        let back = decode_audit_cursor(&raw).unwrap();
        assert_eq!(back.created_at, created_at);
        assert_eq!(back.id, id);
        use base64::Engine;
        let extra = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(
            json!({"ca": "2026-09-26T00:00:00Z", "id": id.to_string(), "x": 1}).to_string(),
        );
        assert!(decode_audit_cursor(&extra).is_none());
        assert!(decode_audit_cursor("not base64 !").is_none());
    }
}
