use chrono::{DateTime, Utc};
use sqlx::{PgPool, Postgres, Transaction};
use uuid::Uuid;

use crate::db::context::{lock_membership_users, recheck_session, session_is_live, set_tenant};
use crate::db::documents::{
    document_permission, membership_role, membership_role_for_update, workspace_is_live,
};
use crate::db::projects::{
    count_project_leads_except, is_private_lead_violation, lock_project, project_permission,
};
use crate::db::workspace::WorkspaceRole;
use crate::projects::{workspace_base_permission, ProjectMemberRole, ProjectPermission};

#[derive(Debug)]
pub enum GroupDbError {
    NotFound,
    Forbidden,
    Conflict,
    LastLead,
    InvalidInput,
}

#[derive(Debug, Clone)]
pub struct GroupRow {
    pub id: Uuid,
    pub workspace_id: Uuid,
    pub name: String,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Clone)]
pub struct GroupMemberRow {
    pub user_id: Uuid,
}

#[derive(Debug, Clone)]
pub struct GroupGrantRow {
    pub group_id: Uuid,
    pub role: ProjectMemberRole,
}

pub fn normalize_group_name(name: &str) -> Result<String, GroupDbError> {
    let trimmed = name.trim();
    if trimmed.is_empty() || trimmed.chars().count() > 100 {
        return Err(GroupDbError::InvalidInput);
    }
    Ok(trimmed.to_string())
}

fn has_workspace_manage(role: WorkspaceRole) -> bool {
    workspace_base_permission(role) == ProjectPermission::Manage
}

async fn load_group(
    tx: &mut Transaction<'_, Postgres>,
    workspace_id: Uuid,
    group_id: Uuid,
) -> Result<Option<GroupRow>, sqlx::Error> {
    let row = sqlx::query_as::<_, (Uuid, Uuid, String, DateTime<Utc>, DateTime<Utc>)>(
        r#"
        SELECT id, workspace_id, name, created_at, updated_at
        FROM fvoci.groups
        WHERE workspace_id = $1 AND id = $2
        "#,
    )
    .bind(workspace_id)
    .bind(group_id)
    .fetch_optional(&mut **tx)
    .await?;
    Ok(row.map(
        |(id, workspace_id, name, created_at, updated_at)| GroupRow {
            id,
            workspace_id,
            name,
            created_at,
            updated_at,
        },
    ))
}

async fn lock_group(
    tx: &mut Transaction<'_, Postgres>,
    workspace_id: Uuid,
    group_id: Uuid,
) -> Result<Option<GroupRow>, sqlx::Error> {
    let row = sqlx::query_as::<_, (Uuid, Uuid, String, DateTime<Utc>, DateTime<Utc>)>(
        r#"
        SELECT id, workspace_id, name, created_at, updated_at
        FROM fvoci.groups
        WHERE workspace_id = $1 AND id = $2
        FOR UPDATE
        "#,
    )
    .bind(workspace_id)
    .bind(group_id)
    .fetch_optional(&mut **tx)
    .await?;
    Ok(row.map(
        |(id, workspace_id, name, created_at, updated_at)| GroupRow {
            id,
            workspace_id,
            name,
            created_at,
            updated_at,
        },
    ))
}

pub async fn list_groups(
    pool: &PgPool,
    workspace_id: Uuid,
    actor_user_id: Uuid,
    session_id: Uuid,
) -> Result<Result<Vec<GroupRow>, GroupDbError>, sqlx::Error> {
    let mut tx = pool.begin().await?;
    set_tenant(&mut tx, workspace_id).await?;
    if !session_is_live(&mut tx, actor_user_id, session_id).await? {
        tx.rollback().await?;
        return Ok(Err(GroupDbError::Forbidden));
    }
    if !workspace_is_live(&mut tx, workspace_id).await? {
        tx.rollback().await?;
        return Ok(Err(GroupDbError::NotFound));
    }
    if membership_role(&mut tx, workspace_id, actor_user_id)
        .await?
        .is_none()
    {
        tx.rollback().await?;
        return Ok(Err(GroupDbError::NotFound));
    }
    let rows = sqlx::query_as::<_, (Uuid, Uuid, String, DateTime<Utc>, DateTime<Utc>)>(
        r#"
        SELECT id, workspace_id, name, created_at, updated_at
        FROM fvoci.groups
        WHERE workspace_id = $1
        ORDER BY created_at, id
        "#,
    )
    .bind(workspace_id)
    .fetch_all(&mut *tx)
    .await?;
    tx.commit().await?;
    Ok(Ok(rows
        .into_iter()
        .map(
            |(id, workspace_id, name, created_at, updated_at)| GroupRow {
                id,
                workspace_id,
                name,
                created_at,
                updated_at,
            },
        )
        .collect()))
}

pub async fn create_group(
    pool: &PgPool,
    workspace_id: Uuid,
    actor_user_id: Uuid,
    session_id: Uuid,
    name: &str,
) -> Result<Result<GroupRow, GroupDbError>, sqlx::Error> {
    let name = match normalize_group_name(name) {
        Ok(name) => name,
        Err(err) => return Ok(Err(err)),
    };
    let group_id = Uuid::now_v7();
    let mut tx = pool.begin().await?;
    set_tenant(&mut tx, workspace_id).await?;
    lock_membership_users(&mut tx, &[actor_user_id]).await?;
    if !recheck_session(&mut tx, actor_user_id, session_id).await? {
        tx.rollback().await?;
        return Ok(Err(GroupDbError::Forbidden));
    }
    if !workspace_is_live(&mut tx, workspace_id).await? {
        tx.rollback().await?;
        return Ok(Err(GroupDbError::NotFound));
    }
    let role = membership_role_for_update(&mut tx, workspace_id, actor_user_id).await?;
    let Some(role) = role else {
        tx.rollback().await?;
        return Ok(Err(GroupDbError::NotFound));
    };
    if !has_workspace_manage(role) {
        tx.rollback().await?;
        return Ok(Err(GroupDbError::NotFound));
    }
    sqlx::query(
        r#"
        INSERT INTO fvoci.groups (id, workspace_id, name)
        VALUES ($1, $2, $3)
        "#,
    )
    .bind(group_id)
    .bind(workspace_id)
    .bind(&name)
    .execute(&mut *tx)
    .await?;
    let created = load_group(&mut tx, workspace_id, group_id)
        .await?
        .expect("group row after insert");
    tx.commit().await?;
    Ok(Ok(created))
}

pub async fn purge_group(
    pool: &PgPool,
    workspace_id: Uuid,
    actor_user_id: Uuid,
    session_id: Uuid,
    group_id: Uuid,
) -> Result<Result<(), GroupDbError>, sqlx::Error> {
    let mut tx = pool.begin().await?;
    set_tenant(&mut tx, workspace_id).await?;
    lock_membership_users(&mut tx, &[actor_user_id]).await?;
    if !recheck_session(&mut tx, actor_user_id, session_id).await? {
        tx.rollback().await?;
        return Ok(Err(GroupDbError::Forbidden));
    }
    if !workspace_is_live(&mut tx, workspace_id).await? {
        tx.rollback().await?;
        return Ok(Err(GroupDbError::NotFound));
    }
    let role = membership_role_for_update(&mut tx, workspace_id, actor_user_id).await?;
    let Some(role) = role else {
        tx.rollback().await?;
        return Ok(Err(GroupDbError::NotFound));
    };
    if !has_workspace_manage(role) {
        tx.rollback().await?;
        return Ok(Err(GroupDbError::NotFound));
    }
    let lead_projects = sqlx::query_as::<_, (Uuid,)>(
        r#"
        SELECT p.id
        FROM fvoci.projects p
        INNER JOIN fvoci.project_members pm
            ON pm.workspace_id = p.workspace_id AND pm.project_id = p.id
        WHERE p.workspace_id = $1
          AND p.deleted_at IS NULL
          AND p.visibility = 'private'
          AND pm.group_id = $2
          AND pm.role = 'lead'
        ORDER BY p.id
        FOR NO KEY UPDATE OF p
        "#,
    )
    .bind(workspace_id)
    .bind(group_id)
    .fetch_all(&mut *tx)
    .await?;
    if lock_group(&mut tx, workspace_id, group_id).await?.is_none() {
        tx.rollback().await?;
        return Ok(Err(GroupDbError::NotFound));
    }
    for (project_id,) in lead_projects {
        if count_project_leads_except(&mut tx, workspace_id, project_id, None, Some(group_id))
            .await?
            == 0
        {
            tx.rollback().await?;
            return Ok(Err(GroupDbError::LastLead));
        }
    }
    sqlx::query("DELETE FROM fvoci.groups WHERE workspace_id = $1 AND id = $2")
        .bind(workspace_id)
        .bind(group_id)
        .execute(&mut *tx)
        .await?;
    let commit = tx.commit().await;
    if let Err(err) = commit {
        if is_private_lead_violation(&err) {
            return Ok(Err(GroupDbError::LastLead));
        }
        return Err(err);
    }
    Ok(Ok(()))
}

pub async fn list_group_members(
    pool: &PgPool,
    workspace_id: Uuid,
    actor_user_id: Uuid,
    session_id: Uuid,
    group_id: Uuid,
) -> Result<Result<Vec<GroupMemberRow>, GroupDbError>, sqlx::Error> {
    let mut tx = pool.begin().await?;
    set_tenant(&mut tx, workspace_id).await?;
    if !session_is_live(&mut tx, actor_user_id, session_id).await? {
        tx.rollback().await?;
        return Ok(Err(GroupDbError::Forbidden));
    }
    if !workspace_is_live(&mut tx, workspace_id).await? {
        tx.rollback().await?;
        return Ok(Err(GroupDbError::NotFound));
    }
    if membership_role(&mut tx, workspace_id, actor_user_id)
        .await?
        .is_none()
    {
        tx.rollback().await?;
        return Ok(Err(GroupDbError::NotFound));
    }
    if load_group(&mut tx, workspace_id, group_id).await?.is_none() {
        tx.rollback().await?;
        return Ok(Err(GroupDbError::NotFound));
    }
    let rows = sqlx::query_as::<_, (Uuid,)>(
        r#"
        SELECT user_id
        FROM fvoci.group_members
        WHERE workspace_id = $1 AND group_id = $2
        ORDER BY user_id
        "#,
    )
    .bind(workspace_id)
    .bind(group_id)
    .fetch_all(&mut *tx)
    .await?;
    tx.commit().await?;
    Ok(Ok(rows
        .into_iter()
        .map(|(user_id,)| GroupMemberRow { user_id })
        .collect()))
}

pub async fn add_group_member(
    pool: &PgPool,
    workspace_id: Uuid,
    actor_user_id: Uuid,
    session_id: Uuid,
    group_id: Uuid,
    target_user_id: Uuid,
) -> Result<Result<(), GroupDbError>, sqlx::Error> {
    let mut tx = pool.begin().await?;
    set_tenant(&mut tx, workspace_id).await?;
    lock_membership_users(&mut tx, &[actor_user_id, target_user_id]).await?;
    if !recheck_session(&mut tx, actor_user_id, session_id).await? {
        tx.rollback().await?;
        return Ok(Err(GroupDbError::Forbidden));
    }
    if !workspace_is_live(&mut tx, workspace_id).await? {
        tx.rollback().await?;
        return Ok(Err(GroupDbError::NotFound));
    }
    let role = membership_role_for_update(&mut tx, workspace_id, actor_user_id).await?;
    let Some(role) = role else {
        tx.rollback().await?;
        return Ok(Err(GroupDbError::NotFound));
    };
    if !has_workspace_manage(role) {
        tx.rollback().await?;
        return Ok(Err(GroupDbError::NotFound));
    }
    if lock_group(&mut tx, workspace_id, group_id).await?.is_none() {
        tx.rollback().await?;
        return Ok(Err(GroupDbError::NotFound));
    }
    let inserted = sqlx::query_as::<_, (Uuid,)>(
        r#"
        INSERT INTO fvoci.group_members (workspace_id, group_id, user_id)
        SELECT $1, $2, $3
        FROM fvoci.groups g
        INNER JOIN fvoci.memberships m
            ON m.workspace_id = g.workspace_id AND m.user_id = $3
        INNER JOIN fvoci.memberships actor
            ON actor.workspace_id = g.workspace_id AND actor.user_id = $4
        WHERE g.workspace_id = $1
          AND g.id = $2
          AND actor.role IN ('owner', 'admin')
        RETURNING user_id
        "#,
    )
    .bind(workspace_id)
    .bind(group_id)
    .bind(target_user_id)
    .bind(actor_user_id)
    .fetch_optional(&mut *tx)
    .await;
    match inserted {
        Ok(Some(_)) => {}
        Ok(None) => {
            tx.rollback().await?;
            return Ok(Err(GroupDbError::NotFound));
        }
        Err(err) => {
            if err
                .as_database_error()
                .and_then(|db| db.constraint())
                .is_some()
            {
                tx.rollback().await?;
                return Ok(Err(GroupDbError::Conflict));
            }
            return Err(err);
        }
    }
    tx.commit().await?;
    Ok(Ok(()))
}

pub async fn remove_group_member(
    pool: &PgPool,
    workspace_id: Uuid,
    actor_user_id: Uuid,
    session_id: Uuid,
    group_id: Uuid,
    target_user_id: Uuid,
) -> Result<Result<(), GroupDbError>, sqlx::Error> {
    let mut tx = pool.begin().await?;
    set_tenant(&mut tx, workspace_id).await?;
    lock_membership_users(&mut tx, &[actor_user_id, target_user_id]).await?;
    if !recheck_session(&mut tx, actor_user_id, session_id).await? {
        tx.rollback().await?;
        return Ok(Err(GroupDbError::Forbidden));
    }
    if !workspace_is_live(&mut tx, workspace_id).await? {
        tx.rollback().await?;
        return Ok(Err(GroupDbError::NotFound));
    }
    let role = membership_role_for_update(&mut tx, workspace_id, actor_user_id).await?;
    let Some(role) = role else {
        tx.rollback().await?;
        return Ok(Err(GroupDbError::NotFound));
    };
    if !has_workspace_manage(role) {
        tx.rollback().await?;
        return Ok(Err(GroupDbError::NotFound));
    }
    if lock_group(&mut tx, workspace_id, group_id).await?.is_none() {
        tx.rollback().await?;
        return Ok(Err(GroupDbError::NotFound));
    }
    let deleted = sqlx::query(
        r#"
        DELETE FROM fvoci.group_members
        WHERE workspace_id = $1 AND group_id = $2 AND user_id = $3
        "#,
    )
    .bind(workspace_id)
    .bind(group_id)
    .bind(target_user_id)
    .execute(&mut *tx)
    .await?;
    if deleted.rows_affected() == 0 {
        tx.rollback().await?;
        return Ok(Err(GroupDbError::NotFound));
    }
    tx.commit().await?;
    Ok(Ok(()))
}

pub async fn list_project_group_grants(
    pool: &PgPool,
    workspace_id: Uuid,
    actor_user_id: Uuid,
    session_id: Uuid,
    project_id: Uuid,
) -> Result<Result<Vec<GroupGrantRow>, GroupDbError>, sqlx::Error> {
    let mut tx = pool.begin().await?;
    set_tenant(&mut tx, workspace_id).await?;
    if !session_is_live(&mut tx, actor_user_id, session_id).await? {
        tx.rollback().await?;
        return Ok(Err(GroupDbError::Forbidden));
    }
    if !workspace_is_live(&mut tx, workspace_id).await? {
        tx.rollback().await?;
        return Ok(Err(GroupDbError::NotFound));
    }
    let locked = lock_project(&mut tx, workspace_id, project_id).await?;
    let Some(locked) = locked else {
        tx.rollback().await?;
        return Ok(Err(GroupDbError::NotFound));
    };
    if !project_permission(&mut tx, workspace_id, actor_user_id, &locked)
        .await?
        .at_least(ProjectPermission::View)
    {
        tx.rollback().await?;
        return Ok(Err(GroupDbError::NotFound));
    }
    let rows = sqlx::query_as::<_, (Uuid, String)>(
        r#"
        SELECT group_id, role
        FROM fvoci.project_members
        WHERE workspace_id = $1 AND project_id = $2 AND group_id IS NOT NULL
        ORDER BY group_id
        "#,
    )
    .bind(workspace_id)
    .bind(project_id)
    .fetch_all(&mut *tx)
    .await?;
    tx.commit().await?;
    Ok(Ok(rows
        .into_iter()
        .filter_map(|(group_id, role)| {
            ProjectMemberRole::parse(&role).map(|role| GroupGrantRow { group_id, role })
        })
        .collect()))
}

pub async fn add_group_to_project(
    pool: &PgPool,
    workspace_id: Uuid,
    actor_user_id: Uuid,
    session_id: Uuid,
    project_id: Uuid,
    group_id: Uuid,
    role: ProjectMemberRole,
) -> Result<Result<(), GroupDbError>, sqlx::Error> {
    let mut tx = pool.begin().await?;
    set_tenant(&mut tx, workspace_id).await?;
    lock_membership_users(&mut tx, &[actor_user_id]).await?;
    if !recheck_session(&mut tx, actor_user_id, session_id).await? {
        tx.rollback().await?;
        return Ok(Err(GroupDbError::Forbidden));
    }
    if !workspace_is_live(&mut tx, workspace_id).await? {
        tx.rollback().await?;
        return Ok(Err(GroupDbError::NotFound));
    }
    let locked = lock_project(&mut tx, workspace_id, project_id).await?;
    let Some(locked) = locked else {
        tx.rollback().await?;
        return Ok(Err(GroupDbError::NotFound));
    };
    if !project_permission(&mut tx, workspace_id, actor_user_id, &locked)
        .await?
        .at_least(ProjectPermission::Manage)
    {
        tx.rollback().await?;
        return Ok(Err(GroupDbError::NotFound));
    }
    if lock_group(&mut tx, workspace_id, group_id).await?.is_none() {
        tx.rollback().await?;
        return Ok(Err(GroupDbError::NotFound));
    }
    let inserted = sqlx::query(
        r#"
        INSERT INTO fvoci.project_members (id, workspace_id, project_id, group_id, role)
        VALUES ($1, $2, $3, $4, $5)
        "#,
    )
    .bind(Uuid::now_v7())
    .bind(workspace_id)
    .bind(project_id)
    .bind(group_id)
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
            return Ok(Err(GroupDbError::Conflict));
        }
        return Err(err);
    }
    tx.commit().await?;
    Ok(Ok(()))
}

pub async fn remove_group_from_project(
    pool: &PgPool,
    workspace_id: Uuid,
    actor_user_id: Uuid,
    session_id: Uuid,
    project_id: Uuid,
    group_id: Uuid,
) -> Result<Result<(), GroupDbError>, sqlx::Error> {
    let mut tx = pool.begin().await?;
    set_tenant(&mut tx, workspace_id).await?;
    lock_membership_users(&mut tx, &[actor_user_id]).await?;
    if !recheck_session(&mut tx, actor_user_id, session_id).await? {
        tx.rollback().await?;
        return Ok(Err(GroupDbError::Forbidden));
    }
    if !workspace_is_live(&mut tx, workspace_id).await? {
        tx.rollback().await?;
        return Ok(Err(GroupDbError::NotFound));
    }
    let locked = lock_project(&mut tx, workspace_id, project_id).await?;
    let Some(locked) = locked else {
        tx.rollback().await?;
        return Ok(Err(GroupDbError::NotFound));
    };
    if !project_permission(&mut tx, workspace_id, actor_user_id, &locked)
        .await?
        .at_least(ProjectPermission::Manage)
    {
        tx.rollback().await?;
        return Ok(Err(GroupDbError::NotFound));
    }
    if lock_group(&mut tx, workspace_id, group_id).await?.is_none() {
        tx.rollback().await?;
        return Ok(Err(GroupDbError::NotFound));
    }
    let current: Option<(String,)> = sqlx::query_as(
        r#"
        SELECT role FROM fvoci.project_members
        WHERE workspace_id = $1 AND project_id = $2 AND group_id = $3
        "#,
    )
    .bind(workspace_id)
    .bind(project_id)
    .bind(group_id)
    .fetch_optional(&mut *tx)
    .await?;
    let Some((current_role,)) = current else {
        tx.rollback().await?;
        return Ok(Err(GroupDbError::NotFound));
    };
    if current_role == "lead"
        && locked.visibility == "private"
        && count_project_leads_except(&mut tx, workspace_id, project_id, None, Some(group_id))
            .await?
            == 0
    {
        tx.rollback().await?;
        return Ok(Err(GroupDbError::LastLead));
    }
    sqlx::query(
        r#"
        DELETE FROM fvoci.project_members
        WHERE workspace_id = $1 AND project_id = $2 AND group_id = $3
        "#,
    )
    .bind(workspace_id)
    .bind(project_id)
    .bind(group_id)
    .execute(&mut *tx)
    .await?;
    let commit = tx.commit().await;
    if let Err(err) = commit {
        if is_private_lead_violation(&err) {
            return Ok(Err(GroupDbError::LastLead));
        }
        return Err(err);
    }
    Ok(Ok(()))
}

async fn load_wiki_document(
    tx: &mut Transaction<'_, Postgres>,
    workspace_id: Uuid,
    document_id: Uuid,
) -> Result<bool, sqlx::Error> {
    let row: Option<(Option<Uuid>, Option<DateTime<Utc>>)> = sqlx::query_as(
        r#"
        SELECT project_id, deleted_at
        FROM fvoci.documents
        WHERE workspace_id = $1 AND id = $2
        "#,
    )
    .bind(workspace_id)
    .bind(document_id)
    .fetch_optional(&mut **tx)
    .await?;
    Ok(matches!(
        row,
        Some((project_id, deleted_at)) if project_id.is_none() && deleted_at.is_none()
    ))
}

pub async fn list_document_group_grants(
    pool: &PgPool,
    workspace_id: Uuid,
    actor_user_id: Uuid,
    session_id: Uuid,
    document_id: Uuid,
) -> Result<Result<Vec<GroupGrantRow>, GroupDbError>, sqlx::Error> {
    let mut tx = pool.begin().await?;
    set_tenant(&mut tx, workspace_id).await?;
    if !session_is_live(&mut tx, actor_user_id, session_id).await? {
        tx.rollback().await?;
        return Ok(Err(GroupDbError::Forbidden));
    }
    if !workspace_is_live(&mut tx, workspace_id).await? {
        tx.rollback().await?;
        return Ok(Err(GroupDbError::NotFound));
    }
    if !document_permission(&mut tx, workspace_id, actor_user_id, document_id, true)
        .await?
        .at_least(ProjectPermission::View)
    {
        tx.rollback().await?;
        return Ok(Err(GroupDbError::NotFound));
    }
    if !load_wiki_document(&mut tx, workspace_id, document_id).await? {
        tx.rollback().await?;
        return Ok(Err(GroupDbError::NotFound));
    }
    let rows = sqlx::query_as::<_, (Uuid, String)>(
        r#"
        SELECT group_id, role
        FROM fvoci.document_members
        WHERE workspace_id = $1 AND document_id = $2 AND group_id IS NOT NULL
        ORDER BY group_id
        "#,
    )
    .bind(workspace_id)
    .bind(document_id)
    .fetch_all(&mut *tx)
    .await?;
    tx.commit().await?;
    Ok(Ok(rows
        .into_iter()
        .filter_map(|(group_id, role)| {
            ProjectMemberRole::parse(&role).map(|role| GroupGrantRow { group_id, role })
        })
        .collect()))
}

pub async fn add_group_to_document(
    pool: &PgPool,
    workspace_id: Uuid,
    actor_user_id: Uuid,
    session_id: Uuid,
    document_id: Uuid,
    group_id: Uuid,
    role: ProjectMemberRole,
) -> Result<Result<(), GroupDbError>, sqlx::Error> {
    let mut tx = pool.begin().await?;
    set_tenant(&mut tx, workspace_id).await?;
    lock_membership_users(&mut tx, &[actor_user_id]).await?;
    if !recheck_session(&mut tx, actor_user_id, session_id).await? {
        tx.rollback().await?;
        return Ok(Err(GroupDbError::Forbidden));
    }
    if !workspace_is_live(&mut tx, workspace_id).await? {
        tx.rollback().await?;
        return Ok(Err(GroupDbError::NotFound));
    }
    if !document_permission(&mut tx, workspace_id, actor_user_id, document_id, true)
        .await?
        .at_least(ProjectPermission::Manage)
    {
        tx.rollback().await?;
        return Ok(Err(GroupDbError::NotFound));
    }
    if !load_wiki_document(&mut tx, workspace_id, document_id).await? {
        tx.rollback().await?;
        return Ok(Err(GroupDbError::NotFound));
    }
    if lock_group(&mut tx, workspace_id, group_id).await?.is_none() {
        tx.rollback().await?;
        return Ok(Err(GroupDbError::NotFound));
    }
    let inserted = sqlx::query(
        r#"
        INSERT INTO fvoci.document_members (id, workspace_id, document_id, group_id, role)
        VALUES ($1, $2, $3, $4, $5)
        "#,
    )
    .bind(Uuid::now_v7())
    .bind(workspace_id)
    .bind(document_id)
    .bind(group_id)
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
            return Ok(Err(GroupDbError::Conflict));
        }
        return Err(err);
    }
    tx.commit().await?;
    Ok(Ok(()))
}

pub async fn remove_group_from_document(
    pool: &PgPool,
    workspace_id: Uuid,
    actor_user_id: Uuid,
    session_id: Uuid,
    document_id: Uuid,
    group_id: Uuid,
) -> Result<Result<(), GroupDbError>, sqlx::Error> {
    let mut tx = pool.begin().await?;
    set_tenant(&mut tx, workspace_id).await?;
    lock_membership_users(&mut tx, &[actor_user_id]).await?;
    if !recheck_session(&mut tx, actor_user_id, session_id).await? {
        tx.rollback().await?;
        return Ok(Err(GroupDbError::Forbidden));
    }
    if !workspace_is_live(&mut tx, workspace_id).await? {
        tx.rollback().await?;
        return Ok(Err(GroupDbError::NotFound));
    }
    if !document_permission(&mut tx, workspace_id, actor_user_id, document_id, true)
        .await?
        .at_least(ProjectPermission::Manage)
    {
        tx.rollback().await?;
        return Ok(Err(GroupDbError::NotFound));
    }
    if !load_wiki_document(&mut tx, workspace_id, document_id).await? {
        tx.rollback().await?;
        return Ok(Err(GroupDbError::NotFound));
    }
    if lock_group(&mut tx, workspace_id, group_id).await?.is_none() {
        tx.rollback().await?;
        return Ok(Err(GroupDbError::NotFound));
    }
    let deleted = sqlx::query(
        r#"
        DELETE FROM fvoci.document_members
        WHERE workspace_id = $1 AND document_id = $2 AND group_id = $3
        "#,
    )
    .bind(workspace_id)
    .bind(document_id)
    .bind(group_id)
    .execute(&mut *tx)
    .await?;
    if deleted.rows_affected() == 0 {
        tx.rollback().await?;
        return Ok(Err(GroupDbError::NotFound));
    }
    tx.commit().await?;
    Ok(Ok(()))
}

#[cfg(test)]
mod tests {
    use super::normalize_group_name;

    #[test]
    fn group_name_trims_and_rejects_empty_or_too_long() {
        assert_eq!(normalize_group_name("  랩팀  ").unwrap(), "랩팀");
        assert!(normalize_group_name("   ").is_err());
        assert!(normalize_group_name(&"한".repeat(101)).is_err());
        assert!(normalize_group_name(&"한".repeat(100)).is_ok());
    }
}
