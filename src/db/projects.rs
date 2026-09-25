#![allow(clippy::too_many_arguments)]

use chrono::{DateTime, Utc};
use serde_json::{json, Value};
use sqlx::{PgPool, Postgres, Transaction};
use uuid::Uuid;

use crate::db::context::{
    lock_membership_users, lock_tree, recheck_session, session_is_live, set_tenant,
};
use crate::db::documents::{empty_document_json, to_path_label, DOCUMENT_SCHEMA_VERSION};
use crate::db::identity::{append_audit, append_event, AuditAppend, EventAppend};
use crate::db::workspace::WorkspaceRole;
use crate::projects::{
    effective_permission, optional_text_to_db, ProjectMemberRole, ProjectPermission,
};

const PRIVATE_LEAD_SQLSTATE: &str = "23514";

#[derive(Debug)]
pub enum ProjectDbError {
    NotFound,
    Forbidden,
    Conflict,
    LastLead,
    GuestLead,
    LeadNotMember,
    Archived,
    InvalidCursor,
    VersionConflict,
    TaskArchived,
    InvalidAnchor,
    StatusNotInWorkflow,
    WipLimitExceeded,
    InvalidMoveAnchors,
    WorkflowHasNoStatuses,
    AssigneeIsNotAMember,
    LabelNotFound,
    MilestoneNotFound,
    DependencyNotFound,
    DependencyCycle,
    DependencyContradiction,
    TaskCannotBlockItself,
    InvalidInput,
}

#[derive(Debug, Clone)]
pub struct ProjectRow {
    pub id: Uuid,
    pub workspace_id: Uuid,
    pub key: String,
    pub name: String,
    pub description: Option<String>,
    pub icon: Option<String>,
    pub visibility: String,
    pub root_document_id: Option<Uuid>,
    pub status: String,
    pub created_by: Uuid,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Clone)]
pub struct ProjectListItem {
    pub project: ProjectRow,
    pub task_count: i64,
    pub open_task_count: i64,
}

#[derive(Debug, Clone)]
pub struct ProjectMemberRow {
    pub user_id: Uuid,
    pub email: String,
    pub given_name: String,
    pub family_name: Option<String>,
    pub role: ProjectMemberRole,
}

#[derive(Debug, Clone)]
pub struct WorkflowStatusRow {
    pub id: Uuid,
    pub name: String,
    pub category: String,
    pub sort_key: String,
    pub wip_limit: Option<i32>,
}

#[derive(Debug, Clone)]
pub struct WorkflowRow {
    pub id: Uuid,
    pub project_id: Uuid,
    pub statuses: Vec<WorkflowStatusRow>,
}

pub struct CreateProjectInput<'a> {
    pub key: &'a str,
    pub name: &'a str,
    pub visibility: &'a str,
    pub description: Option<&'a str>,
    pub icon: Option<&'a str>,
    pub lead_user_id: Option<Uuid>,
}

pub struct UpdateProjectInput<'a> {
    pub name: Option<&'a str>,
    pub visibility: Option<&'a str>,
    pub description: Option<Option<&'a str>>,
    pub icon: Option<Option<&'a str>>,
    pub lead_user_id: Option<Uuid>,
}

struct ProjectChangeRecord<'a> {
    workspace_id: Uuid,
    actor_user_id: Uuid,
    verb: &'a str,
    target_type: &'a str,
    target_id: Uuid,
    payload: Value,
    client_ip: Option<&'a str>,
}

pub(crate) struct LockedProject {
    pub id: Uuid,
    pub key: String,
    pub name: String,
    pub description: Option<String>,
    pub icon: Option<String>,
    pub visibility: String,
    pub root_document_id: Option<Uuid>,
    pub status: String,
    pub created_by: Uuid,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
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

async fn membership_role_for_update(
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

async fn workspace_is_live(
    tx: &mut Transaction<'_, Postgres>,
    workspace_id: Uuid,
) -> Result<bool, sqlx::Error> {
    let row: Option<(Option<DateTime<Utc>>,)> =
        sqlx::query_as("SELECT deleted_at FROM fvoci.workspaces WHERE id = $1")
            .bind(workspace_id)
            .fetch_optional(&mut **tx)
            .await?;
    Ok(row.map(|(deleted,)| deleted.is_none()).unwrap_or(false))
}

pub(crate) async fn project_member_role(
    tx: &mut Transaction<'_, Postgres>,
    workspace_id: Uuid,
    project_id: Uuid,
    user_id: Uuid,
) -> Result<Option<ProjectMemberRole>, sqlx::Error> {
    let rows = sqlx::query_as::<_, (String,)>(
        r#"
        SELECT role FROM fvoci.project_members
        WHERE workspace_id = $1 AND project_id = $2 AND user_id = $3
        UNION ALL
        SELECT pm.role
        FROM fvoci.project_members pm
        INNER JOIN fvoci.group_members gm
            ON gm.workspace_id = pm.workspace_id AND gm.group_id = pm.group_id
        WHERE pm.workspace_id = $1
          AND pm.project_id = $2
          AND gm.user_id = $3
          AND pm.group_id IS NOT NULL
        "#,
    )
    .bind(workspace_id)
    .bind(project_id)
    .bind(user_id)
    .fetch_all(&mut **tx)
    .await?;
    Ok(rows
        .into_iter()
        .filter_map(|(role,)| ProjectMemberRole::parse(&role))
        .max_by_key(|role| role.permission()))
}

async fn direct_project_member_role(
    tx: &mut Transaction<'_, Postgres>,
    workspace_id: Uuid,
    project_id: Uuid,
    user_id: Uuid,
) -> Result<Option<ProjectMemberRole>, sqlx::Error> {
    let row: Option<(String,)> = sqlx::query_as(
        r#"
        SELECT role FROM fvoci.project_members
        WHERE workspace_id = $1 AND project_id = $2 AND user_id = $3
        "#,
    )
    .bind(workspace_id)
    .bind(project_id)
    .bind(user_id)
    .fetch_optional(&mut **tx)
    .await?;
    Ok(row.and_then(|(role,)| ProjectMemberRole::parse(&role)))
}

/// Visibility predicate matching `project_permission`: workspace-visible to
/// non-guests, otherwise a direct user row or a group grant for the actor.
pub(crate) fn visible_project_sql(
    project_alias: &str,
    guest_param: u32,
    actor_param: u32,
) -> String {
    format!(
        "(
            ({project_alias}.visibility = 'workspace' AND ${guest_param} = false)
            OR EXISTS (
                SELECT 1 FROM fvoci.project_members pm
                WHERE pm.workspace_id = {project_alias}.workspace_id
                  AND pm.project_id = {project_alias}.id
                  AND pm.user_id = ${actor_param}
            )
            OR EXISTS (
                SELECT 1
                FROM fvoci.project_members pm
                INNER JOIN fvoci.group_members gm
                    ON gm.workspace_id = pm.workspace_id AND gm.group_id = pm.group_id
                WHERE pm.workspace_id = {project_alias}.workspace_id
                  AND pm.project_id = {project_alias}.id
                  AND gm.user_id = ${actor_param}
                  AND pm.group_id IS NOT NULL
            )
        )"
    )
}

async fn count_project_leads(
    tx: &mut Transaction<'_, Postgres>,
    workspace_id: Uuid,
    project_id: Uuid,
) -> Result<i64, sqlx::Error> {
    let row: (i64,) = sqlx::query_as(
        r#"
        SELECT count(*) FROM fvoci.project_members
        WHERE workspace_id = $1 AND project_id = $2 AND role = 'lead'
        "#,
    )
    .bind(workspace_id)
    .bind(project_id)
    .fetch_one(&mut **tx)
    .await?;
    Ok(row.0)
}

pub(crate) async fn count_project_leads_except(
    tx: &mut Transaction<'_, Postgres>,
    workspace_id: Uuid,
    project_id: Uuid,
    except_user_id: Option<Uuid>,
    except_group_id: Option<Uuid>,
) -> Result<i64, sqlx::Error> {
    let row: (i64,) = sqlx::query_as(
        r#"
        SELECT count(*) FROM fvoci.project_members
        WHERE workspace_id = $1 AND project_id = $2 AND role = 'lead'
          AND ($3::uuid IS NULL OR user_id IS DISTINCT FROM $3)
          AND ($4::uuid IS NULL OR group_id IS DISTINCT FROM $4)
        "#,
    )
    .bind(workspace_id)
    .bind(project_id)
    .bind(except_user_id)
    .bind(except_group_id)
    .fetch_one(&mut **tx)
    .await?;
    Ok(row.0)
}

async fn count_project_leads_excluding(
    tx: &mut Transaction<'_, Postgres>,
    workspace_id: Uuid,
    project_id: Uuid,
    exclude_user_id: Uuid,
) -> Result<i64, sqlx::Error> {
    count_project_leads_except(tx, workspace_id, project_id, Some(exclude_user_id), None).await
}

pub(crate) async fn lock_project(
    tx: &mut Transaction<'_, Postgres>,
    workspace_id: Uuid,
    project_id: Uuid,
) -> Result<Option<LockedProject>, sqlx::Error> {
    let row = sqlx::query_as::<
        _,
        (
            Uuid,
            String,
            String,
            Option<String>,
            Option<String>,
            String,
            Option<Uuid>,
            String,
            Uuid,
            DateTime<Utc>,
            DateTime<Utc>,
        ),
    >(
        r#"
        SELECT id, key, name, description, icon, visibility, root_document_id, status,
               created_by, created_at, updated_at
        FROM fvoci.projects
        WHERE workspace_id = $1 AND id = $2 AND deleted_at IS NULL
        FOR NO KEY UPDATE
        "#,
    )
    .bind(workspace_id)
    .bind(project_id)
    .fetch_optional(&mut **tx)
    .await?;
    Ok(row.map(
        |(
            id,
            key,
            name,
            description,
            icon,
            visibility,
            root_document_id,
            status,
            created_by,
            created_at,
            updated_at,
        )| LockedProject {
            id,
            key,
            name,
            description,
            icon,
            visibility,
            root_document_id,
            status,
            created_by,
            created_at,
            updated_at,
        },
    ))
}

pub(crate) async fn project_permission(
    tx: &mut Transaction<'_, Postgres>,
    workspace_id: Uuid,
    actor_user_id: Uuid,
    project: &LockedProject,
) -> Result<ProjectPermission, sqlx::Error> {
    let workspace_role = membership_role(tx, workspace_id, actor_user_id)
        .await?
        .unwrap_or(WorkspaceRole::Guest);
    let member_role = project_member_role(tx, workspace_id, project.id, actor_user_id).await?;
    Ok(effective_permission(
        workspace_role,
        &project.visibility,
        member_role,
    ))
}

async fn record_project_event_and_audit(
    tx: &mut Transaction<'_, Postgres>,
    change: ProjectChangeRecord<'_>,
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

pub(crate) fn is_private_lead_violation(err: &sqlx::Error) -> bool {
    err.as_database_error()
        .and_then(|db| db.code())
        .is_some_and(|code| code.as_ref() == PRIVATE_LEAD_SQLSTATE)
}

async fn seed_workflow(
    tx: &mut Transaction<'_, Postgres>,
    workspace_id: Uuid,
    project_id: Uuid,
) -> Result<Uuid, sqlx::Error> {
    let workflow_id = Uuid::now_v7();
    sqlx::query(
        r#"
        INSERT INTO fvoci.workflows (id, workspace_id, project_id)
        VALUES ($1, $2, $3)
        "#,
    )
    .bind(workflow_id)
    .bind(workspace_id)
    .bind(project_id)
    .execute(&mut **tx)
    .await?;

    let seeds: [(&str, &str, &str); 6] = [
        ("백로그", "backlog", "V"),
        ("할 일", "todo", "W"),
        ("진행 중", "in_progress", "X"),
        ("검토 대기", "in_progress", "Y"),
        ("완료", "done", "Z"),
        ("취소", "canceled", "a"),
    ];
    for (name, category, sort_key) in seeds {
        sqlx::query(
            r#"
            INSERT INTO fvoci.statuses (id, workspace_id, project_id, workflow_id, name, category, sort_key)
            VALUES ($1, $2, $3, $4, $5, $6, $7)
            "#,
        )
        .bind(Uuid::now_v7())
        .bind(workspace_id)
        .bind(project_id)
        .bind(workflow_id)
        .bind(name)
        .bind(category)
        .bind(sort_key)
        .execute(&mut **tx)
        .await?;
    }
    Ok(workflow_id)
}

pub(crate) async fn workspace_removal_blocked_by_private_leads(
    tx: &mut Transaction<'_, Postgres>,
    workspace_id: Uuid,
    target_user_id: Uuid,
) -> Result<bool, sqlx::Error> {
    let projects = sqlx::query_as::<_, (Uuid, String)>(
        r#"
        SELECT p.id, p.visibility
        FROM fvoci.projects p
        INNER JOIN fvoci.project_members pm
            ON pm.workspace_id = p.workspace_id AND pm.project_id = p.id
        WHERE p.workspace_id = $1
          AND pm.user_id = $2
          AND pm.role = 'lead'
          AND p.deleted_at IS NULL
        ORDER BY p.id
        FOR NO KEY UPDATE OF p
        "#,
    )
    .bind(workspace_id)
    .bind(target_user_id)
    .fetch_all(&mut **tx)
    .await?;
    for (project_id, visibility) in projects {
        if visibility == "private"
            && count_project_leads_excluding(tx, workspace_id, project_id, target_user_id).await?
                == 0
        {
            return Ok(true);
        }
    }
    Ok(false)
}

pub async fn create_project(
    pool: &PgPool,
    workspace_id: Uuid,
    actor_user_id: Uuid,
    session_id: Uuid,
    input: CreateProjectInput<'_>,
    client_ip: Option<&str>,
) -> Result<Result<ProjectRow, ProjectDbError>, sqlx::Error> {
    let project_id = Uuid::now_v7();
    let root_document_id = Uuid::now_v7();
    let mut lock_users = vec![actor_user_id];
    if let Some(lead) = input.lead_user_id {
        if lead != actor_user_id {
            lock_users.push(lead);
        }
    }

    let mut tx = pool.begin().await?;
    set_tenant(&mut tx, workspace_id).await?;
    lock_membership_users(&mut tx, &lock_users).await?;
    if !recheck_session(&mut tx, actor_user_id, session_id).await? {
        tx.rollback().await?;
        return Ok(Err(ProjectDbError::Forbidden));
    }
    if !workspace_is_live(&mut tx, workspace_id).await? {
        tx.rollback().await?;
        return Ok(Err(ProjectDbError::NotFound));
    }
    let actor_role = membership_role_for_update(&mut tx, workspace_id, actor_user_id).await?;
    if !actor_role
        .map(|r| r.at_least(WorkspaceRole::Member))
        .unwrap_or(false)
    {
        tx.rollback().await?;
        return Ok(Err(ProjectDbError::NotFound));
    }
    lock_tree(&mut tx, workspace_id).await?;

    let inserted = sqlx::query_as::<_, (Uuid,)>(
        r#"
        INSERT INTO fvoci.projects (
            id, workspace_id, key, name, description, icon, visibility, status,
            next_number, created_by
        ) VALUES ($1, $2, $3, $4, $5, $6, $7, 'active', 1, $8)
        RETURNING id
        "#,
    )
    .bind(project_id)
    .bind(workspace_id)
    .bind(input.key)
    .bind(input.name.trim())
    .bind(optional_text_to_db(input.description))
    .bind(optional_text_to_db(input.icon))
    .bind(input.visibility)
    .bind(actor_user_id)
    .fetch_optional(&mut *tx)
    .await;

    if let Err(err) = inserted {
        if let Some(db_err) = err.as_database_error() {
            if db_err.constraint() == Some("projects_workspace_id_key_unique") {
                tx.rollback().await?;
                return Ok(Err(ProjectDbError::Conflict));
            }
        }
        return Err(err);
    }
    inserted?;

    sqlx::query(
        r#"
        INSERT INTO fvoci.project_members (id, workspace_id, project_id, user_id, role)
        VALUES ($1, $2, $3, $4, 'lead')
        "#,
    )
    .bind(Uuid::now_v7())
    .bind(workspace_id)
    .bind(project_id)
    .bind(actor_user_id)
    .execute(&mut *tx)
    .await?;

    if let Some(lead_user_id) = input.lead_user_id {
        if lead_user_id != actor_user_id {
            let lead_role = membership_role(&mut tx, workspace_id, lead_user_id).await?;
            let lead_role = match lead_role {
                Some(r) if r.at_least(WorkspaceRole::Member) => r,
                _ => {
                    tx.rollback().await?;
                    return Ok(Err(ProjectDbError::NotFound));
                }
            };
            let _ = lead_role;
            sqlx::query(
                r#"
                INSERT INTO fvoci.project_members (id, workspace_id, project_id, user_id, role)
                VALUES ($1, $2, $3, $4, 'lead')
                ON CONFLICT (workspace_id, project_id, user_id) DO UPDATE SET role = 'lead', updated_at = now()
                "#,
            )
            .bind(Uuid::now_v7())
            .bind(workspace_id)
            .bind(project_id)
            .bind(lead_user_id)
            .execute(&mut *tx)
            .await?;
            sqlx::query(
                r#"
                UPDATE fvoci.project_members
                SET role = 'member', updated_at = now()
                WHERE workspace_id = $1 AND project_id = $2 AND user_id = $3
                "#,
            )
            .bind(workspace_id)
            .bind(project_id)
            .bind(actor_user_id)
            .execute(&mut *tx)
            .await?;
        }
    }

    let doc_number: (i32,) = sqlx::query_as(
        r#"
        UPDATE fvoci.projects
        SET next_number = next_number + 1, updated_at = now()
        WHERE workspace_id = $1 AND id = $2
        RETURNING next_number - 1
        "#,
    )
    .bind(workspace_id)
    .bind(project_id)
    .fetch_one(&mut *tx)
    .await?;

    sqlx::query(
        r#"
        INSERT INTO fvoci.documents (
            id, workspace_id, title, path, parent_id, sort_key, project_id, number, status,
            schema_version, content_json, created_by
        ) VALUES ($1, $2, $3, $4, NULL, 'V', $5, $6, 'published', $7, $8, $9)
        "#,
    )
    .bind(root_document_id)
    .bind(workspace_id)
    .bind(input.name.trim())
    .bind(to_path_label(root_document_id))
    .bind(project_id)
    .bind(doc_number.0)
    .bind(DOCUMENT_SCHEMA_VERSION)
    .bind(empty_document_json())
    .bind(actor_user_id)
    .execute(&mut *tx)
    .await?;

    sqlx::query(
        r#"
        UPDATE fvoci.projects
        SET root_document_id = $3, updated_at = now()
        WHERE workspace_id = $1 AND id = $2
        "#,
    )
    .bind(workspace_id)
    .bind(project_id)
    .bind(root_document_id)
    .execute(&mut *tx)
    .await?;

    seed_workflow(&mut tx, workspace_id, project_id).await?;

    record_project_event_and_audit(
        &mut tx,
        ProjectChangeRecord {
            workspace_id,
            actor_user_id,
            verb: "project.created",
            target_type: "project",
            target_id: project_id,
            payload: json!({
                "projectId": project_id.to_string(),
                "key": input.key,
                "name": input.name.trim(),
                "visibility": input.visibility,
                "rootDocumentId": root_document_id.to_string(),
            }),
            client_ip,
        },
    )
    .await?;

    let row = sqlx::query_as::<
        _,
        (
            Uuid,
            String,
            String,
            Option<String>,
            Option<String>,
            String,
            Option<Uuid>,
            String,
            Uuid,
            DateTime<Utc>,
            DateTime<Utc>,
        ),
    >(
        r#"
        SELECT id, key, name, description, icon, visibility, root_document_id, status,
               created_by, created_at, updated_at
        FROM fvoci.projects
        WHERE workspace_id = $1 AND id = $2
        "#,
    )
    .bind(workspace_id)
    .bind(project_id)
    .fetch_one(&mut *tx)
    .await?;

    tx.commit().await?;
    Ok(Ok(ProjectRow {
        id: row.0,
        workspace_id,
        key: row.1,
        name: row.2,
        description: row.3,
        icon: row.4,
        visibility: row.5,
        root_document_id: row.6,
        status: row.7,
        created_by: row.8,
        created_at: row.9,
        updated_at: row.10,
    }))
}

pub async fn list_projects(
    pool: &PgPool,
    workspace_id: Uuid,
    actor_user_id: Uuid,
    session_id: Uuid,
) -> Result<Result<Vec<ProjectListItem>, ProjectDbError>, sqlx::Error> {
    let mut tx = pool.begin().await?;
    set_tenant(&mut tx, workspace_id).await?;
    if !session_is_live(&mut tx, actor_user_id, session_id).await? {
        tx.rollback().await?;
        return Ok(Err(ProjectDbError::Forbidden));
    }
    if !workspace_is_live(&mut tx, workspace_id).await? {
        tx.rollback().await?;
        return Ok(Err(ProjectDbError::NotFound));
    }
    let workspace_role = membership_role(&mut tx, workspace_id, actor_user_id).await?;
    let workspace_role = match workspace_role {
        Some(r) if r.at_least(WorkspaceRole::Guest) => r,
        _ => {
            tx.rollback().await?;
            return Ok(Err(ProjectDbError::NotFound));
        }
    };

    let guest = workspace_role == WorkspaceRole::Guest;
    let visible = visible_project_sql("p", 2, 3);
    let list_sql = format!(
        r#"
        SELECT p.id, p.key, p.name, p.description, p.icon, p.visibility, p.root_document_id,
               p.status, p.created_by, p.created_at, p.updated_at
        FROM fvoci.projects p
        WHERE p.workspace_id = $1
          AND p.deleted_at IS NULL
          AND {visible}
        ORDER BY p.key COLLATE "C"
        "#
    );
    let rows = sqlx::query_as::<
        _,
        (
            Uuid,
            String,
            String,
            Option<String>,
            Option<String>,
            String,
            Option<Uuid>,
            String,
            Uuid,
            DateTime<Utc>,
            DateTime<Utc>,
        ),
    >(&list_sql)
    .bind(workspace_id)
    .bind(guest)
    .bind(actor_user_id)
    .fetch_all(&mut *tx)
    .await?;

    let mut items = Vec::new();
    for row in rows {
        let project_id = row.0;
        let counts: (i64, i64) = sqlx::query_as(
            r#"
            SELECT
                count(*) FILTER (WHERE t.deleted_at IS NULL AND t.archived_at IS NULL),
                count(*) FILTER (
                    WHERE t.deleted_at IS NULL
                      AND t.archived_at IS NULL
                      AND s.category NOT IN ('done', 'canceled')
                )
            FROM fvoci.tasks t
            INNER JOIN fvoci.statuses s
                ON s.workspace_id = t.workspace_id
               AND s.project_id = t.project_id
               AND s.id = t.status_id
            WHERE t.workspace_id = $1 AND t.project_id = $2
            "#,
        )
        .bind(workspace_id)
        .bind(project_id)
        .fetch_one(&mut *tx)
        .await?;

        items.push(ProjectListItem {
            project: ProjectRow {
                id: project_id,
                workspace_id,
                key: row.1,
                name: row.2,
                description: row.3,
                icon: row.4,
                visibility: row.5,
                root_document_id: row.6,
                status: row.7,
                created_by: row.8,
                created_at: row.9,
                updated_at: row.10,
            },
            task_count: counts.0,
            open_task_count: counts.1,
        });
    }
    tx.commit().await?;
    Ok(Ok(items))
}

pub async fn get_project(
    pool: &PgPool,
    workspace_id: Uuid,
    project_id: Uuid,
    actor_user_id: Uuid,
    session_id: Uuid,
) -> Result<Result<ProjectRow, ProjectDbError>, sqlx::Error> {
    let mut tx = pool.begin().await?;
    set_tenant(&mut tx, workspace_id).await?;
    if !session_is_live(&mut tx, actor_user_id, session_id).await? {
        tx.rollback().await?;
        return Ok(Err(ProjectDbError::Forbidden));
    }
    if !workspace_is_live(&mut tx, workspace_id).await? {
        tx.rollback().await?;
        return Ok(Err(ProjectDbError::NotFound));
    }
    let locked = lock_project(&mut tx, workspace_id, project_id).await?;
    let Some(locked) = locked else {
        tx.rollback().await?;
        return Ok(Err(ProjectDbError::NotFound));
    };
    if project_permission(&mut tx, workspace_id, actor_user_id, &locked)
        .await?
        .at_least(ProjectPermission::View)
    {
        tx.commit().await?;
        Ok(Ok(ProjectRow {
            id: locked.id,
            workspace_id,
            key: locked.key,
            name: locked.name,
            description: locked.description,
            icon: locked.icon,
            visibility: locked.visibility,
            root_document_id: locked.root_document_id,
            status: locked.status,
            created_by: locked.created_by,
            created_at: locked.created_at,
            updated_at: locked.updated_at,
        }))
    } else {
        tx.rollback().await?;
        Ok(Err(ProjectDbError::NotFound))
    }
}

pub async fn update_project(
    pool: &PgPool,
    workspace_id: Uuid,
    project_id: Uuid,
    actor_user_id: Uuid,
    session_id: Uuid,
    input: UpdateProjectInput<'_>,
    client_ip: Option<&str>,
) -> Result<Result<ProjectRow, ProjectDbError>, sqlx::Error> {
    let mut tx = pool.begin().await?;
    set_tenant(&mut tx, workspace_id).await?;
    let mut lock_users = vec![actor_user_id];
    if let Some(lead) = input.lead_user_id {
        lock_users.push(lead);
    }
    lock_membership_users(&mut tx, &lock_users).await?;
    if !recheck_session(&mut tx, actor_user_id, session_id).await? {
        tx.rollback().await?;
        return Ok(Err(ProjectDbError::Forbidden));
    }
    if !workspace_is_live(&mut tx, workspace_id).await? {
        tx.rollback().await?;
        return Ok(Err(ProjectDbError::NotFound));
    }
    let locked = lock_project(&mut tx, workspace_id, project_id).await?;
    let Some(locked) = locked else {
        tx.rollback().await?;
        return Ok(Err(ProjectDbError::NotFound));
    };
    if locked.status == "archived" {
        tx.rollback().await?;
        return Ok(Err(ProjectDbError::Archived));
    }
    let pre_permission = project_permission(&mut tx, workspace_id, actor_user_id, &locked).await?;
    if !pre_permission.at_least(ProjectPermission::Manage) {
        tx.rollback().await?;
        return Ok(Err(ProjectDbError::NotFound));
    }

    let mut name = locked.name.clone();
    let mut visibility = locked.visibility.clone();
    let mut description = locked.description.clone();
    let mut icon = locked.icon.clone();

    if let Some(next_name) = input.name {
        name = next_name.trim().to_string();
    }
    if let Some(next_visibility) = input.visibility {
        visibility = next_visibility.to_string();
    }
    if let Some(next_description) = input.description {
        description = optional_text_to_db(next_description);
    }
    if let Some(next_icon) = input.icon {
        icon = optional_text_to_db(next_icon);
    }

    if visibility == "private"
        && locked.visibility == "workspace"
        && count_project_leads(&mut tx, workspace_id, project_id).await? == 0
    {
        tx.rollback().await?;
        return Ok(Err(ProjectDbError::LastLead));
    }

    if let Some(lead_user_id) = input.lead_user_id {
        let lead_ws_role = membership_role(&mut tx, workspace_id, lead_user_id).await?;
        if !lead_ws_role
            .map(|r| r.at_least(WorkspaceRole::Member))
            .unwrap_or(false)
        {
            tx.rollback().await?;
            return Ok(Err(ProjectDbError::GuestLead));
        }
        let member =
            direct_project_member_role(&mut tx, workspace_id, project_id, lead_user_id).await?;
        if member.is_none() {
            tx.rollback().await?;
            return Ok(Err(ProjectDbError::LeadNotMember));
        }
        let promoted = sqlx::query(
            r#"
            UPDATE fvoci.project_members
            SET role = 'lead', updated_at = now()
            WHERE workspace_id = $1 AND project_id = $2 AND user_id = $3
            "#,
        )
        .bind(workspace_id)
        .bind(project_id)
        .bind(lead_user_id)
        .execute(&mut *tx)
        .await?;
        if promoted.rows_affected() == 0 {
            tx.rollback().await?;
            return Ok(Err(ProjectDbError::LeadNotMember));
        }
        sqlx::query(
            r#"
            UPDATE fvoci.project_members
            SET role = 'member', updated_at = now()
            WHERE workspace_id = $1 AND project_id = $2 AND user_id <> $3 AND role = 'lead'
            "#,
        )
        .bind(workspace_id)
        .bind(project_id)
        .bind(lead_user_id)
        .execute(&mut *tx)
        .await?;
    }

    let updated = sqlx::query_as::<
        _,
        (
            Uuid,
            String,
            String,
            Option<String>,
            Option<String>,
            String,
            Option<Uuid>,
            String,
            Uuid,
            DateTime<Utc>,
            DateTime<Utc>,
        ),
    >(
        r#"
        UPDATE fvoci.projects
        SET name = $3, visibility = $4, description = $5, icon = $6, updated_at = now()
        WHERE workspace_id = $1 AND id = $2 AND deleted_at IS NULL
        RETURNING id, key, name, description, icon, visibility, root_document_id, status,
                  created_by, created_at, updated_at
        "#,
    )
    .bind(workspace_id)
    .bind(project_id)
    .bind(&name)
    .bind(&visibility)
    .bind(&description)
    .bind(&icon)
    .fetch_one(&mut *tx)
    .await?;

    if visibility == "private" && count_project_leads(&mut tx, workspace_id, project_id).await? == 0
    {
        tx.rollback().await?;
        return Ok(Err(ProjectDbError::LastLead));
    }

    record_project_event_and_audit(
        &mut tx,
        ProjectChangeRecord {
            workspace_id,
            actor_user_id,
            verb: "project.updated",
            target_type: "project",
            target_id: project_id,
            payload: json!({
                "projectId": project_id.to_string(),
                "name": updated.2,
                "visibility": updated.5,
            }),
            client_ip,
        },
    )
    .await?;

    match tx.commit().await {
        Ok(()) => Ok(Ok(ProjectRow {
            id: updated.0,
            workspace_id,
            key: updated.1,
            name: updated.2,
            description: updated.3,
            icon: updated.4,
            visibility: updated.5,
            root_document_id: updated.6,
            status: updated.7,
            created_by: updated.8,
            created_at: updated.9,
            updated_at: updated.10,
        })),
        Err(err) if is_private_lead_violation(&err) => Ok(Err(ProjectDbError::LastLead)),
        Err(err) => Err(err),
    }
}

pub async fn list_project_members(
    pool: &PgPool,
    workspace_id: Uuid,
    project_id: Uuid,
    actor_user_id: Uuid,
    session_id: Uuid,
) -> Result<Result<Vec<ProjectMemberRow>, ProjectDbError>, sqlx::Error> {
    let mut tx = pool.begin().await?;
    set_tenant(&mut tx, workspace_id).await?;
    if !session_is_live(&mut tx, actor_user_id, session_id).await? {
        tx.rollback().await?;
        return Ok(Err(ProjectDbError::Forbidden));
    }
    if !workspace_is_live(&mut tx, workspace_id).await? {
        tx.rollback().await?;
        return Ok(Err(ProjectDbError::NotFound));
    }
    let locked = lock_project(&mut tx, workspace_id, project_id).await?;
    let Some(locked) = locked else {
        tx.rollback().await?;
        return Ok(Err(ProjectDbError::NotFound));
    };
    if !project_permission(&mut tx, workspace_id, actor_user_id, &locked)
        .await?
        .at_least(ProjectPermission::View)
    {
        tx.rollback().await?;
        return Ok(Err(ProjectDbError::NotFound));
    }
    let rows = sqlx::query_as::<_, (Uuid, String, String, Option<String>, String)>(
        r#"
        SELECT u.id, u.email, u.given_name, u.family_name, pm.role
        FROM fvoci.project_members pm
        INNER JOIN fvoci.users u ON u.id = pm.user_id
        WHERE pm.workspace_id = $1 AND pm.project_id = $2
          AND pm.user_id IS NOT NULL
          AND u.deleted_at IS NULL
        ORDER BY u.email COLLATE "C"
        "#,
    )
    .bind(workspace_id)
    .bind(project_id)
    .fetch_all(&mut *tx)
    .await?;
    tx.commit().await?;
    Ok(Ok(rows
        .into_iter()
        .filter_map(|(user_id, email, given_name, family_name, role)| {
            ProjectMemberRole::parse(&role).map(|role| ProjectMemberRow {
                user_id,
                email,
                given_name,
                family_name,
                role,
            })
        })
        .collect()))
}

pub async fn add_project_member(
    pool: &PgPool,
    workspace_id: Uuid,
    project_id: Uuid,
    actor_user_id: Uuid,
    session_id: Uuid,
    target_user_id: Uuid,
    role: ProjectMemberRole,
    client_ip: Option<&str>,
) -> Result<Result<(), ProjectDbError>, sqlx::Error> {
    let mut tx = pool.begin().await?;
    set_tenant(&mut tx, workspace_id).await?;
    lock_membership_users(&mut tx, &[actor_user_id, target_user_id]).await?;
    if !recheck_session(&mut tx, actor_user_id, session_id).await? {
        tx.rollback().await?;
        return Ok(Err(ProjectDbError::Forbidden));
    }
    if !workspace_is_live(&mut tx, workspace_id).await? {
        tx.rollback().await?;
        return Ok(Err(ProjectDbError::NotFound));
    }
    let locked = lock_project(&mut tx, workspace_id, project_id).await?;
    let Some(locked) = locked else {
        tx.rollback().await?;
        return Ok(Err(ProjectDbError::NotFound));
    };
    if !project_permission(&mut tx, workspace_id, actor_user_id, &locked)
        .await?
        .at_least(ProjectPermission::Manage)
    {
        tx.rollback().await?;
        return Ok(Err(ProjectDbError::NotFound));
    }
    if role == ProjectMemberRole::Lead {
        let target_ws_role = membership_role(&mut tx, workspace_id, target_user_id).await?;
        if !target_ws_role
            .map(|r| r.at_least(WorkspaceRole::Member))
            .unwrap_or(false)
        {
            tx.rollback().await?;
            return Ok(Err(ProjectDbError::GuestLead));
        }
    }
    let inserted = sqlx::query(
        r#"
        INSERT INTO fvoci.project_members (id, workspace_id, project_id, user_id, role)
        VALUES ($1, $2, $3, $4, $5)
        "#,
    )
    .bind(Uuid::now_v7())
    .bind(workspace_id)
    .bind(project_id)
    .bind(target_user_id)
    .bind(role.as_str())
    .execute(&mut *tx)
    .await;
    if let Err(err) = inserted {
        if err
            .as_database_error()
            .and_then(|db| db.constraint())
            .is_some()
        {
            tx.rollback().await?;
            return Ok(Err(ProjectDbError::Conflict));
        }
        return Err(err);
    }

    record_project_event_and_audit(
        &mut tx,
        ProjectChangeRecord {
            workspace_id,
            actor_user_id,
            verb: "project_member.added",
            target_type: "project_member",
            target_id: target_user_id,
            payload: json!({
                "projectId": project_id.to_string(),
                "userId": target_user_id.to_string(),
                "role": role.as_str(),
            }),
            client_ip,
        },
    )
    .await?;

    tx.commit().await?;
    Ok(Ok(()))
}

pub async fn update_project_member_role(
    pool: &PgPool,
    workspace_id: Uuid,
    project_id: Uuid,
    actor_user_id: Uuid,
    session_id: Uuid,
    target_user_id: Uuid,
    role: ProjectMemberRole,
    client_ip: Option<&str>,
) -> Result<Result<(), ProjectDbError>, sqlx::Error> {
    let mut tx = pool.begin().await?;
    set_tenant(&mut tx, workspace_id).await?;
    lock_membership_users(&mut tx, &[actor_user_id, target_user_id]).await?;
    if !recheck_session(&mut tx, actor_user_id, session_id).await? {
        tx.rollback().await?;
        return Ok(Err(ProjectDbError::Forbidden));
    }
    if !workspace_is_live(&mut tx, workspace_id).await? {
        tx.rollback().await?;
        return Ok(Err(ProjectDbError::NotFound));
    }
    let locked = lock_project(&mut tx, workspace_id, project_id).await?;
    let Some(locked) = locked else {
        tx.rollback().await?;
        return Ok(Err(ProjectDbError::NotFound));
    };
    if !project_permission(&mut tx, workspace_id, actor_user_id, &locked)
        .await?
        .at_least(ProjectPermission::Manage)
    {
        tx.rollback().await?;
        return Ok(Err(ProjectDbError::NotFound));
    }
    let current =
        direct_project_member_role(&mut tx, workspace_id, project_id, target_user_id).await?;
    let Some(current) = current else {
        tx.rollback().await?;
        return Ok(Err(ProjectDbError::NotFound));
    };
    if role == ProjectMemberRole::Lead {
        let target_ws_role = membership_role(&mut tx, workspace_id, target_user_id).await?;
        if !target_ws_role
            .map(|r| r.at_least(WorkspaceRole::Member))
            .unwrap_or(false)
        {
            tx.rollback().await?;
            return Ok(Err(ProjectDbError::GuestLead));
        }
    }
    if current == ProjectMemberRole::Lead
        && role != ProjectMemberRole::Lead
        && locked.visibility == "private"
        && count_project_leads_excluding(&mut tx, workspace_id, project_id, target_user_id).await?
            == 0
    {
        tx.rollback().await?;
        return Ok(Err(ProjectDbError::LastLead));
    }
    let updated = sqlx::query(
        r#"
        UPDATE fvoci.project_members
        SET role = $4, updated_at = now()
        WHERE workspace_id = $1 AND project_id = $2 AND user_id = $3
        "#,
    )
    .bind(workspace_id)
    .bind(project_id)
    .bind(target_user_id)
    .bind(role.as_str())
    .execute(&mut *tx)
    .await?;
    if updated.rows_affected() == 0 {
        tx.rollback().await?;
        return Ok(Err(ProjectDbError::NotFound));
    }

    record_project_event_and_audit(
        &mut tx,
        ProjectChangeRecord {
            workspace_id,
            actor_user_id,
            verb: "project_member.role_changed",
            target_type: "project_member",
            target_id: target_user_id,
            payload: json!({
                "projectId": project_id.to_string(),
                "userId": target_user_id.to_string(),
                "fromRole": current.as_str(),
                "role": role.as_str(),
            }),
            client_ip,
        },
    )
    .await?;

    let commit = tx.commit().await;
    if let Err(err) = commit {
        if is_private_lead_violation(&err) {
            return Ok(Err(ProjectDbError::LastLead));
        }
        return Err(err);
    }
    Ok(Ok(()))
}

pub async fn remove_project_member(
    pool: &PgPool,
    workspace_id: Uuid,
    project_id: Uuid,
    actor_user_id: Uuid,
    session_id: Uuid,
    target_user_id: Uuid,
    client_ip: Option<&str>,
) -> Result<Result<(), ProjectDbError>, sqlx::Error> {
    let mut tx = pool.begin().await?;
    set_tenant(&mut tx, workspace_id).await?;
    lock_membership_users(&mut tx, &[actor_user_id, target_user_id]).await?;
    if !recheck_session(&mut tx, actor_user_id, session_id).await? {
        tx.rollback().await?;
        return Ok(Err(ProjectDbError::Forbidden));
    }
    if !workspace_is_live(&mut tx, workspace_id).await? {
        tx.rollback().await?;
        return Ok(Err(ProjectDbError::NotFound));
    }
    let locked = lock_project(&mut tx, workspace_id, project_id).await?;
    let Some(locked) = locked else {
        tx.rollback().await?;
        return Ok(Err(ProjectDbError::NotFound));
    };
    if !project_permission(&mut tx, workspace_id, actor_user_id, &locked)
        .await?
        .at_least(ProjectPermission::Manage)
    {
        tx.rollback().await?;
        return Ok(Err(ProjectDbError::NotFound));
    }
    let current =
        direct_project_member_role(&mut tx, workspace_id, project_id, target_user_id).await?;
    let Some(current) = current else {
        tx.rollback().await?;
        return Ok(Err(ProjectDbError::NotFound));
    };
    if current == ProjectMemberRole::Lead
        && locked.visibility == "private"
        && count_project_leads_excluding(&mut tx, workspace_id, project_id, target_user_id).await?
            == 0
    {
        tx.rollback().await?;
        return Ok(Err(ProjectDbError::LastLead));
    }
    let deleted = sqlx::query(
        r#"
        DELETE FROM fvoci.project_members
        WHERE workspace_id = $1 AND project_id = $2 AND user_id = $3
        "#,
    )
    .bind(workspace_id)
    .bind(project_id)
    .bind(target_user_id)
    .execute(&mut *tx)
    .await?;
    if deleted.rows_affected() == 0 {
        tx.rollback().await?;
        return Ok(Err(ProjectDbError::NotFound));
    }

    record_project_event_and_audit(
        &mut tx,
        ProjectChangeRecord {
            workspace_id,
            actor_user_id,
            verb: "project_member.removed",
            target_type: "project_member",
            target_id: target_user_id,
            payload: json!({
                "projectId": project_id.to_string(),
                "userId": target_user_id.to_string(),
                "role": current.as_str(),
            }),
            client_ip,
        },
    )
    .await?;

    let commit = tx.commit().await;
    if let Err(err) = commit {
        if is_private_lead_violation(&err) {
            return Ok(Err(ProjectDbError::LastLead));
        }
        return Err(err);
    }
    Ok(Ok(()))
}

pub async fn get_project_workflow(
    pool: &PgPool,
    workspace_id: Uuid,
    project_id: Uuid,
    actor_user_id: Uuid,
    session_id: Uuid,
) -> Result<Result<WorkflowRow, ProjectDbError>, sqlx::Error> {
    let mut tx = pool.begin().await?;
    set_tenant(&mut tx, workspace_id).await?;
    if !session_is_live(&mut tx, actor_user_id, session_id).await? {
        tx.rollback().await?;
        return Ok(Err(ProjectDbError::Forbidden));
    }
    if !workspace_is_live(&mut tx, workspace_id).await? {
        tx.rollback().await?;
        return Ok(Err(ProjectDbError::NotFound));
    }
    let locked = lock_project(&mut tx, workspace_id, project_id).await?;
    let Some(locked) = locked else {
        tx.rollback().await?;
        return Ok(Err(ProjectDbError::NotFound));
    };
    if !project_permission(&mut tx, workspace_id, actor_user_id, &locked)
        .await?
        .at_least(ProjectPermission::View)
    {
        tx.rollback().await?;
        return Ok(Err(ProjectDbError::NotFound));
    }
    let workflow: Option<(Uuid,)> = sqlx::query_as(
        r#"
        SELECT id FROM fvoci.workflows
        WHERE workspace_id = $1 AND project_id = $2
        LIMIT 1
        "#,
    )
    .bind(workspace_id)
    .bind(project_id)
    .fetch_optional(&mut *tx)
    .await?;
    let Some((workflow_id,)) = workflow else {
        tx.rollback().await?;
        return Ok(Err(ProjectDbError::NotFound));
    };
    let statuses = sqlx::query_as::<_, (Uuid, String, String, String, Option<i32>)>(
        r#"
        SELECT id, name, category, sort_key, wip_limit
        FROM fvoci.statuses
        WHERE workspace_id = $1 AND project_id = $2
        ORDER BY sort_key COLLATE "C"
        "#,
    )
    .bind(workspace_id)
    .bind(project_id)
    .fetch_all(&mut *tx)
    .await?;
    tx.commit().await?;
    Ok(Ok(WorkflowRow {
        id: workflow_id,
        project_id,
        statuses: statuses
            .into_iter()
            .map(
                |(id, name, category, sort_key, wip_limit)| WorkflowStatusRow {
                    id,
                    name,
                    category,
                    sort_key,
                    wip_limit,
                },
            )
            .collect(),
    }))
}
