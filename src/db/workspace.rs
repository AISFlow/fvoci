use serde_json::json;
use sqlx::{PgPool, Postgres, Transaction};
use uuid::Uuid;

use crate::db::context::{clear_self_user, lock_key_from_uuid, set_self_user, set_tenant};
use crate::db::identity::{append_audit, append_event, lock_sign_in, AuditAppend, EventAppend};

const MEMBERSHIP_LOCK_NAMESPACE: i32 = 1_907_006;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WorkspaceRole {
    Owner,
    Admin,
    Member,
    Guest,
}

impl WorkspaceRole {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Owner => "owner",
            Self::Admin => "admin",
            Self::Member => "member",
            Self::Guest => "guest",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "owner" => Some(Self::Owner),
            "admin" => Some(Self::Admin),
            "member" => Some(Self::Member),
            "guest" => Some(Self::Guest),
            _ => None,
        }
    }

    fn rank(self) -> u8 {
        match self {
            Self::Guest => 0,
            Self::Member => 1,
            Self::Admin => 2,
            Self::Owner => 3,
        }
    }

    pub fn at_least(self, min: Self) -> bool {
        self.rank() >= min.rank()
    }
}

#[derive(Debug)]
pub enum WorkspaceDbError {
    NotFound,
    Forbidden,
    PersonalImmutable,
    LastOwner,
    SelfChange,
    RoleCap,
    SlugTaken,
}

pub struct WorkspaceListItem {
    pub id: Uuid,
    pub name: String,
    pub slug: String,
    pub role: WorkspaceRole,
    pub kind: String,
}

pub struct WorkspaceMeta {
    pub id: Uuid,
    pub name: String,
    pub slug: String,
}

pub struct MemberRow {
    pub user_id: Uuid,
    pub email: String,
    pub given_name: String,
    pub family_name: Option<String>,
    pub role: WorkspaceRole,
}

pub fn personal_workspace_slug(user_id: Uuid) -> String {
    let hex = user_id.simple().to_string();
    let tail = hex.chars().rev().take(12).collect::<String>();
    format!("u-{}", tail.chars().rev().collect::<String>())
}

async fn lock_membership_users(
    tx: &mut Transaction<'_, Postgres>,
    user_ids: &[Uuid],
) -> Result<(), sqlx::Error> {
    let mut keys = user_ids
        .iter()
        .map(|id| lock_key_from_uuid(*id))
        .collect::<Vec<_>>();
    keys.sort_unstable();
    keys.dedup();
    for key in keys {
        sqlx::query("SELECT pg_advisory_xact_lock($1, $2)")
            .bind(MEMBERSHIP_LOCK_NAMESPACE)
            .bind(key)
            .execute(&mut **tx)
            .await?;
    }
    Ok(())
}

async fn session_is_live(
    tx: &mut Transaction<'_, Postgres>,
    user_id: Uuid,
    session_id: Uuid,
) -> Result<bool, sqlx::Error> {
    let live: Option<(bool,)> = sqlx::query_as(
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
    .fetch_optional(&mut **tx)
    .await?;
    Ok(live.map(|(v,)| v).unwrap_or(false))
}

async fn recheck_session(
    tx: &mut Transaction<'_, Postgres>,
    user_id: Uuid,
    session_id: Uuid,
) -> Result<bool, sqlx::Error> {
    let live: Option<(bool,)> = sqlx::query_as(
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
        FOR UPDATE OF u, s
        "#,
    )
    .bind(user_id)
    .bind(session_id)
    .fetch_optional(&mut **tx)
    .await?;
    Ok(live.map(|(v,)| v).unwrap_or(false))
}

async fn user_is_active(
    tx: &mut Transaction<'_, Postgres>,
    user_id: Uuid,
) -> Result<bool, sqlx::Error> {
    let row: Option<(bool,)> =
        sqlx::query_as("SELECT deleted_at IS NULL FROM fvoci.users WHERE id = $1")
            .bind(user_id)
            .fetch_optional(&mut **tx)
            .await?;
    Ok(row.map(|(active,)| active).unwrap_or(false))
}

struct WorkspaceChangeRecord<'a> {
    workspace_id: Uuid,
    actor_user_id: Uuid,
    verb: &'a str,
    target_type: &'a str,
    target_id: Uuid,
    payload: serde_json::Value,
    client_ip: Option<&'a str>,
}

async fn record_workspace_event_and_audit(
    tx: &mut Transaction<'_, Postgres>,
    change: WorkspaceChangeRecord<'_>,
) -> Result<(), sqlx::Error> {
    append_event(
        tx,
        EventAppend {
            id: Uuid::now_v7(),
            workspace_id: Some(change.workspace_id),
            actor_user_id: Some(change.actor_user_id),
            verb: change.verb.to_string(),
            target_type: Some(change.target_type.to_string()),
            target_id: Some(change.target_id),
            payload: change.payload.clone(),
        },
    )
    .await?;
    append_audit(
        tx,
        AuditAppend {
            id: Uuid::now_v7(),
            workspace_id: Some(change.workspace_id),
            actor_user_id: Some(change.actor_user_id),
            verb: change.verb.to_string(),
            target_type: Some(change.target_type.to_string()),
            target_id: Some(change.target_id),
            payload: change.payload,
            ip: change.client_ip.map(str::to_string),
        },
    )
    .await?;
    Ok(())
}

async fn membership_role(
    tx: &mut Transaction<'_, Postgres>,
    workspace_id: Uuid,
    user_id: Uuid,
) -> Result<Option<WorkspaceRole>, sqlx::Error> {
    let row: Option<(String,)> = sqlx::query_as(
        "SELECT role FROM fvoci.memberships WHERE workspace_id = $1 AND user_id = $2",
    )
    .bind(workspace_id)
    .bind(user_id)
    .fetch_optional(&mut **tx)
    .await?;
    Ok(row.and_then(|(role,)| WorkspaceRole::parse(&role)))
}

async fn workspace_kind_read(
    tx: &mut Transaction<'_, Postgres>,
    workspace_id: Uuid,
) -> Result<Option<String>, sqlx::Error> {
    let row: Option<(Option<String>, Option<chrono::DateTime<chrono::Utc>>)> =
        sqlx::query_as("SELECT kind, deleted_at FROM fvoci.workspaces WHERE id = $1")
            .bind(workspace_id)
            .fetch_optional(&mut **tx)
            .await?;
    match row {
        Some((kind, deleted)) if deleted.is_none() => Ok(kind),
        _ => Ok(None),
    }
}

async fn workspace_kind(
    tx: &mut Transaction<'_, Postgres>,
    workspace_id: Uuid,
) -> Result<Option<String>, sqlx::Error> {
    let row: Option<(Option<String>, Option<chrono::DateTime<chrono::Utc>>)> =
        sqlx::query_as("SELECT kind, deleted_at FROM fvoci.workspaces WHERE id = $1 FOR UPDATE")
            .bind(workspace_id)
            .fetch_optional(&mut **tx)
            .await?;
    match row {
        Some((kind, deleted)) if deleted.is_none() => Ok(kind),
        _ => Ok(None),
    }
}

pub async fn list_workspaces_for_user(
    pool: &PgPool,
    user_id: Uuid,
) -> Result<Vec<WorkspaceListItem>, sqlx::Error> {
    let mut tx = pool.begin().await?;
    set_self_user(&mut tx, user_id).await?;
    let memberships = sqlx::query_as::<_, (Uuid, String)>(
        "SELECT workspace_id, role FROM fvoci.memberships WHERE user_id = $1",
    )
    .bind(user_id)
    .fetch_all(&mut *tx)
    .await?;
    clear_self_user(&mut tx).await?;
    tx.commit().await?;

    let mut items = Vec::new();
    for (workspace_id, role_str) in memberships {
        let role = WorkspaceRole::parse(&role_str).unwrap_or(WorkspaceRole::Guest);
        let mut tx = pool.begin().await?;
        set_tenant(&mut tx, workspace_id).await?;
        let row = sqlx::query_as::<_, (Uuid, String, String, String)>(
            "SELECT id, name, slug, kind FROM fvoci.workspaces WHERE id = $1 AND deleted_at IS NULL",
        )
        .bind(workspace_id)
        .fetch_optional(&mut *tx)
        .await?;
        tx.commit().await?;
        if let Some((id, name, slug, kind)) = row {
            items.push(WorkspaceListItem {
                id,
                name,
                slug,
                role,
                kind,
            });
        }
    }
    Ok(items)
}

pub async fn get_workspace_meta(
    pool: &PgPool,
    workspace_id: Uuid,
    actor_user_id: Uuid,
    session_id: Uuid,
) -> Result<Result<WorkspaceMeta, WorkspaceDbError>, sqlx::Error> {
    let mut tx = pool.begin().await?;
    set_tenant(&mut tx, workspace_id).await?;
    if !session_is_live(&mut tx, actor_user_id, session_id).await? {
        tx.rollback().await?;
        return Ok(Err(WorkspaceDbError::Forbidden));
    }
    let role = membership_role(&mut tx, workspace_id, actor_user_id).await?;
    if !role
        .map(|r| r.at_least(WorkspaceRole::Guest))
        .unwrap_or(false)
    {
        tx.rollback().await?;
        return Ok(Err(WorkspaceDbError::Forbidden));
    }
    if workspace_kind_read(&mut tx, workspace_id).await?.is_none() {
        tx.rollback().await?;
        return Ok(Err(WorkspaceDbError::NotFound));
    }
    let row = sqlx::query_as::<_, (Uuid, String, String)>(
        "SELECT id, name, slug FROM fvoci.workspaces WHERE id = $1 AND deleted_at IS NULL",
    )
    .bind(workspace_id)
    .fetch_optional(&mut *tx)
    .await?;
    tx.commit().await?;
    match row {
        Some((id, name, slug)) => Ok(Ok(WorkspaceMeta { id, name, slug })),
        None => Ok(Err(WorkspaceDbError::NotFound)),
    }
}

pub async fn update_workspace_meta(
    pool: &PgPool,
    workspace_id: Uuid,
    actor_user_id: Uuid,
    session_id: Uuid,
    name: &str,
    client_ip: Option<&str>,
) -> Result<Result<WorkspaceMeta, WorkspaceDbError>, sqlx::Error> {
    let mut tx = pool.begin().await?;
    set_tenant(&mut tx, workspace_id).await?;
    lock_membership_users(&mut tx, &[actor_user_id]).await?;
    if !recheck_session(&mut tx, actor_user_id, session_id).await? {
        tx.rollback().await?;
        return Ok(Err(WorkspaceDbError::Forbidden));
    }
    let kind = workspace_kind(&mut tx, workspace_id).await?;
    if kind.is_none() {
        tx.rollback().await?;
        return Ok(Err(WorkspaceDbError::NotFound));
    }
    let role = membership_role(&mut tx, workspace_id, actor_user_id).await?;
    if !role
        .map(|r| r.at_least(WorkspaceRole::Admin))
        .unwrap_or(false)
    {
        tx.rollback().await?;
        return Ok(Err(WorkspaceDbError::Forbidden));
    }
    if kind.as_deref() == Some("personal") {
        tx.rollback().await?;
        return Ok(Err(WorkspaceDbError::PersonalImmutable));
    }
    let previous_name: Option<(String,)> =
        sqlx::query_as("SELECT name FROM fvoci.workspaces WHERE id = $1 AND deleted_at IS NULL")
            .bind(workspace_id)
            .fetch_optional(&mut *tx)
            .await?;
    let Some((from_name,)) = previous_name else {
        tx.rollback().await?;
        return Ok(Err(WorkspaceDbError::NotFound));
    };
    let row = sqlx::query_as::<_, (Uuid, String, String)>(
        r#"
        UPDATE fvoci.workspaces
        SET name = $2, updated_at = now()
        WHERE id = $1 AND deleted_at IS NULL
        RETURNING id, name, slug
        "#,
    )
    .bind(workspace_id)
    .bind(name)
    .fetch_optional(&mut *tx)
    .await?;
    let Some((id, name, slug)) = row else {
        tx.rollback().await?;
        return Ok(Err(WorkspaceDbError::NotFound));
    };
    let payload = json!({
        "workspaceId": workspace_id.to_string(),
        "name": name,
        "fromName": from_name,
    });
    record_workspace_event_and_audit(
        &mut tx,
        WorkspaceChangeRecord {
            workspace_id,
            actor_user_id,
            verb: "workspace.name_updated",
            target_type: "workspace",
            target_id: workspace_id,
            payload,
            client_ip,
        },
    )
    .await?;
    tx.commit().await?;
    Ok(Ok(WorkspaceMeta { id, name, slug }))
}

pub async fn create_workspace_as_instance_admin(
    pool: &PgPool,
    actor_user_id: Uuid,
    session_id: Uuid,
    name: &str,
    slug: &str,
    client_ip: Option<&str>,
) -> Result<Result<WorkspaceMeta, WorkspaceDbError>, sqlx::Error> {
    let workspace_id = Uuid::now_v7();
    let mut tx = pool.begin().await?;
    lock_membership_users(&mut tx, &[actor_user_id]).await?;
    if !recheck_session(&mut tx, actor_user_id, session_id).await? {
        tx.rollback().await?;
        return Ok(Err(WorkspaceDbError::Forbidden));
    }
    let admin: Option<(bool,)> = sqlx::query_as(
        "SELECT is_instance_admin FROM fvoci.users WHERE id = $1 AND deleted_at IS NULL AND suspended_at IS NULL",
    )
    .bind(actor_user_id)
    .fetch_optional(&mut *tx)
    .await?;
    if !admin.map(|(v,)| v).unwrap_or(false) {
        tx.rollback().await?;
        return Ok(Err(WorkspaceDbError::Forbidden));
    }
    set_tenant(&mut tx, workspace_id).await?;
    let inserted = sqlx::query_as::<_, (Uuid, String, String)>(
        "INSERT INTO fvoci.workspaces (id, slug, name) VALUES ($1, $2, $3) RETURNING id, name, slug",
    )
    .bind(workspace_id)
    .bind(slug)
    .bind(name)
    .fetch_optional(&mut *tx)
    .await;
    if let Err(err) = inserted {
        if let Some(db_err) = err.as_database_error() {
            if db_err.constraint() == Some("workspaces_slug_unique") {
                tx.rollback().await?;
                return Ok(Err(WorkspaceDbError::SlugTaken));
            }
        }
        return Err(err);
    }
    let Some((id, name, slug)) = inserted? else {
        tx.rollback().await?;
        return Ok(Err(WorkspaceDbError::NotFound));
    };
    sqlx::query(
        "INSERT INTO fvoci.memberships (workspace_id, user_id, role) VALUES ($1, $2, 'owner')",
    )
    .bind(workspace_id)
    .bind(actor_user_id)
    .execute(&mut *tx)
    .await?;
    let payload = json!({
        "workspaceId": workspace_id.to_string(),
        "ownerId": actor_user_id.to_string(),
        "name": name,
        "slug": slug,
    });
    record_workspace_event_and_audit(
        &mut tx,
        WorkspaceChangeRecord {
            workspace_id,
            actor_user_id,
            verb: "workspace.created",
            target_type: "workspace",
            target_id: workspace_id,
            payload,
            client_ip,
        },
    )
    .await?;
    tx.commit().await?;
    Ok(Ok(WorkspaceMeta { id, name, slug }))
}

pub async fn ensure_personal_workspace(
    pool: &PgPool,
    user_id: Uuid,
    session_id: Uuid,
    client_ip: Option<&str>,
) -> Result<Result<WorkspaceMeta, WorkspaceDbError>, sqlx::Error> {
    let existing: Option<(Option<Uuid>,)> = sqlx::query_as(
        "SELECT personal_workspace_id FROM fvoci.users WHERE id = $1 AND deleted_at IS NULL",
    )
    .bind(user_id)
    .fetch_optional(pool)
    .await?;
    if let Some((Some(personal_id),)) = existing {
        let mut tx = pool.begin().await?;
        set_tenant(&mut tx, personal_id).await?;
        let row = sqlx::query_as::<_, (Uuid, String, String)>(
            "SELECT id, name, slug FROM fvoci.workspaces WHERE id = $1 AND kind = 'personal' AND deleted_at IS NULL",
        )
        .bind(personal_id)
        .fetch_optional(&mut *tx)
        .await?;
        tx.commit().await?;
        if let Some((id, name, slug)) = row {
            return Ok(Ok(WorkspaceMeta { id, name, slug }));
        }
    }

    let workspace_id = Uuid::now_v7();
    let slug = personal_workspace_slug(user_id);
    let mut tx = pool.begin().await?;
    lock_membership_users(&mut tx, &[user_id]).await?;
    if !recheck_session(&mut tx, user_id, session_id).await? {
        tx.rollback().await?;
        return Ok(Err(WorkspaceDbError::Forbidden));
    }
    let current: Option<(Option<Uuid>,)> =
        sqlx::query_as("SELECT personal_workspace_id FROM fvoci.users WHERE id = $1 FOR UPDATE")
            .bind(user_id)
            .fetch_optional(&mut *tx)
            .await?;
    if let Some((Some(personal_id),)) = current {
        set_tenant(&mut tx, personal_id).await?;
        let row = sqlx::query_as::<_, (Uuid, String, String)>(
            "SELECT id, name, slug FROM fvoci.workspaces WHERE id = $1 AND kind = 'personal' AND deleted_at IS NULL",
        )
        .bind(personal_id)
        .fetch_optional(&mut *tx)
        .await?;
        if let Some((id, name, slug)) = row {
            tx.commit().await?;
            return Ok(Ok(WorkspaceMeta { id, name, slug }));
        }
    }
    set_tenant(&mut tx, workspace_id).await?;
    sqlx::query(
        "INSERT INTO fvoci.workspaces (id, slug, name, kind) VALUES ($1, $2, 'Personal', 'personal')",
    )
    .bind(workspace_id)
    .bind(&slug)
    .execute(&mut *tx)
    .await?;
    sqlx::query(
        "INSERT INTO fvoci.memberships (workspace_id, user_id, role) VALUES ($1, $2, 'owner')",
    )
    .bind(workspace_id)
    .bind(user_id)
    .execute(&mut *tx)
    .await?;
    sqlx::query(
        "UPDATE fvoci.users SET personal_workspace_id = $2, updated_at = now() WHERE id = $1",
    )
    .bind(user_id)
    .bind(workspace_id)
    .execute(&mut *tx)
    .await?;
    let payload = json!({
        "workspaceId": workspace_id.to_string(),
        "ownerId": user_id.to_string(),
        "kind": "personal",
        "slug": slug,
    });
    record_workspace_event_and_audit(
        &mut tx,
        WorkspaceChangeRecord {
            workspace_id,
            actor_user_id: user_id,
            verb: "workspace.personal_created",
            target_type: "workspace",
            target_id: workspace_id,
            payload,
            client_ip,
        },
    )
    .await?;
    tx.commit().await?;
    Ok(Ok(WorkspaceMeta {
        id: workspace_id,
        name: "Personal".to_string(),
        slug,
    }))
}

pub async fn set_member_role(
    pool: &PgPool,
    workspace_id: Uuid,
    actor_user_id: Uuid,
    session_id: Uuid,
    target_user_id: Uuid,
    next_role: WorkspaceRole,
    client_ip: Option<&str>,
) -> Result<Result<MemberRow, WorkspaceDbError>, sqlx::Error> {
    let mut tx = pool.begin().await?;
    set_tenant(&mut tx, workspace_id).await?;
    lock_membership_users(&mut tx, &[actor_user_id, target_user_id]).await?;
    if !recheck_session(&mut tx, actor_user_id, session_id).await? {
        tx.rollback().await?;
        return Ok(Err(WorkspaceDbError::Forbidden));
    }
    let kind = workspace_kind(&mut tx, workspace_id).await?;
    if kind.is_none() {
        tx.rollback().await?;
        return Ok(Err(WorkspaceDbError::NotFound));
    }
    let actor_role = membership_role(&mut tx, workspace_id, actor_user_id).await?;
    let actor_role = match actor_role {
        Some(r) if r.at_least(WorkspaceRole::Admin) => r,
        _ => {
            tx.rollback().await?;
            return Ok(Err(WorkspaceDbError::Forbidden));
        }
    };
    if kind.as_deref() == Some("personal") {
        tx.rollback().await?;
        return Ok(Err(WorkspaceDbError::PersonalImmutable));
    }
    if actor_user_id == target_user_id {
        tx.rollback().await?;
        return Ok(Err(WorkspaceDbError::SelfChange));
    }
    if !user_is_active(&mut tx, target_user_id).await? {
        tx.rollback().await?;
        return Ok(Err(WorkspaceDbError::NotFound));
    }
    let target_role = membership_role(&mut tx, workspace_id, target_user_id).await?;
    let target_role = match target_role {
        Some(r) => r,
        None => {
            tx.rollback().await?;
            return Ok(Err(WorkspaceDbError::NotFound));
        }
    };
    if !actor_role.at_least(target_role) || !actor_role.at_least(next_role) {
        tx.rollback().await?;
        return Ok(Err(WorkspaceDbError::RoleCap));
    }
    if target_role == WorkspaceRole::Owner
        && next_role != WorkspaceRole::Owner
        && count_owners(&mut tx, workspace_id).await? <= 1
    {
        tx.rollback().await?;
        return Ok(Err(WorkspaceDbError::LastOwner));
    }
    if next_role == target_role {
        tx.commit().await?;
        let member = fetch_member(pool, workspace_id, target_user_id).await?;
        return Ok(member.ok_or(WorkspaceDbError::NotFound));
    }
    sqlx::query(
        "UPDATE fvoci.memberships SET role = $3, updated_at = now() WHERE workspace_id = $1 AND user_id = $2",
    )
    .bind(workspace_id)
    .bind(target_user_id)
    .bind(next_role.as_str())
    .execute(&mut *tx)
    .await?;
    let payload = json!({
        "userId": target_user_id.to_string(),
        "fromRole": target_role.as_str(),
        "role": next_role.as_str(),
    });
    record_workspace_event_and_audit(
        &mut tx,
        WorkspaceChangeRecord {
            workspace_id,
            actor_user_id,
            verb: "workspace_member.role_changed",
            target_type: "workspace_member",
            target_id: target_user_id,
            payload,
            client_ip,
        },
    )
    .await?;
    tx.commit().await?;
    let member = fetch_member(pool, workspace_id, target_user_id).await?;
    Ok(member.ok_or(WorkspaceDbError::NotFound))
}

pub async fn remove_member(
    pool: &PgPool,
    workspace_id: Uuid,
    actor_user_id: Uuid,
    session_id: Uuid,
    target_user_id: Uuid,
    client_ip: Option<&str>,
) -> Result<Result<(), WorkspaceDbError>, sqlx::Error> {
    let mut tx = pool.begin().await?;
    set_tenant(&mut tx, workspace_id).await?;
    lock_membership_users(&mut tx, &[actor_user_id, target_user_id]).await?;
    if !recheck_session(&mut tx, actor_user_id, session_id).await? {
        tx.rollback().await?;
        return Ok(Err(WorkspaceDbError::Forbidden));
    }
    let kind = workspace_kind(&mut tx, workspace_id).await?;
    if kind.is_none() {
        tx.rollback().await?;
        return Ok(Err(WorkspaceDbError::NotFound));
    }
    let actor_role = membership_role(&mut tx, workspace_id, actor_user_id).await?;
    let actor_role = match actor_role {
        Some(r) if r.at_least(WorkspaceRole::Admin) => r,
        _ => {
            tx.rollback().await?;
            return Ok(Err(WorkspaceDbError::Forbidden));
        }
    };
    if kind.as_deref() == Some("personal") {
        tx.rollback().await?;
        return Ok(Err(WorkspaceDbError::PersonalImmutable));
    }
    if actor_user_id == target_user_id {
        tx.rollback().await?;
        return Ok(Err(WorkspaceDbError::SelfChange));
    }
    if !user_is_active(&mut tx, target_user_id).await? {
        tx.rollback().await?;
        return Ok(Err(WorkspaceDbError::NotFound));
    }
    let target_role = membership_role(&mut tx, workspace_id, target_user_id).await?;
    let target_role = match target_role {
        Some(r) => r,
        None => {
            tx.rollback().await?;
            return Ok(Err(WorkspaceDbError::NotFound));
        }
    };
    if !actor_role.at_least(target_role) {
        tx.rollback().await?;
        return Ok(Err(WorkspaceDbError::RoleCap));
    }
    if target_role == WorkspaceRole::Owner && count_owners(&mut tx, workspace_id).await? <= 1 {
        tx.rollback().await?;
        return Ok(Err(WorkspaceDbError::LastOwner));
    }
    sqlx::query("DELETE FROM fvoci.memberships WHERE workspace_id = $1 AND user_id = $2")
        .bind(workspace_id)
        .bind(target_user_id)
        .execute(&mut *tx)
        .await?;
    let payload = json!({
        "userId": target_user_id.to_string(),
        "role": target_role.as_str(),
    });
    record_workspace_event_and_audit(
        &mut tx,
        WorkspaceChangeRecord {
            workspace_id,
            actor_user_id,
            verb: "workspace_member.removed",
            target_type: "workspace_member",
            target_id: target_user_id,
            payload,
            client_ip,
        },
    )
    .await?;
    tx.commit().await?;
    Ok(Ok(()))
}

async fn count_owners(
    tx: &mut Transaction<'_, Postgres>,
    workspace_id: Uuid,
) -> Result<i64, sqlx::Error> {
    let row: (i64,) = sqlx::query_as(
        "SELECT count(*) FROM fvoci.memberships WHERE workspace_id = $1 AND role = 'owner'",
    )
    .bind(workspace_id)
    .fetch_one(&mut **tx)
    .await?;
    Ok(row.0)
}

async fn fetch_member(
    pool: &PgPool,
    workspace_id: Uuid,
    user_id: Uuid,
) -> Result<Option<MemberRow>, sqlx::Error> {
    let mut tx = pool.begin().await?;
    set_tenant(&mut tx, workspace_id).await?;
    let row = sqlx::query_as::<_, (Uuid, String, String, Option<String>, String)>(
        r#"
        SELECT u.id, u.email, u.given_name, u.family_name, m.role
        FROM fvoci.memberships m
        INNER JOIN fvoci.users u ON u.id = m.user_id
        WHERE m.workspace_id = $1 AND m.user_id = $2 AND u.deleted_at IS NULL
        "#,
    )
    .bind(workspace_id)
    .bind(user_id)
    .fetch_optional(&mut *tx)
    .await?;
    tx.commit().await?;
    Ok(
        row.and_then(|(user_id, email, given_name, family_name, role)| {
            WorkspaceRole::parse(&role).map(|role| MemberRow {
                user_id,
                email,
                given_name,
                family_name,
                role,
            })
        }),
    )
}

pub async fn insert_user_for_test(
    pool: &PgPool,
    user_id: Uuid,
    email: &str,
    given_name: &str,
) -> Result<(), sqlx::Error> {
    sqlx::query(
        "INSERT INTO fvoci.users (id, email, given_name) VALUES ($1, $2, $3) ON CONFLICT DO NOTHING",
    )
    .bind(user_id)
    .bind(email)
    .bind(given_name)
    .execute(pool)
    .await?;
    Ok(())
}

pub async fn add_membership_for_test(
    pool: &PgPool,
    workspace_id: Uuid,
    user_id: Uuid,
    role: WorkspaceRole,
) -> Result<(), sqlx::Error> {
    let mut tx = pool.begin().await?;
    set_tenant(&mut tx, workspace_id).await?;
    sqlx::query(
        "INSERT INTO fvoci.memberships (workspace_id, user_id, role) VALUES ($1, $2, $3) ON CONFLICT DO NOTHING",
    )
    .bind(workspace_id)
    .bind(user_id)
    .bind(role.as_str())
    .execute(&mut *tx)
    .await?;
    tx.commit().await?;
    Ok(())
}

pub async fn suspend_user_for_test(pool: &PgPool, user_id: Uuid) -> Result<(), sqlx::Error> {
    sqlx::query("UPDATE fvoci.users SET suspended_at = now() WHERE id = $1")
        .bind(user_id)
        .execute(pool)
        .await?;
    Ok(())
}

pub async fn tenant_context_probe(
    pool: &PgPool,
    tenant_workspace_id: Uuid,
    query_workspace_id: Uuid,
) -> Result<Option<Uuid>, sqlx::Error> {
    let mut tx = pool.begin().await?;
    set_tenant(&mut tx, tenant_workspace_id).await?;
    let row: Option<(Uuid,)> = sqlx::query_as("SELECT id FROM fvoci.workspaces WHERE id = $1")
        .bind(query_workspace_id)
        .fetch_optional(&mut *tx)
        .await?;
    tx.rollback().await?;
    Ok(row.map(|(id,)| id))
}

pub async fn lock_sign_in_user(
    tx: &mut Transaction<'_, Postgres>,
    user_id: Uuid,
) -> Result<(), sqlx::Error> {
    lock_sign_in(tx, user_id).await
}
