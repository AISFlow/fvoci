use chrono::{DateTime, Utc};
use sqlx::{PgPool, Postgres, Transaction};
use uuid::Uuid;

use crate::db::context::{begin_read, session_is_live, set_tenant};
use crate::db::projects::{
    require_project_edit, require_project_view, visible_project_sql, ProjectDbError,
};
use crate::db::workspace::{membership_role, workspace_is_live, WorkspaceRole};

pub const LABEL_NAME_MAX: usize = 100;
pub const LABEL_COLORS: &[&str] = &[
    "gray", "red", "orange", "amber", "green", "teal", "blue", "violet", "pink",
];

#[derive(Debug, Clone)]
pub struct LabelRow {
    pub id: Uuid,
    pub project_id: Uuid,
    pub name: String,
    pub color: String,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

pub fn label_name_is_valid(name: &str) -> bool {
    let trimmed = name.trim();
    !trimmed.is_empty() && trimmed.chars().count() <= LABEL_NAME_MAX
}

pub fn label_color_is_valid(color: &str) -> bool {
    LABEL_COLORS.contains(&color)
}

fn map_label_row(
    id: Uuid,
    project_id: Uuid,
    name: String,
    color: String,
    created_at: DateTime<Utc>,
    updated_at: DateTime<Utc>,
) -> LabelRow {
    LabelRow {
        id,
        project_id,
        name,
        color,
        created_at,
        updated_at,
    }
}

/// Selected project read: the credential, current grants and returned rows
/// share one snapshot. The public PostgreSQL reader remains unchanged.
pub async fn list_project_labels_backend(
    backend: &crate::db::backend::Backend,
    workspace: Uuid,
    project: Uuid,
    actor: Uuid,
    credential: Uuid,
) -> Result<Result<Vec<LabelRow>, ProjectDbError>, sqlx::Error> {
    use crate::db::backend::{Backend, OperationTx};
    use crate::db::codec::Cell;
    use crate::projects::ProjectPermission;

    if let Backend::Postgres(pool) = backend {
        return list_project_labels(pool, workspace, project, actor, credential).await;
    }
    let mut tx = backend.begin_read().await?;
    let result: Result<Result<Vec<LabelRow>, ProjectDbError>, sqlx::Error> = async {
        let mut op = tx.operation();
        op.set_tenant(workspace).await?;
        if !op.session_is_live(actor, credential).await? {
            return Ok(Err(ProjectDbError::Forbidden));
        }
        if !op.workspace_is_live(workspace).await?
            || !op.project_permission_by_id(workspace, actor, project).await?
                .is_some_and(|permission| permission.at_least(ProjectPermission::View))
        {
            return Ok(Err(ProjectDbError::NotFound));
        }
        let OperationTx::SqliteFamily(family) = op else {
            unreachable!("PostgreSQL uses the preserved project reader")
        };
        family.require_tenant(workspace)?;
        let rows = family.query(
            "SELECT id,project_id,name,color,created_at,updated_at FROM labels WHERE workspace_id=?1 AND project_id=?2 ORDER BY name,id",
            &[Cell::uuid(workspace),Cell::uuid(project)],
        ).await?;
        Ok(Ok(rows.iter().map(|row| Ok(map_label_row(row.cell(0)?.id()?,row.cell(1)?.id()?,row.cell(2)?.string()?,row.cell(3)?.string()?,row.cell(4)?.datetime()?,row.cell(5)?.datetime()?)))
            .collect::<Result<Vec<_>,sqlx::Error>>()?))
    }.await;
    // A failed release is not a successful read. Preserve both the original
    // domain/driver refusal and the failed cleanup when there is one.
    if let Err(cleanup) = tx.rollback().await {
        let original: Option<Box<dyn std::error::Error + Send + Sync>> = match result {
            Ok(Err(refusal)) => Some(Box::new(ProjectLabelsReadRefusal(refusal))),
            Err(error) => Some(Box::new(error)),
            Ok(Ok(_)) => None,
        };
        return Err(crate::db::backend::rollback_cleanup_unknown(
            original, cleanup,
        ));
    }
    result
}

#[derive(Debug, thiserror::Error)]
#[error("project labels read refused: {0:?}")]
struct ProjectLabelsReadRefusal(ProjectDbError);

pub async fn list_project_labels(
    pool: &PgPool,
    workspace_id: Uuid,
    project_id: Uuid,
    actor_user_id: Uuid,
    session_id: Uuid,
) -> Result<Result<Vec<LabelRow>, ProjectDbError>, sqlx::Error> {
    let mut tx = begin_read(pool).await?;
    set_tenant(&mut tx, workspace_id).await?;
    match require_project_view(&mut tx, workspace_id, actor_user_id, session_id, project_id).await?
    {
        Ok(()) => {}
        Err(err) => {
            tx.rollback().await?;
            return Ok(Err(err));
        }
    }
    let rows = sqlx::query_as::<_, (Uuid, Uuid, String, String, DateTime<Utc>, DateTime<Utc>)>(
        r#"
        SELECT id, project_id, name, color, created_at, updated_at
        FROM fvoci.labels
        WHERE workspace_id = $1 AND project_id = $2
        ORDER BY name, id
        "#,
    )
    .bind(workspace_id)
    .bind(project_id)
    .fetch_all(&mut *tx)
    .await?;
    tx.commit().await?;
    Ok(Ok(rows
        .into_iter()
        .map(|(id, project_id, name, color, created_at, updated_at)| {
            map_label_row(id, project_id, name, color, created_at, updated_at)
        })
        .collect()))
}

pub async fn list_workspace_labels(
    pool: &PgPool,
    workspace_id: Uuid,
    actor_user_id: Uuid,
    session_id: Uuid,
) -> Result<Result<Vec<LabelRow>, ProjectDbError>, sqlx::Error> {
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
    let Some(role) = membership_role(&mut tx, workspace_id, actor_user_id).await? else {
        tx.rollback().await?;
        return Ok(Err(ProjectDbError::NotFound));
    };
    let guest = role == WorkspaceRole::Guest;
    let visible = visible_project_sql("p", 2, 3);
    let sql = format!(
        r#"
        SELECT l.id, l.project_id, l.name, l.color, l.created_at, l.updated_at
        FROM fvoci.labels l
        INNER JOIN fvoci.projects p
            ON p.workspace_id = l.workspace_id AND p.id = l.project_id
        WHERE l.workspace_id = $1
          AND p.deleted_at IS NULL
          AND {visible}
        ORDER BY l.name, l.id
        "#
    );
    let rows =
        sqlx::query_as::<_, (Uuid, Uuid, String, String, DateTime<Utc>, DateTime<Utc>)>(&sql)
            .bind(workspace_id)
            .bind(guest)
            .bind(actor_user_id)
            .fetch_all(&mut *tx)
            .await?;
    tx.commit().await?;
    Ok(Ok(rows
        .into_iter()
        .map(|(id, project_id, name, color, created_at, updated_at)| {
            map_label_row(id, project_id, name, color, created_at, updated_at)
        })
        .collect()))
}

pub async fn create_label(
    pool: &PgPool,
    workspace_id: Uuid,
    project_id: Uuid,
    actor_user_id: Uuid,
    session_id: Uuid,
    name: &str,
    color: &str,
) -> Result<Result<LabelRow, ProjectDbError>, sqlx::Error> {
    let name = name.trim();
    if !label_name_is_valid(name) || !label_color_is_valid(color) {
        return Ok(Err(ProjectDbError::InvalidInput));
    }
    let mut tx = pool.begin().await?;
    set_tenant(&mut tx, workspace_id).await?;
    match require_project_edit(&mut tx, workspace_id, actor_user_id, session_id, project_id).await?
    {
        Ok(()) => {}
        Err(err) => {
            tx.rollback().await?;
            return Ok(Err(err));
        }
    }
    let id = Uuid::now_v7();
    let row = sqlx::query_as::<_, (Uuid, Uuid, String, String, DateTime<Utc>, DateTime<Utc>)>(
        r#"
        INSERT INTO fvoci.labels (id, workspace_id, project_id, name, color)
        VALUES ($1, $2, $3, $4, $5)
        RETURNING id, project_id, name, color, created_at, updated_at
        "#,
    )
    .bind(id)
    .bind(workspace_id)
    .bind(project_id)
    .bind(name)
    .bind(color)
    .fetch_one(&mut *tx)
    .await?;
    tx.commit().await?;
    Ok(Ok(map_label_row(row.0, row.1, row.2, row.3, row.4, row.5)))
}

#[allow(clippy::too_many_arguments)]
pub async fn update_label(
    pool: &PgPool,
    workspace_id: Uuid,
    project_id: Uuid,
    label_id: Uuid,
    actor_user_id: Uuid,
    session_id: Uuid,
    name: Option<&str>,
    color: Option<&str>,
) -> Result<Result<(), ProjectDbError>, sqlx::Error> {
    if name.is_none() && color.is_none() {
        return Ok(Err(ProjectDbError::InvalidInput));
    }
    let trimmed = name.map(str::trim);
    if let Some(name) = trimmed {
        if !label_name_is_valid(name) {
            return Ok(Err(ProjectDbError::InvalidInput));
        }
    }
    if let Some(color) = color {
        if !label_color_is_valid(color) {
            return Ok(Err(ProjectDbError::InvalidInput));
        }
    }
    let mut tx = pool.begin().await?;
    set_tenant(&mut tx, workspace_id).await?;
    match require_project_edit(&mut tx, workspace_id, actor_user_id, session_id, project_id).await?
    {
        Ok(()) => {}
        Err(err) => {
            tx.rollback().await?;
            return Ok(Err(err));
        }
    }
    let exists: Option<(Uuid,)> = sqlx::query_as(
        "SELECT id FROM fvoci.labels WHERE workspace_id = $1 AND project_id = $2 AND id = $3",
    )
    .bind(workspace_id)
    .bind(project_id)
    .bind(label_id)
    .fetch_optional(&mut *tx)
    .await?;
    if exists.is_none() {
        tx.rollback().await?;
        return Ok(Err(ProjectDbError::LabelNotFound));
    }
    sqlx::query(
        r#"
        UPDATE fvoci.labels
        SET name = COALESCE($4, name),
            color = COALESCE($5, color),
            updated_at = now()
        WHERE workspace_id = $1 AND project_id = $2 AND id = $3
        "#,
    )
    .bind(workspace_id)
    .bind(project_id)
    .bind(label_id)
    .bind(trimmed)
    .bind(color)
    .execute(&mut *tx)
    .await?;
    tx.commit().await?;
    Ok(Ok(()))
}

pub async fn purge_label(
    pool: &PgPool,
    workspace_id: Uuid,
    project_id: Uuid,
    label_id: Uuid,
    actor_user_id: Uuid,
    session_id: Uuid,
) -> Result<Result<(), ProjectDbError>, sqlx::Error> {
    let mut tx = pool.begin().await?;
    set_tenant(&mut tx, workspace_id).await?;
    match require_project_edit(&mut tx, workspace_id, actor_user_id, session_id, project_id).await?
    {
        Ok(()) => {}
        Err(err) => {
            tx.rollback().await?;
            return Ok(Err(err));
        }
    }
    let deleted: Option<(Uuid,)> = sqlx::query_as(
        r#"
        DELETE FROM fvoci.labels
        WHERE workspace_id = $1 AND project_id = $2 AND id = $3
        RETURNING id
        "#,
    )
    .bind(workspace_id)
    .bind(project_id)
    .bind(label_id)
    .fetch_optional(&mut *tx)
    .await?;
    if deleted.is_none() {
        tx.rollback().await?;
        return Ok(Err(ProjectDbError::LabelNotFound));
    }
    tx.commit().await?;
    Ok(Ok(()))
}

/// Task list `labelId` filter: the label must belong to the listed project (the
/// caller has already been checked for view access to that project).
pub async fn project_label_exists(
    tx: &mut Transaction<'_, Postgres>,
    workspace_id: Uuid,
    project_id: Uuid,
    label_id: Uuid,
) -> Result<bool, sqlx::Error> {
    let exists: (bool,) = sqlx::query_as(
        r#"
        SELECT EXISTS (
            SELECT 1 FROM fvoci.labels
            WHERE workspace_id = $1 AND project_id = $2 AND id = $3
        )
        "#,
    )
    .bind(workspace_id)
    .bind(project_id)
    .bind(label_id)
    .fetch_one(&mut **tx)
    .await?;
    Ok(exists.0)
}

pub async fn assignee_filter_member_exists(
    tx: &mut Transaction<'_, Postgres>,
    workspace_id: Uuid,
    user_id: Uuid,
) -> Result<bool, sqlx::Error> {
    let exists: (bool,) = sqlx::query_as(
        r#"
        SELECT EXISTS (
            SELECT 1
            FROM fvoci.memberships m
            INNER JOIN fvoci.users u ON u.id = m.user_id
            WHERE m.workspace_id = $1
              AND u.id = $2
              AND u.deleted_at IS NULL
        )
        "#,
    )
    .bind(workspace_id)
    .bind(user_id)
    .fetch_one(&mut **tx)
    .await?;
    Ok(exists.0)
}

#[cfg(test)]
mod selected_project_read_tests {
    use super::*;
    use crate::db::attachment_preview::tests::Fixture;

    #[tokio::test]
    async fn selected_labels_read_nonempty_order_current_grants_and_credential_refusals() {
        let f = Fixture::new().await;
        let (_, task) = f.task_attachment().await;
        let project: Vec<u8> = sqlx::query_scalar("SELECT project_id FROM tasks WHERE id=?1")
            .bind(task.as_bytes().as_slice())
            .fetch_one(&f.pool)
            .await
            .unwrap();
        let project = Uuid::from_slice(&project).unwrap();
        let credential = Uuid::now_v7();
        sqlx::query("INSERT INTO sessions(id,user_id,token_hash,expires_at) VALUES(?1,?2,?3,9223372036854775807)")
            .bind(credential.as_bytes().as_slice()).bind(f.user.as_bytes().as_slice())
            .bind(credential.to_string()).execute(&f.pool).await.unwrap();
        let first = Uuid::from_u128(100);
        let last = Uuid::from_u128(200);
        // Reverse insertion, equal primary sort value: id is the stable tie.
        for id in [last, first] {
            sqlx::query("INSERT INTO labels(id,workspace_id,project_id,name,color,created_at,updated_at) VALUES(?1,?2,?3,'same 😀','violet',1760000000000000,1760000000000001)")
                .bind(id.as_bytes().as_slice()).bind(f.workspace.as_bytes().as_slice())
                .bind(project.as_bytes().as_slice()).execute(&f.pool).await.unwrap();
        }
        let earlier = Uuid::from_u128(300);
        sqlx::query("INSERT INTO labels(id,workspace_id,project_id,name,color) VALUES(?1,?2,?3,'Alpha','red')")
            .bind(earlier.as_bytes().as_slice()).bind(f.workspace.as_bytes().as_slice())
            .bind(project.as_bytes().as_slice()).execute(&f.pool).await.unwrap();
        let other_project = Uuid::now_v7();
        sqlx::query("INSERT INTO projects(id,workspace_id,key,name,visibility,created_by) VALUES(?1,?2,'OTHER','Other','workspace',?3)")
            .bind(other_project.as_bytes().as_slice()).bind(f.workspace.as_bytes().as_slice())
            .bind(f.user.as_bytes().as_slice()).execute(&f.pool).await.unwrap();
        sqlx::query("INSERT INTO labels(id,workspace_id,project_id,name,color) VALUES(?1,?2,?3,'FOREIGN','gray')")
            .bind(Uuid::from_u128(1).as_bytes().as_slice()).bind(f.workspace.as_bytes().as_slice())
            .bind(other_project.as_bytes().as_slice()).execute(&f.pool).await.unwrap();
        let rows =
            list_project_labels_backend(&f.backend, f.workspace, project, f.user, credential)
                .await
                .unwrap()
                .unwrap();
        assert_eq!(
            rows.iter().map(|r| r.id).collect::<Vec<_>>(),
            vec![earlier, first, last]
        );
        assert_eq!(rows[1].project_id, project);
        assert_eq!(rows[1].name, "same 😀");
        assert_eq!(rows[1].created_at.timestamp_micros(), 1760000000000000);
        assert_eq!(rows[1].updated_at.timestamp_micros(), 1760000000000001);
        assert_eq!(rows[1].color, "violet");
        // Wrong tenant/target cannot disclose the populated rows.
        assert!(matches!(
            list_project_labels_backend(&f.backend, Uuid::now_v7(), project, f.user, credential)
                .await
                .unwrap(),
            Err(ProjectDbError::NotFound)
        ));
        assert!(matches!(
            list_project_labels_backend(
                &f.backend,
                f.workspace,
                Uuid::now_v7(),
                f.user,
                credential
            )
            .await
            .unwrap(),
            Err(ProjectDbError::NotFound)
        ));
        assert!(matches!(
            list_project_labels_backend(&f.backend, f.workspace, project, f.user, Uuid::now_v7())
                .await
                .unwrap(),
            Err(ProjectDbError::Forbidden)
        ));
        // A private project has no implicit owner/admin grant.
        sqlx::query("UPDATE projects SET visibility='private' WHERE id=?1")
            .bind(project.as_bytes().as_slice())
            .execute(&f.pool)
            .await
            .unwrap();
        assert!(matches!(
            list_project_labels_backend(&f.backend, f.workspace, project, f.user, credential)
                .await
                .unwrap(),
            Err(ProjectDbError::NotFound)
        ));
        let group = Uuid::now_v7();
        sqlx::query("INSERT INTO groups(id,workspace_id,name) VALUES(?1,?2,'readers')")
            .bind(group.as_bytes().as_slice())
            .bind(f.workspace.as_bytes().as_slice())
            .execute(&f.pool)
            .await
            .unwrap();
        sqlx::query("INSERT INTO group_members(workspace_id,group_id,user_id) VALUES(?1,?2,?3)")
            .bind(f.workspace.as_bytes().as_slice())
            .bind(group.as_bytes().as_slice())
            .bind(f.user.as_bytes().as_slice())
            .execute(&f.pool)
            .await
            .unwrap();
        sqlx::query("INSERT INTO project_members(id,workspace_id,project_id,group_id,role) VALUES(?1,?2,?3,?4,'viewer')")
            .bind(Uuid::now_v7().as_bytes().as_slice()).bind(f.workspace.as_bytes().as_slice())
            .bind(project.as_bytes().as_slice()).bind(group.as_bytes().as_slice()).execute(&f.pool).await.unwrap();
        assert_eq!(
            list_project_labels_backend(&f.backend, f.workspace, project, f.user, credential)
                .await
                .unwrap()
                .unwrap()
                .len(),
            3
        );
        sqlx::query("DELETE FROM group_members WHERE group_id=?1")
            .bind(group.as_bytes().as_slice())
            .execute(&f.pool)
            .await
            .unwrap();
        assert!(matches!(
            list_project_labels_backend(&f.backend, f.workspace, project, f.user, credential)
                .await
                .unwrap(),
            Err(ProjectDbError::NotFound)
        ));
        // Restore ordinary access, then prove each current revocation independently.
        sqlx::query("UPDATE projects SET visibility='workspace' WHERE id=?1")
            .bind(project.as_bytes().as_slice())
            .execute(&f.pool)
            .await
            .unwrap();
        sqlx::query("UPDATE projects SET deleted_at=1 WHERE id=?1")
            .bind(project.as_bytes().as_slice())
            .execute(&f.pool)
            .await
            .unwrap();
        assert!(matches!(
            list_project_labels_backend(&f.backend, f.workspace, project, f.user, credential)
                .await
                .unwrap(),
            Err(ProjectDbError::NotFound)
        ));
        sqlx::query("UPDATE projects SET deleted_at=NULL WHERE id=?1")
            .bind(project.as_bytes().as_slice())
            .execute(&f.pool)
            .await
            .unwrap();
        sqlx::query("UPDATE memberships SET role='guest' WHERE workspace_id=?1 AND user_id=?2")
            .bind(f.workspace.as_bytes().as_slice())
            .bind(f.user.as_bytes().as_slice())
            .execute(&f.pool)
            .await
            .unwrap();
        assert!(matches!(
            list_project_labels_backend(&f.backend, f.workspace, project, f.user, credential)
                .await
                .unwrap(),
            Err(ProjectDbError::NotFound)
        ));
        sqlx::query("UPDATE memberships SET role='owner' WHERE workspace_id=?1 AND user_id=?2")
            .bind(f.workspace.as_bytes().as_slice())
            .bind(f.user.as_bytes().as_slice())
            .execute(&f.pool)
            .await
            .unwrap();
        sqlx::query("UPDATE sessions SET revoked_at=1 WHERE id=?1")
            .bind(credential.as_bytes().as_slice())
            .execute(&f.pool)
            .await
            .unwrap();
        assert!(matches!(
            list_project_labels_backend(&f.backend, f.workspace, project, f.user, credential)
                .await
                .unwrap(),
            Err(ProjectDbError::Forbidden)
        ));
        sqlx::query("UPDATE sessions SET revoked_at=NULL WHERE id=?1")
            .bind(credential.as_bytes().as_slice())
            .execute(&f.pool)
            .await
            .unwrap();
        sqlx::query("UPDATE users SET suspended_at=1 WHERE id=?1")
            .bind(f.user.as_bytes().as_slice())
            .execute(&f.pool)
            .await
            .unwrap();
        assert!(matches!(
            list_project_labels_backend(&f.backend, f.workspace, project, f.user, credential)
                .await
                .unwrap(),
            Err(ProjectDbError::Forbidden)
        ));
        sqlx::query("UPDATE users SET suspended_at=NULL WHERE id=?1")
            .bind(f.user.as_bytes().as_slice())
            .execute(&f.pool)
            .await
            .unwrap();
        sqlx::query("UPDATE sessions SET expires_at=1 WHERE id=?1")
            .bind(credential.as_bytes().as_slice())
            .execute(&f.pool)
            .await
            .unwrap();
        assert!(matches!(
            list_project_labels_backend(&f.backend, f.workspace, project, f.user, credential)
                .await
                .unwrap(),
            Err(ProjectDbError::Forbidden)
        ));
        sqlx::query("UPDATE sessions SET expires_at=9223372036854775807 WHERE id=?1")
            .bind(credential.as_bytes().as_slice())
            .execute(&f.pool)
            .await
            .unwrap();
        sqlx::query("UPDATE workspaces SET deleted_at=1 WHERE id=?1")
            .bind(f.workspace.as_bytes().as_slice())
            .execute(&f.pool)
            .await
            .unwrap();
        assert!(matches!(
            list_project_labels_backend(&f.backend, f.workspace, project, f.user, credential)
                .await
                .unwrap(),
            Err(ProjectDbError::NotFound)
        ));
        sqlx::query("UPDATE workspaces SET deleted_at=NULL WHERE id=?1")
            .bind(f.workspace.as_bytes().as_slice())
            .execute(&f.pool)
            .await
            .unwrap();
        assert!(matches!(
            list_project_labels_backend(
                &f.backend,
                f.workspace,
                project,
                Uuid::now_v7(),
                credential
            )
            .await
            .unwrap(),
            Err(ProjectDbError::Forbidden)
        ));
        // Denials do not poison the pool or edit business state.
        assert_eq!(
            list_project_labels_backend(&f.backend, f.workspace, project, f.user, credential)
                .await
                .unwrap()
                .unwrap()
                .len(),
            3
        );
        let effects:(i64,i64,i64)=sqlx::query_as("SELECT (SELECT count(*) FROM labels),(SELECT count(*) FROM events),(SELECT count(*) FROM audit_log)")
            .fetch_one(&f.pool).await.unwrap();
        assert_eq!(effects, (4, 0, 0));
        f.close().await;
    }
}
