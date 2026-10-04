use serde_json::json;
use sqlx::{PgPool, Postgres, Transaction};
use uuid::Uuid;

use super::backend::{Backend, OperationTx};
use super::codec::Cell;

use crate::db::context::{
    clear_self_user, lock_membership_users, recheck_session, session_is_live, set_self_user,
    set_tenant,
};
use crate::db::identity::{append_audit, append_event, AuditAppend, EventAppend};
use crate::db::projects::{self, visible_project_sql};
use crate::db::quota::{
    acquire_admission_lock, require_membership_admission, require_new_instance_billable_user,
    QuotaError,
};

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
    LastProjectLead,
    SeatLimit,
    GuestLimit,
    InvalidInput,
}

pub struct WorkspaceListItem {
    pub id: Uuid,
    pub name: String,
    pub slug: String,
    pub role: WorkspaceRole,
    pub kind: String,
    pub document_count: i32,
    pub assigned_count: i32,
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

fn quota_error(err: QuotaError) -> WorkspaceDbError {
    match err {
        QuotaError::SeatLimit => WorkspaceDbError::SeatLimit,
        QuotaError::GuestLimit => WorkspaceDbError::GuestLimit,
    }
}

pub fn personal_workspace_slug(user_id: Uuid) -> String {
    let hex = user_id.simple().to_string();
    let tail = hex.chars().rev().take(12).collect::<String>();
    format!("u-{}", tail.chars().rev().collect::<String>())
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

pub(crate) struct WorkspaceChangeRecord<'a> {
    pub workspace_id: Uuid,
    pub actor_user_id: Uuid,
    pub verb: &'a str,
    pub target_type: &'a str,
    pub target_id: Uuid,
    pub payload: serde_json::Value,
    pub client_ip: Option<&'a str>,
}

pub(crate) async fn record_workspace_event_and_audit(
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

pub(crate) async fn membership_role(
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

pub(crate) async fn membership_role_for_update(
    tx: &mut Transaction<'_, Postgres>,
    workspace_id: Uuid,
    user_id: Uuid,
) -> Result<Option<WorkspaceRole>, sqlx::Error> {
    let row: Option<(String,)> = sqlx::query_as(
        "SELECT role FROM fvoci.memberships WHERE workspace_id = $1 AND user_id = $2 FOR UPDATE",
    )
    .bind(workspace_id)
    .bind(user_id)
    .fetch_optional(&mut **tx)
    .await?;
    Ok(row.and_then(|(role,)| WorkspaceRole::parse(&role)))
}

pub(crate) async fn workspace_is_live(
    tx: &mut Transaction<'_, Postgres>,
    workspace_id: Uuid,
) -> Result<bool, sqlx::Error> {
    let row: Option<(bool,)> =
        sqlx::query_as("SELECT deleted_at IS NULL FROM fvoci.workspaces WHERE id = $1")
            .bind(workspace_id)
            .fetch_optional(&mut **tx)
            .await?;
    Ok(row.map(|(live,)| live).unwrap_or(false))
}

impl OperationTx<'_, '_> {
    pub(crate) async fn membership_role(
        &mut self,
        workspace: Uuid,
        user: Uuid,
        for_update: bool,
    ) -> Result<Option<WorkspaceRole>, sqlx::Error> {
        match self {
            Self::Postgres(tx) if for_update => {
                membership_role_for_update(tx, workspace, user).await
            }
            Self::Postgres(tx) => membership_role(tx, workspace, user).await,
            Self::SqliteFamily(tx) => {
                tx.require_tenant(workspace)?;
                if for_update {
                    tx.require_writer()?;
                }
                let rows = tx
                    .query(
                        "SELECT role FROM memberships WHERE workspace_id=?1 AND user_id=?2",
                        &[Cell::uuid(workspace), Cell::uuid(user)],
                    )
                    .await?;
                let role = rows.first().map(|row| row.cell(0)?.string()).transpose()?;
                Ok(role.as_deref().and_then(WorkspaceRole::parse))
            }
        }
    }

    pub(crate) async fn workspace_is_live(&mut self, workspace: Uuid) -> Result<bool, sqlx::Error> {
        match self {
            Self::Postgres(tx) => workspace_is_live(tx, workspace).await,
            Self::SqliteFamily(tx) => {
                tx.require_tenant(workspace)?;
                let rows = tx
                    .query(
                        "SELECT deleted_at IS NULL FROM workspaces WHERE id=?1",
                        &[Cell::uuid(workspace)],
                    )
                    .await?;
                Ok(rows
                    .first()
                    .map(|row| row.cell(0)?.boolean())
                    .transpose()?
                    .unwrap_or(false))
            }
        }
    }
}

pub(crate) async fn workspace_kind_read(
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

#[cfg(feature = "db-tests")]
static WORKSPACE_CARD_BARRIERS: std::sync::LazyLock<
    tokio::sync::Mutex<
        std::collections::HashMap<
            Uuid,
            (
                tokio::sync::oneshot::Sender<()>,
                tokio::sync::oneshot::Receiver<()>,
            ),
        >,
    >,
> = std::sync::LazyLock::new(|| tokio::sync::Mutex::new(std::collections::HashMap::new()));

/// Pause this actor only, after the self-list transaction has actually committed
/// and before any card transaction reserves its writer/current authority.
#[cfg(feature = "db-tests")]
pub async fn arm_workspace_card_barrier(
    user: Uuid,
) -> (
    tokio::sync::oneshot::Receiver<()>,
    tokio::sync::oneshot::Sender<()>,
) {
    let (reached_tx, reached_rx) = tokio::sync::oneshot::channel();
    let (proceed_tx, proceed_rx) = tokio::sync::oneshot::channel();
    assert!(WORKSPACE_CARD_BARRIERS
        .lock()
        .await
        .insert(user, (reached_tx, proceed_rx))
        .is_none());
    (reached_rx, proceed_tx)
}

#[cfg(feature = "db-tests")]
async fn pause_before_workspace_cards(user: Uuid) {
    let barrier = WORKSPACE_CARD_BARRIERS.lock().await.remove(&user);
    if let Some((reached, proceed)) = barrier {
        let _ = reached.send(());
        let _ = proceed.await;
    }
}

pub async fn list_workspaces_for_user(
    pool: &PgPool,
    user_id: Uuid,
) -> Result<Vec<WorkspaceListItem>, sqlx::Error> {
    list_workspaces_for_user_backend(&Backend::Postgres(pool.clone()), user_id).await
}

pub async fn list_workspaces_for_user_backend(
    backend: &Backend,
    user_id: Uuid,
) -> Result<Vec<WorkspaceListItem>, sqlx::Error> {
    // Retain the PG transaction boundaries/isolation and self-user RLS prefix.
    let mut tx = backend.begin_write().await?;
    let memberships = tx.operation().self_workspace_memberships(user_id).await?;
    tx.commit().await.map_err(|unknown| unknown.source)?;
    #[cfg(feature = "db-tests")]
    pause_before_workspace_cards(user_id).await;
    let mut items = Vec::new();
    for (workspace_id, _) in memberships {
        let mut tx = backend.begin_write().await?;
        tx.operation().set_tenant(workspace_id).await?;
        // Enumeration only identifies candidates. Hold current membership
        // authority through this card's read/count transaction.
        let Some(role) = tx
            .operation()
            .membership_role(workspace_id, user_id, true)
            .await?
        else {
            tx.rollback().await?;
            continue;
        };
        let row = tx.operation().live_workspace_card(workspace_id).await?;
        if let Some((id, name, slug, kind)) = row {
            let (document_count, assigned_count) = tx
                .operation()
                .workspace_card_counts(workspace_id, user_id, role)
                .await?;
            items.push(WorkspaceListItem {
                id,
                name,
                slug,
                role,
                kind,
                document_count,
                assigned_count,
            });
        }
        tx.commit().await.map_err(|unknown| unknown.source)?;
    }
    Ok(items)
}

impl OperationTx<'_, '_> {
    async fn self_workspace_memberships(
        &mut self,
        user: Uuid,
    ) -> Result<Vec<(Uuid, String)>, sqlx::Error> {
        match self {
            Self::Postgres(tx) => {
                set_self_user(tx, user).await?;
                let rows = sqlx::query_as(
                    "SELECT workspace_id, role FROM fvoci.memberships WHERE user_id=$1",
                )
                .bind(user)
                .fetch_all(&mut ***tx)
                .await?;
                clear_self_user(tx).await?;
                Ok(rows)
            }
            Self::SqliteFamily(tx) => {
                // This named self read carries its actor explicitly; it cannot
                // enumerate another user's rows through an unscoped query.
                tx.query(
                    "SELECT workspace_id,role FROM memberships WHERE user_id=?1",
                    &[Cell::uuid(user)],
                )
                .await?
                .iter()
                .map(|r| Ok((r.cell(0)?.id()?, r.cell(1)?.string()?)))
                .collect()
            }
        }
    }
    async fn live_workspace_card(
        &mut self,
        workspace: Uuid,
    ) -> Result<Option<(Uuid, String, String, String)>, sqlx::Error> {
        match self {
            Self::Postgres(tx) => sqlx::query_as(
                "SELECT id,name,slug,kind FROM fvoci.workspaces WHERE id=$1 AND deleted_at IS NULL",
            )
            .bind(workspace)
            .fetch_optional(&mut ***tx)
            .await,
            Self::SqliteFamily(tx) => {
                tx.require_tenant(workspace)?;
                let rows=tx.query("SELECT id,name,slug,kind FROM workspaces WHERE id=?1 AND deleted_at IS NULL", &[Cell::uuid(workspace)]).await?;
                rows.first()
                    .map(|r| {
                        Ok((
                            r.cell(0)?.id()?,
                            r.cell(1)?.string()?,
                            r.cell(2)?.string()?,
                            r.cell(3)?.string()?,
                        ))
                    })
                    .transpose()
            }
        }
    }
    async fn workspace_card_counts(
        &mut self,
        workspace: Uuid,
        user: Uuid,
        role: WorkspaceRole,
    ) -> Result<(i32, i32), sqlx::Error> {
        match self {
            Self::Postgres(tx) => workspace_card_counts_in_tx(tx, workspace, user, role).await,
            Self::SqliteFamily(tx) => {
                tx.require_tenant(workspace)?;
                let rows=tx.query(
                    r#"WITH visible_projects AS (
                        SELECT p.id FROM projects p
                        WHERE p.workspace_id=?1 AND p.deleted_at IS NULL
                          AND ((p.visibility='workspace' AND ?2=0)
                            OR EXISTS(SELECT 1 FROM project_members pm
                                      WHERE pm.workspace_id=p.workspace_id AND pm.project_id=p.id AND pm.user_id=?3)
                            OR EXISTS(SELECT 1 FROM project_members pm
                                      INNER JOIN group_members gm ON gm.workspace_id=pm.workspace_id AND gm.group_id=pm.group_id
                                      WHERE pm.workspace_id=p.workspace_id AND pm.project_id=p.id AND gm.user_id=?3 AND pm.group_id IS NOT NULL))
                    )
                    SELECT
                      (SELECT count(*) FROM documents d
                       WHERE d.workspace_id=?1 AND d.deleted_at IS NULL
                         AND ((d.project_id IS NULL AND ?2=0) OR d.project_id IN (SELECT id FROM visible_projects))),
                      (SELECT count(*) FROM tasks t
                       WHERE t.workspace_id=?1 AND t.deleted_at IS NULL AND t.archived_at IS NULL
                         AND t.project_id IN (SELECT id FROM visible_projects)
                         AND EXISTS(SELECT 1 FROM task_assignees a WHERE a.workspace_id=t.workspace_id AND a.task_id=t.id AND a.user_id=?3)
                         AND EXISTS(SELECT 1 FROM statuses s_open WHERE s_open.workspace_id=t.workspace_id AND s_open.project_id=t.project_id AND s_open.id=t.status_id AND s_open.category NOT IN ('done','canceled')))
                    "#,
                    &[Cell::uuid(workspace),Cell::Integer(i64::from(role==WorkspaceRole::Guest)),Cell::uuid(user)]
                ).await?;
                let row = rows
                    .first()
                    .ok_or_else(|| sqlx::Error::Protocol("workspace card counts absent".into()))?;
                let documents = row.cell(0)?.integer()?;
                let assigned = row.cell(1)?.integer()?;
                if documents < 0 || assigned < 0 {
                    return Err(sqlx::Error::Protocol("negative workspace count".into()));
                }
                Ok((
                    i32::try_from(documents).unwrap_or(i32::MAX),
                    i32::try_from(assigned).unwrap_or(i32::MAX),
                ))
            }
        }
    }
}

pub async fn list_members(
    pool: &PgPool,
    workspace_id: Uuid,
    actor_user_id: Uuid,
    session_id: Uuid,
) -> Result<Result<Vec<MemberRow>, WorkspaceDbError>, sqlx::Error> {
    let mut tx = pool.begin().await?;
    set_tenant(&mut tx, workspace_id).await?;
    if !session_is_live(&mut tx, actor_user_id, session_id).await? {
        tx.rollback().await?;
        return Ok(Err(WorkspaceDbError::Forbidden));
    }
    let role = membership_role(&mut tx, workspace_id, actor_user_id).await?;
    if !role
        .map(|r| r.at_least(WorkspaceRole::Member))
        .unwrap_or(false)
    {
        tx.rollback().await?;
        return Ok(Err(WorkspaceDbError::Forbidden));
    }
    if workspace_kind_read(&mut tx, workspace_id).await?.is_none() {
        tx.rollback().await?;
        return Ok(Err(WorkspaceDbError::NotFound));
    }
    let rows = sqlx::query_as::<_, (Uuid, String, String, Option<String>, String)>(
        r#"
        SELECT u.id, u.email, u.given_name, u.family_name, m.role
        FROM fvoci.memberships m
        INNER JOIN fvoci.users u ON u.id = m.user_id
        WHERE m.workspace_id = $1 AND u.deleted_at IS NULL
        ORDER BY m.created_at ASC, u.id ASC
        "#,
    )
    .bind(workspace_id)
    .fetch_all(&mut *tx)
    .await?;
    tx.commit().await?;
    Ok(Ok(rows
        .into_iter()
        .filter_map(|(user_id, email, given_name, family_name, role)| {
            WorkspaceRole::parse(&role).map(|role| MemberRow {
                user_id,
                email,
                given_name,
                family_name,
                role,
            })
        })
        .collect()))
}

pub async fn get_workspace_meta(
    pool: &PgPool,
    workspace_id: Uuid,
    actor_user_id: Uuid,
    session_id: Uuid,
) -> Result<Result<WorkspaceMeta, WorkspaceDbError>, sqlx::Error> {
    get_workspace_meta_backend(
        &Backend::Postgres(pool.clone()),
        workspace_id,
        actor_user_id,
        session_id,
    )
    .await
}

pub async fn get_workspace_meta_backend(
    backend: &Backend,
    workspace_id: Uuid,
    actor_user_id: Uuid,
    session_id: Uuid,
) -> Result<Result<WorkspaceMeta, WorkspaceDbError>, sqlx::Error> {
    let mut tx = backend.begin_write().await?;
    tx.operation().set_tenant(workspace_id).await?;
    if !tx
        .operation()
        .session_is_live(actor_user_id, session_id)
        .await?
    {
        tx.rollback().await?;
        return Ok(Err(WorkspaceDbError::Forbidden));
    }
    let role = tx
        .operation()
        .membership_role(workspace_id, actor_user_id, false)
        .await?;
    if !role
        .map(|r| r.at_least(WorkspaceRole::Guest))
        .unwrap_or(false)
    {
        tx.rollback().await?;
        return Ok(Err(WorkspaceDbError::Forbidden));
    }
    let row = tx.operation().live_workspace_card(workspace_id).await?;
    tx.commit().await.map_err(|unknown| unknown.source)?;
    Ok(match row {
        Some((id, name, slug, _)) => Ok(WorkspaceMeta { id, name, slug }),
        None => Err(WorkspaceDbError::NotFound),
    })
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
    license: &crate::license::Entitlements,
    actor_user_id: Uuid,
    session_id: Uuid,
    name: &str,
    slug: &str,
    client_ip: Option<&str>,
) -> Result<Result<WorkspaceMeta, WorkspaceDbError>, sqlx::Error> {
    let workspace_id = Uuid::now_v7();
    let mut tx = pool.begin().await?;
    acquire_admission_lock(&mut tx).await?;
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
    if let Err(err) =
        require_new_instance_billable_user(&mut tx, Some(actor_user_id), license).await?
    {
        tx.rollback().await?;
        return Ok(Err(quota_error(err)));
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
    license: &crate::license::Entitlements,
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
    let mut slug = personal_workspace_slug(user_id);
    let mut tx = pool.begin().await?;
    acquire_admission_lock(&mut tx).await?;
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
    if let Err(err) = require_new_instance_billable_user(&mut tx, Some(user_id), license).await? {
        tx.rollback().await?;
        return Ok(Err(quota_error(err)));
    }
    set_tenant(&mut tx, workspace_id).await?;
    // A supported team may already own the deterministic address. Do not
    // look up, disclose or convert that tenant. Only the slug conflict is
    // handled; other constraints/errors still propagate. Existing mappings
    // and free deterministic addresses above remain stable.
    let mut inserted = false;
    for attempt in 0..4 {
        if attempt > 0 {
            let alternate = Uuid::now_v7().simple().to_string();
            slug = format!("u-{}", &alternate[4..]);
        }
        let id: Option<Uuid> = sqlx::query_scalar(
            "INSERT INTO fvoci.workspaces (id, slug, name, kind) VALUES ($1, $2, 'Personal', 'personal') ON CONFLICT (slug) DO NOTHING RETURNING id",
        )
        .bind(workspace_id)
        .bind(&slug)
        .fetch_optional(&mut *tx)
        .await?;
        if id.is_some() {
            inserted = true;
            break;
        }
    }
    if !inserted {
        tx.rollback().await?;
        return Ok(Err(WorkspaceDbError::SlugTaken));
    }
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
    db: &crate::db::Db,
    workspace_id: Uuid,
    actor_user_id: Uuid,
    session_id: Uuid,
    target_user_id: Uuid,
    next_role: WorkspaceRole,
    client_ip: Option<&str>,
) -> Result<Result<MemberRow, WorkspaceDbError>, sqlx::Error> {
    let pool = db.pool.postgres("workspace.member_role")?;
    let license = &db.license;
    let mut tx = pool.begin().await?;
    acquire_admission_lock(&mut tx).await?;
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
    if let Err(err) = require_membership_admission(
        &mut tx,
        target_user_id,
        next_role,
        Some(target_role),
        license,
    )
    .await?
    {
        tx.rollback().await?;
        return Ok(Err(quota_error(err)));
    }
    sqlx::query(
        "UPDATE fvoci.memberships SET role = $3, updated_at = now() WHERE workspace_id = $1 AND user_id = $2",
    )
    .bind(workspace_id)
    .bind(target_user_id)
    .bind(next_role.as_str())
    .execute(&mut *tx)
    .await?;
    let revoke_roles = if next_role.at_least(WorkspaceRole::Admin) {
        [
            WorkspaceRole::Owner,
            WorkspaceRole::Admin,
            WorkspaceRole::Member,
            WorkspaceRole::Guest,
        ]
        .into_iter()
        .filter(|role| !next_role.at_least(*role))
        .collect::<Vec<_>>()
    } else {
        vec![
            WorkspaceRole::Owner,
            WorkspaceRole::Admin,
            WorkspaceRole::Member,
            WorkspaceRole::Guest,
        ]
    };
    let revoked_invitations = crate::db::invitations::remove_pending_by_inviter(
        &mut tx,
        workspace_id,
        target_user_id,
        &revoke_roles,
    )
    .await?;
    let payload = json!({
        "userId": target_user_id.to_string(),
        "fromRole": target_role.as_str(),
        "role": next_role.as_str(),
        "revokedInvitations": revoked_invitations,
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
    if projects::workspace_removal_blocked_by_private_leads(&mut tx, workspace_id, target_user_id)
        .await?
    {
        tx.rollback().await?;
        return Ok(Err(WorkspaceDbError::LastProjectLead));
    }
    let revoked_invitations = crate::db::invitations::remove_pending_by_inviter(
        &mut tx,
        workspace_id,
        target_user_id,
        &[
            WorkspaceRole::Owner,
            WorkspaceRole::Admin,
            WorkspaceRole::Member,
            WorkspaceRole::Guest,
        ],
    )
    .await?;
    let transferred_shared_views = crate::db::collections::transfer_shared_view_ownership(
        &mut tx,
        workspace_id,
        target_user_id,
        actor_user_id,
    )
    .await?;
    sqlx::query("DELETE FROM fvoci.memberships WHERE workspace_id = $1 AND user_id = $2")
        .bind(workspace_id)
        .bind(target_user_id)
        .execute(&mut *tx)
        .await?;
    let payload = json!({
        "userId": target_user_id.to_string(),
        "role": target_role.as_str(),
        "revokedInvitations": revoked_invitations,
        "transferredSharedViews": transferred_shared_views,
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
    match tx.commit().await {
        Ok(()) => Ok(Ok(())),
        Err(err) if crate::db::projects::is_private_lead_violation(&err) => {
            Ok(Err(WorkspaceDbError::LastProjectLead))
        }
        Err(err) => Err(err),
    }
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

const WORKSPACE_PURGE_AFTER_DAYS: i64 = 30;

fn card_count_visible_sql(project_alias: &str) -> String {
    visible_project_sql(project_alias, 2, 3)
}

pub(crate) async fn workspace_card_counts_in_tx(
    tx: &mut Transaction<'_, Postgres>,
    workspace_id: Uuid,
    user_id: Uuid,
    role: WorkspaceRole,
) -> Result<(i32, i32), sqlx::Error> {
    let is_guest = role == WorkspaceRole::Guest;
    let visible = card_count_visible_sql("p");
    let document_sql = format!(
        r#"
        SELECT count(*)::bigint
        FROM fvoci.documents d
        WHERE d.workspace_id = $1
          AND d.deleted_at IS NULL
          AND (
            (d.project_id IS NULL AND $2 = false)
            OR (
              d.project_id IS NOT NULL
              AND EXISTS (
                SELECT 1
                FROM fvoci.projects p
                WHERE p.workspace_id = d.workspace_id
                  AND p.id = d.project_id
                  AND p.deleted_at IS NULL
                  AND {visible}
              )
            )
          )
        "#
    );
    let assigned_sql = format!(
        r#"
        SELECT count(*)::bigint
        FROM fvoci.tasks t
        INNER JOIN fvoci.projects p
          ON p.workspace_id = t.workspace_id
         AND p.id = t.project_id
         AND p.deleted_at IS NULL
        WHERE t.workspace_id = $1
          AND t.deleted_at IS NULL
          AND t.archived_at IS NULL
          AND EXISTS (
            SELECT 1 FROM fvoci.task_assignees a
            WHERE a.workspace_id = t.workspace_id
              AND a.task_id = t.id
              AND a.user_id = $3
          )
          AND EXISTS (
            SELECT 1 FROM fvoci.statuses s_open
            WHERE s_open.workspace_id = t.workspace_id
              AND s_open.project_id = t.project_id
              AND s_open.id = t.status_id
              AND s_open.category NOT IN ('done', 'canceled')
          )
          AND {visible}
        "#
    );
    let document_count: i64 = sqlx::query_scalar(&document_sql)
        .bind(workspace_id)
        .bind(is_guest)
        .bind(user_id)
        .fetch_one(&mut **tx)
        .await?;
    let assigned_count: i64 = sqlx::query_scalar(&assigned_sql)
        .bind(workspace_id)
        .bind(is_guest)
        .bind(user_id)
        .fetch_one(&mut **tx)
        .await?;
    Ok((
        i32::try_from(document_count).unwrap_or(i32::MAX),
        i32::try_from(assigned_count).unwrap_or(i32::MAX),
    ))
}

pub async fn trash_workspace(
    pool: &PgPool,
    workspace_id: Uuid,
    actor_user_id: Uuid,
    session_id: Uuid,
    confirm_slug: &str,
    client_ip: Option<&str>,
) -> Result<Result<(), WorkspaceDbError>, sqlx::Error> {
    let mut tx = pool.begin().await?;
    set_tenant(&mut tx, workspace_id).await?;
    acquire_admission_lock(&mut tx).await?;
    let member_ids: Vec<(Uuid,)> =
        sqlx::query_as("SELECT user_id FROM fvoci.memberships WHERE workspace_id = $1")
            .bind(workspace_id)
            .fetch_all(&mut *tx)
            .await?;
    let mut lock_ids: Vec<Uuid> = member_ids.into_iter().map(|(id,)| id).collect();
    if !lock_ids.contains(&actor_user_id) {
        lock_ids.push(actor_user_id);
    }
    lock_membership_users(&mut tx, &lock_ids).await?;
    if !recheck_session(&mut tx, actor_user_id, session_id).await? {
        tx.rollback().await?;
        return Ok(Err(WorkspaceDbError::Forbidden));
    }
    if !user_is_active(&mut tx, actor_user_id).await? {
        tx.rollback().await?;
        return Ok(Err(WorkspaceDbError::Forbidden));
    }
    let actor_role = membership_role(&mut tx, workspace_id, actor_user_id).await?;
    if !actor_role
        .map(|role| role.at_least(WorkspaceRole::Owner))
        .unwrap_or(false)
    {
        tx.rollback().await?;
        return Ok(Err(WorkspaceDbError::Forbidden));
    }
    let locked = sqlx::query_as::<_, (String, String, Option<chrono::DateTime<chrono::Utc>>)>(
        "SELECT slug, kind, deleted_at FROM fvoci.workspaces WHERE id = $1 FOR UPDATE",
    )
    .bind(workspace_id)
    .fetch_optional(&mut *tx)
    .await?;
    let Some((slug, kind, deleted_at)) = locked else {
        tx.rollback().await?;
        return Ok(Err(WorkspaceDbError::NotFound));
    };
    if deleted_at.is_some() {
        tx.rollback().await?;
        return Ok(Err(WorkspaceDbError::NotFound));
    }
    if kind == "personal" {
        tx.rollback().await?;
        return Ok(Err(WorkspaceDbError::PersonalImmutable));
    }
    if slug != confirm_slug {
        tx.rollback().await?;
        return Ok(Err(WorkspaceDbError::InvalidInput));
    }
    crate::db::invitations::remove_by_workspace(&mut tx, workspace_id).await?;
    crate::db::api_tokens::remove_by_workspace(&mut tx, workspace_id).await?;
    sqlx::query("DELETE FROM fvoci.memberships WHERE workspace_id = $1")
        .bind(workspace_id)
        .execute(&mut *tx)
        .await?;
    let marked: Option<(Uuid,)> = sqlx::query_as(
        r#"
        UPDATE fvoci.workspaces
        SET deleted_at = now(), updated_at = now()
        WHERE id = $1 AND deleted_at IS NULL
        RETURNING id
        "#,
    )
    .bind(workspace_id)
    .fetch_optional(&mut *tx)
    .await?;
    if marked.is_none() {
        tx.rollback().await?;
        return Ok(Err(WorkspaceDbError::NotFound));
    }
    record_workspace_event_and_audit(
        &mut tx,
        WorkspaceChangeRecord {
            workspace_id,
            actor_user_id,
            verb: "workspace.deleted",
            target_type: "workspace",
            target_id: workspace_id,
            payload: json!({}),
            client_ip,
        },
    )
    .await?;
    tx.commit().await?;
    Ok(Ok(()))
}

#[derive(Debug, Clone)]
pub struct WorkspacePurgeResult {
    pub purged: bool,
    pub storage_keys: Vec<String>,
}

pub async fn list_deleted_workspace_ids(
    pool: &PgPool,
    before: chrono::DateTime<chrono::Utc>,
) -> Result<Vec<Uuid>, sqlx::Error> {
    let mut tx = pool.begin().await?;
    crate::db::context::set_system(&mut tx).await?;
    let rows = sqlx::query_as::<_, (Uuid,)>(
        r#"
        SELECT id
        FROM fvoci.workspaces
        WHERE deleted_at IS NOT NULL
          AND (kind = 'personal' OR deleted_at <= $1)
        ORDER BY deleted_at ASC, id ASC
        "#,
    )
    .bind(before)
    .fetch_all(&mut *tx)
    .await?;
    tx.commit().await?;
    Ok(rows.into_iter().map(|(id,)| id).collect())
}

pub async fn purge_workspace(
    pool: &PgPool,
    workspace_id: Uuid,
) -> Result<WorkspacePurgeResult, sqlx::Error> {
    let mut tx = pool.begin().await?;
    let previous = crate::db::context::set_system(&mut tx).await?;
    set_tenant(&mut tx, workspace_id).await?;
    let locked: Option<(Option<chrono::DateTime<chrono::Utc>>,)> =
        sqlx::query_as("SELECT deleted_at FROM fvoci.workspaces WHERE id = $1 FOR UPDATE")
            .bind(workspace_id)
            .fetch_optional(&mut *tx)
            .await?;
    let Some((Some(_),)) = locked else {
        crate::db::context::restore_system(&mut tx, &previous).await?;
        tx.commit().await?;
        return Ok(WorkspacePurgeResult {
            purged: false,
            storage_keys: Vec::new(),
        });
    };
    let keys: Vec<(String,)> = sqlx::query_as(
        "DELETE FROM fvoci.attachments WHERE workspace_id = $1 RETURNING storage_key",
    )
    .bind(workspace_id)
    .fetch_all(&mut *tx)
    .await?;
    sqlx::query("DELETE FROM fvoci.memberships WHERE workspace_id = $1")
        .bind(workspace_id)
        .execute(&mut *tx)
        .await?;
    let deleted: Option<(Uuid,)> = sqlx::query_as(
        "DELETE FROM fvoci.workspaces WHERE id = $1 AND deleted_at IS NOT NULL RETURNING id",
    )
    .bind(workspace_id)
    .fetch_optional(&mut *tx)
    .await?;
    crate::db::context::restore_system(&mut tx, &previous).await?;
    if deleted.is_none() {
        tx.rollback().await?;
        return Err(sqlx::Error::Protocol(
            "workspaces.purge: locked workspace was not deleted".into(),
        ));
    }
    tx.commit().await?;
    Ok(WorkspacePurgeResult {
        purged: true,
        storage_keys: keys.into_iter().map(|(key,)| key).collect(),
    })
}

pub async fn sweep_deleted_workspaces(
    pool: &PgPool,
    now: chrono::DateTime<chrono::Utc>,
) -> Result<Vec<WorkspacePurgeResult>, sqlx::Error> {
    let cutoff = now - chrono::Duration::days(WORKSPACE_PURGE_AFTER_DAYS);
    let ids = list_deleted_workspace_ids(pool, cutoff).await?;
    let mut results = Vec::new();
    for id in ids {
        match purge_workspace(pool, id).await {
            Ok(result) => results.push(result),
            Err(err) => {
                tracing::error!(workspace_id = %id, error = %err, "cleanup.workspace_purge_failed");
            }
        }
    }
    Ok(results)
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

#[cfg(feature = "db-tests")]
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

#[cfg(feature = "db-tests")]
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

#[cfg(feature = "db-tests")]
pub async fn suspend_user_for_test(pool: &PgPool, user_id: Uuid) -> Result<(), sqlx::Error> {
    sqlx::query("UPDATE fvoci.users SET suspended_at = now() WHERE id = $1")
        .bind(user_id)
        .execute(pool)
        .await?;
    Ok(())
}

#[cfg(feature = "db-tests")]
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

#[cfg(feature = "db-tests")]
pub async fn lock_sign_in_user(
    tx: &mut Transaction<'_, Postgres>,
    user_id: Uuid,
) -> Result<(), sqlx::Error> {
    crate::db::identity::lock_sign_in(tx, user_id).await
}
