//! Instance administration (source `apps/server/src/domains/admin`,
//! `packages/core/src/{audit,legal}.ts` patchInstanceUser).
//!
//! Every operation rechecks the actor's instance-admin status inside its own
//! transaction; the HTTP layer never decides authorization from the session
//! alone. Writes also recheck the actor's session and keep its rows locked
//! until commit (`require_admin_session`). Reads of cross-tenant tables
//! switch to the system context only after that check.

use super::backend::{Backend, FamilyTx, OperationTx};
use super::codec::{Cell, FamilyRow};
use chrono::{DateTime, SecondsFormat, Utc};
use serde_json::{json, Value};
use sqlx::{PgPool, Postgres, Transaction};
use uuid::Uuid;

use crate::db::context::{
    lock_membership_users, recheck_session, set_self_user, set_system, set_tenant,
};
use crate::db::identity::{
    append_audit, append_event, lock_instance_admin_changes, lock_sign_in,
    revoke_all_sessions_for_user, AuditAppend, EventAppend,
};
use crate::db::quota::{require_new_instance_billable_user, QuotaError};

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

/// The gate of every instance-admin write: the actor's session, rechecked
/// `FOR UPDATE` as workspace writes do, then the admin flag. Callers run it
/// after the admission, instance-admin and membership advisory locks they
/// take (and, when the write targets another user, after that user's row
/// lock), and before any resource lock of their own, such as legal's
/// per-kind advisory lock or the settings revision row. The actor's users
/// and session row locks taken here are held until commit, so a logout,
/// password change or session revocation that commits before the check is
/// seen and the write answers 404, and one that comes later waits for the
/// write to commit. Locking the users row `FOR UPDATE` first also makes the
/// flag's `FOR SHARE` a no-op rather than a lock upgrade.
pub(crate) async fn require_admin_session(
    tx: &mut Transaction<'_, Postgres>,
    user_id: Uuid,
    session_id: Uuid,
) -> Result<bool, sqlx::Error> {
    if !recheck_session(tx, user_id, session_id).await? {
        return Ok(false);
    }
    require_live_instance_admin(tx, user_id).await
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

pub async fn list_audit_backend(
    backend: &Backend,
    actor: Uuid,
    cursor: Option<AuditCursor>,
    limit: i64,
) -> Result<Option<AuditPage>, sqlx::Error> {
    if let Backend::Postgres(pool) = backend {
        return list_audit(pool, actor, cursor, limit).await;
    }
    let mut tx = backend.begin_read().await?;
    if !tx.operation().require_live_instance_admin(actor).await? {
        tx.rollback().await?;
        return Ok(None);
    }
    tx.operation().set_system().await?;
    let OperationTx::SqliteFamily(family) = tx.operation() else {
        unreachable!()
    };
    family.require_system_context()?;
    let rows = family
        .query(
            r#"SELECT id,actor_user_id,workspace_id,verb,target_type,target_id,payload,ip,created_at
        FROM audit_log
        WHERE ?1 IS NULL
        OR (created_at,id)<(?1,?2)
        ORDER BY created_at DESC,id DESC LIMIT ?3"#,
            &[
                cursor
                    .map(|c| Cell::instant(c.created_at))
                    .transpose()?
                    .unwrap_or(Cell::Null),
                Cell::optional_uuid(cursor.map(|c| c.id)),
                Cell::Integer(limit + 1),
            ],
        )
        .await?;
    let more = rows.len() as i64 > limit;
    let items: Vec<AuditRow> = rows
        .iter()
        .take(limit as usize)
        .map(|r| {
            Ok(AuditRow {
                id: r.cell(0)?.id()?,
                actor_user_id: r.cell(1)?.optional(Cell::id)?,
                workspace_id: r.cell(2)?.optional(Cell::id)?,
                verb: r.cell(3)?.string()?,
                target_type: r.cell(4)?.optional(Cell::string)?,
                target_id: r.cell(5)?.optional(Cell::id)?,
                payload: r.cell(6)?.value()?,
                ip: r.cell(7)?.optional(Cell::string)?,
                created_at: r.cell(8)?.datetime()?,
            })
        })
        .collect::<Result<_, sqlx::Error>>()?;
    let next_cursor = if more {
        items.last().map(|r| {
            encode_audit_cursor(AuditCursor {
                created_at: r.created_at,
                id: r.id,
            })
        })
    } else {
        None
    };
    tx.commit_with_cleanup()
        .await
        .map_err(|error| sqlx::Error::AnyDriverError(Box::new(error)))?;
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

pub async fn list_users_backend(
    backend: &Backend,
    actor: Uuid,
) -> Result<Option<Vec<AdminUserRow>>, sqlx::Error> {
    if let Backend::Postgres(pool) = backend {
        return list_users(pool, actor).await;
    }
    let mut tx = backend.begin_read().await?;
    if !tx.operation().require_live_instance_admin(actor).await? {
        tx.rollback().await?;
        return Ok(None);
    }
    let OperationTx::SqliteFamily(family) = tx.operation() else {
        unreachable!()
    };
    let rows=family.query(r#"SELECT id,email,given_name,family_name,is_instance_admin,suspended_at,deleted_at,created_at
        FROM users
        WHERE anonymized_at IS NULL
        ORDER BY created_at,id"#,&[]).await?;
    let users = rows.iter().map(family_user_row).collect::<Result<_, _>>()?;
    tx.commit_with_cleanup()
        .await
        .map_err(|error| sqlx::Error::AnyDriverError(Box::new(error)))?;
    Ok(Some(users))
}

fn family_user_row(r: &FamilyRow) -> Result<AdminUserRow, sqlx::Error> {
    Ok(AdminUserRow {
        id: r.cell(0)?.id()?,
        email: r.cell(1)?.string()?,
        given_name: r.cell(2)?.string()?,
        family_name: r.cell(3)?.optional(Cell::string)?,
        instance_admin: r.cell(4)?.boolean()?,
        suspended_at: r.cell(5)?.optional(Cell::datetime)?,
        deleted_at: r.cell(6)?.optional(Cell::datetime)?,
        created_at: r.cell(7)?.datetime()?,
    })
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

pub async fn list_workspaces_backend(
    backend: &Backend,
    actor: Uuid,
) -> Result<Option<Vec<AdminWorkspaceRow>>, sqlx::Error> {
    if let Backend::Postgres(pool) = backend {
        return list_workspaces(pool, actor).await;
    }
    let mut tx = backend.begin_read().await?;
    if !tx.operation().require_live_instance_admin(actor).await? {
        tx.rollback().await?;
        return Ok(None);
    }
    tx.operation().set_system().await?;
    let OperationTx::SqliteFamily(family) = tx.operation() else {
        unreachable!()
    };
    let rows = family_workspaces(family).await?;
    tx.commit_with_cleanup()
        .await
        .map_err(|error| sqlx::Error::AnyDriverError(Box::new(error)))?;
    Ok(Some(rows))
}

async fn family_workspaces(tx: &mut FamilyTx) -> Result<Vec<AdminWorkspaceRow>, sqlx::Error> {
    tx.require_system_context()?;
    tx.query(
        r#"SELECT id,slug,name,created_at
        FROM workspaces
        WHERE deleted_at IS NULL
        ORDER BY created_at,id"#,
        &[],
    )
    .await?
    .iter()
    .map(|r| {
        Ok(AdminWorkspaceRow {
            id: r.cell(0)?.id()?,
            slug: r.cell(1)?.string()?,
            name: r.cell(2)?.string()?,
            created_at: r.cell(3)?.datetime()?,
        })
    })
    .collect()
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

pub async fn instance_directory_backend(
    backend: &Backend,
    actor: Uuid,
) -> Result<Option<InstanceDirectory>, sqlx::Error> {
    if let Backend::Postgres(pool) = backend {
        return instance_directory(pool, actor).await;
    }
    let mut tx = backend.begin_read().await?;
    if !tx.operation().require_live_instance_admin(actor).await? {
        tx.rollback().await?;
        return Ok(None);
    }
    tx.operation().set_system().await?;
    let OperationTx::SqliteFamily(family) = tx.operation() else {
        unreachable!()
    };
    family.require_system_context()?;
    let rows = family
        .query(
            r#"SELECT (SELECT count(*)
        FROM users
        WHERE anonymized_at IS NULL),(SELECT count(*)
        FROM workspaces
        WHERE deleted_at IS NULL),(SELECT count(*)
        FROM documents d
        JOIN workspaces w ON w.id=d.workspace_id
        WHERE w.deleted_at IS NULL
        AND d.deleted_at IS NULL),(SELECT count(*)
        FROM tasks t
        JOIN workspaces w ON w.id=t.workspace_id
        WHERE w.deleted_at IS NULL)"#,
            &[],
        )
        .await?;
    let r = rows.first().ok_or(sqlx::Error::RowNotFound)?;
    let result = InstanceDirectory {
        users: r.cell(0)?.integer()?,
        workspaces: r.cell(1)?.integer()?,
        documents: r.cell(2)?.integer()?,
        tasks: r.cell(3)?.integer()?,
    };
    tx.commit_with_cleanup()
        .await
        .map_err(|error| sqlx::Error::AnyDriverError(Box::new(error)))?;
    Ok(Some(result))
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
/// -> the target's sign-in row -> the actor's session and users row. The
/// session and admin check, the last-admin rule and both flag writes happen
/// under those locks; suspension revokes every session and API token of the
/// target in the same transaction as its audit rows.
pub async fn patch_instance_user(
    pool: &PgPool,
    license: &crate::license::Entitlements,
    actor: Uuid,
    session_id: Uuid,
    target: Uuid,
    patch: InstanceUserPatch,
    ip: Option<&str>,
) -> Result<Option<PatchUserOutcome>, sqlx::Error> {
    let mut tx = pool.begin().await?;
    lock_instance_admin_changes(&mut tx).await?;
    lock_membership_users(&mut tx, &[actor, target]).await?;
    lock_sign_in(&mut tx, target).await?;
    let current: Option<(bool, Option<DateTime<Utc>>)> = sqlx::query_as(
        "SELECT is_instance_admin, suspended_at FROM fvoci.users WHERE id = $1 AND deleted_at IS NULL",
    )
    .bind(target)
    .fetch_optional(&mut *tx)
    .await?;
    if !require_admin_session(&mut tx, actor, session_id).await? {
        tx.rollback().await?;
        return Ok(None);
    }
    let Some(current) = current else {
        tx.rollback().await?;
        return Ok(Some(PatchUserOutcome::NotFound));
    };
    let outcome = patch_locked(&mut tx, actor, target, patch, current, ip, license).await?;
    match outcome {
        PatchUserOutcome::Ok { .. } => tx.commit().await?,
        _ => tx.rollback().await?,
    }
    Ok(Some(outcome))
}

#[allow(clippy::too_many_arguments)]
pub async fn patch_instance_user_backend(
    backend: &Backend,
    license: &crate::license::Entitlements,
    actor: Uuid,
    session: Uuid,
    target: Uuid,
    patch: InstanceUserPatch,
    ip: Option<&str>,
) -> Result<Option<PatchUserOutcome>, sqlx::Error> {
    if let Backend::Postgres(pool) = backend {
        return patch_instance_user(pool, license, actor, session, target, patch, ip).await;
    }
    let mut tx = backend.begin_write().await?;
    if !tx.operation().require_admin_session(actor, session).await? {
        tx.rollback().await?;
        return Ok(None);
    }
    let OperationTx::SqliteFamily(family) = tx.operation() else {
        unreachable!()
    };
    let rows = family
        .query(
            "SELECT is_instance_admin,suspended_at FROM users WHERE id=?1 AND deleted_at IS NULL",
            &[Cell::uuid(target)],
        )
        .await?;
    let Some(r) = rows.first() else {
        tx.rollback().await?;
        return Ok(Some(PatchUserOutcome::NotFound));
    };
    let is_admin = r.cell(0)?.boolean()?;
    let suspended_at = r.cell(1)?.optional(Cell::datetime)?;
    let suspends = patch.suspended == Some(true) && suspended_at.is_none();
    let restores = patch.suspended == Some(false) && suspended_at.is_some();
    if is_admin && (patch.instance_admin == Some(false) || suspends) {
        let rows=family.query("SELECT count(*) FROM users WHERE is_instance_admin=1 AND deleted_at IS NULL AND suspended_at IS NULL",&[]).await?;
        if rows[0].cell(0)?.integer()? <= 1 {
            tx.rollback().await?;
            return Ok(Some(PatchUserOutcome::LastInstanceAdmin));
        }
    }
    if suspends && actor == target {
        tx.rollback().await?;
        return Ok(Some(PatchUserOutcome::SelfSuspension));
    }
    if patch.instance_admin == Some(true)
        && !is_admin
        && tx
            .operation()
            .require_new_instance_billable_user(Some(target), license)
            .await?
            .is_err()
    {
        tx.rollback().await?;
        return Ok(Some(PatchUserOutcome::SeatLimit));
    }
    if let Some(value) = patch.instance_admin.filter(|v| *v != is_admin) {
        let OperationTx::SqliteFamily(family) = tx.operation() else {
            unreachable!()
        };
        family.execute(r#"UPDATE users SET is_instance_admin=?2,updated_at=unixepoch()*1000000+CAST(substr(strftime('%f','now'),4,3) AS INTEGER)*1000
            WHERE id=?1"#,&[Cell::uuid(target),Cell::Integer(i64::from(value))]).await?;
        tx.operation()
            .record_instance_change(InstanceChange {
                actor_user_id: actor,
                verb: "admin.instance_admin_set",
                target: Some(("user", target)),
                payload: json!({"targetId":target.to_string(),"granted":value}),
                ip,
            })
            .await?;
    }
    if suspends || restores {
        let mut op = tx.operation();
        let OperationTx::SqliteFamily(family) = &mut op else {
            unreachable!()
        };
        family.execute(r#"UPDATE users SET suspended_at=CASE WHEN ?2=1 THEN unixepoch()*1000000+CAST(substr(strftime('%f','now'),4,3) AS INTEGER)*1000 ELSE NULL END,updated_at=unixepoch()*1000000+CAST(substr(strftime('%f','now'),4,3) AS INTEGER)*1000
            WHERE id=?1"#,&[Cell::uuid(target),Cell::Integer(i64::from(suspends))]).await?;
        if suspends {
            op.revoke_auth_sessions(target, None).await?;
            let OperationTx::SqliteFamily(family) = &mut op else {
                unreachable!()
            };
            family
                .execute(
                    "DELETE FROM api_tokens WHERE user_id=?1",
                    &[Cell::uuid(target)],
                )
                .await?;
        }
        op.record_instance_change(InstanceChange {
            actor_user_id: actor,
            verb: "admin.user_suspended_set",
            target: Some(("user", target)),
            payload: json!({"targetId":target.to_string(),"suspended":suspends}),
            ip,
        })
        .await?;
    }
    let OperationTx::SqliteFamily(family) = tx.operation() else {
        unreachable!()
    };
    let rows = family
        .query(
            "SELECT suspended_at FROM users WHERE id=?1",
            &[Cell::uuid(target)],
        )
        .await?;
    let suspended_at = rows[0].cell(0)?.optional(Cell::datetime)?;
    tx.commit_with_cleanup()
        .await
        .map_err(|error| sqlx::Error::AnyDriverError(Box::new(error)))?;
    Ok(Some(PatchUserOutcome::Ok { suspended_at }))
}

async fn patch_locked(
    tx: &mut Transaction<'_, Postgres>,
    actor: Uuid,
    target: Uuid,
    patch: InstanceUserPatch,
    current: (bool, Option<DateTime<Utc>>),
    ip: Option<&str>,
    license: &crate::license::Entitlements,
) -> Result<PatchUserOutcome, sqlx::Error> {
    let (is_admin, suspended_at) = current;
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

pub async fn is_live_instance_admin_backend(
    backend: &Backend,
    user: Uuid,
) -> Result<bool, sqlx::Error> {
    if let Backend::Postgres(pool) = backend {
        // Preserve the route's original transaction: the authority check
        // takes FOR SHARE, which is forbidden in PG's read-only transactions.
        let mut tx = pool.begin().await?;
        let allowed = require_live_instance_admin(&mut tx, user).await?;
        tx.commit().await?;
        return Ok(allowed);
    }
    let mut tx = backend.begin_read().await?;
    let allowed = tx.operation().require_live_instance_admin(user).await?;
    tx.commit_with_cleanup()
        .await
        .map_err(|error| sqlx::Error::AnyDriverError(Box::new(error)))?;
    Ok(allowed)
}

impl OperationTx<'_, '_> {
    pub(crate) async fn require_live_instance_admin(
        &mut self,
        user: Uuid,
    ) -> Result<bool, sqlx::Error> {
        match self {
            Self::Postgres(tx) => require_live_instance_admin(tx, user).await,
            Self::SqliteFamily(tx) => {
                let rows = tx
                    .query(
                        r#"SELECT EXISTS(SELECT 1
                    FROM users
                    WHERE id=?1
                    AND is_instance_admin=1
                    AND deleted_at IS NULL
                    AND suspended_at IS NULL)"#,
                        &[Cell::uuid(user)],
                    )
                    .await?;
                rows[0].cell(0)?.boolean()
            }
        }
    }
    pub(crate) async fn require_admin_session(
        &mut self,
        user: Uuid,
        credential: Uuid,
    ) -> Result<bool, sqlx::Error> {
        match self {
            Self::Postgres(tx) => require_admin_session(tx, user, credential).await,
            Self::SqliteFamily(tx) => {
                tx.require_writer()?;
                // Instance administration uses the same credential union as PG;
                // PAT visibility retains PG's tenant/system RLS policy. A fresh
                // instance transaction has neither context and refuses PATs.
                let tenant = Cell::optional_uuid(tx.tenant());
                let system = Cell::Integer(i64::from(tx.require_system_context().is_ok()));
                let rows=tx.query(r#"SELECT EXISTS(SELECT 1
                    FROM users u
                    WHERE u.id=?1
                    AND u.is_instance_admin=1
                    AND u.deleted_at IS NULL
                    AND u.suspended_at IS NULL
                    AND (EXISTS(SELECT 1
                    FROM sessions s
                    WHERE s.id=?2
                    AND s.user_id=u.id
                    AND s.revoked_at IS NULL
                    AND s.expires_at>(unixepoch()*1000000+CAST(substr(strftime('%f','now'),4,3) AS INTEGER)*1000))
                    OR EXISTS(SELECT 1
                    FROM api_tokens t
                    WHERE t.id=?2
                    AND t.user_id=u.id
                    AND NOT EXISTS(SELECT 1 FROM sessions prior WHERE prior.id=?2 AND prior.user_id=u.id)
                    AND (t.workspace_id=?3 OR ?4=1)
                    AND (t.expires_at IS NULL
                    OR t.expires_at>(unixepoch()*1000000+CAST(substr(strftime('%f','now'),4,3) AS INTEGER)*1000)))))"#,&[Cell::uuid(user),Cell::uuid(credential),tenant,system]).await?;
                rows[0].cell(0)?.boolean()
            }
        }
    }
    pub(crate) async fn record_instance_change(
        &mut self,
        change: InstanceChange<'_>,
    ) -> Result<(), sqlx::Error> {
        if let Self::Postgres(tx) = self {
            return record_instance_change(tx, change).await;
        }
        self.set_system().await?;
        self.append_event(EventAppend {
            id: Uuid::now_v7(),
            workspace_id: None,
            actor_user_id: Some(change.actor_user_id),
            verb: change.verb.into(),
            target_type: change.target.map(|(t, _)| t.into()),
            target_id: change.target.map(|(_, id)| id),
            payload: change.payload.clone(),
        })
        .await?;
        self.append_audit(AuditAppend {
            id: Uuid::now_v7(),
            workspace_id: None,
            actor_user_id: Some(change.actor_user_id),
            verb: change.verb.into(),
            target_type: change.target.map(|(t, _)| t.into()),
            target_id: change.target.map(|(_, id)| id),
            payload: change.payload,
            ip: change.ip.map(str::to_string),
        })
        .await
    }
}

// Explicit official-sqld witness; excluded from the registered default cohort.
#[cfg(all(test, feature = "db-tests"))]
mod selected_admin_family_sdk {
    use super::*;
    use crate::db::backend::RemoteDatabase;
    use crate::db::libsql_finish_fixture::LibsqlFinishFixture;
    use futures_util::FutureExt;
    use std::{num::NonZeroU32, path::PathBuf, sync::Arc};

    #[tokio::test]
    async fn selected_admin_legal_settings_sdk_commit_and_denial_readback() {
        let sqld = PathBuf::from(
            std::env::var_os("FVOCI_TEST_SQLD").expect("explicit official sqld required"),
        );
        let root = std::env::temp_dir().join(format!("fvoci-admin-sdk-{}", Uuid::now_v7()));
        let driver = LibsqlFinishFixture::start(&sqld, &root).await.unwrap();
        let backend = Backend::LibsqlRemote(Arc::new(RemoteDatabase::from_test_driver(
            driver.database().await.unwrap(),
            NonZeroU32::new(1).unwrap(),
        )));
        let result=std::panic::AssertUnwindSafe(async{
            crate::db::migrate::initialize_family_backend_for_test(&backend).await.unwrap();
            let(actor,session,other)=(Uuid::now_v7(),Uuid::now_v7(),Uuid::now_v7());
            let mut tx=backend.begin_write().await.unwrap();
            let OperationTx::SqliteFamily(family)=tx.operation() else{unreachable!()};
            family.execute("INSERT INTO users(id,email,given_name,is_instance_admin) VALUES(?1,'sdk-admin@example.invalid','Admin',1)",&[Cell::uuid(actor)]).await.unwrap();
            family.execute("INSERT INTO users(id,email,given_name) VALUES(?1,'sdk-member@example.invalid','Member')",&[Cell::uuid(other)]).await.unwrap();
            family.execute(r#"INSERT INTO sessions(id,user_id,token_hash,expires_at) VALUES(?1,?2,'synthetic-sdk-admin',unixepoch()*1000000+3600000000)"#,&[Cell::uuid(session),Cell::uuid(actor)]).await.unwrap();
            tx.commit_with_cleanup().await.unwrap();
            assert!(list_users_backend(&backend,other).await.unwrap().is_none());
            assert_eq!(list_users_backend(&backend,actor).await.unwrap().unwrap().len(),2);
            let changed=patch_instance_user_backend(&backend,&crate::license::absent(),actor,session,other,InstanceUserPatch{instance_admin:None,suspended:Some(true)},None).await.unwrap().unwrap();assert!(matches!(changed,PatchUserOutcome::Ok{..}));
            let fresh=Backend::LibsqlRemote(Arc::new(RemoteDatabase::from_test_driver(driver.database().await.unwrap(),NonZeroU32::new(1).unwrap())));
            assert!(list_users_backend(&fresh,actor).await.unwrap().unwrap().iter().find(|u|u.id==other).unwrap().suspended_at.is_some());
            let input=crate::db::legal::LegalPublishInput{kind:"terms".into(),title:"Terms".into(),body_markdown:"Terms".into(),body_html:"<p>Terms</p>".into(),required:true,effective_at:crate::db::identity::stored_now()};
            let doc=crate::db::legal::publish_legal_backend(&backend,actor,session,input,None).await.unwrap().unwrap();
            assert_eq!(crate::db::legal::latest_legal_backend(&fresh,"terms").await.unwrap().unwrap().id,doc.id);
            assert_eq!(crate::db::legal::pending_consents_backend(&fresh,actor).await.unwrap().len(),1);
            assert_eq!(crate::db::legal::record_consents_backend(&backend,actor,&[("terms".into(),1)],None).await.unwrap(),crate::db::legal::RecordConsentsOutcome::Recorded);
            assert!(crate::db::legal::pending_consents_backend(&fresh,actor).await.unwrap().is_empty());
            let boot=crate::settings::SettingsBoot::default();
            let changed=crate::settings::apply_change_backend(&backend,actor,session,None,"FVOCI",crate::settings::SettingsChange::Patch(vec![(crate::settings::SettingsKey::Security,Some(json!({"contact":"mailto:security@example.invalid"})))]),&boot).await.unwrap().unwrap();assert_eq!(changed.snapshot.revision,1);
            assert_eq!(crate::settings::load_backend(&fresh,&boot,"FVOCI").await.unwrap().revision,1);

            let(source,destination)=(Uuid::now_v7(),Uuid::now_v7());
            let mut tx=backend.begin_write().await.unwrap();let OperationTx::SqliteFamily(family)=tx.operation()else{unreachable!()};
            for(id,slug,kind)in[(source,"sdk-personal","personal"),(destination,"sdk-team","team")]{family.execute(r#"INSERT INTO workspaces(id,slug,name,kind) VALUES(?1,?2,'SDK transfer',?3)"#,&[Cell::uuid(id),Cell::text(slug),Cell::text(kind)]).await.unwrap();family.execute(r#"INSERT INTO memberships(workspace_id,user_id,role) VALUES(?1,?2,'owner')"#,&[Cell::uuid(id),Cell::uuid(actor)]).await.unwrap();}
            family.execute("UPDATE users SET personal_workspace_id=?2 WHERE id=?1",&[Cell::uuid(actor),Cell::uuid(source)]).await.unwrap();tx.commit_with_cleanup().await.unwrap();
            let pat=Uuid::now_v7();
            let mut pat_tx=backend.begin_write().await.unwrap();
            let OperationTx::SqliteFamily(family)=pat_tx.operation()else{unreachable!()};
            family.execute("INSERT INTO api_tokens(id,workspace_id,user_id,token_hash,name,scopes,expires_at) VALUES(?1,?2,?3,'synthetic-sdk-pat','SDK PAT','[\"workspace.manage\"]',unixepoch()*1000000+3600000000)",&[Cell::uuid(pat),Cell::uuid(destination),Cell::uuid(actor)]).await.unwrap();
            pat_tx.commit_with_cleanup().await.unwrap();
            for (tenant,system,allowed) in [(None,false,false),(Some(destination),false,true),(Some(source),false,false),(None,true,true)] {
                let mut check=backend.begin_write().await.unwrap();
                if let Some(tenant)=tenant {check.operation().set_tenant(tenant).await.unwrap();}
                if system {check.operation().set_system().await.unwrap();}
                assert_eq!(check.operation().require_admin_session(actor,pat).await.unwrap(),allowed);
                check.rollback().await.unwrap();
            }
            let mut expired=backend.begin_write().await.unwrap();expired.operation().set_system().await.unwrap();
            let OperationTx::SqliteFamily(family)=expired.operation()else{unreachable!()};
            family.execute("UPDATE api_tokens SET expires_at=0 WHERE id=?1",&[Cell::uuid(pat)]).await.unwrap();
            assert!(!expired.operation().require_admin_session(actor,pat).await.unwrap());expired.rollback().await.unwrap();
            let project=crate::db::projects::create_project_backend(&backend,destination,actor,session,crate::db::projects::CreateProjectInput{key:"SDK",name:"SDK",visibility:"workspace",description:None,icon:None,lead_user_id:None},None).await.unwrap().unwrap();
            let document=crate::db::personal_input::create_personal_input_backend(&backend,source,actor,session,&crate::api::personal_input_dto::PersonalInputBody{request_id:Uuid::now_v7(),intent:crate::api::personal_input_dto::PersonalInputIntent::Note,title:"SDK personal note".into(),project_id:None,source:None},None,"web").await.unwrap().unwrap();
            let selection=crate::api::personal_transfer::PersonalTransferSelection{action:crate::api::personal_transfer::PersonalTransferAction::Move,document_id:Uuid::parse_str(&document.document_id).unwrap(),task_id:None,expected_document_version:1,expected_task_version:None,destination_workspace_id:destination,destination_project_id:project.id,destination_status_id:None};
            let preview=crate::db::personal_transfer::preview_personal_transfer_backend(&backend,source,actor,session,&selection,None).await.unwrap().unwrap();
            let body=crate::api::personal_transfer::PersonalTransferBody{request_id:Uuid::now_v7(),selection,preview_digest:preview.digest,confirmed:true};
            let moved=crate::db::personal_transfer::transfer_personal_item_backend(&backend,source,actor,session,&body,None,"web",None,None).await.unwrap().unwrap();
            assert_eq!(moved.document_id,body.selection.document_id);
            assert!(crate::db::personal_transfer::transfer_personal_item_backend(&fresh,source,actor,session,&body,None,"web",None,None).await.unwrap().unwrap().replayed);
            let mut tx=backend.begin_write().await.unwrap();let OperationTx::SqliteFamily(family)=tx.operation()else{unreachable!()};family.execute(r#"UPDATE sessions SET revoked_at=unixepoch()*1000000
                WHERE id=?1"#,&[Cell::uuid(session)]).await.unwrap();tx.commit_with_cleanup().await.unwrap();
            assert!(patch_instance_user_backend(&backend,&crate::license::absent(),actor,session,other,InstanceUserPatch{instance_admin:Some(true),suspended:None},None).await.unwrap().is_none());
            assert!(list_audit_backend(&fresh,actor,None,100).await.unwrap().unwrap().items.iter().any(|row|row.verb=="document.deleted"));
        }).catch_unwind().await;
        driver.finish().await.unwrap();
        if let Err(panic) = result {
            std::panic::resume_unwind(panic);
        }
    }
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
