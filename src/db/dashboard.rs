//! GET /me/dashboard and GET /me/locate.
//!
//! Source: packages/core/src/dashboard.ts (buildDashboard) and
//! packages/core/src/workspace.ts (locateForUser). Each workspace is read in
//! its own tenant transaction after the session and membership are rechecked;
//! visibility reuses the search ACL (`project_permission` /
//! `document_permission`), the same boundary as search hydrate.

use std::cmp::Ordering;
use std::collections::HashSet;

use chrono::{DateTime, NaiveDate, Utc};
use sqlx::{PgPool, Postgres, Transaction};
use uuid::Uuid;

use crate::db::context::{clear_self_user, session_is_live, set_self_user, set_tenant};
use crate::db::documents::document_permission;
use crate::db::projects::{load_live_project, project_permission};
use crate::db::tasks::{list_open_assigned_in_tx, TaskListItemRow};
use crate::db::workspace::{workspace_card_counts_in_tx, MemberRow, WorkspaceRole};
use crate::projects::ProjectPermission;
use crate::search::query::{load_search_acl, SearchAcl};

/// Source `DASHBOARD_ASSIGNED_LIMIT`.
pub const DASHBOARD_ASSIGNED_LIMIT: i64 = 50;
/// Source `RECENT_LIMIT`.
pub const DASHBOARD_RECENT_LIMIT: i64 = 12;

pub struct DashboardRecentItem {
    pub kind: &'static str,
    pub id: Uuid,
    pub title: String,
    pub project_id: Option<Uuid>,
    pub number: i32,
    pub updated_at: DateTime<Utc>,
    pub workspace_id: Uuid,
}

pub struct DashboardLabel {
    pub id: Uuid,
    pub project_id: Uuid,
    pub name: String,
    pub color: String,
}

pub struct DashboardStatus {
    pub id: Uuid,
    pub workflow_id: Uuid,
    pub project_id: Uuid,
    pub name: String,
    pub sort_key: String,
    pub category: String,
    pub wip_limit: Option<i32>,
}

pub struct DashboardProject {
    pub id: Uuid,
    pub workspace_id: Uuid,
    pub key: String,
    pub name: String,
}

pub struct DashboardWorkspace {
    pub id: Uuid,
    pub name: String,
    pub slug: String,
    pub role: WorkspaceRole,
    pub kind: String,
    pub document_count: i32,
    pub assigned_count: i32,
    pub unread_count: i64,
}

pub struct Dashboard {
    pub assigned: Vec<TaskListItemRow>,
    pub recent: Vec<DashboardRecentItem>,
    pub labels: Vec<DashboardLabel>,
    pub statuses: Vec<DashboardStatus>,
    pub projects: Vec<DashboardProject>,
    pub members: Vec<MemberRow>,
    pub workspaces: Vec<DashboardWorkspace>,
    pub unread_count: i64,
}

#[derive(Debug, PartialEq, Eq)]
pub enum DashboardError {
    SessionGone,
}

async fn my_memberships(pool: &PgPool, user_id: Uuid) -> Result<Vec<(Uuid, String)>, sqlx::Error> {
    let mut tx = pool.begin().await?;
    set_self_user(&mut tx, user_id).await?;
    let rows = sqlx::query_as::<_, (Uuid, String)>(
        r#"
        SELECT workspace_id, role FROM fvoci.memberships
        WHERE user_id = $1
        ORDER BY created_at ASC, workspace_id ASC
        "#,
    )
    .bind(user_id)
    .fetch_all(&mut *tx)
    .await?;
    clear_self_user(&mut tx).await?;
    tx.commit().await?;
    Ok(rows)
}

/// Zone names PostgreSQL accepts for `AT TIME ZONE`, read once per process:
/// scanning `pg_timezone_names` walks the zone database on every call.
static TIME_ZONES: tokio::sync::OnceCell<HashSet<String>> = tokio::sync::OnceCell::const_new();

async fn known_time_zones(pool: &PgPool) -> Result<&'static HashSet<String>, sqlx::Error> {
    TIME_ZONES
        .get_or_try_init(|| async {
            let names: Vec<String> =
                sqlx::query_scalar("SELECT name FROM pg_catalog.pg_timezone_names")
                    .fetch_all(pool)
                    .await?;
            Ok(names.into_iter().collect())
        })
        .await
}

pub(crate) async fn user_time_zone(pool: &PgPool, user_id: Uuid) -> Result<String, sqlx::Error> {
    let tz: Option<String> = sqlx::query_scalar("SELECT timezone FROM fvoci.users WHERE id = $1")
        .bind(user_id)
        .fetch_optional(pool)
        .await?;
    // Source falls back to UTC; an unknown zone name must not fail the query.
    let known = known_time_zones(pool).await?;
    Ok(tz
        .filter(|name| known.contains(name))
        .unwrap_or_else(|| "UTC".to_string()))
}

pub async fn user_time_zone_backend(
    backend: &super::backend::Backend,
    user_id: Uuid,
) -> Result<String, sqlx::Error> {
    use super::backend::{Backend, DbTx};
    use super::codec::Cell;
    if let Backend::Postgres(pool) = backend {
        return user_time_zone(pool, user_id).await;
    }
    let mut tx = backend.begin_read().await?;
    let DbTx::SqliteFamily(family) = &mut tx else {
        unreachable!("family timezone read");
    };
    let rows = family
        .query(
            "SELECT timezone FROM users WHERE id=?1",
            &[Cell::uuid(user_id)],
        )
        .await?;
    let zone = rows.first().map(|row| row.cell(0)?.string()).transpose()?;
    tx.rollback().await?;
    Ok(zone
        .filter(|name| jiff_tzdb::get(name).is_some_and(|(known, _)| known == *name))
        .unwrap_or_else(|| "UTC".into()))
}

/// Current membership role and live workspace under the tenant context.
async fn live_member_role(
    tx: &mut Transaction<'_, Postgres>,
    workspace_id: Uuid,
    user_id: Uuid,
) -> Result<Option<(WorkspaceRole, String, String, String)>, sqlx::Error> {
    let row: Option<(String, String, String, String)> = sqlx::query_as(
        r#"
        SELECT m.role, w.name, w.slug, w.kind
        FROM fvoci.memberships m
        INNER JOIN fvoci.workspaces w ON w.id = m.workspace_id AND w.deleted_at IS NULL
        WHERE m.workspace_id = $1 AND m.user_id = $2
        "#,
    )
    .bind(workspace_id)
    .bind(user_id)
    .fetch_optional(&mut **tx)
    .await?;
    Ok(row.and_then(|(role, name, slug, kind)| {
        WorkspaceRole::parse(&role).map(|role| (role, name, slug, kind))
    }))
}

struct WorkspacePart {
    workspace: DashboardWorkspace,
    assigned: Vec<(TaskListItemRow, Option<NaiveDate>)>,
    recent: Vec<DashboardRecentItem>,
    labels: Vec<DashboardLabel>,
    statuses: Vec<DashboardStatus>,
    projects: Vec<DashboardProject>,
    members: Vec<MemberRow>,
}

async fn collect_workspace(
    pool: &PgPool,
    workspace_id: Uuid,
    user_id: Uuid,
    session_id: Uuid,
    time_zone: &str,
) -> Result<Result<Option<WorkspacePart>, DashboardError>, sqlx::Error> {
    let mut tx = pool.begin().await?;
    set_tenant(&mut tx, workspace_id).await?;
    if !session_is_live(&mut tx, user_id, session_id).await? {
        tx.rollback().await?;
        return Ok(Err(DashboardError::SessionGone));
    }
    let Some((role, name, slug, kind)) = live_member_role(&mut tx, workspace_id, user_id).await?
    else {
        tx.rollback().await?;
        return Ok(Ok(None));
    };
    let acl = load_search_acl(&mut tx, workspace_id, user_id, role, None).await?;
    let assigned = list_open_assigned_in_tx(
        &mut tx,
        workspace_id,
        user_id,
        &acl.project_ids,
        time_zone,
        DASHBOARD_ASSIGNED_LIMIT,
    )
    .await?;
    let recent = recent_in_tx(&mut tx, workspace_id, &acl, DASHBOARD_RECENT_LIMIT).await?;
    let labels = labels_in_tx(&mut tx, workspace_id, &acl.project_ids).await?;
    let statuses = statuses_in_tx(&mut tx, workspace_id, &acl.project_ids).await?;
    let projects = projects_in_tx(&mut tx, workspace_id, &acl.project_ids).await?;
    let assignee_ids: Vec<Uuid> = assigned
        .iter()
        .flat_map(|(row, _)| row.assignee_ids.iter().copied())
        .collect::<HashSet<_>>()
        .into_iter()
        .collect();
    let members = members_in_tx(&mut tx, workspace_id, &assignee_ids).await?;
    let (document_count, assigned_count) =
        workspace_card_counts_in_tx(&mut tx, workspace_id, user_id, role).await?;
    tx.commit().await?;
    let unread_count = if role == WorkspaceRole::Guest {
        0
    } else {
        crate::db::notifications::unread_count(pool, workspace_id, user_id, session_id, None)
            .await?
            .unwrap_or_default()
    };
    Ok(Ok(Some(WorkspacePart {
        workspace: DashboardWorkspace {
            id: workspace_id,
            name,
            slug,
            role,
            kind,
            document_count,
            assigned_count,
            unread_count,
        },
        assigned,
        recent,
        labels,
        statuses,
        projects,
        members,
    })))
}

/// Source `search.listRecent`: live documents and tasks the actor can read,
/// updated_at (ms) descending, id descending.
async fn recent_in_tx(
    tx: &mut Transaction<'_, Postgres>,
    workspace_id: Uuid,
    acl: &SearchAcl,
    limit: i64,
) -> Result<Vec<DashboardRecentItem>, sqlx::Error> {
    let rows = sqlx::query_as::<_, (String, Uuid, String, Option<Uuid>, i32, DateTime<Utc>)>(
        r#"
        SELECT kind, id, title, project_id, number, ua FROM (
            SELECT 'document'::text AS kind, d.id, d.title, d.project_id, d.number,
                   date_trunc('milliseconds', d.updated_at) AS ua
            FROM fvoci.documents d
            WHERE d.workspace_id = $1 AND d.deleted_at IS NULL
              AND (
                (d.project_id IS NOT NULL AND d.project_id = ANY($2))
                OR (d.project_id IS NULL AND ($3 OR d.id = ANY($4)))
              )
            UNION ALL
            SELECT 'task'::text AS kind, t.id, t.title, t.project_id, t.number,
                   date_trunc('milliseconds', t.updated_at) AS ua
            FROM fvoci.tasks t
            WHERE t.workspace_id = $1 AND t.deleted_at IS NULL AND t.archived_at IS NULL
              AND t.project_id = ANY($2)
        ) u
        ORDER BY ua DESC, id DESC
        LIMIT $5
        "#,
    )
    .bind(workspace_id)
    .bind(&acl.project_ids)
    .bind(acl.include_wiki)
    .bind(&acl.wiki_document_ids)
    .bind(limit)
    .fetch_all(&mut **tx)
    .await?;
    Ok(rows
        .into_iter()
        .map(
            |(kind, id, title, project_id, number, updated_at)| DashboardRecentItem {
                kind: if kind == "task" { "task" } else { "document" },
                id,
                title,
                project_id,
                number,
                updated_at,
                workspace_id,
            },
        )
        .collect())
}

async fn labels_in_tx(
    tx: &mut Transaction<'_, Postgres>,
    workspace_id: Uuid,
    project_ids: &[Uuid],
) -> Result<Vec<DashboardLabel>, sqlx::Error> {
    let rows = sqlx::query_as::<_, (Uuid, Uuid, String, String)>(
        r#"
        SELECT id, project_id, name, color FROM fvoci.labels
        WHERE workspace_id = $1 AND project_id = ANY($2)
        ORDER BY project_id, name COLLATE "C", id
        "#,
    )
    .bind(workspace_id)
    .bind(project_ids)
    .fetch_all(&mut **tx)
    .await?;
    Ok(rows
        .into_iter()
        .map(|(id, project_id, name, color)| DashboardLabel {
            id,
            project_id,
            name,
            color,
        })
        .collect())
}

async fn statuses_in_tx(
    tx: &mut Transaction<'_, Postgres>,
    workspace_id: Uuid,
    project_ids: &[Uuid],
) -> Result<Vec<DashboardStatus>, sqlx::Error> {
    let rows = sqlx::query_as::<_, (Uuid, Uuid, Uuid, String, String, String, Option<i32>)>(
        r#"
        SELECT id, workflow_id, project_id, name, sort_key, category, wip_limit
        FROM fvoci.statuses
        WHERE workspace_id = $1 AND project_id = ANY($2)
        ORDER BY project_id, sort_key COLLATE "C", id
        "#,
    )
    .bind(workspace_id)
    .bind(project_ids)
    .fetch_all(&mut **tx)
    .await?;
    Ok(rows
        .into_iter()
        .map(
            |(id, workflow_id, project_id, name, sort_key, category, wip_limit)| DashboardStatus {
                id,
                workflow_id,
                project_id,
                name,
                sort_key,
                category,
                wip_limit,
            },
        )
        .collect())
}

async fn projects_in_tx(
    tx: &mut Transaction<'_, Postgres>,
    workspace_id: Uuid,
    project_ids: &[Uuid],
) -> Result<Vec<DashboardProject>, sqlx::Error> {
    let rows = sqlx::query_as::<_, (Uuid, String, String)>(
        r#"
        SELECT id, key, name FROM fvoci.projects
        WHERE workspace_id = $1 AND id = ANY($2) AND deleted_at IS NULL
        ORDER BY key COLLATE "C"
        "#,
    )
    .bind(workspace_id)
    .bind(project_ids)
    .fetch_all(&mut **tx)
    .await?;
    Ok(rows
        .into_iter()
        .map(|(id, key, name)| DashboardProject {
            id,
            workspace_id,
            key,
            name,
        })
        .collect())
}

async fn members_in_tx(
    tx: &mut Transaction<'_, Postgres>,
    workspace_id: Uuid,
    user_ids: &[Uuid],
) -> Result<Vec<MemberRow>, sqlx::Error> {
    if user_ids.is_empty() {
        return Ok(Vec::new());
    }
    let rows = sqlx::query_as::<_, (Uuid, String, String, Option<String>, String)>(
        r#"
        SELECT u.id, u.email, u.given_name, u.family_name, m.role
        FROM fvoci.memberships m
        INNER JOIN fvoci.users u ON u.id = m.user_id
        WHERE m.workspace_id = $1 AND m.user_id = ANY($2) AND u.deleted_at IS NULL
        ORDER BY m.created_at ASC, u.id ASC
        "#,
    )
    .bind(workspace_id)
    .bind(user_ids)
    .fetch_all(&mut **tx)
    .await?;
    Ok(rows
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
        .collect())
}

/// Source `compareAssignedDue`: due day ascending, missing due last, then id.
fn compare_assigned(
    a: &(TaskListItemRow, Option<NaiveDate>),
    b: &(TaskListItemRow, Option<NaiveDate>),
) -> Ordering {
    match (a.1, b.1) {
        (None, None) => a.0.meta.id.cmp(&b.0.meta.id),
        (None, Some(_)) => Ordering::Greater,
        (Some(_), None) => Ordering::Less,
        (Some(da), Some(db)) => da.cmp(&db).then_with(|| a.0.meta.id.cmp(&b.0.meta.id)),
    }
}

pub async fn build_dashboard(
    pool: &PgPool,
    user_id: Uuid,
    session_id: Uuid,
    last_visited: Option<Uuid>,
) -> Result<Result<Dashboard, DashboardError>, sqlx::Error> {
    let memberships = my_memberships(pool, user_id).await?;
    let time_zone = user_time_zone(pool, user_id).await?;
    let mut parts = Vec::with_capacity(memberships.len());
    for (workspace_id, _) in memberships {
        match collect_workspace(pool, workspace_id, user_id, session_id, &time_zone).await? {
            Ok(Some(part)) => parts.push(part),
            Ok(None) => {}
            Err(err) => return Ok(Err(err)),
        }
    }

    let mut recent: Vec<DashboardRecentItem> = Vec::new();
    let mut assigned_all = Vec::new();
    let mut labels = Vec::new();
    let mut statuses = Vec::new();
    let mut projects = Vec::new();
    let mut members_all: Vec<MemberRow> = Vec::new();
    let mut workspaces = Vec::new();
    let mut unread_total = 0i64;
    for part in parts {
        recent.extend(part.recent);
        assigned_all.extend(part.assigned);
        labels.extend(part.labels);
        statuses.extend(part.statuses);
        projects.extend(part.projects);
        members_all.extend(part.members);
        unread_total += part.workspace.unread_count;
        workspaces.push(part.workspace);
    }
    recent.sort_by(|a, b| {
        b.updated_at
            .cmp(&a.updated_at)
            .then_with(|| b.id.to_string().cmp(&a.id.to_string()))
    });
    recent.truncate(DASHBOARD_RECENT_LIMIT as usize);
    assigned_all.sort_by(compare_assigned);
    assigned_all.truncate(DASHBOARD_ASSIGNED_LIMIT as usize);
    let assigned: Vec<TaskListItemRow> = assigned_all.into_iter().map(|(row, _)| row).collect();
    let needed: HashSet<Uuid> = assigned
        .iter()
        .flat_map(|row| row.assignee_ids.iter().copied())
        .collect();
    let members = members_all
        .into_iter()
        .filter(|member| needed.contains(&member.user_id))
        .collect();
    if let Some(visited) = last_visited {
        if let Some(index) = workspaces.iter().position(|w| w.id == visited) {
            if index > 0 {
                let item = workspaces.remove(index);
                workspaces.insert(0, item);
            }
        }
    }
    Ok(Ok(Dashboard {
        assigned,
        recent,
        labels,
        statuses,
        projects,
        members,
        workspaces,
        unread_count: unread_total,
    }))
}

pub async fn build_dashboard_backend(
    backend: &super::backend::Backend,
    user_id: Uuid,
    session_id: Uuid,
    last_visited: Option<Uuid>,
) -> Result<Result<Dashboard, DashboardError>, sqlx::Error> {
    if let super::backend::Backend::Postgres(pool) = backend {
        return build_dashboard(pool, user_id, session_id, last_visited).await;
    }
    let memberships = dashboard_memberships_backend(backend, user_id).await?;
    let time_zone = user_time_zone_backend(backend, user_id).await?;
    let mut parts = Vec::with_capacity(memberships.len());
    for workspace in memberships {
        match collect_workspace_backend(backend, workspace, user_id, session_id, &time_zone).await?
        {
            Ok(Some(part)) => parts.push(part),
            Ok(None) => {}
            Err(e) => return Ok(Err(e)),
        }
    }
    let mut recent: Vec<DashboardRecentItem> = Vec::new();
    let mut assigned_all = Vec::new();
    let mut labels = Vec::new();
    let mut statuses = Vec::new();
    let mut projects = Vec::new();
    let mut members_all: Vec<MemberRow> = Vec::new();
    let mut workspaces = Vec::new();
    let mut unread_total = 0i64;
    for part in parts {
        recent.extend(part.recent);
        assigned_all.extend(part.assigned);
        labels.extend(part.labels);
        statuses.extend(part.statuses);
        projects.extend(part.projects);
        members_all.extend(part.members);
        unread_total += part.workspace.unread_count;
        workspaces.push(part.workspace);
    }
    recent.sort_by(|a, b| {
        b.updated_at
            .cmp(&a.updated_at)
            .then_with(|| b.id.to_string().cmp(&a.id.to_string()))
    });
    recent.truncate(DASHBOARD_RECENT_LIMIT as usize);
    assigned_all.sort_by(compare_assigned);
    assigned_all.truncate(DASHBOARD_ASSIGNED_LIMIT as usize);
    let assigned: Vec<TaskListItemRow> = assigned_all.into_iter().map(|(row, _)| row).collect();
    let needed: HashSet<Uuid> = assigned
        .iter()
        .flat_map(|row| row.assignee_ids.iter().copied())
        .collect();
    let members = members_all
        .into_iter()
        .filter(|member| needed.contains(&member.user_id))
        .collect();
    if let Some(visited) = last_visited {
        if let Some(index) = workspaces.iter().position(|w| w.id == visited) {
            if index > 0 {
                let item = workspaces.remove(index);
                workspaces.insert(0, item);
            }
        }
    }
    Ok(Ok(Dashboard {
        assigned,
        recent,
        labels,
        statuses,
        projects,
        members,
        workspaces,
        unread_count: unread_total,
    }))
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LocateKind {
    Task,
    Document,
}

/// Source `locateForUser`: the first membership workspace (in membership
/// order) where the target exists and the actor has at least `view`.
pub async fn locate_for_user(
    pool: &PgPool,
    user_id: Uuid,
    session_id: Uuid,
    kind: LocateKind,
    target_id: Uuid,
) -> Result<Result<Option<Uuid>, DashboardError>, sqlx::Error> {
    for (workspace_id, _) in my_memberships(pool, user_id).await? {
        let mut tx = pool.begin().await?;
        set_tenant(&mut tx, workspace_id).await?;
        if !session_is_live(&mut tx, user_id, session_id).await? {
            tx.rollback().await?;
            return Ok(Err(DashboardError::SessionGone));
        }
        if live_member_role(&mut tx, workspace_id, user_id)
            .await?
            .is_none()
        {
            tx.rollback().await?;
            continue;
        }
        let visible = match kind {
            LocateKind::Task => {
                let project: Option<(Uuid,)> = sqlx::query_as(
                    "SELECT project_id FROM fvoci.tasks WHERE workspace_id = $1 AND id = $2 AND deleted_at IS NULL",
                )
                .bind(workspace_id)
                .bind(target_id)
                .fetch_optional(&mut *tx)
                .await?;
                match project {
                    Some((project_id,)) => {
                        project_view(&mut tx, workspace_id, user_id, project_id).await?
                    }
                    None => false,
                }
            }
            LocateKind::Document => {
                let doc: Option<(Option<Uuid>,)> = sqlx::query_as(
                    "SELECT project_id FROM fvoci.documents WHERE workspace_id = $1 AND id = $2 AND deleted_at IS NULL",
                )
                .bind(workspace_id)
                .bind(target_id)
                .fetch_optional(&mut *tx)
                .await?;
                match doc {
                    Some((Some(project_id),)) => {
                        project_view(&mut tx, workspace_id, user_id, project_id).await?
                    }
                    Some((None,)) => {
                        document_permission(&mut tx, workspace_id, user_id, target_id, true)
                            .await?
                            .at_least(ProjectPermission::View)
                    }
                    None => false,
                }
            }
        };
        tx.commit().await?;
        if visible {
            return Ok(Ok(Some(workspace_id)));
        }
    }
    Ok(Ok(None))
}

pub async fn locate_for_user_backend(
    backend: &super::backend::Backend,
    user_id: Uuid,
    session_id: Uuid,
    kind: LocateKind,
    target_id: Uuid,
) -> Result<Result<Option<Uuid>, DashboardError>, sqlx::Error> {
    use super::backend::{Backend, OperationTx};
    use super::codec::Cell;
    if let Backend::Postgres(pool) = backend {
        return locate_for_user(pool, user_id, session_id, kind, target_id).await;
    }
    for workspace in dashboard_memberships_backend(backend, user_id).await? {
        let mut tx = backend.begin_read().await?;
        let mut op = tx.operation();
        op.set_tenant(workspace).await?;
        if !op.session_is_live(user_id, session_id).await? {
            tx.rollback().await?;
            return Ok(Err(DashboardError::SessionGone));
        }
        if !op.workspace_is_live(workspace).await?
            || op
                .membership_role(workspace, user_id, false)
                .await?
                .is_none()
        {
            tx.rollback().await?;
            continue;
        }
        let OperationTx::SqliteFamily(family) = &mut op else {
            unreachable!("family locate")
        };
        let rows=match kind{LocateKind::Task=>family.query("SELECT project_id FROM tasks WHERE workspace_id=?1 AND id=?2 AND deleted_at IS NULL",&[Cell::uuid(workspace),Cell::uuid(target_id)]).await?,LocateKind::Document=>family.query("SELECT project_id FROM documents WHERE workspace_id=?1 AND id=?2 AND deleted_at IS NULL",&[Cell::uuid(workspace),Cell::uuid(target_id)]).await?};
        let visible = match rows.first() {
            None => false,
            Some(row) => match row.cell(0)?.optional(Cell::id)? {
                Some(project) => op
                    .project_permission_by_id(workspace, user_id, project)
                    .await?
                    .is_some_and(|p| p.at_least(ProjectPermission::View)),
                None => op
                    .document_permission(workspace, user_id, target_id, true)
                    .await?
                    .at_least(ProjectPermission::View),
            },
        };
        tx.rollback().await?;
        if visible {
            return Ok(Ok(Some(workspace)));
        }
    }
    Ok(Ok(None))
}

async fn project_view(
    tx: &mut Transaction<'_, Postgres>,
    workspace_id: Uuid,
    user_id: Uuid,
    project_id: Uuid,
) -> Result<bool, sqlx::Error> {
    let Some(project) = load_live_project(tx, workspace_id, project_id).await? else {
        return Ok(false);
    };
    Ok(project_permission(tx, workspace_id, user_id, &project)
        .await?
        .at_least(ProjectPermission::View))
}

async fn dashboard_memberships_backend(
    backend: &super::backend::Backend,
    user: Uuid,
) -> Result<Vec<Uuid>, sqlx::Error> {
    super::user_export::export_membership_workspaces_backend(backend, user).await
}

async fn collect_workspace_backend(
    backend: &super::backend::Backend,
    workspace: Uuid,
    user: Uuid,
    session: Uuid,
    time_zone: &str,
) -> Result<Result<Option<WorkspacePart>, DashboardError>, sqlx::Error> {
    use super::backend::OperationTx;
    use super::codec::Cell;
    let mut tx = backend.begin_read().await?;
    let mut op = tx.operation();
    op.set_tenant(workspace).await?;
    if !op.session_is_live(user, session).await? {
        tx.rollback().await?;
        return Ok(Err(DashboardError::SessionGone));
    }
    if !op.workspace_is_live(workspace).await? {
        tx.rollback().await?;
        return Ok(Ok(None));
    }
    let Some(role) = op.membership_role(workspace, user, false).await? else {
        tx.rollback().await?;
        return Ok(Ok(None));
    };
    let OperationTx::SqliteFamily(family) = &mut op else {
        unreachable!("family dashboard")
    };
    let workspace_rows = family
        .query(
            "SELECT name,slug,kind FROM workspaces WHERE id=?1",
            &[Cell::uuid(workspace)],
        )
        .await?;
    let row = workspace_rows.first().ok_or(sqlx::Error::RowNotFound)?;
    let name = row.cell(0)?.string()?;
    let slug = row.cell(1)?.string()?;
    let kind = row.cell(2)?.string()?;
    let candidates=family.query("SELECT id,key,name FROM projects WHERE workspace_id=?1 AND deleted_at IS NULL ORDER BY key,id",&[Cell::uuid(workspace)]).await?;
    let mut projects = Vec::new();
    let mut project_ids = Vec::new();
    for row in candidates {
        let project = row.cell(0)?.id()?;
        if op
            .project_permission_by_id(workspace, user, project)
            .await?
            .is_some_and(|p| p.at_least(ProjectPermission::View))
        {
            project_ids.push(project);
            projects.push(DashboardProject {
                id: project,
                workspace_id: workspace,
                key: row.cell(1)?.string()?,
                name: row.cell(2)?.string()?,
            });
        }
    }
    let mut wiki_ids = Vec::new();
    if role == WorkspaceRole::Guest {
        let OperationTx::SqliteFamily(family) = &mut op else {
            unreachable!("family wiki candidates")
        };
        let docs=family.query("SELECT id FROM documents WHERE workspace_id=?1 AND project_id IS NULL AND deleted_at IS NULL",&[Cell::uuid(workspace)]).await?;
        for row in docs {
            let id = row.cell(0)?.id()?;
            if op
                .document_permission(workspace, user, id, true)
                .await?
                .at_least(ProjectPermission::View)
            {
                wiki_ids.push(id);
            }
        }
    }
    let projects_arg = Cell::json(&serde_json::json!(project_ids
        .iter()
        .map(|id| hex::encode_upper(id.as_bytes()))
        .collect::<Vec<_>>()))?;
    let wiki_arg = Cell::json(&serde_json::json!(wiki_ids
        .iter()
        .map(|id| hex::encode_upper(id.as_bytes()))
        .collect::<Vec<_>>()))?;
    let OperationTx::SqliteFamily(family) = &mut op else {
        unreachable!("family dashboard rows")
    };
    let recent_rows=family.query("SELECT kind,id,title,project_id,number,ua FROM (SELECT 'document' AS kind,d.id,d.title,d.project_id,d.number,d.updated_at-((d.updated_at%1000+1000)%1000) AS ua FROM documents d WHERE d.workspace_id=?1 AND d.deleted_at IS NULL AND (hex(d.project_id) IN (SELECT value FROM json_each(?2)) OR (d.project_id IS NULL AND (?3=1 OR hex(d.id) IN (SELECT value FROM json_each(?4))))) UNION ALL SELECT 'task',t.id,t.title,t.project_id,t.number,t.updated_at-((t.updated_at%1000+1000)%1000) FROM tasks t WHERE t.workspace_id=?1 AND t.deleted_at IS NULL AND t.archived_at IS NULL AND hex(t.project_id) IN (SELECT value FROM json_each(?2))) ORDER BY ua DESC,id DESC LIMIT ?5",&[Cell::uuid(workspace),projects_arg.clone(),Cell::Integer(i64::from(role!=WorkspaceRole::Guest)),wiki_arg,Cell::Integer(DASHBOARD_RECENT_LIMIT)]).await?;
    let recent = recent_rows
        .iter()
        .map(|r| {
            Ok::<_, sqlx::Error>(DashboardRecentItem {
                kind: if r.cell(0)?.string()? == "task" {
                    "task"
                } else {
                    "document"
                },
                id: r.cell(1)?.id()?,
                title: r.cell(2)?.string()?,
                project_id: r.cell(3)?.optional(Cell::id)?,
                number: r.cell(4)?.int32()?,
                updated_at: r.cell(5)?.datetime()?,
                workspace_id: workspace,
            })
        })
        .collect::<Result<_, _>>()?;
    let label_rows=family.query("SELECT id,project_id,name,color FROM labels WHERE workspace_id=?1 AND hex(project_id) IN (SELECT value FROM json_each(?2)) ORDER BY project_id,name,id",&[Cell::uuid(workspace),projects_arg.clone()]).await?;
    let labels = label_rows
        .iter()
        .map(|r| {
            Ok::<_, sqlx::Error>(DashboardLabel {
                id: r.cell(0)?.id()?,
                project_id: r.cell(1)?.id()?,
                name: r.cell(2)?.string()?,
                color: r.cell(3)?.string()?,
            })
        })
        .collect::<Result<_, _>>()?;
    let status_rows=family.query("SELECT id,workflow_id,project_id,name,sort_key,category,wip_limit FROM statuses WHERE workspace_id=?1 AND hex(project_id) IN (SELECT value FROM json_each(?2)) ORDER BY project_id,sort_key,id",&[Cell::uuid(workspace),projects_arg.clone()]).await?;
    let statuses = status_rows
        .iter()
        .map(|r| {
            Ok::<_, sqlx::Error>(DashboardStatus {
                id: r.cell(0)?.id()?,
                workflow_id: r.cell(1)?.id()?,
                project_id: r.cell(2)?.id()?,
                name: r.cell(3)?.string()?,
                sort_key: r.cell(4)?.string()?,
                category: r.cell(5)?.string()?,
                wip_limit: r.cell(6)?.optional(Cell::int32)?,
            })
        })
        .collect::<Result<_, _>>()?;
    // The PostgreSQL path first limits by UTC due day within each workspace,
    // then the dashboard merges those pages using the actor's local due day.
    let task_rows=family.query("SELECT t.id,t.project_id,t.number,t.title,t.type,t.priority,t.status_id,t.start_date,t.due_date,t.due_at,t.estimate,t.parent_id,t.milestone_id,t.sort_key,t.schema_version,t.version,t.archived_at,t.created_by,t.created_at,t.updated_at,t.recurrence FROM tasks t WHERE t.workspace_id=?1 AND t.deleted_at IS NULL AND t.archived_at IS NULL AND hex(t.project_id) IN (SELECT value FROM json_each(?2)) AND EXISTS(SELECT 1 FROM task_assignees a WHERE a.workspace_id=t.workspace_id AND a.task_id=t.id AND a.user_id=?3) AND EXISTS(SELECT 1 FROM statuses s WHERE s.workspace_id=t.workspace_id AND s.project_id=t.project_id AND s.id=t.status_id AND s.category NOT IN ('done','canceled'))",&[Cell::uuid(workspace),projects_arg.clone(),Cell::uuid(user)]).await?;
    let assigned_count = i32::try_from(task_rows.len()).unwrap_or(i32::MAX);
    let mut metas = task_rows
        .iter()
        .map(|r| {
            let record = super::tasks::TaskRowRecord {
                id: r.cell(0)?.id()?,
                project_id: r.cell(1)?.id()?,
                number: r.cell(2)?.int32()?,
                title: r.cell(3)?.string()?,
                task_type: r.cell(4)?.string()?,
                priority: r.cell(5)?.string()?,
                status_id: r.cell(6)?.id()?,
                start_date: r.cell(7)?.optional(Cell::date)?,
                due_date: r.cell(8)?.optional(Cell::date)?,
                due_at: r.cell(9)?.optional(Cell::datetime)?,
                estimate: r.cell(10)?.optional(Cell::string)?,
                parent_id: r.cell(11)?.optional(Cell::id)?,
                milestone_id: r.cell(12)?.optional(Cell::id)?,
                sort_key: r.cell(13)?.string()?,
                schema_version: r.cell(14)?.int32()?,
                version: r.cell(15)?.int32()?,
                archived_at: r.cell(16)?.optional(Cell::datetime)?,
                created_by: r.cell(17)?.id()?,
                created_at: r.cell(18)?.datetime()?,
                updated_at: r.cell(19)?.datetime()?,
            };
            Ok::<_, sqlx::Error>(super::tasks::row_to_meta(
                workspace,
                record,
                r.cell(20)?.optional(Cell::value)?,
            ))
        })
        .collect::<Result<Vec<_>, _>>()?;
    metas.sort_by(|a, b| {
        let a_due = a.due_date.or(a.due_at.map(|at| at.date_naive()));
        let b_due = b.due_date.or(b.due_at.map(|at| at.date_naive()));
        match (a_due, b_due) {
            (None, None) => a.id.cmp(&b.id),
            (None, Some(_)) => Ordering::Greater,
            (Some(_), None) => Ordering::Less,
            (Some(a_day), Some(b_day)) => a_day.cmp(&b_day).then_with(|| a.id.cmp(&b.id)),
        }
    });
    metas.truncate(DASHBOARD_ASSIGNED_LIMIT as usize);
    let mut assigned = Vec::with_capacity(metas.len());
    let mut needed = HashSet::new();
    for meta in metas {
        let due_key = match (meta.due_date, meta.due_at) {
            (Some(day), _) => Some(day),
            (None, Some(at)) => Some(dashboard_local_day(at, time_zone)?),
            _ => None,
        };
        let assignee_ids=family.query("SELECT user_id FROM task_assignees WHERE workspace_id=?1 AND task_id=?2 ORDER BY user_id",&[Cell::uuid(workspace),Cell::uuid(meta.id)]).await?.iter().map(|r|r.cell(0)?.id()).collect::<Result<Vec<_>,_>>()?;
        needed.extend(assignee_ids.iter().copied());
        let label_ids=family.query("SELECT label_id FROM task_labels WHERE workspace_id=?1 AND task_id=?2 ORDER BY label_id",&[Cell::uuid(workspace),Cell::uuid(meta.id)]).await?.iter().map(|r|r.cell(0)?.id()).collect::<Result<Vec<_>,_>>()?;
        assigned.push((
            TaskListItemRow {
                meta,
                assignee_ids,
                label_ids,
            },
            due_key,
        ));
    }
    let needed_arg = Cell::json(&serde_json::json!(needed
        .iter()
        .map(|id| hex::encode_upper(id.as_bytes()))
        .collect::<Vec<_>>()))?;
    let member_rows=family.query("SELECT u.id,u.email,u.given_name,u.family_name,m.role FROM memberships m JOIN users u ON u.id=m.user_id WHERE m.workspace_id=?1 AND hex(m.user_id) IN (SELECT value FROM json_each(?2)) AND u.deleted_at IS NULL ORDER BY m.created_at,u.id",&[Cell::uuid(workspace),needed_arg]).await?;
    let mut members = Vec::new();
    for r in member_rows {
        if let Some(role) = WorkspaceRole::parse(&r.cell(4)?.string()?) {
            members.push(MemberRow {
                user_id: r.cell(0)?.id()?,
                email: r.cell(1)?.string()?,
                given_name: r.cell(2)?.string()?,
                family_name: r.cell(3)?.optional(Cell::string)?,
                role,
            });
        }
    }
    let counts=family.query("SELECT count(*) FROM documents WHERE workspace_id=?1 AND deleted_at IS NULL AND ((project_id IS NULL AND ?3=1) OR hex(project_id) IN (SELECT value FROM json_each(?2)))",&[Cell::uuid(workspace),projects_arg,Cell::Integer(i64::from(role!=WorkspaceRole::Guest))]).await?;
    let document_count = i32::try_from(
        counts
            .first()
            .ok_or(sqlx::Error::RowNotFound)?
            .cell(0)?
            .integer()?,
    )
    .unwrap_or(i32::MAX);
    tx.rollback().await?;
    let unread_count = if role == WorkspaceRole::Guest {
        0
    } else {
        super::notifications::unread_count_backend(backend, workspace, user, session, None)
            .await?
            .unwrap_or_default()
    };
    Ok(Ok(Some(WorkspacePart {
        workspace: DashboardWorkspace {
            id: workspace,
            name,
            slug,
            role,
            kind,
            document_count,
            assigned_count,
            unread_count,
        },
        assigned,
        recent,
        labels,
        statuses,
        projects,
        members,
    })))
}

fn dashboard_local_day(at: DateTime<Utc>, name: &str) -> Result<NaiveDate, sqlx::Error> {
    let (zone_name, bytes) = jiff_tzdb::get(name)
        .ok_or_else(|| sqlx::Error::Protocol("dashboard timezone is unknown".into()))?;
    let zone =
        jiff::tz::TimeZone::tzif(zone_name, bytes).map_err(|e| sqlx::Error::Decode(Box::new(e)))?;
    let timestamp = jiff::Timestamp::from_microsecond(at.timestamp_micros())
        .map_err(|e| sqlx::Error::Decode(Box::new(e)))?;
    NaiveDate::parse_from_str(&timestamp.to_zoned(zone).date().to_string(), "%Y-%m-%d")
        .map_err(|e| sqlx::Error::Decode(Box::new(e)))
}

#[cfg(test)]
mod backend_timezone_tests {
    use super::*;

    #[tokio::test]
    async fn sqlite_backend_reads_zone_and_falls_back_for_unknown_users_and_zones() {
        let root = std::env::temp_dir().join(format!("fvoci-timezone-{}", Uuid::now_v7()));
        std::fs::create_dir(&root).unwrap();
        let path = root.join("timezone.sqlite");
        crate::db::migrate::run_sqlite_migrations(&path)
            .await
            .unwrap();
        let pool = crate::db::pool::connect_sqlite_app(&path, 1).await.unwrap();
        let backend = super::super::backend::Backend::Sqlite(pool.clone());
        let user = Uuid::now_v7();
        assert_eq!(user_time_zone_backend(&backend, user).await.unwrap(), "UTC");
        sqlx::query("INSERT INTO users (id,email,given_name,timezone) VALUES (?1,'timezone@example.invalid','Synthetic','Asia/Seoul')")
            .bind(user.as_bytes().as_slice()).execute(&pool).await.unwrap();
        let fresh = crate::db::pool::connect_sqlite_app(&path, 1).await.unwrap();
        let fresh_backend = super::super::backend::Backend::Sqlite(fresh.clone());
        assert_eq!(
            user_time_zone_backend(&fresh_backend, user).await.unwrap(),
            "Asia/Seoul"
        );
        sqlx::query("UPDATE users SET timezone='Unknown/Zone' WHERE id=?1")
            .bind(user.as_bytes().as_slice())
            .execute(&pool)
            .await
            .unwrap();
        assert_eq!(
            user_time_zone_backend(&fresh_backend, user).await.unwrap(),
            "UTC"
        );
        fresh.close().await;
        pool.close().await;
        std::fs::remove_dir_all(root).unwrap();
    }
}
