use crate::db::backend::{Backend, DbTx, OperationTx};
use crate::db::codec::{Cell, FamilyRow};

use chrono::{DateTime, Utc};
use sqlx::{PgPool, Postgres, Transaction};
use uuid::Uuid;

use crate::db::context::{
    begin_read, lock_membership_users, recheck_session, session_is_live, set_tenant,
};
use crate::db::documents::document_permission;
use crate::db::projects::{
    count_project_leads_except, is_private_lead_violation, load_live_project, lock_project,
    project_permission,
};
use crate::db::workspace::{
    membership_role, membership_role_for_update, workspace_is_live, WorkspaceRole,
};
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

pub(crate) async fn list_group_member_user_ids(
    tx: &mut Transaction<'_, Postgres>,
    workspace_id: Uuid,
    group_id: Uuid,
) -> Result<Vec<Uuid>, sqlx::Error> {
    group_member_ids(tx, workspace_id, group_id).await
}

async fn group_member_ids(
    tx: &mut Transaction<'_, Postgres>,
    workspace_id: Uuid,
    group_id: Uuid,
) -> Result<Vec<Uuid>, sqlx::Error> {
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
    .fetch_all(&mut **tx)
    .await?;
    Ok(rows.into_iter().map(|(id,)| id).collect())
}

async fn lock_actor_and_group_members(
    tx: &mut Transaction<'_, Postgres>,
    workspace_id: Uuid,
    actor_user_id: Uuid,
    group_id: Uuid,
) -> Result<(), sqlx::Error> {
    let mut lock_users = group_member_ids(tx, workspace_id, group_id).await?;
    lock_users.push(actor_user_id);
    lock_membership_users(tx, &lock_users).await
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

/// Selected workspace group list only. Preserve PG wrappers and current
/// member (not manager-only) read authority, with one family read snapshot.
pub async fn list_groups_backend(
    backend: &crate::db::backend::Backend,
    workspace_id: Uuid,
    actor_user_id: Uuid,
    credential_id: Uuid,
) -> Result<Result<Vec<GroupRow>, GroupDbError>, sqlx::Error> {
    use crate::db::backend::{Backend, OperationTx};
    use crate::db::codec::Cell;
    if let Backend::Postgres(pool) = backend {
        return list_groups(pool, workspace_id, actor_user_id, credential_id).await;
    }
    let mut tx = backend.begin_read().await?;
    let result = async {
        let mut op = tx.operation();
        op.set_tenant(workspace_id).await?;
        if !op.session_is_live(actor_user_id,credential_id).await? {
            return Ok(Err(GroupDbError::Forbidden));
        }
        if !op.workspace_is_live(workspace_id).await? {
            return Ok(Err(GroupDbError::NotFound));
        }
        if op.membership_role(workspace_id,actor_user_id,false).await?.is_none() {
            return Ok(Err(GroupDbError::NotFound));
        }
        let OperationTx::SqliteFamily(family) = &mut op else { unreachable!() };
        let rows = family.query("SELECT id,workspace_id,name,created_at,updated_at FROM groups WHERE workspace_id=?1 ORDER BY created_at,id", &[Cell::uuid(workspace_id)]).await?;
        let items = rows.into_iter().map(|r| Ok(GroupRow {
            id:r.cell(0)?.id()?,workspace_id:r.cell(1)?.id()?,name:r.cell(2)?.string()?,
            created_at:r.cell(3)?.datetime()?,updated_at:r.cell(4)?.datetime()?,
        })).collect::<Result<Vec<_>,sqlx::Error>>()?;
        Ok(Ok(items))
    }.await;
    let cleanup = tx.rollback().await;
    group_read_after_rollback(result, cleanup)
}

#[derive(Debug, thiserror::Error)]
#[error("group list read refused: {0:?}")]
struct GroupReadRefusal(GroupDbError);

fn group_read_after_rollback(
    result: Result<Result<Vec<GroupRow>, GroupDbError>, sqlx::Error>,
    cleanup: Result<(), sqlx::Error>,
) -> Result<Result<Vec<GroupRow>, GroupDbError>, sqlx::Error> {
    match cleanup {
        Ok(()) => result,
        Err(cleanup) => {
            let original: Option<Box<dyn std::error::Error + Send + Sync>> = match result {
                Err(driver) => Some(Box::new(driver)),
                Ok(Err(refusal)) => Some(Box::new(GroupReadRefusal(refusal))),
                Ok(Ok(_)) => None,
            };
            Err(crate::db::backend::rollback_cleanup_unknown(
                original, cleanup,
            ))
        }
    }
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

pub async fn create_group_backend(
    backend: &Backend,
    ws: Uuid,
    actor: Uuid,
    credential: Uuid,
    name: &str,
) -> Result<Result<GroupRow, GroupDbError>, sqlx::Error> {
    if let Backend::Postgres(pool) = backend {
        return create_group(pool, ws, actor, credential, name).await;
    }
    let name = match normalize_group_name(name) {
        Ok(v) => v,
        Err(e) => return Ok(Err(e)),
    };
    let mut tx = backend.begin_write().await?;
    let result=async {
        let mut op=tx.operation();
        if let Err(e)=family_group_actor(&mut op,ws,actor,credential,true).await? {return Ok(Err(e));}
        let OperationTx::SqliteFamily(f)=&mut op else {unreachable!()};
        let rows=f.query("INSERT INTO groups(id,workspace_id,name) VALUES(?1,?2,?3) RETURNING id,workspace_id,name,created_at,updated_at", &[Cell::uuid(Uuid::now_v7()),Cell::uuid(ws),Cell::text(name)]).await?;
        Ok(Ok(family_group_row(rows.first().ok_or(sqlx::Error::RowNotFound)?)?))
    }.await;
    finish_group(tx, result, true).await
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
    lock_actor_and_group_members(&mut tx, workspace_id, actor_user_id, group_id).await?;
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
    let granted_projects = sqlx::query_as::<_, (Uuid, String, String)>(
        r#"
        SELECT p.id, p.visibility, pm.role
        FROM fvoci.projects p
        INNER JOIN fvoci.project_members pm
            ON pm.workspace_id = p.workspace_id AND pm.project_id = p.id
        WHERE p.workspace_id = $1
          AND p.deleted_at IS NULL
          AND pm.group_id = $2
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
    for (project_id, visibility, grant_role) in granted_projects {
        if visibility == "private"
            && grant_role == "lead"
            && count_project_leads_except(&mut tx, workspace_id, project_id, None, Some(group_id))
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

pub async fn purge_group_backend(
    backend: &Backend,
    ws: Uuid,
    actor: Uuid,
    credential: Uuid,
    group: Uuid,
) -> Result<Result<(), GroupDbError>, sqlx::Error> {
    if let Backend::Postgres(pool) = backend {
        return purge_group(pool, ws, actor, credential, group).await;
    }
    let mut tx = backend.begin_write().await?;
    let result = async {
        let mut op = tx.operation();
        if let Err(e) = family_group_actor(&mut op, ws, actor, credential, true).await? {
            return Ok(Err(e));
        }
        if !family_group_exists(&mut op, ws, group).await? {
            return Ok(Err(GroupDbError::NotFound));
        }
        if family_last_group_lead(&mut op, ws, group, None).await? {
            return Ok(Err(GroupDbError::LastLead));
        }
        let OperationTx::SqliteFamily(f) = &mut op else {
            unreachable!()
        };
        f.execute(
            "DELETE FROM groups WHERE workspace_id=?1 AND id=?2",
            &[Cell::uuid(ws), Cell::uuid(group)],
        )
        .await?;
        Ok(Ok(()))
    }
    .await;
    finish_group(tx, result, true).await
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

pub async fn list_group_members_backend(
    backend: &Backend,
    ws: Uuid,
    actor: Uuid,
    credential: Uuid,
    group: Uuid,
) -> Result<Result<Vec<GroupMemberRow>, GroupDbError>, sqlx::Error> {
    if let Backend::Postgres(pool) = backend {
        return list_group_members(pool, ws, actor, credential, group).await;
    }
    let mut tx = backend.begin_read().await?;
    let result= async {
        let mut op = tx.operation();
        if let Err(e) = family_group_actor(&mut op, ws, actor, credential, false).await? {
            return Ok(Err(e));
        }
        if !family_group_exists(&mut op, ws, group).await? {
            return Ok(Err(GroupDbError::NotFound));
        }
        let OperationTx::SqliteFamily(f) = &mut op else { unreachable!() };
        let rows = f.query("SELECT user_id FROM group_members WHERE workspace_id=?1 AND group_id=?2 ORDER BY user_id", &[Cell::uuid(ws), Cell::uuid(group)]).await?;
        Ok(Ok(rows.iter().map(|r| Ok(GroupMemberRow { user_id: r.cell(0)?.id()? })).collect::<Result<Vec<_>, sqlx::Error>>()?))
    }.await;
    finish_group(tx, result, false).await
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

pub async fn add_group_member_backend(
    backend: &Backend,
    ws: Uuid,
    actor: Uuid,
    credential: Uuid,
    group: Uuid,
    target: Uuid,
) -> Result<Result<(), GroupDbError>, sqlx::Error> {
    if let Backend::Postgres(pool) = backend {
        return add_group_member(pool, ws, actor, credential, group, target).await;
    }
    let mut tx = backend.begin_write().await?;
    let result= async {
        let mut op = tx.operation();
        if let Err(e) = family_group_actor(&mut op, ws, actor, credential, true).await? {
            return Ok(Err(e));
        }
        if !family_group_exists(&mut op, ws, group).await? {
            return Ok(Err(GroupDbError::NotFound));
        }
        if op.membership_role(ws, target, true).await?.is_none() {
            return Ok(Err(GroupDbError::NotFound));
        }
        let OperationTx::SqliteFamily(f) = &mut op else { unreachable!() };
        let n = f.execute("INSERT INTO group_members(workspace_id,group_id,user_id) VALUES(?1,?2,?3) ON CONFLICT(workspace_id,group_id,user_id) DO NOTHING", &[Cell::uuid(ws), Cell::uuid(group), Cell::uuid(target)]).await?;
        Ok(if n == 0 { Err(GroupDbError::Conflict) } else { Ok(()) })
    }.await;
    finish_group(tx, result, true).await
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

pub async fn remove_group_member_backend(
    backend: &Backend,
    ws: Uuid,
    actor: Uuid,
    credential: Uuid,
    group: Uuid,
    target: Uuid,
) -> Result<Result<(), GroupDbError>, sqlx::Error> {
    if let Backend::Postgres(pool) = backend {
        return remove_group_member(pool, ws, actor, credential, group, target).await;
    }
    let mut tx = backend.begin_write().await?;
    let result = async {
        let mut op = tx.operation();
        if let Err(e) = family_group_actor(&mut op, ws, actor, credential, true).await? {
            return Ok(Err(e));
        }
        if !family_group_exists(&mut op, ws, group).await? {
            return Ok(Err(GroupDbError::NotFound));
        }
        let OperationTx::SqliteFamily(f) = &mut op else {
            unreachable!()
        };
        let n = f
            .execute(
                "DELETE FROM group_members WHERE workspace_id=?1 AND group_id=?2 AND user_id=?3",
                &[Cell::uuid(ws), Cell::uuid(group), Cell::uuid(target)],
            )
            .await?;
        Ok(if n == 0 {
            Err(GroupDbError::NotFound)
        } else {
            Ok(())
        })
    }
    .await;
    finish_group(tx, result, true).await
}

pub async fn list_project_group_grants(
    pool: &PgPool,
    workspace_id: Uuid,
    actor_user_id: Uuid,
    session_id: Uuid,
    project_id: Uuid,
) -> Result<Result<Vec<GroupGrantRow>, GroupDbError>, sqlx::Error> {
    let mut tx = begin_read(pool).await?;
    set_tenant(&mut tx, workspace_id).await?;
    if !session_is_live(&mut tx, actor_user_id, session_id).await? {
        tx.rollback().await?;
        return Ok(Err(GroupDbError::Forbidden));
    }
    if !workspace_is_live(&mut tx, workspace_id).await? {
        tx.rollback().await?;
        return Ok(Err(GroupDbError::NotFound));
    }
    let project = load_live_project(&mut tx, workspace_id, project_id).await?;
    let Some(project) = project else {
        tx.rollback().await?;
        return Ok(Err(GroupDbError::NotFound));
    };
    if !project_permission(&mut tx, workspace_id, actor_user_id, &project)
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

pub async fn list_project_group_grants_backend(
    backend: &Backend,
    ws: Uuid,
    actor: Uuid,
    credential: Uuid,
    target: Uuid,
) -> Result<Result<Vec<GroupGrantRow>, GroupDbError>, sqlx::Error> {
    if let Backend::Postgres(pool) = backend {
        return list_project_group_grants(pool, ws, actor, credential, target).await;
    }
    let mut tx = backend.begin_read().await?;
    let result= async {
        let mut op = tx.operation();
        if let Err(e) = family_group_actor(&mut op, ws, actor, credential, false).await? {
            return Ok(Err(e));
        }
        if !(op.project_permission_by_id(ws, actor, target).await?.unwrap_or(ProjectPermission::None)).at_least(ProjectPermission::View) {
            return Ok(Err(GroupDbError::NotFound));
        }
        let OperationTx::SqliteFamily(f) = &mut op else { unreachable!() };
        let rows = f.query("SELECT group_id,role FROM project_members WHERE workspace_id=?1 AND project_id=?2 AND group_id IS NOT NULL ORDER BY group_id", &[Cell::uuid(ws), Cell::uuid(target)]).await?;
        let mut grants = Vec::new();
        for row in rows {
            if let Some(role) = ProjectMemberRole::parse(&row.cell(1)?.string()?) {
                grants.push(GroupGrantRow { group_id: row.cell(0)?.id()?, role });
            }
        }
        Ok(Ok(grants))
    }.await;
    finish_group(tx, result, false).await
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

#[allow(clippy::too_many_arguments)]
pub async fn add_group_to_project_backend(
    backend: &Backend,
    ws: Uuid,
    actor: Uuid,
    credential: Uuid,
    target: Uuid,
    group: Uuid,
    role: ProjectMemberRole,
) -> Result<Result<(), GroupDbError>, sqlx::Error> {
    if let Backend::Postgres(pool) = backend {
        return add_group_to_project(pool, ws, actor, credential, target, group, role).await;
    }
    let mut tx = backend.begin_write().await?;
    let result= async {
        let mut op = tx.operation();
        if let Err(e) = family_group_actor(&mut op, ws, actor, credential, false).await? {
            return Ok(Err(e));
        }
        if !(op.project_permission_by_id(ws, actor, target).await?.unwrap_or(ProjectPermission::None)).at_least(ProjectPermission::Manage) {
            return Ok(Err(GroupDbError::NotFound));
        }
        if !family_group_exists(&mut op, ws, group).await? {
            return Ok(Err(GroupDbError::NotFound));
        }
        let OperationTx::SqliteFamily(f) = &mut op else { unreachable!() };
        let n = f.execute("INSERT INTO project_members(id,workspace_id,project_id,group_id,role) VALUES(?4,?1,?2,?3,?5) ON CONFLICT DO NOTHING", &[Cell::uuid(ws), Cell::uuid(target), Cell::uuid(group), Cell::uuid(Uuid::now_v7()), Cell::text(role.as_str())]).await?;
        Ok(if n == 0 { Err(GroupDbError::Conflict) } else { Ok(()) })
    }.await;
    finish_group(tx, result, true).await
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

pub async fn remove_group_from_project_backend(
    backend: &Backend,
    ws: Uuid,
    actor: Uuid,
    credential: Uuid,
    target: Uuid,
    group: Uuid,
) -> Result<Result<(), GroupDbError>, sqlx::Error> {
    if let Backend::Postgres(pool) = backend {
        return remove_group_from_project(pool, ws, actor, credential, target, group).await;
    }
    let mut tx = backend.begin_write().await?;
    let result= async {
        let mut op = tx.operation();
        if let Err(e) = family_group_actor(&mut op, ws, actor, credential, false).await? {
            return Ok(Err(e));
        }
        if !(op.project_permission_by_id(ws, actor, target).await?.unwrap_or(ProjectPermission::None)).at_least(ProjectPermission::Manage) {
            return Ok(Err(GroupDbError::NotFound));
        }
        if !family_group_exists(&mut op, ws, group).await? {
            return Ok(Err(GroupDbError::NotFound));
        }
        if family_last_group_lead(&mut op, ws, group, Some(target)).await? {
            return Ok(Err(GroupDbError::LastLead));
        }
        let OperationTx::SqliteFamily(f) = &mut op else { unreachable!() };
        let n = f.execute("DELETE FROM project_members WHERE workspace_id=?1 AND project_id=?2 AND group_id=?3", &[Cell::uuid(ws), Cell::uuid(target), Cell::uuid(group)]).await?;
        Ok(if n == 0 { Err(GroupDbError::NotFound) } else { Ok(()) })
    }.await;
    finish_group(tx, result, true).await
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

pub async fn list_document_group_grants_backend(
    backend: &Backend,
    ws: Uuid,
    actor: Uuid,
    credential: Uuid,
    target: Uuid,
) -> Result<Result<Vec<GroupGrantRow>, GroupDbError>, sqlx::Error> {
    if let Backend::Postgres(pool) = backend {
        return list_document_group_grants(pool, ws, actor, credential, target).await;
    }
    let mut tx = backend.begin_read().await?;
    let result= async {
        let mut op = tx.operation();
        if let Err(e) = family_group_actor(&mut op, ws, actor, credential, false).await? {
            return Ok(Err(e));
        }
        if !(op.document_permission(ws, actor, target, true).await?).at_least(ProjectPermission::View) {
            return Ok(Err(GroupDbError::NotFound));
        }
        {
            let OperationTx::SqliteFamily(f) = &mut op else { unreachable!() };
            if f.query("SELECT 1 FROM documents WHERE workspace_id=?1 AND id=?2 AND project_id IS NULL AND deleted_at IS NULL", &[Cell::uuid(ws), Cell::uuid(target)]).await?.is_empty() {
                return Ok(Err(GroupDbError::NotFound));
            }
        }
        let OperationTx::SqliteFamily(f) = &mut op else { unreachable!() };
        let rows = f.query("SELECT group_id,role FROM document_members WHERE workspace_id=?1 AND document_id=?2 AND group_id IS NOT NULL ORDER BY group_id", &[Cell::uuid(ws), Cell::uuid(target)]).await?;
        let mut grants = Vec::new();
        for row in rows {
            if let Some(role) = ProjectMemberRole::parse(&row.cell(1)?.string()?) {
                grants.push(GroupGrantRow { group_id: row.cell(0)?.id()?, role });
            }
        }
        Ok(Ok(grants))
    }.await;
    finish_group(tx, result, false).await
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

#[allow(clippy::too_many_arguments)]
pub async fn add_group_to_document_backend(
    backend: &Backend,
    ws: Uuid,
    actor: Uuid,
    credential: Uuid,
    target: Uuid,
    group: Uuid,
    role: ProjectMemberRole,
) -> Result<Result<(), GroupDbError>, sqlx::Error> {
    if let Backend::Postgres(pool) = backend {
        return add_group_to_document(pool, ws, actor, credential, target, group, role).await;
    }
    let mut tx = backend.begin_write().await?;
    let result= async {
        let mut op = tx.operation();
        if let Err(e) = family_group_actor(&mut op, ws, actor, credential, false).await? {
            return Ok(Err(e));
        }
        if !(op.document_permission(ws, actor, target, true).await?).at_least(ProjectPermission::Manage) {
            return Ok(Err(GroupDbError::NotFound));
        }
        {
            let OperationTx::SqliteFamily(f) = &mut op else { unreachable!() };
            if f.query("SELECT 1 FROM documents WHERE workspace_id=?1 AND id=?2 AND project_id IS NULL AND deleted_at IS NULL", &[Cell::uuid(ws), Cell::uuid(target)]).await?.is_empty() {
                return Ok(Err(GroupDbError::NotFound));
            }
        }
        if !family_group_exists(&mut op, ws, group).await? {
            return Ok(Err(GroupDbError::NotFound));
        }
        let OperationTx::SqliteFamily(f) = &mut op else { unreachable!() };
        let n = f.execute("INSERT INTO document_members(id,workspace_id,document_id,group_id,role) VALUES(?4,?1,?2,?3,?5) ON CONFLICT DO NOTHING", &[Cell::uuid(ws), Cell::uuid(target), Cell::uuid(group), Cell::uuid(Uuid::now_v7()), Cell::text(role.as_str())]).await?;
        Ok(if n == 0 { Err(GroupDbError::Conflict) } else { Ok(()) })
    }.await;
    finish_group(tx, result, true).await
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
    lock_actor_and_group_members(&mut tx, workspace_id, actor_user_id, group_id).await?;
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

pub async fn remove_group_from_document_backend(
    backend: &Backend,
    ws: Uuid,
    actor: Uuid,
    credential: Uuid,
    target: Uuid,
    group: Uuid,
) -> Result<Result<(), GroupDbError>, sqlx::Error> {
    if let Backend::Postgres(pool) = backend {
        return remove_group_from_document(pool, ws, actor, credential, target, group).await;
    }
    let mut tx = backend.begin_write().await?;
    let result= async {
        let mut op = tx.operation();
        if let Err(e) = family_group_actor(&mut op, ws, actor, credential, false).await? {
            return Ok(Err(e));
        }
        if !(op.document_permission(ws, actor, target, true).await?).at_least(ProjectPermission::Manage) {
            return Ok(Err(GroupDbError::NotFound));
        }
        {
            let OperationTx::SqliteFamily(f) = &mut op else { unreachable!() };
            if f.query("SELECT 1 FROM documents WHERE workspace_id=?1 AND id=?2 AND project_id IS NULL AND deleted_at IS NULL", &[Cell::uuid(ws), Cell::uuid(target)]).await?.is_empty() {
                return Ok(Err(GroupDbError::NotFound));
            }
        }
        if !family_group_exists(&mut op, ws, group).await? {
            return Ok(Err(GroupDbError::NotFound));
        }
        let OperationTx::SqliteFamily(f) = &mut op else { unreachable!() };
        let n = f.execute("DELETE FROM document_members WHERE workspace_id=?1 AND document_id=?2 AND group_id=?3", &[Cell::uuid(ws), Cell::uuid(target), Cell::uuid(group)]).await?;
        Ok(if n == 0 { Err(GroupDbError::NotFound) } else { Ok(()) })
    }.await;
    finish_group(tx, result, true).await
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

#[cfg(test)]
mod selected_group_read_finish_tests {
    use super::*;
    #[test]
    fn group_read_cleanup_retains_refusal_and_driver_without_returning_rows() {
        for domain in [true, false] {
            let result = if domain {
                Ok(Err(GroupDbError::Forbidden))
            } else {
                Err(sqlx::Error::Protocol("original group SQL fault".into()))
            };
            let error = group_read_after_rollback(
                result,
                Err(sqlx::Error::Protocol(
                    "synthetic returned cleanup failure".into(),
                )),
            )
            .unwrap_err();
            let sqlx::Error::AnyDriverError(source) = error else {
                panic!("typed cleanup receipt required")
            };
            let receipt = source
                .downcast_ref::<crate::db::backend::RollbackCleanupUnknown>()
                .unwrap();
            let original = receipt.original.as_ref().unwrap();
            if domain {
                assert!(matches!(
                    original.downcast_ref::<GroupReadRefusal>().unwrap().0,
                    GroupDbError::Forbidden
                ));
            } else {
                assert!(
                    matches!(original.downcast_ref::<sqlx::Error>(),Some(sqlx::Error::Protocol(message)) if message=="original group SQL fault")
                );
            }
        }
        assert!(group_read_after_rollback(
            Ok(Ok(Vec::new())),
            Err(sqlx::Error::Protocol(
                "synthetic returned cleanup failure".into()
            ))
        )
        .is_err());
        // Propagation only: does not claim remote failure/settlement execution.
    }
}

async fn finish_group<T>(
    tx: DbTx,
    result: Result<Result<T, GroupDbError>, sqlx::Error>,
    write: bool,
) -> Result<Result<T, GroupDbError>, sqlx::Error> {
    if write && matches!(&result, Ok(Ok(_))) {
        tx.commit_with_cleanup()
            .await
            .map_err(|e| sqlx::Error::AnyDriverError(Box::new(e)))?;
    } else if let Err(cleanup) = tx.rollback().await {
        let original: Option<Box<dyn std::error::Error + Send + Sync>> = match result {
            Err(e) => Some(Box::new(e)),
            Ok(Err(e)) => Some(Box::new(GroupReadRefusal(e))),
            Ok(Ok(_)) => None,
        };
        return Err(crate::db::backend::rollback_cleanup_unknown(
            original, cleanup,
        ));
    }
    result
}
async fn family_group_actor(
    op: &mut OperationTx<'_, '_>,
    ws: Uuid,
    actor: Uuid,
    credential: Uuid,
    manage: bool,
) -> Result<Result<(), GroupDbError>, sqlx::Error> {
    op.set_tenant(ws).await?;
    if !op.session_is_live(actor, credential).await? {
        return Ok(Err(GroupDbError::Forbidden));
    }
    if !op.workspace_is_live(ws).await? {
        return Ok(Err(GroupDbError::NotFound));
    }
    match op.membership_role(ws, actor, manage).await? {
        Some(role) if !manage || has_workspace_manage(role) => Ok(Ok(())),
        _ => Ok(Err(GroupDbError::NotFound)),
    }
}
async fn family_group_exists(
    op: &mut OperationTx<'_, '_>,
    ws: Uuid,
    group: Uuid,
) -> Result<bool, sqlx::Error> {
    let OperationTx::SqliteFamily(tx) = op else {
        unreachable!()
    };
    tx.require_tenant(ws)?;
    Ok(!tx
        .query(
            "SELECT 1 FROM groups WHERE workspace_id=?1 AND id=?2",
            &[Cell::uuid(ws), Cell::uuid(group)],
        )
        .await?
        .is_empty())
}
fn family_group_row(r: &FamilyRow) -> Result<GroupRow, sqlx::Error> {
    Ok(GroupRow {
        id: r.cell(0)?.id()?,
        workspace_id: r.cell(1)?.id()?,
        name: r.cell(2)?.string()?,
        created_at: r.cell(3)?.datetime()?,
        updated_at: r.cell(4)?.datetime()?,
    })
}
async fn family_last_group_lead(
    op: &mut OperationTx<'_, '_>,
    ws: Uuid,
    group: Uuid,
    project: Option<Uuid>,
) -> Result<bool, sqlx::Error> {
    let OperationTx::SqliteFamily(tx) = op else {
        unreachable!()
    };
    tx.require_writer()?;
    let rows=tx.query("SELECT 1 FROM projects p JOIN project_members pm ON pm.workspace_id=p.workspace_id AND pm.project_id=p.id WHERE p.workspace_id=?1 AND p.deleted_at IS NULL AND p.visibility='private' AND pm.group_id=?2 AND pm.role='lead' AND (?3 IS NULL OR p.id=?3) AND NOT EXISTS(SELECT 1 FROM project_members other WHERE other.workspace_id=p.workspace_id AND other.project_id=p.id AND other.role='lead' AND (other.group_id IS NULL OR other.group_id<>?2)) LIMIT 1", &[Cell::uuid(ws),Cell::uuid(group),Cell::optional_uuid(project)]).await?;
    Ok(!rows.is_empty())
}

#[cfg(all(test, feature = "db-tests"))]
mod remote_membership_port_tests {
    use super::*;
    use crate::db::backend::{DbTransaction, RemoteDatabase};
    use futures_util::FutureExt;

    #[tokio::test]
    async fn official_sqld_group_invitation_share_and_star_commit_readback() {
        let sqld = std::path::PathBuf::from(
            std::env::var_os("FVOCI_TEST_SQLD").expect("explicit official sqld fixture required"),
        );
        let root = std::env::temp_dir().join(format!("fvoci-remote-membership-{}", Uuid::now_v7()));
        let driver = crate::db::libsql_finish_fixture::LibsqlFinishFixture::start(&sqld, &root)
            .await
            .unwrap();
        let backend = Backend::LibsqlRemote(std::sync::Arc::new(RemoteDatabase::from_test_driver(
            driver.database().await.unwrap(),
            std::num::NonZeroU32::new(2).unwrap(),
        )));
        let result = std::panic::AssertUnwindSafe(async {
            crate::db::migrate::initialize_family_backend_for_test(&backend).await.unwrap();
            let ws=Uuid::now_v7();let actor=Uuid::now_v7();let credential=Uuid::now_v7();let doc=Uuid::now_v7();
            let mut tx=backend.begin_write().await.unwrap();
            let DbTransaction::SqliteFamily(f)=&mut tx else{unreachable!()};f.replace_system_context(true);
            f.execute("INSERT INTO users(id,email,given_name) VALUES(?1,'owner@example.invalid','Owner')",&[Cell::uuid(actor)]).await.unwrap();
            f.execute("INSERT INTO workspaces(id,slug,name) VALUES(?1,'remote-membership','Remote')",&[Cell::uuid(ws)]).await.unwrap();
            f.execute("INSERT INTO memberships(workspace_id,user_id,role) VALUES(?1,?2,'owner')",&[Cell::uuid(ws),Cell::uuid(actor)]).await.unwrap();
            f.execute("INSERT INTO sessions(id,user_id,token_hash,expires_at) VALUES(?1,?2,'synthetic-remote-credential',(unixepoch()+3600)*1000000)",&[Cell::uuid(credential),Cell::uuid(actor)]).await.unwrap();
            f.execute("INSERT INTO documents(id,workspace_id,title,path,sort_key,number,status,schema_version,created_by,content_json) VALUES(?1,?2,'Remote',?3,'a',1,'published',1,?4,'{\"type\":\"doc\",\"content\":[]}')",&[Cell::uuid(doc),Cell::uuid(ws),Cell::text(doc.simple().to_string()),Cell::uuid(actor)]).await.unwrap();
            tx.commit_with_cleanup().await.unwrap();
            let group=create_group_backend(&backend,ws,actor,credential,"Remote group").await.unwrap().unwrap();
            assert_eq!(list_groups_backend(&backend,ws,actor,credential).await.unwrap().unwrap()[0].id,group.id);
            let invite=crate::db::invitations::create_invitation_backend(&backend,ws,actor,credential,"invited@example.invalid",WorkspaceRole::Member,None).await.unwrap().unwrap();
            assert_eq!(crate::db::invitations::get_invitation_public_backend(&backend,&invite.accept_path_token).await.unwrap().unwrap().workspace_name,"Remote");
            let link=crate::db::share::create_share_link_backend(&backend,ws,actor,credential,crate::db::share::ShareTarget::Document(doc),Some(1),&crate::settings::catalog::SettingsValues::defaults("FVOCI").share,None).await.unwrap().unwrap();
            assert_eq!(crate::db::share::share_document_backend(&backend,&link.token,None).await.unwrap().unwrap().id,doc);
            let star=crate::db::stars::add_star_backend(&backend,ws,actor,credential,crate::db::stars::StarTarget::Document(doc),None).await.unwrap().unwrap();
            assert_eq!(crate::db::stars::list_stars_backend(&backend,ws,actor,credential,None).await.unwrap().unwrap()[0].id,star.id);
            crate::db::share::revoke_share_link_backend(&backend,ws,actor,credential,link.record.id).await.unwrap().unwrap();
            assert!(crate::db::share::share_document_backend(&backend,&link.token,None).await.unwrap().is_none());
            crate::db::stars::remove_star_backend(&backend,ws,actor,credential,star.id,None).await.unwrap().unwrap();
            purge_group_backend(&backend,ws,actor,credential,group.id).await.unwrap().unwrap();
        }).catch_unwind().await;
        let close = backend.close().await;
        let finish = driver.finish().await;
        assert!(close.is_ok());
        let receipt = finish.unwrap();
        assert!(receipt.sqld_reaped && receipt.proxy_joined);
        std::fs::remove_dir_all(root).unwrap();
        if let Err(panic) = result {
            std::panic::resume_unwind(panic);
        }
    }
}
