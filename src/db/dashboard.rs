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
use crate::db::projects::project_permission;
use crate::db::tasks::{list_open_assigned_in_tx, TaskListItemRow};
use crate::db::workspace::{workspace_card_counts_in_tx, MemberRow, WorkspaceRole};
use crate::projects::ProjectPermission;
use crate::search::query::{load_live_project, load_search_acl, SearchAcl};

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

async fn user_time_zone(pool: &PgPool, user_id: Uuid) -> Result<String, sqlx::Error> {
    // Source falls back to UTC; an unknown zone name must not fail the query.
    let tz: Option<String> = sqlx::query_scalar(
        r#"
        SELECT tz.name FROM fvoci.users u
        INNER JOIN pg_catalog.pg_timezone_names tz ON tz.name = u.timezone
        WHERE u.id = $1
        "#,
    )
    .bind(user_id)
    .fetch_optional(pool)
    .await?;
    Ok(tz.unwrap_or_else(|| "UTC".to_string()))
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
