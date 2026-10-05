//! Tasks created from a document block (source `packages/core/src/task-origin.ts`,
//! `apps/server/src/domains/documents/task-origins.ts`).
//!
//! `POST documents/{id}/tasks` creates the task and its `task_origins` row in one
//! transaction. The actor membership/session fence and tenant tree lock precede
//! the per-document command lock. Source/target project rows are locked in UUID
//! order, followed by the source document row; authorization is rechecked at
//! that serialization point for fresh creates AND replay. The unique command
//! key decides replay and the payload hash preserves old omitted/false intent.

use sha2::{Digest, Sha256};
use sqlx::{PgPool, Postgres, Transaction};
use uuid::Uuid;

use crate::db::backend::{Backend, DbTx, FamilyTx, OperationTx};
use crate::db::codec::Cell;

use crate::db::context::{
    lock_key_from_uuid, lock_membership_users, lock_tree, recheck_session, session_is_live,
    set_tenant,
};
use crate::db::documents::document_permission;
use crate::db::group_grants::group_members_join_sql;
use crate::db::projects::{
    lock_project, project_permission, project_permission_by_id, visible_project_sql, ProjectDbError,
};
use crate::db::tasks::{create_task_tx, replace_task_assignees, CreateTaskInput, OriginTaskCreate};
use crate::db::workspace::{membership_role, workspace_is_live, WorkspaceRole};
use crate::projects::ProjectPermission;

/// Serialises origin creation per source document (see module docs).
pub const TASK_ORIGIN_LOCK_NAMESPACE: i32 = 1_907_021;
pub const TASK_ORIGIN_ANCHOR_MAX_CHARS: usize = 200;

#[derive(Debug)]
pub enum TaskOriginDbError {
    NotFound,
    Forbidden,
    /// Same `requestId` with a different request (409 `document_version_mismatch`).
    RequestMismatch,
    /// Task creation refused (archived project, invalid status, …).
    Task(ProjectDbError),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DocumentTaskOutcome {
    Created(Uuid),
    Replayed(Uuid),
}

impl DocumentTaskOutcome {
    pub fn task_id(&self) -> Uuid {
        match self {
            Self::Created(id) | Self::Replayed(id) => *id,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TaskOriginItem {
    pub task_id: Uuid,
    pub document_id: Uuid,
    pub task_display_id: String,
    pub document_display_id: String,
    pub task_title: String,
    pub document_title: String,
    pub anchor: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TaskOriginPage {
    pub items: Vec<TaskOriginItem>,
    pub count: usize,
    pub next_cursor: Option<Uuid>,
}

pub struct TaskProject {
    pub id: Uuid,
    pub name: String,
    pub key: String,
    pub visibility: String,
}

pub struct TaskProjectPicker {
    pub items: Vec<TaskProject>,
    pub suggested_id: Option<Uuid>,
    pub can_create_project: bool,
}

#[derive(Debug, thiserror::Error)]
#[error("task origin operation refused: {0:?}")]
struct OriginOperationRefusal(TaskOriginDbError);

async fn finish_origin_operation<T>(
    tx: DbTx,
    result: Result<Result<T, TaskOriginDbError>, sqlx::Error>,
) -> Result<Result<T, TaskOriginDbError>, sqlx::Error> {
    match result {
        Ok(Ok(value)) => {
            tx.commit_with_cleanup()
                .await
                .map_err(|error| sqlx::Error::AnyDriverError(Box::new(error)))?;
            Ok(Ok(value))
        }
        Ok(Err(refusal)) => {
            if let Err(cleanup) = tx.rollback().await {
                return Err(crate::db::backend::rollback_cleanup_unknown(
                    Some(Box::new(OriginOperationRefusal(refusal))),
                    cleanup,
                ));
            }
            Ok(Err(refusal))
        }
        Err(original) => {
            if let Err(cleanup) = tx.rollback().await {
                return Err(crate::db::backend::rollback_cleanup_unknown(
                    Some(Box::new(original)),
                    cleanup,
                ));
            }
            Err(original)
        }
    }
}

async fn origin_read_source(
    op: &mut OperationTx<'_, '_>,
    workspace: Uuid,
    actor: Uuid,
    credential: Uuid,
    document: Uuid,
) -> Result<Option<Option<Uuid>>, sqlx::Error> {
    op.set_tenant(workspace).await?;
    if !op.session_is_live(actor, credential).await? || !op.workspace_is_live(workspace).await? {
        return Ok(None);
    }
    let Some(row) = op.document_row(workspace, document).await? else {
        return Ok(None);
    };
    // DocumentRow stores project_id in its ninth tuple field.
    let source_project = row.8;
    let permission = if let Some(project) = source_project {
        op.project_permission_by_id(workspace, actor, project)
            .await?
            .unwrap_or(ProjectPermission::None)
    } else {
        op.document_permission(workspace, actor, document, true)
            .await?
    };
    Ok(permission
        .at_least(ProjectPermission::View)
        .then_some(source_project))
}

/// Current-Edit picker; both policy and result are read in one selected snapshot.
pub async fn task_projects_backend(
    backend: &Backend,
    workspace: Uuid,
    actor: Uuid,
    credential: Uuid,
    document: Uuid,
) -> Result<Result<TaskProjectPicker, TaskOriginDbError>, sqlx::Error> {
    if let Backend::Postgres(pool) = backend {
        return task_projects(pool, workspace, actor, credential, document).await;
    }
    let mut tx = backend.begin_read().await?;
    let result = async {
        let mut op = tx.operation();
        let Some(source_project) = origin_read_source(&mut op, workspace, actor, credential, document).await? else { return Ok(Err(TaskOriginDbError::NotFound)); };
        let role = op.membership_role(workspace, actor, false).await?;
        let guest = role.unwrap_or(WorkspaceRole::Guest) == WorkspaceRole::Guest;
        let OperationTx::SqliteFamily(family) = op else { unreachable!() };
        family.require_tenant(workspace)?;
        let rows = family.query(
            "SELECT p.id,p.name,p.key,p.visibility FROM projects p
             WHERE p.workspace_id=?1 AND p.deleted_at IS NULL AND p.status<>'archived'
             AND ((p.visibility='workspace' AND ?2=0)
              OR EXISTS(SELECT 1 FROM project_members pm WHERE pm.workspace_id=p.workspace_id AND pm.project_id=p.id AND pm.user_id=?3 AND pm.role IN ('member','lead'))
              OR EXISTS(SELECT 1 FROM project_members pm JOIN group_members gm ON gm.workspace_id=pm.workspace_id AND gm.group_id=pm.group_id WHERE pm.workspace_id=p.workspace_id AND pm.project_id=p.id AND gm.user_id=?3 AND pm.group_id IS NOT NULL AND pm.role IN ('member','lead')))
             ORDER BY p.updated_at DESC,p.id ASC",
            &[Cell::uuid(workspace),Cell::Integer(i64::from(guest)),Cell::uuid(actor)],
        ).await?;
        let items = rows.iter().map(|row| Ok(TaskProject { id:row.cell(0)?.id()?,name:row.cell(1)?.string()?,key:row.cell(2)?.string()?,visibility:row.cell(3)?.string()? })).collect::<Result<Vec<_>,sqlx::Error>>()?;
        let suggested_id = source_project.filter(|id| items.iter().any(|item|item.id==*id)).or_else(||items.first().map(|item|item.id));
        Ok(Ok(TaskProjectPicker { items,suggested_id,can_create_project:role.is_some_and(|role|role.at_least(WorkspaceRole::Member)) }))
    }.await;
    finish_origin_operation(tx, result).await
}

pub async fn list_document_task_origins_backend(
    backend: &Backend,
    workspace: Uuid,
    actor: Uuid,
    credential: Uuid,
    document: Uuid,
    after: Option<Uuid>,
    limit: i64,
) -> Result<Result<TaskOriginPage, TaskOriginDbError>, sqlx::Error> {
    if let Backend::Postgres(pool) = backend {
        return list_document_task_origins(
            pool, workspace, actor, credential, document, after, limit,
        )
        .await;
    }
    let mut tx = backend.begin_read().await?;
    let result = async {
        let mut op = tx.operation();
        if origin_read_source(&mut op, workspace, actor, credential, document)
            .await?
            .is_none()
        {
            return Ok(Err(TaskOriginDbError::NotFound));
        }
        let guest = op
            .membership_role(workspace, actor, false)
            .await?
            .unwrap_or(WorkspaceRole::Guest)
            == WorkspaceRole::Guest;
        let OperationTx::SqliteFamily(family) = op else {
            unreachable!()
        };
        family_origin_page(family, workspace, actor, guest, document, after, limit).await
    }
    .await;
    finish_origin_operation(tx, result).await
}

async fn family_origin_page(
    family: &mut FamilyTx,
    workspace: Uuid,
    actor: Uuid,
    guest: bool,
    document: Uuid,
    after: Option<Uuid>,
    limit: i64,
) -> Result<Result<TaskOriginPage, TaskOriginDbError>, sqlx::Error> {
    family.require_tenant(workspace)?;
    // One predicate is shared by authorized count and pagination. Filtering
    // precedes LIMIT; direct and group viewer grants are visible here.
    // Compile both statements from the same predicate into static SQL.
    macro_rules! origin_page_sql {
        ($select:literal, $tail:literal) => {
            concat!($select, " ", "FROM task_origins o
        JOIN tasks t ON t.workspace_id=o.workspace_id AND t.id=o.task_id AND t.deleted_at IS NULL
        JOIN projects tp ON tp.workspace_id=t.workspace_id AND tp.id=t.project_id AND tp.deleted_at IS NULL
        AND ((tp.visibility='workspace' AND ?2=0)
         OR EXISTS(SELECT 1 FROM project_members pm WHERE pm.workspace_id=tp.workspace_id AND pm.project_id=tp.id AND pm.user_id=?3)
         OR EXISTS(SELECT 1 FROM project_members pm JOIN group_members gm ON gm.workspace_id=pm.workspace_id AND gm.group_id=pm.group_id WHERE pm.workspace_id=tp.workspace_id AND pm.project_id=tp.id AND gm.user_id=?3 AND pm.group_id IS NOT NULL))
        JOIN documents d ON d.workspace_id=o.workspace_id AND d.id=o.document_id AND d.deleted_at IS NULL
        LEFT JOIN projects dp ON dp.workspace_id=d.workspace_id AND dp.id=d.project_id
        WHERE o.workspace_id=?1 AND o.document_id=?4", $tail)
        };
    }
    let args = [
        Cell::uuid(workspace),
        Cell::Integer(i64::from(guest)),
        Cell::uuid(actor),
        Cell::uuid(document),
    ];
    let count_rows = family
        .query(origin_page_sql!("SELECT count(*)", ""), &args)
        .await?;
    let total = count_rows
        .first()
        .ok_or(sqlx::Error::RowNotFound)?
        .cell(0)?
        .integer()?;
    let mut page_args = args.to_vec();
    page_args.extend([
        Cell::optional_uuid(after),
        Cell::Integer(limit.saturating_add(1)),
    ]);
    let rows = family.query(origin_page_sql!("SELECT o.task_id,o.document_id,tp.key,t.number,t.title,dp.key,d.number,d.title,o.anchor", " AND (?5 IS NULL OR o.task_id>?5) ORDER BY o.task_id LIMIT ?6"),&page_args).await?;
    let mut items = rows
        .iter()
        .map(|row| {
            Ok(origin_item_from_row((
                row.cell(0)?.id()?,
                row.cell(1)?.id()?,
                row.cell(2)?.optional(Cell::string)?,
                row.cell(3)?.int32()?,
                row.cell(4)?.string()?,
                row.cell(5)?.optional(Cell::string)?,
                row.cell(6)?.int32()?,
                row.cell(7)?.string()?,
                row.cell(8)?.optional(Cell::string)?,
            )))
        })
        .collect::<Result<Vec<_>, sqlx::Error>>()?;
    let next_cursor = if items.len() as i64 > limit {
        items.truncate(limit as usize);
        items.last().map(|item| item.task_id)
    } else {
        None
    };
    Ok(Ok(TaskOriginPage {
        items,
        count: total.max(0) as usize,
        next_cursor,
    }))
}

/// The picker checks current document View and project Edit before exposing
/// project names. Archived and deleted projects are excluded.
pub async fn task_projects(
    pool: &PgPool,
    workspace_id: Uuid,
    actor_user_id: Uuid,
    session_id: Uuid,
    document_id: Uuid,
) -> Result<Result<TaskProjectPicker, TaskOriginDbError>, sqlx::Error> {
    let mut tx = pool.begin().await?;
    set_tenant(&mut tx, workspace_id).await?;
    if !session_is_live(&mut tx, actor_user_id, session_id).await?
        || !workspace_is_live(&mut tx, workspace_id).await?
        || !document_view_permission(&mut tx, workspace_id, actor_user_id, document_id)
            .await?
            .at_least(ProjectPermission::View)
    {
        return Ok(Err(TaskOriginDbError::NotFound));
    }
    let source_project_id: Option<Uuid> = sqlx::query_scalar(
        "SELECT project_id FROM fvoci.documents WHERE workspace_id = $1 AND id = $2",
    )
    .bind(workspace_id)
    .bind(document_id)
    .fetch_one(&mut *tx)
    .await?;
    let workspace_role = membership_role(&mut tx, workspace_id, actor_user_id).await?;
    let guest = workspace_role.unwrap_or(WorkspaceRole::Guest) == WorkspaceRole::Guest;
    let editable = editable_project_sql("p", 2, 3);
    let picker_sql = format!(
        r#"
        SELECT p.id, p.name, p.key, p.visibility
        FROM fvoci.projects p
        WHERE p.workspace_id = $1
          AND p.deleted_at IS NULL
          AND p.status <> 'archived'
          AND {editable}
        ORDER BY p.updated_at DESC, p.id ASC
        "#
    );
    let rows: Vec<(Uuid, String, String, String)> = sqlx::query_as(&picker_sql)
        .bind(workspace_id)
        .bind(guest)
        .bind(actor_user_id)
        .fetch_all(&mut *tx)
        .await?;
    let items = rows
        .into_iter()
        .map(|(id, name, key, visibility)| TaskProject {
            id,
            name,
            key,
            visibility,
        })
        .collect::<Vec<_>>();
    let suggested_id = source_project_id
        .filter(|id| items.iter().any(|item| item.id == *id))
        .or_else(|| items.first().map(|item| item.id));
    let can_create_project =
        workspace_role.is_some_and(|role| role.at_least(WorkspaceRole::Member));
    tx.commit().await?;
    Ok(Ok(TaskProjectPicker {
        items,
        suggested_id,
        can_create_project,
    }))
}

/// Source request hash: sha256 over the user, target project, anchor and the
/// parsed task input.
pub fn origin_request_hash(
    user_id: Uuid,
    project_id: Uuid,
    anchor: Option<&str>,
    task: &serde_json::Value,
) -> String {
    let canonical = serde_json::json!({
        "userId": user_id,
        "projectId": project_id,
        "anchor": anchor,
        "task": task,
    });
    Sha256::digest(canonical.to_string().as_bytes())
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

/// View permission on a live wiki or project document (a trashed document or
/// project yields `None`).
pub(crate) async fn document_view_permission(
    tx: &mut Transaction<'_, Postgres>,
    workspace_id: Uuid,
    actor_user_id: Uuid,
    document_id: Uuid,
) -> Result<ProjectPermission, sqlx::Error> {
    let row: Option<(Option<Uuid>,)> = sqlx::query_as(
        "SELECT project_id FROM fvoci.documents WHERE workspace_id = $1 AND id = $2 AND deleted_at IS NULL",
    )
    .bind(workspace_id)
    .bind(document_id)
    .fetch_optional(&mut **tx)
    .await?;
    match row {
        None => Ok(ProjectPermission::None),
        Some((Some(project_id),)) => {
            Ok(
                project_permission_by_id(tx, workspace_id, actor_user_id, project_id)
                    .await?
                    .unwrap_or(ProjectPermission::None),
            )
        }
        Some((None,)) => {
            document_permission(tx, workspace_id, actor_user_id, document_id, true).await
        }
    }
}

/// View permission on a live task in a live project.
pub(crate) async fn task_view_permission(
    tx: &mut Transaction<'_, Postgres>,
    workspace_id: Uuid,
    actor_user_id: Uuid,
    task_id: Uuid,
) -> Result<ProjectPermission, sqlx::Error> {
    let project_id: Option<Uuid> = sqlx::query_scalar(
        "SELECT project_id FROM fvoci.tasks WHERE workspace_id = $1 AND id = $2 AND deleted_at IS NULL",
    )
    .bind(workspace_id)
    .bind(task_id)
    .fetch_optional(&mut **tx)
    .await?;
    let Some(project_id) = project_id else {
        return Ok(ProjectPermission::None);
    };
    Ok(
        project_permission_by_id(tx, workspace_id, actor_user_id, project_id)
            .await?
            .unwrap_or(ProjectPermission::None),
    )
}

pub struct DocumentTaskRequest<'a> {
    pub document_id: Uuid,
    pub project_id: Uuid,
    pub request_id: Uuid,
    pub anchor: Option<&'a str>,
    pub request_hash: &'a str,
    pub task: CreateTaskInput<'a>,
    pub self_assign: bool,
}

/// Selected version of the existing stable document command. PostgreSQL keeps
/// its original per-document advisory/project/source-row locks and wrapper.
pub async fn create_document_task_backend(
    backend: &Backend,
    workspace_id: Uuid,
    actor_user_id: Uuid,
    session_id: Uuid,
    request: DocumentTaskRequest<'_>,
    client_ip: Option<&str>,
    channel: &str,
) -> Result<Result<DocumentTaskOutcome, TaskOriginDbError>, sqlx::Error> {
    if let Backend::Postgres(pool) = backend {
        return create_document_task(
            pool,
            workspace_id,
            actor_user_id,
            session_id,
            request,
            client_ip,
            channel,
        )
        .await;
    }
    // A failed finish retains the original stream's typed unknown outcome;
    // do not observe through a fresh pool or allocate a replacement command.
    let mut tx = backend.begin_write().await?;
    let result=async {
        let mut op=tx.operation();
        op.set_tenant(workspace_id).await?;
        op.lock_membership_users(&[actor_user_id]).await?;
        if !op.recheck_session(actor_user_id,session_id).await? {
            return Ok(Err(TaskOriginDbError::Forbidden));
        }
        if !op.workspace_is_live(workspace_id).await? {
            return Ok(Err(TaskOriginDbError::NotFound));
        }
        op.lock_tree(workspace_id).await?;
        // The family reserved writer serializes source commands, affiliation,
        // project and group-grant changes; the native receipt key still decides
        // replay. Keep the PG source View/destination Edit admission ordering.
        let Some(source_project)=origin_write_source(&mut op,workspace_id,actor_user_id,request.document_id).await? else {
            return Ok(Err(TaskOriginDbError::NotFound));
        };
        let mut projects=vec![request.project_id];
        if let Some(source)=source_project { projects.push(source); }
        projects.sort_unstable();projects.dedup();
        for project in projects {
            if op.share_lock_project_permission(workspace_id,actor_user_id,project).await?.is_none() {
                return Ok(Err(TaskOriginDbError::NotFound));
            }
        }
        if origin_write_source(&mut op,workspace_id,actor_user_id,request.document_id).await? != Some(source_project) {
            return Ok(Err(TaskOriginDbError::NotFound));
        }
        let Some((permission,archived))=op.share_lock_project_permission(workspace_id,actor_user_id,request.project_id).await? else {
            return Ok(Err(TaskOriginDbError::NotFound));
        };
        if !permission.at_least(ProjectPermission::Edit) {
            return Ok(Err(TaskOriginDbError::NotFound));
        }
        if request.self_assign && !op.origin_personal_workspace_owner(workspace_id,actor_user_id).await? {
            return Ok(Err(TaskOriginDbError::NotFound));
        }
        let OperationTx::SqliteFamily(family)=&mut op else {unreachable!("family origin")};
        let rows=family.query("SELECT task_id,request_hash FROM task_origins WHERE workspace_id=?1 AND document_id=?2 AND request_id=?3",
            &[Cell::uuid(workspace_id),Cell::uuid(request.document_id),Cell::uuid(request.request_id)]).await?;
        if let Some(row)=rows.first() {
            let task_id=row.cell(0)?.id()?;
            if row.cell(1)?.string()?!=request.request_hash {
                return Ok(Err(TaskOriginDbError::RequestMismatch));
            }
            if !op.origin_task_view_permission(workspace_id,actor_user_id,task_id).await?.at_least(ProjectPermission::View) {
                return Ok(Err(TaskOriginDbError::NotFound));
            }
            return Ok(Ok(DocumentTaskOutcome::Replayed(task_id)));
        }
        // Existing authorized same-ID archived replay is admitted above;
        // fresh creates retain create_task_tx's archived-project refusal.
        if archived { return Ok(Err(TaskOriginDbError::Task(ProjectDbError::Archived))); }
        let task_id=Uuid::now_v7();
        if let Err(error)=op.create_origin_task_family(OriginTaskCreate {
            workspace_id,project_id:request.project_id,actor_user_id,task_id,input:&request.task,client_ip,channel,
        }).await? {
            return Ok(Err(TaskOriginDbError::Task(error)));
        }
        if request.self_assign {
            if let Err(error)=op.assign_origin_task_creator_family(workspace_id,request.project_id,actor_user_id,task_id,client_ip).await? {
                return Ok(Err(TaskOriginDbError::Task(error)));
            }
        }
        let OperationTx::SqliteFamily(family)=&mut op else {unreachable!("family origin receipt")};
        family.execute("INSERT INTO task_origins(workspace_id,task_id,document_id,request_id,request_hash,anchor) VALUES(?1,?2,?3,?4,?5,?6)",
            &[Cell::uuid(workspace_id),Cell::uuid(task_id),Cell::uuid(request.document_id),Cell::uuid(request.request_id),Cell::text(request.request_hash),Cell::optional_text(request.anchor)]).await?;
        Ok(Ok(DocumentTaskOutcome::Created(task_id)))
    }.await;
    finish_origin_operation(tx, result).await
}

async fn origin_write_source(
    op: &mut OperationTx<'_, '_>,
    workspace: Uuid,
    actor: Uuid,
    document: Uuid,
) -> Result<Option<Option<Uuid>>, sqlx::Error> {
    let Some(row) = op.document_row(workspace, document).await? else {
        return Ok(None);
    };
    let source = row.8;
    let permission = if let Some(project) = source {
        op.project_permission_by_id(workspace, actor, project)
            .await?
            .unwrap_or(ProjectPermission::None)
    } else {
        op.document_permission(workspace, actor, document, true)
            .await?
    };
    Ok(permission
        .at_least(ProjectPermission::View)
        .then_some(source))
}

impl OperationTx<'_, '_> {
    async fn origin_personal_workspace_owner(
        &mut self,
        workspace: Uuid,
        actor: Uuid,
    ) -> Result<bool, sqlx::Error> {
        match self {
            Self::Postgres(tx) => {
                crate::db::personal_input::owns_personal_workspace(tx, workspace, actor).await
            }
            Self::SqliteFamily(tx) => {
                tx.require_writer()?;
                tx.require_tenant(workspace)?;
                let rows=tx.query("SELECT EXISTS(SELECT 1 FROM workspaces w JOIN users u ON u.personal_workspace_id=w.id JOIN memberships m ON m.workspace_id=w.id AND m.user_id=u.id WHERE w.id=?1 AND w.kind='personal' AND w.deleted_at IS NULL AND u.id=?2 AND u.deleted_at IS NULL AND m.role='owner')",
                    &[Cell::uuid(workspace),Cell::uuid(actor)]).await?;
                rows.first()
                    .ok_or(sqlx::Error::RowNotFound)?
                    .cell(0)?
                    .boolean()
            }
        }
    }
    async fn origin_task_view_permission(
        &mut self,
        workspace: Uuid,
        actor: Uuid,
        task: Uuid,
    ) -> Result<ProjectPermission, sqlx::Error> {
        match self {
            Self::Postgres(tx) => task_view_permission(tx, workspace, actor, task).await,
            Self::SqliteFamily(tx) => {
                tx.require_writer()?;
                tx.require_tenant(workspace)?;
                let rows=tx.query("SELECT project_id FROM tasks WHERE workspace_id=?1 AND id=?2 AND deleted_at IS NULL",&[Cell::uuid(workspace),Cell::uuid(task)]).await?;
                let Some(row) = rows.first() else {
                    return Ok(ProjectPermission::None);
                };
                let project = row.cell(0)?.id()?;
                Ok(self
                    .project_permission_by_id(workspace, actor, project)
                    .await?
                    .unwrap_or(ProjectPermission::None))
            }
        }
    }
}

/// Source `createDocumentTask`: `view` on the document, `edit` on the target
/// project; a replay of the same `requestId` returns the first task.
pub async fn create_document_task(
    pool: &PgPool,
    workspace_id: Uuid,
    actor_user_id: Uuid,
    session_id: Uuid,
    request: DocumentTaskRequest<'_>,
    client_ip: Option<&str>,
    channel: &str,
) -> Result<Result<DocumentTaskOutcome, TaskOriginDbError>, sqlx::Error> {
    let mut tx = pool.begin().await?;
    set_tenant(&mut tx, workspace_id).await?;
    let result = create_document_task_tx(
        &mut tx,
        workspace_id,
        actor_user_id,
        session_id,
        request,
        client_ip,
        channel,
    )
    .await?;
    match result {
        Ok(outcome) => {
            tx.commit().await?;
            Ok(Ok(outcome))
        }
        Err(err) => {
            tx.rollback().await?;
            Ok(Err(err))
        }
    }
}

pub(crate) async fn create_document_task_tx(
    tx: &mut Transaction<'_, Postgres>,
    workspace_id: Uuid,
    actor_user_id: Uuid,
    session_id: Uuid,
    request: DocumentTaskRequest<'_>,
    client_ip: Option<&str>,
    channel: &str,
) -> Result<Result<DocumentTaskOutcome, TaskOriginDbError>, sqlx::Error> {
    lock_membership_users(tx, &[actor_user_id]).await?;
    if !recheck_session(tx, actor_user_id, session_id).await? {
        return Ok(Err(TaskOriginDbError::Forbidden));
    }
    if !workspace_is_live(tx, workspace_id).await? {
        return Ok(Err(TaskOriginDbError::NotFound));
    }
    lock_tree(tx, workspace_id).await?;
    sqlx::query("SELECT pg_advisory_xact_lock($1, $2)")
        .bind(TASK_ORIGIN_LOCK_NAMESPACE)
        .bind(lock_key_from_uuid(request.document_id))
        .execute(&mut **tx)
        .await?;
    let document_permission =
        document_view_permission(tx, workspace_id, actor_user_id, request.document_id).await?;
    if !document_permission.at_least(ProjectPermission::View) {
        return Ok(Err(TaskOriginDbError::NotFound));
    }
    // Read affiliation before locking projects, then lock every project in UUID
    // order and the source row last. Reciprocal origins cannot invert project
    // locks; moves/trash and source grant/visibility writes serialize here.
    let source_project: Option<(Option<Uuid>,)> = sqlx::query_as(
        "SELECT project_id FROM fvoci.documents WHERE workspace_id = $1 AND id = $2 AND deleted_at IS NULL",
    ).bind(workspace_id).bind(request.document_id).fetch_optional(&mut **tx).await?;
    let Some((source_project,)) = source_project else {
        return Ok(Err(TaskOriginDbError::NotFound));
    };
    let mut project_ids = vec![request.project_id];
    if let Some(source) = source_project {
        project_ids.push(source);
    }
    project_ids.sort_unstable();
    project_ids.dedup();
    for id in project_ids {
        if lock_project(tx, workspace_id, id).await?.is_none() {
            return Ok(Err(TaskOriginDbError::NotFound));
        }
    }
    let current_source: Option<(Option<Uuid>,)> = sqlx::query_as(
        "SELECT project_id FROM fvoci.documents WHERE workspace_id = $1 AND id = $2 AND deleted_at IS NULL FOR SHARE",
    ).bind(workspace_id).bind(request.document_id).fetch_optional(&mut **tx).await?;
    if current_source != Some((source_project,))
        || !document_view_permission(tx, workspace_id, actor_user_id, request.document_id)
            .await?
            .at_least(ProjectPermission::View)
    {
        return Ok(Err(TaskOriginDbError::NotFound));
    }
    let Some(project) = lock_project(tx, workspace_id, request.project_id).await? else {
        return Ok(Err(TaskOriginDbError::NotFound));
    };
    if !project_permission(tx, workspace_id, actor_user_id, &project)
        .await?
        .at_least(ProjectPermission::Edit)
    {
        return Ok(Err(TaskOriginDbError::NotFound));
    }
    if request.self_assign
        && !crate::db::personal_input::owns_personal_workspace(tx, workspace_id, actor_user_id)
            .await?
    {
        return Ok(Err(TaskOriginDbError::NotFound));
    }
    let existing: Option<(Uuid, String)> = sqlx::query_as(
        r#"
        SELECT task_id, request_hash
        FROM fvoci.task_origins
        WHERE workspace_id = $1 AND document_id = $2 AND request_id = $3
        "#,
    )
    .bind(workspace_id)
    .bind(request.document_id)
    .bind(request.request_id)
    .fetch_optional(&mut **tx)
    .await?;
    if let Some((task_id, hash)) = existing {
        if hash != request.request_hash {
            return Ok(Err(TaskOriginDbError::RequestMismatch));
        }
        let permission = task_view_permission(tx, workspace_id, actor_user_id, task_id).await?;
        if !permission.at_least(ProjectPermission::View) {
            return Ok(Err(TaskOriginDbError::NotFound));
        }
        return Ok(Ok(DocumentTaskOutcome::Replayed(task_id)));
    }
    let created = create_task_tx(
        tx,
        workspace_id,
        request.project_id,
        actor_user_id,
        session_id,
        request.task,
        client_ip,
        channel,
    )
    .await?;
    let task = match created {
        Ok(task) => task,
        Err(err) => return Ok(Err(TaskOriginDbError::Task(err))),
    };
    if request.self_assign {
        if let Err(err) = replace_task_assignees(
            tx,
            workspace_id,
            actor_user_id,
            request.project_id,
            task.id,
            &[actor_user_id],
            client_ip,
        )
        .await?
        {
            return Ok(Err(TaskOriginDbError::Task(err)));
        }
    }
    sqlx::query(
        r#"
        INSERT INTO fvoci.task_origins (
            workspace_id, task_id, document_id, request_id, request_hash, anchor
        ) VALUES ($1, $2, $3, $4, $5, $6)
        "#,
    )
    .bind(workspace_id)
    .bind(task.id)
    .bind(request.document_id)
    .bind(request.request_id)
    .bind(request.request_hash)
    .bind(request.anchor)
    .execute(&mut **tx)
    .await?;
    Ok(Ok(DocumentTaskOutcome::Created(task.id)))
}

type OriginRow = (
    Uuid,
    Uuid,
    Option<String>,
    i32,
    String,
    Option<String>,
    i32,
    String,
    Option<String>,
);

fn origin_item_from_row(row: OriginRow) -> TaskOriginItem {
    let (
        task_id,
        document_id,
        task_project_key,
        task_number,
        task_title,
        document_project_key,
        document_number,
        document_title,
        anchor,
    ) = row;
    TaskOriginItem {
        task_id,
        document_id,
        task_display_id: format!(
            "{}-{task_number}",
            task_project_key.as_deref().unwrap_or_default()
        ),
        document_display_id: format!(
            "{}-{document_number}",
            document_project_key.as_deref().unwrap_or("WIKI")
        ),
        task_title,
        document_title,
        anchor,
    }
}

/// Current Edit, matching `effective_permission` at or above Edit. Local to
/// this picker so we do not add a generic list framework.
fn editable_project_sql(project_alias: &str, guest_param: u32, actor_param: u32) -> String {
    let join = group_members_join_sql("pm", "gm");
    format!(
        "(
            ({project_alias}.visibility = 'workspace' AND ${guest_param} = false)
            OR EXISTS (
                SELECT 1 FROM fvoci.project_members pm
                WHERE pm.workspace_id = {project_alias}.workspace_id
                  AND pm.project_id = {project_alias}.id
                  AND pm.user_id = ${actor_param}
                  AND pm.role IN ('member', 'lead')
            )
            OR EXISTS (
                SELECT 1
                FROM fvoci.project_members pm
                {join}
                WHERE pm.workspace_id = {project_alias}.workspace_id
                  AND pm.project_id = {project_alias}.id
                  AND gm.user_id = ${actor_param}
                  AND pm.group_id IS NOT NULL
                  AND pm.role IN ('member', 'lead')
            )
        )"
    )
}

/// Source `listTaskOrigin` (`GET tasks/{id}/origin`): the task must be visible;
/// an origin whose document the caller cannot view is omitted (200, empty).
pub async fn get_task_origin_backend(
    backend: &Backend,
    workspace_id: Uuid,
    actor_user_id: Uuid,
    session_id: Uuid,
    task_id: Uuid,
    after: Option<Uuid>,
    limit: i64,
) -> Result<Result<TaskOriginPage, TaskOriginDbError>, sqlx::Error> {
    if let Backend::Postgres(pool) = backend {
        return get_task_origin(
            pool,
            workspace_id,
            actor_user_id,
            session_id,
            task_id,
            after,
            limit,
        )
        .await;
    }
    let mut tx = backend.begin_read().await?;
    let result = match &mut tx {
        DbTx::SqliteFamily(family) => {
            get_task_origin_family(
                family,
                workspace_id,
                actor_user_id,
                session_id,
                task_id,
                after,
                limit,
            )
            .await
        }
        DbTx::Postgres(_) => Err(sqlx::Error::Protocol(
            "family task origin requires selected family transaction".into(),
        )),
    };
    finish_origin_operation(tx, result).await
}

async fn get_task_origin_family(
    family: &mut FamilyTx,
    workspace: Uuid,
    actor: Uuid,
    credential: Uuid,
    task: Uuid,
    after: Option<Uuid>,
    limit: i64,
) -> Result<Result<TaskOriginPage, TaskOriginDbError>, sqlx::Error> {
    let mut op = OperationTx::SqliteFamily(&mut *family);
    op.set_tenant(workspace).await?;
    if !op.session_is_live(actor, credential).await? {
        return Ok(Err(TaskOriginDbError::Forbidden));
    }
    if !op.workspace_is_live(workspace).await?
        || op.membership_role(workspace, actor, false).await?.is_none()
    {
        return Ok(Err(TaskOriginDbError::NotFound));
    }
    let rows = family
        .query(
            "SELECT project_id FROM tasks WHERE workspace_id=?1 AND id=?2 AND deleted_at IS NULL",
            &[Cell::uuid(workspace), Cell::uuid(task)],
        )
        .await?;
    let Some(row) = rows.first() else {
        return Ok(Err(TaskOriginDbError::NotFound));
    };
    let project = row.cell(0)?.id()?;
    if !OperationTx::SqliteFamily(&mut *family)
        .project_permission_by_id(workspace, actor, project)
        .await?
        .unwrap_or(ProjectPermission::None)
        .at_least(ProjectPermission::View)
    {
        return Ok(Err(TaskOriginDbError::NotFound));
    }
    let rows = family.query(
        "SELECT o.task_id,o.document_id,tp.key,t.number,t.title,dp.key,d.number,d.title,o.anchor
         FROM task_origins o
         JOIN tasks t ON t.workspace_id=o.workspace_id AND t.id=o.task_id AND t.deleted_at IS NULL
         JOIN projects tp ON tp.workspace_id=t.workspace_id AND tp.id=t.project_id
         JOIN documents d ON d.workspace_id=o.workspace_id AND d.id=o.document_id AND d.deleted_at IS NULL
         LEFT JOIN projects dp ON dp.workspace_id=d.workspace_id AND dp.id=d.project_id
         WHERE o.workspace_id=?1 AND o.task_id=?2 AND (?3 IS NULL OR o.task_id>?3)
         ORDER BY o.task_id LIMIT ?4",
        &[Cell::uuid(workspace),Cell::uuid(task),Cell::optional_uuid(after),Cell::Integer(limit.saturating_add(1))],
    ).await?;
    let mut items = Vec::with_capacity(rows.len());
    for row in rows {
        if origin_read_source(
            &mut OperationTx::SqliteFamily(&mut *family),
            workspace,
            actor,
            credential,
            row.cell(1)?.id()?,
        )
        .await?
        .is_none()
        {
            continue;
        }
        items.push(origin_item_from_row((
            row.cell(0)?.id()?,
            row.cell(1)?.id()?,
            row.cell(2)?.optional(Cell::string)?,
            row.cell(3)?.int32()?,
            row.cell(4)?.string()?,
            row.cell(5)?.optional(Cell::string)?,
            row.cell(6)?.int32()?,
            row.cell(7)?.string()?,
            row.cell(8)?.optional(Cell::string)?,
        )));
    }
    let count = items.len();
    let next_cursor = if items.len() as i64 > limit {
        items.truncate(limit as usize);
        items.last().map(|item| item.task_id)
    } else {
        None
    };
    Ok(Ok(TaskOriginPage {
        items,
        count,
        next_cursor,
    }))
}

/// Preserved PostgreSQL task-origin reader.
pub async fn get_task_origin(
    pool: &PgPool,
    workspace_id: Uuid,
    actor_user_id: Uuid,
    session_id: Uuid,
    task_id: Uuid,
    after: Option<Uuid>,
    limit: i64,
) -> Result<Result<TaskOriginPage, TaskOriginDbError>, sqlx::Error> {
    let mut tx = pool.begin().await?;
    set_tenant(&mut tx, workspace_id).await?;
    if !session_is_live(&mut tx, actor_user_id, session_id).await? {
        tx.rollback().await?;
        return Ok(Err(TaskOriginDbError::Forbidden));
    }
    if !workspace_is_live(&mut tx, workspace_id).await? {
        tx.rollback().await?;
        return Ok(Err(TaskOriginDbError::NotFound));
    }
    let permission = task_view_permission(&mut tx, workspace_id, actor_user_id, task_id).await?;
    if !permission.at_least(ProjectPermission::View) {
        tx.rollback().await?;
        return Ok(Err(TaskOriginDbError::NotFound));
    }
    let rows: Vec<OriginRow> = sqlx::query_as(
        r#"
        SELECT o.task_id, o.document_id, tp.key, t.number, t.title,
               dp.key, d.number, d.title, o.anchor
        FROM fvoci.task_origins o
        JOIN fvoci.tasks t
          ON t.workspace_id = o.workspace_id AND t.id = o.task_id AND t.deleted_at IS NULL
        JOIN fvoci.projects tp
          ON tp.workspace_id = t.workspace_id AND tp.id = t.project_id
        JOIN fvoci.documents d
          ON d.workspace_id = o.workspace_id AND d.id = o.document_id AND d.deleted_at IS NULL
        LEFT JOIN fvoci.projects dp
          ON dp.workspace_id = d.workspace_id AND dp.id = d.project_id
        WHERE o.workspace_id = $1
          AND o.task_id = $2
          AND ($3::uuid IS NULL OR o.task_id > $3)
        ORDER BY o.task_id
        LIMIT $4
        "#,
    )
    .bind(workspace_id)
    .bind(task_id)
    .bind(after)
    .bind(limit.saturating_add(1))
    .fetch_all(&mut *tx)
    .await?;
    let mut items = Vec::with_capacity(rows.len());
    for row in rows {
        let document_id = row.1;
        let visible = document_view_permission(&mut tx, workspace_id, actor_user_id, document_id)
            .await?
            .at_least(ProjectPermission::View);
        if !visible {
            continue;
        }
        items.push(origin_item_from_row(row));
    }
    tx.commit().await?;
    let count = items.len();
    let next_cursor = if items.len() as i64 > limit {
        items.truncate(limit as usize);
        items.last().map(|item| item.task_id)
    } else {
        None
    };
    Ok(Ok(TaskOriginPage {
        items,
        count,
        next_cursor,
    }))
}

/// Lists links from a visible live document. Permission filtering happens
/// before the cursor and page size so hidden tasks cannot consume slots or
/// inflate the total count.
pub async fn list_document_task_origins(
    pool: &PgPool,
    workspace_id: Uuid,
    actor_user_id: Uuid,
    session_id: Uuid,
    document_id: Uuid,
    after: Option<Uuid>,
    limit: i64,
) -> Result<Result<TaskOriginPage, TaskOriginDbError>, sqlx::Error> {
    let mut tx = pool.begin().await?;
    set_tenant(&mut tx, workspace_id).await?;
    if !session_is_live(&mut tx, actor_user_id, session_id).await?
        || !workspace_is_live(&mut tx, workspace_id).await?
        || !document_view_permission(&mut tx, workspace_id, actor_user_id, document_id)
            .await?
            .at_least(ProjectPermission::View)
    {
        return Ok(Err(TaskOriginDbError::NotFound));
    }
    let guest = membership_role(&mut tx, workspace_id, actor_user_id)
        .await?
        .unwrap_or(WorkspaceRole::Guest)
        == WorkspaceRole::Guest;
    let visible = visible_project_sql("tp", 2, 3);
    let from_where = format!(
        r#"
        FROM fvoci.task_origins o
        JOIN fvoci.tasks t
          ON t.workspace_id = o.workspace_id AND t.id = o.task_id AND t.deleted_at IS NULL
        JOIN fvoci.projects tp
          ON tp.workspace_id = t.workspace_id AND tp.id = t.project_id AND tp.deleted_at IS NULL
         AND {visible}
        JOIN fvoci.documents d
          ON d.workspace_id = o.workspace_id AND d.id = o.document_id AND d.deleted_at IS NULL
        LEFT JOIN fvoci.projects dp
          ON dp.workspace_id = d.workspace_id AND dp.id = d.project_id
        WHERE o.workspace_id = $1 AND o.document_id = $4
        "#
    );
    let count_sql = format!("SELECT count(*)::bigint {from_where}");
    let total: i64 = sqlx::query_scalar(&count_sql)
        .bind(workspace_id)
        .bind(guest)
        .bind(actor_user_id)
        .bind(document_id)
        .fetch_one(&mut *tx)
        .await?;
    let page_sql = format!(
        r#"
        SELECT o.task_id, o.document_id, tp.key, t.number, t.title,
               dp.key, d.number, d.title, o.anchor
        {from_where}
          AND ($5::uuid IS NULL OR o.task_id > $5)
        ORDER BY o.task_id
        LIMIT $6
        "#
    );
    let rows: Vec<OriginRow> = sqlx::query_as(&page_sql)
        .bind(workspace_id)
        .bind(guest)
        .bind(actor_user_id)
        .bind(document_id)
        .bind(after)
        .bind(limit.saturating_add(1))
        .fetch_all(&mut *tx)
        .await?;
    tx.commit().await?;
    let mut items: Vec<_> = rows.into_iter().map(origin_item_from_row).collect();
    let next_cursor = if items.len() as i64 > limit {
        items.truncate(limit as usize);
        items.last().map(|item| item.task_id)
    } else {
        None
    };
    Ok(Ok(TaskOriginPage {
        items,
        count: total.max(0) as usize,
        next_cursor,
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn request_hash_depends_on_every_part() {
        let user = Uuid::nil();
        let project = Uuid::from_u128(1);
        let task = serde_json::json!({"title": "a"});
        let base = origin_request_hash(user, project, Some("b1"), &task);
        assert_eq!(base.len(), 64);
        assert_eq!(base, origin_request_hash(user, project, Some("b1"), &task));
        assert_ne!(base, origin_request_hash(user, project, None, &task));
        assert_ne!(
            base,
            origin_request_hash(user, Uuid::from_u128(2), Some("b1"), &task)
        );
        assert_ne!(
            base,
            origin_request_hash(
                user,
                project,
                Some("b1"),
                &serde_json::json!({"title": "b"})
            )
        );
    }
}

#[cfg(all(test, feature = "db-tests"))]
mod selected_document_origin_read_tests {
    use super::*;
    use crate::db::attachment_preview::tests::Fixture;

    async fn project_with_origin(
        f: &Fixture,
        ordinal: u128,
        visibility: &str,
        state: &str,
    ) -> (Uuid, Uuid) {
        let project = Uuid::from_u128(ordinal);
        let workflow = Uuid::now_v7();
        let status = Uuid::now_v7();
        let task = Uuid::from_u128(ordinal + 1000);
        sqlx::query("INSERT INTO projects(id,workspace_id,key,name,visibility,status,created_by,updated_at) VALUES(?1,?2,?3,?4,?5,?6,?7,1000000)")
            .bind(project.as_bytes().as_slice()).bind(f.workspace.as_bytes().as_slice()).bind(format!("PX{ordinal}"))
            .bind(format!("프로젝트 {ordinal}" )).bind(visibility).bind(state).bind(f.user.as_bytes().as_slice()).execute(&f.pool).await.unwrap();
        sqlx::query("INSERT INTO workflows(id,workspace_id,project_id) VALUES(?1,?2,?3)")
            .bind(workflow.as_bytes().as_slice())
            .bind(f.workspace.as_bytes().as_slice())
            .bind(project.as_bytes().as_slice())
            .execute(&f.pool)
            .await
            .unwrap();
        sqlx::query("INSERT INTO statuses(id,workspace_id,project_id,workflow_id,name,category,sort_key) VALUES(?1,?2,?3,?4,'Todo','todo','V')").bind(status.as_bytes().as_slice()).bind(f.workspace.as_bytes().as_slice()).bind(project.as_bytes().as_slice()).bind(workflow.as_bytes().as_slice()).execute(&f.pool).await.unwrap();
        sqlx::query("INSERT INTO tasks(id,workspace_id,project_id,number,title,status_id,created_by,content_json) VALUES(?1,?2,?3,1,?4,?5,?6,?7)").bind(task.as_bytes().as_slice()).bind(f.workspace.as_bytes().as_slice()).bind(project.as_bytes().as_slice()).bind(format!("작업 😀 {ordinal}")).bind(status.as_bytes().as_slice()).bind(f.user.as_bytes().as_slice()).bind(serde_json::json!({"type":"doc","content":[{"type":"paragraph","attrs":{"id":"literal-anchor"},"content":[{"type":"text","text":format!("작업 본문 😀 {ordinal}")}]}]}).to_string()).execute(&f.pool).await.unwrap();
        sqlx::query("INSERT INTO task_origins(workspace_id,task_id,document_id,request_id,request_hash,anchor) VALUES(?1,?2,?3,?4,'read-fixture','literal-anchor')").bind(f.workspace.as_bytes().as_slice()).bind(task.as_bytes().as_slice()).bind(f.document.as_bytes().as_slice()).bind(Uuid::now_v7().as_bytes().as_slice()).execute(&f.pool).await.unwrap();
        (project, task)
    }

    #[tokio::test]
    async fn wiki_aux_origins_selected_count_before_limit_picker_roles_groups_and_current_denial() {
        let f = Fixture::new().await;
        let credential = Uuid::now_v7();
        sqlx::query("INSERT INTO sessions(id,user_id,token_hash,expires_at) VALUES(?1,?2,'origins-read',?3)").bind(credential.as_bytes().as_slice()).bind(f.user.as_bytes().as_slice()).bind(chrono::Utc::now().timestamp_micros()+86_400_000_000).execute(&f.pool).await.unwrap();
        let (public, task1) = project_with_origin(&f, 301, "workspace", "active").await;
        let (private, task2) = project_with_origin(&f, 302, "private", "active").await;
        let (archived, task3) = project_with_origin(&f, 303, "workspace", "archived").await;
        let (deleted, _) = project_with_origin(&f, 304, "workspace", "active").await;
        sqlx::query("UPDATE projects SET deleted_at=1 WHERE id=?1")
            .bind(deleted.as_bytes().as_slice())
            .execute(&f.pool)
            .await
            .unwrap();
        // Workspace owner sees workspace projects, not an ungranted private one.
        let first = list_document_task_origins_backend(
            &f.backend,
            f.workspace,
            f.user,
            credential,
            f.document,
            None,
            1,
        )
        .await
        .unwrap()
        .unwrap();
        assert_eq!(first.count, 2);
        assert_eq!(first.items[0].task_id, task1);
        assert_eq!(first.next_cursor, Some(task1));
        assert_eq!(first.items[0].document_id, f.document);
        assert_eq!(first.items[0].anchor.as_deref(), Some("literal-anchor"));
        assert_eq!(first.items[0].task_title, "작업 😀 301");
        assert_eq!(first.items[0].task_display_id, "PX301-1");
        assert_eq!(first.items[0].document_display_id, "WIKI-1");
        let next = list_document_task_origins_backend(
            &f.backend,
            f.workspace,
            f.user,
            credential,
            f.document,
            first.next_cursor,
            1,
        )
        .await
        .unwrap()
        .unwrap();
        assert_eq!(next.count, 2);
        assert_eq!(next.items[0].task_id, task3);
        assert!(next.next_cursor.is_none());
        let picker = task_projects_backend(&f.backend, f.workspace, f.user, credential, f.document)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(
            picker.items.iter().map(|item| item.id).collect::<Vec<_>>(),
            vec![public]
        );
        assert_eq!(picker.suggested_id, Some(public));
        assert!(picker.can_create_project);
        let grant = Uuid::now_v7();
        sqlx::query("INSERT INTO project_members(id,workspace_id,project_id,user_id,role) VALUES(?1,?2,?3,?4,'viewer')").bind(grant.as_bytes().as_slice()).bind(f.workspace.as_bytes().as_slice()).bind(private.as_bytes().as_slice()).bind(f.user.as_bytes().as_slice()).execute(&f.pool).await.unwrap();
        let visible = list_document_task_origins_backend(
            &f.backend,
            f.workspace,
            f.user,
            credential,
            f.document,
            None,
            50,
        )
        .await
        .unwrap()
        .unwrap();
        assert_eq!(
            visible
                .items
                .iter()
                .map(|item| item.task_id)
                .collect::<Vec<_>>(),
            vec![task1, task2, task3]
        );
        assert_eq!(visible.count, 3);
        assert_eq!(
            task_projects_backend(&f.backend, f.workspace, f.user, credential, f.document)
                .await
                .unwrap()
                .unwrap()
                .items
                .len(),
            1
        );
        sqlx::query("UPDATE project_members SET role='member' WHERE id=?1")
            .bind(grant.as_bytes().as_slice())
            .execute(&f.pool)
            .await
            .unwrap();
        let editable =
            task_projects_backend(&f.backend, f.workspace, f.user, credential, f.document)
                .await
                .unwrap()
                .unwrap();
        assert_eq!(
            editable
                .items
                .iter()
                .map(|item| item.id)
                .collect::<Vec<_>>(),
            vec![public, private]
        );
        assert_eq!(editable.items[1].name, "프로젝트 302");
        assert_eq!(editable.items[1].key, "PX302");
        assert_eq!(editable.items[1].visibility, "private");
        // Actual source project suggestion uses authorized items, not the first
        // row's sort position, and keeps project documents' View policy.
        sqlx::query("UPDATE documents SET project_id=?1 WHERE id=?2")
            .bind(private.as_bytes().as_slice())
            .bind(f.document.as_bytes().as_slice())
            .execute(&f.pool)
            .await
            .unwrap();
        assert_eq!(
            task_projects_backend(&f.backend, f.workspace, f.user, credential, f.document)
                .await
                .unwrap()
                .unwrap()
                .suggested_id,
            Some(private)
        );
        sqlx::query("UPDATE documents SET project_id=NULL WHERE id=?1")
            .bind(f.document.as_bytes().as_slice())
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
            task_projects_backend(&f.backend, f.workspace, f.user, credential, f.document)
                .await
                .unwrap(),
            Err(TaskOriginDbError::NotFound)
        ));
        assert!(matches!(
            list_document_task_origins_backend(
                &f.backend,
                f.workspace,
                f.user,
                credential,
                f.document,
                None,
                50
            )
            .await
            .unwrap(),
            Err(TaskOriginDbError::NotFound)
        ));
        let group = Uuid::now_v7();
        sqlx::query("INSERT INTO groups(id,workspace_id,name) VALUES(?1,?2,'Read group')")
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
        sqlx::query("INSERT INTO document_members(id,workspace_id,document_id,group_id,role) VALUES(?1,?2,?3,?4,'viewer')").bind(Uuid::now_v7().as_bytes().as_slice()).bind(f.workspace.as_bytes().as_slice()).bind(f.document.as_bytes().as_slice()).bind(group.as_bytes().as_slice()).execute(&f.pool).await.unwrap();
        sqlx::query("INSERT INTO project_members(id,workspace_id,project_id,group_id,role) VALUES(?1,?2,?3,?4,'viewer')").bind(Uuid::now_v7().as_bytes().as_slice()).bind(f.workspace.as_bytes().as_slice()).bind(public.as_bytes().as_slice()).bind(group.as_bytes().as_slice()).execute(&f.pool).await.unwrap();
        let guest = task_projects_backend(&f.backend, f.workspace, f.user, credential, f.document)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(
            guest.items.iter().map(|item| item.id).collect::<Vec<_>>(),
            vec![private]
        );
        assert!(!guest.can_create_project);
        let guest_origins = list_document_task_origins_backend(
            &f.backend,
            f.workspace,
            f.user,
            credential,
            f.document,
            None,
            50,
        )
        .await
        .unwrap()
        .unwrap();
        assert_eq!(
            guest_origins
                .items
                .iter()
                .map(|item| item.task_id)
                .collect::<Vec<_>>(),
            vec![task1, task2]
        );
        assert_eq!(guest_origins.count, 2);
        sqlx::query("UPDATE project_members SET role='member' WHERE group_id=?1 AND project_id=?2")
            .bind(group.as_bytes().as_slice())
            .bind(public.as_bytes().as_slice())
            .execute(&f.pool)
            .await
            .unwrap();
        assert_eq!(
            task_projects_backend(&f.backend, f.workspace, f.user, credential, f.document)
                .await
                .unwrap()
                .unwrap()
                .items
                .iter()
                .map(|item| item.id)
                .collect::<Vec<_>>(),
            vec![public, private]
        );
        sqlx::query(
            "DELETE FROM group_members WHERE workspace_id=?1 AND group_id=?2 AND user_id=?3",
        )
        .bind(f.workspace.as_bytes().as_slice())
        .bind(group.as_bytes().as_slice())
        .bind(f.user.as_bytes().as_slice())
        .execute(&f.pool)
        .await
        .unwrap();
        assert!(matches!(
            list_document_task_origins_backend(
                &f.backend,
                f.workspace,
                f.user,
                credential,
                f.document,
                None,
                50
            )
            .await
            .unwrap(),
            Err(TaskOriginDbError::NotFound)
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
            task_projects_backend(&f.backend, f.workspace, f.user, credential, f.document)
                .await
                .unwrap(),
            Err(TaskOriginDbError::NotFound)
        ));
        assert!(matches!(
            list_document_task_origins_backend(
                &f.backend,
                f.workspace,
                f.user,
                credential,
                f.document,
                None,
                50
            )
            .await
            .unwrap(),
            Err(TaskOriginDbError::NotFound)
        ));
        sqlx::query("UPDATE sessions SET revoked_at=NULL WHERE id=?1")
            .bind(credential.as_bytes().as_slice())
            .execute(&f.pool)
            .await
            .unwrap();
        let healthy =
            task_projects_backend(&f.backend, f.workspace, f.user, credential, f.document)
                .await
                .unwrap()
                .unwrap();
        assert_eq!(
            healthy.items.iter().map(|item| item.id).collect::<Vec<_>>(),
            vec![public, private]
        );
        assert!(!healthy
            .items
            .iter()
            .any(|item| item.id == archived || item.id == deleted));
        assert_eq!(
            list_document_task_origins_backend(
                &f.backend,
                f.workspace,
                f.user,
                credential,
                f.document,
                None,
                50
            )
            .await
            .unwrap()
            .unwrap()
            .count,
            3
        );
        f.pool.close().await;
        std::fs::remove_dir_all(&f.root).unwrap();
    }
}

#[cfg(all(test, feature = "db-tests"))]
mod selected_document_origin_create_tests {
    use super::*;
    use crate::db::attachment_preview::tests::Fixture;
    use serde_json::{json, Value};

    /// Exact columns read back for the created task (unchanged query).
    type CreatedTaskRow = (
        Vec<u8>,
        Vec<u8>,
        i64,
        String,
        String,
        String,
        String,
        i64,
        Vec<u8>,
        i64,
    );

    async fn setup() -> (Fixture, Uuid, Uuid) {
        let f = Fixture::new().await;
        let credential = Uuid::now_v7();
        let mut tx = f.backend.begin_write().await.unwrap();
        tx.operation()
            .create_session(
                credential,
                f.user,
                "origin-create",
                chrono::DateTime::from_timestamp_micros(
                    chrono::Utc::now().timestamp_micros() + 86_400_000_000,
                )
                .unwrap(),
            )
            .await
            .unwrap();
        tx.commit().await.unwrap();
        let project = create_project_fixture(&f, credential, "ORIGIN").await;
        (f, credential, project)
    }
    async fn create_project_fixture(f: &Fixture, credential: Uuid, key: &str) -> Uuid {
        crate::db::projects::create_project_backend(
            &f.backend,
            f.workspace,
            f.user,
            credential,
            crate::db::projects::CreateProjectInput {
                key,
                name: "실제 대상 中 😀",
                visibility: "private",
                description: None,
                icon: None,
                lead_user_id: None,
            },
            None,
        )
        .await
        .unwrap()
        .unwrap()
        .id
    }
    fn request<'a>(
        document: Uuid,
        project: Uuid,
        command: Uuid,
        title: &'a str,
        hash: &'a str,
    ) -> DocumentTaskRequest<'a> {
        DocumentTaskRequest {
            document_id: document,
            project_id: project,
            request_id: command,
            anchor: Some("literal-source-block"),
            request_hash: hash,
            self_assign: false,
            task: CreateTaskInput {
                title,
                task_type: "task",
                priority: "none",
                status_id: None,
                start_date: None,
                due_date: None,
                parent_id: None,
                milestone_id: None,
                recurrence: None,
            },
        }
    }
    fn command_hash(
        actor: Uuid,
        project: Uuid,
        title: &str,
        anchor: Option<&str>,
        self_assign: bool,
    ) -> String {
        // The real route's maintained typed DTO and normalization, not a second
        // command serializer. Only true selfAssign participates in its hash.
        let dto: crate::api::dto::CreateTaskBody =
            serde_json::from_value(json!({"title":title})).unwrap();
        let mut normalized = crate::http::routes::task_body::normalized_task_input(&dto);
        if self_assign {
            normalized["selfAssign"] = Value::Bool(true);
        }
        origin_request_hash(actor, project, anchor, &normalized)
    }
    fn request_hash_for(actor: Uuid, request: &DocumentTaskRequest<'_>) -> String {
        let input = &request.task;
        let dto = crate::api::dto::CreateTaskBody {
            title: input.title.into(),
            task_type: input.task_type.into(),
            priority: input.priority.into(),
            status_id: input.status_id,
            start_date: input.start_date,
            due_date: input.due_date,
            parent_id: input.parent_id,
            milestone_id: input.milestone_id,
            recurrence: input.recurrence.clone(),
        };
        let mut normalized = crate::http::routes::task_body::normalized_task_input(&dto);
        if request.self_assign {
            normalized["selfAssign"] = Value::Bool(true);
        }
        origin_request_hash(actor, request.project_id, request.anchor, &normalized)
    }
    async fn create(
        f: &Fixture,
        credential: Uuid,
        request: DocumentTaskRequest<'_>,
    ) -> Result<Result<DocumentTaskOutcome, TaskOriginDbError>, sqlx::Error> {
        create_document_task_backend(
            &f.backend,
            f.workspace,
            f.user,
            credential,
            request,
            Some("127.0.0.1"),
            "api",
        )
        .await
    }
    async fn state(f: &Fixture, project: Uuid) -> (i64, i64, i64, i64, i64, i64, i64) {
        sqlx::query_as("SELECT (SELECT count(*) FROM tasks),(SELECT count(*) FROM task_origins),(SELECT next_number FROM projects WHERE id=?1),(SELECT count(*) FROM events),(SELECT count(*) FROM audit_log),(SELECT count(*) FROM task_activity),(SELECT count(*) FROM task_assignees)")
            .bind(project.as_bytes().as_slice()).fetch_one(&f.pool).await.unwrap()
    }

    async fn inverse(
        f: &Fixture,
        credential: Uuid,
        task: Uuid,
    ) -> Result<Result<TaskOriginPage, TaskOriginDbError>, sqlx::Error> {
        get_task_origin_backend(&f.backend, f.workspace, f.user, credential, task, None, 50).await
    }

    #[tokio::test]
    async fn wiki_aux_task_origin_inverse_normal_new_client_literal_replay_and_cursor() {
        let (f, credential, project) = setup().await;
        let command = Uuid::now_v7();
        let hash = command_hash(
            f.user,
            project,
            "Inverse 中 😀",
            Some("literal-source-block"),
            false,
        );
        let task = create(
            &f,
            credential,
            request(f.document, project, command, "Inverse 中 😀", &hash),
        )
        .await
        .unwrap()
        .unwrap()
        .task_id();
        let before = state(&f, project).await;
        let expected = TaskOriginPage {
            items: vec![TaskOriginItem {
                task_id: task,
                document_id: f.document,
                task_display_id: "ORIGIN-2".into(),
                document_display_id: "WIKI-1".into(),
                task_title: "Inverse 中 😀".into(),
                document_title: "S31".into(),
                anchor: Some("literal-source-block".into()),
            }],
            count: 1,
            next_cursor: None,
        };
        assert_eq!(
            inverse(&f, credential, task).await.unwrap().unwrap(),
            expected
        );
        let client = crate::db::pool::connect_sqlite_app(&f.path, 1)
            .await
            .unwrap();
        assert_eq!(
            get_task_origin_backend(
                &Backend::Sqlite(client.clone()),
                f.workspace,
                f.user,
                credential,
                task,
                None,
                1
            )
            .await
            .unwrap()
            .unwrap(),
            expected
        );
        for after in [
            Some(Uuid::nil()),
            Some(task),
            Some(Uuid::from_u128(u128::MAX)),
        ] {
            let page = get_task_origin_backend(
                &f.backend,
                f.workspace,
                f.user,
                credential,
                task,
                after,
                1,
            )
            .await
            .unwrap()
            .unwrap();
            assert_eq!(
                page,
                if after == Some(Uuid::nil()) {
                    expected.clone()
                } else {
                    TaskOriginPage {
                        items: vec![],
                        count: 0,
                        next_cursor: None,
                    }
                }
            );
        }
        assert_eq!(
            create(
                &f,
                credential,
                request(f.document, project, command, "Inverse 中 😀", &hash)
            )
            .await
            .unwrap()
            .unwrap(),
            DocumentTaskOutcome::Replayed(task)
        );
        assert_eq!(state(&f, project).await, before);
        assert_eq!(
            inverse(&f, credential, task).await.unwrap().unwrap(),
            expected
        );
        // Archive is not a read denial. Trash hides the source without hiding the task.
        sqlx::query("UPDATE tasks SET archived_at=1 WHERE id=?1")
            .bind(task.as_bytes().as_slice())
            .execute(&f.pool)
            .await
            .unwrap();
        sqlx::query("UPDATE projects SET status='archived' WHERE id=?1")
            .bind(project.as_bytes().as_slice())
            .execute(&f.pool)
            .await
            .unwrap();
        assert_eq!(
            inverse(&f, credential, task).await.unwrap().unwrap(),
            expected
        );
        sqlx::query("UPDATE documents SET deleted_at=1 WHERE id=?1")
            .bind(f.document.as_bytes().as_slice())
            .execute(&f.pool)
            .await
            .unwrap();
        assert_eq!(
            inverse(&f, credential, task).await.unwrap().unwrap(),
            TaskOriginPage {
                items: vec![],
                count: 0,
                next_cursor: None
            }
        );
        sqlx::query("UPDATE documents SET deleted_at=NULL WHERE id=?1")
            .bind(f.document.as_bytes().as_slice())
            .execute(&f.pool)
            .await
            .unwrap();
        assert_eq!(
            inverse(&f, credential, task).await.unwrap().unwrap(),
            expected
        );
        client.close().await;
        f.close().await;
    }

    #[tokio::test]
    async fn wiki_aux_task_origin_inverse_current_group_source_target_credentials_and_tenant() {
        let (f, credential, project) = setup().await;
        let command = Uuid::now_v7();
        let hash = command_hash(
            f.user,
            project,
            "Private",
            Some("literal-source-block"),
            false,
        );
        let task = create(
            &f,
            credential,
            request(f.document, project, command, "Private", &hash),
        )
        .await
        .unwrap()
        .unwrap()
        .task_id();
        let empty = TaskOriginPage {
            items: vec![],
            count: 0,
            next_cursor: None,
        };
        // A real current Guest with target Edit has no wiki source View.
        sqlx::query("UPDATE memberships SET role='guest' WHERE workspace_id=?1 AND user_id=?2")
            .bind(f.workspace.as_bytes().as_slice())
            .bind(f.user.as_bytes().as_slice())
            .execute(&f.pool)
            .await
            .unwrap();
        assert_eq!(inverse(&f, credential, task).await.unwrap().unwrap(), empty);
        let group = Uuid::now_v7();
        sqlx::query("INSERT INTO groups(id,workspace_id,name) VALUES(?1,?2,'Inverse viewers')")
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
        sqlx::query("INSERT INTO document_members(id,workspace_id,document_id,group_id,role) VALUES(?1,?2,?3,?4,'viewer')").bind(Uuid::now_v7().as_bytes().as_slice()).bind(f.workspace.as_bytes().as_slice()).bind(f.document.as_bytes().as_slice()).bind(group.as_bytes().as_slice()).execute(&f.pool).await.unwrap();
        assert_eq!(
            inverse(&f, credential, task).await.unwrap().unwrap().items[0].document_id,
            f.document
        );
        let mut revoke = f.backend.begin_write().await.unwrap();
        let OperationTx::SqliteFamily(writer) = revoke.operation() else {
            unreachable!()
        };
        writer.execute("DELETE FROM document_members WHERE workspace_id=?1 AND document_id=?2 AND group_id=?3", &[Cell::uuid(f.workspace),Cell::uuid(f.document),Cell::uuid(group)]).await.unwrap();
        revoke.commit().await.unwrap();
        assert_eq!(inverse(&f, credential, task).await.unwrap().unwrap(), empty);
        sqlx::query("UPDATE memberships SET role='owner' WHERE workspace_id=?1 AND user_id=?2")
            .bind(f.workspace.as_bytes().as_slice())
            .bind(f.user.as_bytes().as_slice())
            .execute(&f.pool)
            .await
            .unwrap();
        assert_eq!(
            inverse(&f, credential, task).await.unwrap().unwrap().count,
            1
        );
        // Current source affiliation retires wiki's workspace grant.
        let source = create_project_fixture(&f, credential, "SOURCE").await;
        let parent: Vec<u8> =
            sqlx::query_scalar("SELECT root_document_id FROM projects WHERE id=?1")
                .bind(source.as_bytes().as_slice())
                .fetch_one(&f.pool)
                .await
                .unwrap();
        let parent = Uuid::from_slice(&parent).unwrap();
        sqlx::query(
            "UPDATE documents SET project_id=?1,parent_id=?2,number=17,path=?3 WHERE id=?4",
        )
        .bind(source.as_bytes().as_slice())
        .bind(parent.as_bytes().as_slice())
        .bind(format!(
            "{}.{}",
            crate::db::documents::to_path_label(parent),
            crate::db::documents::to_path_label(f.document)
        ))
        .bind(f.document.as_bytes().as_slice())
        .execute(&f.pool)
        .await
        .unwrap();
        sqlx::query("UPDATE projects SET next_number=18 WHERE id=?1")
            .bind(source.as_bytes().as_slice())
            .execute(&f.pool)
            .await
            .unwrap();
        sqlx::query("DELETE FROM project_members WHERE project_id=?1 AND user_id=?2")
            .bind(source.as_bytes().as_slice())
            .bind(f.user.as_bytes().as_slice())
            .execute(&f.pool)
            .await
            .unwrap();
        assert_eq!(inverse(&f, credential, task).await.unwrap().unwrap(), empty);
        sqlx::query("INSERT INTO project_members(id,workspace_id,project_id,user_id,role) VALUES(?1,?2,?3,?4,'viewer')").bind(Uuid::now_v7().as_bytes().as_slice()).bind(f.workspace.as_bytes().as_slice()).bind(source.as_bytes().as_slice()).bind(f.user.as_bytes().as_slice()).execute(&f.pool).await.unwrap();
        let visible = inverse(&f, credential, task).await.unwrap().unwrap();
        assert_eq!(visible.items[0].document_display_id, "SOURCE-17");
        assert!(matches!(
            get_task_origin_backend(
                &f.backend,
                Uuid::now_v7(),
                f.user,
                credential,
                task,
                None,
                50
            )
            .await
            .unwrap(),
            Err(TaskOriginDbError::NotFound)
        ));
        assert!(matches!(
            get_task_origin_backend(
                &f.backend,
                f.workspace,
                Uuid::now_v7(),
                credential,
                task,
                None,
                50
            )
            .await
            .unwrap(),
            Err(TaskOriginDbError::Forbidden)
        ));
        for (table, column, id) in [
            ("sessions", "revoked_at", credential),
            ("users", "suspended_at", f.user),
            ("users", "deleted_at", f.user),
            ("tasks", "deleted_at", task),
            ("projects", "deleted_at", project),
            ("workspaces", "deleted_at", f.workspace),
        ] {
            sqlx::query(&format!("UPDATE {table} SET {column}=1 WHERE id=?1"))
                .bind(id.as_bytes().as_slice())
                .execute(&f.pool)
                .await
                .unwrap();
            let denied = inverse(&f, credential, task).await.unwrap();
            if table == "sessions" || table == "users" {
                assert!(matches!(denied, Err(TaskOriginDbError::Forbidden)));
            } else {
                assert!(matches!(denied, Err(TaskOriginDbError::NotFound)));
            }
            sqlx::query(&format!("UPDATE {table} SET {column}=NULL WHERE id=?1"))
                .bind(id.as_bytes().as_slice())
                .execute(&f.pool)
                .await
                .unwrap();
            assert_eq!(
                inverse(&f, credential, task).await.unwrap().unwrap(),
                visible
            );
        }
        sqlx::query("DELETE FROM project_members WHERE project_id=?1 AND user_id=?2")
            .bind(project.as_bytes().as_slice())
            .bind(f.user.as_bytes().as_slice())
            .execute(&f.pool)
            .await
            .unwrap();
        assert!(matches!(
            inverse(&f, credential, task).await.unwrap(),
            Err(TaskOriginDbError::NotFound)
        ));
        sqlx::query("INSERT INTO project_members(id,workspace_id,project_id,group_id,role) VALUES(?1,?2,?3,?4,'viewer')").bind(Uuid::now_v7().as_bytes().as_slice()).bind(f.workspace.as_bytes().as_slice()).bind(project.as_bytes().as_slice()).bind(group.as_bytes().as_slice()).execute(&f.pool).await.unwrap();
        assert_eq!(
            inverse(&f, credential, task).await.unwrap().unwrap(),
            visible
        );
        sqlx::query("DELETE FROM memberships WHERE workspace_id=?1 AND user_id=?2")
            .bind(f.workspace.as_bytes().as_slice())
            .bind(f.user.as_bytes().as_slice())
            .execute(&f.pool)
            .await
            .unwrap();
        assert!(matches!(
            inverse(&f, credential, task).await.unwrap(),
            Err(TaskOriginDbError::NotFound)
        ));
        f.close().await;
    }

    #[tokio::test]
    async fn wiki_aux_task_origin_inverse_original_decode_rollback_fk_then_healthy_read() {
        let (f, credential, project) = setup().await;
        let hash = command_hash(
            f.user,
            project,
            "Decode",
            Some("literal-source-block"),
            false,
        );
        let task = create(
            &f,
            credential,
            request(f.document, project, Uuid::now_v7(), "Decode", &hash),
        )
        .await
        .unwrap()
        .unwrap()
        .task_id();
        let expected = inverse(&f, credential, task).await.unwrap().unwrap();
        let updated: i64 = sqlx::query_scalar("SELECT updated_at FROM documents WHERE id=?1")
            .bind(f.document.as_bytes().as_slice())
            .fetch_one(&f.pool)
            .await
            .unwrap();
        // Valid INTEGER storage, outside chrono's supported instant. No schema weakening.
        sqlx::query("UPDATE documents SET updated_at=?1 WHERE id=?2")
            .bind(i64::MAX)
            .bind(f.document.as_bytes().as_slice())
            .execute(&f.pool)
            .await
            .unwrap();
        assert!(
            matches!(inverse(&f, credential, task).await.unwrap_err(), sqlx::Error::Protocol(message) if message == "SQLite instant out of range")
        );
        let mut repair = f.backend.begin_write().await.unwrap();
        let OperationTx::SqliteFamily(writer) = repair.operation() else {
            unreachable!()
        };
        writer
            .execute(
                "UPDATE documents SET updated_at=?1 WHERE id=?2",
                &[Cell::Integer(updated), Cell::uuid(f.document)],
            )
            .await
            .unwrap();
        repair.commit().await.unwrap();
        assert_eq!(
            inverse(&f, credential, task).await.unwrap().unwrap(),
            expected
        );
        let fk = sqlx::query(
            "UPDATE task_origins SET document_id=?1 WHERE workspace_id=?2 AND task_id=?3",
        )
        .bind(Uuid::now_v7().as_bytes().as_slice())
        .bind(f.workspace.as_bytes().as_slice())
        .bind(task.as_bytes().as_slice())
        .execute(&f.pool)
        .await
        .unwrap_err();
        assert!(fk
            .as_database_error()
            .is_some_and(|error| error.is_foreign_key_violation()));
        assert_eq!(
            inverse(&f, credential, task).await.unwrap().unwrap(),
            expected
        );
        assert_eq!(
            sqlx::query_scalar::<_, i64>("PRAGMA foreign_keys")
                .fetch_one(&f.pool)
                .await
                .unwrap(),
            1
        );
        f.close().await;
    }

    #[tokio::test]
    async fn wiki_aux_origin_create_normal_literal_publication_new_client_replay_and_pagination() {
        let (f, credential, project) = setup().await;
        let before = state(&f, project).await;
        let title = "  실제 원본 작업 中 😀  ";
        let command = Uuid::now_v7();
        let hash = command_hash(f.user, project, title, Some("literal-source-block"), false);
        let first = create(
            &f,
            credential,
            request(f.document, project, command, title, &hash),
        )
        .await
        .unwrap()
        .unwrap();
        let DocumentTaskOutcome::Created(task) = first else {
            panic!("fresh command must create")
        };
        let row: CreatedTaskRow = sqlx::query_as("SELECT workspace_id,project_id,number,title,type,priority,content_json,schema_version,created_by,version FROM tasks WHERE id=?1")
            .bind(task.as_bytes().as_slice()).fetch_one(&f.pool).await.unwrap();
        assert_eq!(row.0, f.workspace.as_bytes());
        assert_eq!(row.1, project.as_bytes());
        assert_eq!(row.2, 2);
        assert_eq!(row.3, title.trim());
        assert_eq!(row.4, "task");
        assert_eq!(row.5, "none");
        assert_eq!(
            serde_json::from_str::<Value>(&row.6).unwrap(),
            crate::db::documents::empty_document_json()
        );
        assert_eq!(
            row.7,
            i64::from(crate::db::documents::DOCUMENT_SCHEMA_VERSION)
        );
        assert_eq!(row.8, f.user.as_bytes());
        assert_eq!(row.9, 1);
        let receipt:(Vec<u8>,Vec<u8>,String,String)=sqlx::query_as("SELECT task_id,document_id,request_hash,anchor FROM task_origins WHERE workspace_id=?1 AND document_id=?2 AND request_id=?3")
            .bind(f.workspace.as_bytes().as_slice()).bind(f.document.as_bytes().as_slice()).bind(command.as_bytes().as_slice()).fetch_one(&f.pool).await.unwrap();
        assert_eq!(
            receipt,
            (
                task.as_bytes().to_vec(),
                f.document.as_bytes().to_vec(),
                hash.clone(),
                "literal-source-block".into()
            )
        );
        let payload =
            json!({"taskId":task.to_string(),"projectId":project.to_string(),"title":title.trim()});
        let event: (String, String, String) =
            sqlx::query_as("SELECT verb,channel,payload FROM events WHERE target_id=?1")
                .bind(task.as_bytes().as_slice())
                .fetch_one(&f.pool)
                .await
                .unwrap();
        assert_eq!(event.0, "task.created");
        assert_eq!(event.1, "web");
        assert_eq!(serde_json::from_str::<Value>(&event.2).unwrap(), payload);
        let audit: (String, String, String) =
            sqlx::query_as("SELECT verb,payload,ip FROM audit_log WHERE target_id=?1")
                .bind(task.as_bytes().as_slice())
                .fetch_one(&f.pool)
                .await
                .unwrap();
        assert_eq!(audit.0, "task.created");
        assert_eq!(serde_json::from_str::<Value>(&audit.1).unwrap(), payload);
        assert_eq!(audit.2, "127.0.0.1");
        let activity: (Vec<u8>, String, String, String) = sqlx::query_as(
            "SELECT actor_user_id,channel,kind,changes FROM task_activity WHERE task_id=?1",
        )
        .bind(task.as_bytes().as_slice())
        .fetch_one(&f.pool)
        .await
        .unwrap();
        assert_eq!(
            activity,
            (
                f.user.as_bytes().to_vec(),
                "api".into(),
                "created".into(),
                "[]".into()
            )
        );
        let committed = state(&f, project).await;
        assert_eq!(
            committed,
            (
                before.0 + 1,
                before.1 + 1,
                3,
                before.3 + 1,
                before.4 + 1,
                before.5 + 1,
                before.6
            )
        );
        // Simulate a lost response after actual commit. A separately connected
        // client repeats the exact logical command, never a fresh request UUID.
        let client = crate::db::pool::connect_sqlite_app(&f.path, 1)
            .await
            .unwrap();
        let backend = Backend::Sqlite(client.clone());
        let replay = create_document_task_backend(
            &backend,
            f.workspace,
            f.user,
            credential,
            request(f.document, project, command, title, &hash),
            Some("127.0.0.1"),
            "api",
        )
        .await
        .unwrap()
        .unwrap();
        assert_eq!(replay, DocumentTaskOutcome::Replayed(task));
        assert_eq!(state(&f, project).await, committed);
        let page = list_document_task_origins_backend(
            &backend,
            f.workspace,
            f.user,
            credential,
            f.document,
            None,
            1,
        )
        .await
        .unwrap()
        .unwrap();
        assert_eq!(page.count, 1);
        assert_eq!(page.items.len(), 1);
        assert_eq!(page.items[0].task_id, task);
        assert_eq!(page.items[0].document_id, f.document);
        assert_eq!(page.items[0].task_display_id, "ORIGIN-2");
        assert_eq!(page.items[0].task_title, title.trim());
        assert_eq!(
            page.items[0].anchor.as_deref(),
            Some("literal-source-block")
        );
        let second_command = Uuid::now_v7();
        let second_title = "두 번째 작업";
        let second_hash = command_hash(
            f.user,
            project,
            second_title,
            Some("literal-source-block"),
            false,
        );
        let second = create(
            &f,
            credential,
            request(
                f.document,
                project,
                second_command,
                second_title,
                &second_hash,
            ),
        )
        .await
        .unwrap()
        .unwrap()
        .task_id();
        let page = list_document_task_origins_backend(
            &backend,
            f.workspace,
            f.user,
            credential,
            f.document,
            None,
            1,
        )
        .await
        .unwrap()
        .unwrap();
        assert_eq!(page.count, 2);
        assert_eq!(page.items.len(), 1);
        assert_eq!(page.next_cursor, Some(page.items[0].task_id));
        let tail = list_document_task_origins_backend(
            &backend,
            f.workspace,
            f.user,
            credential,
            f.document,
            page.next_cursor,
            1,
        )
        .await
        .unwrap()
        .unwrap();
        assert_eq!(tail.count, 2);
        assert_eq!(tail.items.len(), 1);
        assert_ne!(tail.items[0].task_id, page.items[0].task_id);
        let mut ids = vec![page.items[0].task_id, tail.items[0].task_id];
        ids.sort_unstable();
        let mut expected = vec![task, second];
        expected.sort_unstable();
        assert_eq!(ids, expected);
        assert_eq!(tail.next_cursor, None);
        backend.close().await.unwrap();
        f.close().await;
    }

    #[tokio::test]
    async fn wiki_aux_origin_create_hash_changes_archived_replay_and_current_target_task_denial() {
        let (f, credential, project) = setup().await;
        let command = Uuid::now_v7();
        let title = "Original";
        let original = command_hash(f.user, project, title, Some("literal-source-block"), false);
        let task = create(
            &f,
            credential,
            request(f.document, project, command, title, &original),
        )
        .await
        .unwrap()
        .unwrap()
        .task_id();
        let alternate = create_project_fixture(&f, credential, "OTHER").await;
        let before = state(&f, project).await;
        let other_actor = Uuid::now_v7();
        let other_credential = Uuid::now_v7();
        sqlx::query("INSERT INTO users(id,email,given_name) VALUES(?1,?2,'Other actor')")
            .bind(other_actor.as_bytes().as_slice())
            .bind(format!("{other_actor}@example.test"))
            .execute(&f.pool)
            .await
            .unwrap();
        sqlx::query("INSERT INTO memberships(workspace_id,user_id,role) VALUES(?1,?2,'member')")
            .bind(f.workspace.as_bytes().as_slice())
            .bind(other_actor.as_bytes().as_slice())
            .execute(&f.pool)
            .await
            .unwrap();
        sqlx::query("INSERT INTO project_members(id,workspace_id,project_id,user_id,role) VALUES(?1,?2,?3,?4,'member')").bind(Uuid::now_v7().as_bytes().as_slice()).bind(f.workspace.as_bytes().as_slice()).bind(project.as_bytes().as_slice()).bind(other_actor.as_bytes().as_slice()).execute(&f.pool).await.unwrap();
        let mut session = f.backend.begin_write().await.unwrap();
        session
            .operation()
            .create_session(
                other_credential,
                other_actor,
                "other-origin-actor",
                chrono::DateTime::from_timestamp_micros(
                    chrono::Utc::now().timestamp_micros() + 86_400_000_000,
                )
                .unwrap(),
            )
            .await
            .unwrap();
        session.commit().await.unwrap();
        let changed_actor = command_hash(
            other_actor,
            project,
            title,
            Some("literal-source-block"),
            false,
        );
        assert!(matches!(
            create_document_task_backend(
                &f.backend,
                f.workspace,
                other_actor,
                other_credential,
                request(f.document, project, command, title, &changed_actor),
                None,
                "api"
            )
            .await
            .unwrap(),
            Err(TaskOriginDbError::RequestMismatch)
        ));
        assert_eq!(state(&f, project).await, before);
        for (target, changed_title, anchor, self_assign) in [
            (project, "Changed", Some("literal-source-block"), false),
            (alternate, title, Some("literal-source-block"), false),
            (project, title, Some("changed-anchor"), false),
        ] {
            let changed = command_hash(f.user, target, changed_title, anchor, self_assign);
            let mut req = request(f.document, target, command, changed_title, &changed);
            req.anchor = anchor;
            assert!(matches!(
                create(&f, credential, req).await.unwrap(),
                Err(TaskOriginDbError::RequestMismatch)
            ));
            assert_eq!(state(&f, project).await, before);
        }
        sqlx::query("UPDATE projects SET status='archived' WHERE id=?1")
            .bind(project.as_bytes().as_slice())
            .execute(&f.pool)
            .await
            .unwrap();
        assert_eq!(
            create(
                &f,
                credential,
                request(f.document, project, command, title, &original)
            )
            .await
            .unwrap()
            .unwrap(),
            DocumentTaskOutcome::Replayed(task)
        );
        assert!(matches!(
            create(
                &f,
                credential,
                request(f.document, project, Uuid::now_v7(), title, &original)
            )
            .await
            .unwrap(),
            Err(TaskOriginDbError::Task(ProjectDbError::Archived))
        ));
        assert_eq!(state(&f, project).await, before);
        sqlx::query("UPDATE project_members SET role='viewer' WHERE project_id=?1 AND user_id=?2")
            .bind(project.as_bytes().as_slice())
            .bind(f.user.as_bytes().as_slice())
            .execute(&f.pool)
            .await
            .unwrap();
        assert!(matches!(
            create(
                &f,
                credential,
                request(f.document, project, command, title, &original)
            )
            .await
            .unwrap(),
            Err(TaskOriginDbError::NotFound)
        ));
        assert_eq!(state(&f, project).await, before);
        sqlx::query("UPDATE project_members SET role='lead' WHERE project_id=?1 AND user_id=?2")
            .bind(project.as_bytes().as_slice())
            .bind(f.user.as_bytes().as_slice())
            .execute(&f.pool)
            .await
            .unwrap();
        sqlx::query("UPDATE tasks SET deleted_at=1 WHERE id=?1")
            .bind(task.as_bytes().as_slice())
            .execute(&f.pool)
            .await
            .unwrap();
        assert!(matches!(
            create(
                &f,
                credential,
                request(f.document, project, command, title, &original)
            )
            .await
            .unwrap(),
            Err(TaskOriginDbError::NotFound)
        ));
        assert_eq!(state(&f, project).await, before);
        sqlx::query("UPDATE tasks SET deleted_at=NULL WHERE id=?1")
            .bind(task.as_bytes().as_slice())
            .execute(&f.pool)
            .await
            .unwrap();
        assert_eq!(
            create(
                &f,
                credential,
                request(f.document, project, command, title, &original)
            )
            .await
            .unwrap()
            .unwrap(),
            DocumentTaskOutcome::Replayed(task)
        );
        f.close().await;
    }

    #[tokio::test]
    async fn wiki_aux_origin_create_source_view_destination_edit_and_current_affiliation() {
        let (f, credential, target) = setup().await;
        let source = create_project_fixture(&f, credential, "SOURCE").await;
        let root: Vec<u8> = sqlx::query_scalar("SELECT root_document_id FROM projects WHERE id=?1")
            .bind(source.as_bytes().as_slice())
            .fetch_one(&f.pool)
            .await
            .unwrap();
        let parent = Uuid::from_slice(&root).unwrap();
        // A normal project child can move; the project root is not used as an
        // invalid cross-project move fixture.
        let document = Uuid::now_v7();
        sqlx::query("INSERT INTO documents(id,workspace_id,title,path,parent_id,sort_key,project_id,number,status,schema_version,content_json,created_by) VALUES(?1,?2,'Source child',?3,?4,'V',?5,17,'published',?6,?7,?8)")
            .bind(document.as_bytes().as_slice()).bind(f.workspace.as_bytes().as_slice())
            .bind(format!("{}.{}",crate::db::documents::to_path_label(parent),crate::db::documents::to_path_label(document)))
            .bind(parent.as_bytes().as_slice()).bind(source.as_bytes().as_slice()).bind(crate::db::documents::DOCUMENT_SCHEMA_VERSION)
            .bind(crate::db::documents::empty_document_json().to_string()).bind(f.user.as_bytes().as_slice()).execute(&f.pool).await.unwrap();
        sqlx::query("UPDATE projects SET next_number=18 WHERE id=?1")
            .bind(source.as_bytes().as_slice())
            .execute(&f.pool)
            .await
            .unwrap();
        sqlx::query("UPDATE project_members SET role='viewer' WHERE project_id=?1 AND user_id=?2")
            .bind(source.as_bytes().as_slice())
            .bind(f.user.as_bytes().as_slice())
            .execute(&f.pool)
            .await
            .unwrap();
        let mut read = f.backend.begin_read().await.unwrap();
        let mut op = read.operation();
        op.set_tenant(f.workspace).await.unwrap();
        assert_eq!(
            op.project_permission_by_id(f.workspace, f.user, source)
                .await
                .unwrap(),
            Some(ProjectPermission::View)
        );
        read.commit().await.unwrap();
        let command = Uuid::now_v7();
        let h = command_hash(
            f.user,
            target,
            "View source",
            Some("literal-source-block"),
            false,
        );
        let task = create(
            &f,
            credential,
            request(document, target, command, "View source", &h),
        )
        .await
        .unwrap()
        .unwrap()
        .task_id();
        let before = state(&f, target).await;
        sqlx::query("DELETE FROM project_members WHERE project_id=?1 AND user_id=?2")
            .bind(source.as_bytes().as_slice())
            .bind(f.user.as_bytes().as_slice())
            .execute(&f.pool)
            .await
            .unwrap();
        assert!(matches!(
            create(
                &f,
                credential,
                request(document, target, command, "View source", &h)
            )
            .await
            .unwrap(),
            Err(TaskOriginDbError::NotFound)
        ));
        assert_eq!(state(&f, target).await, before);
        sqlx::query("INSERT INTO project_members(id,workspace_id,project_id,user_id,role) VALUES(?1,?2,?3,?4,'viewer')").bind(Uuid::now_v7().as_bytes().as_slice()).bind(f.workspace.as_bytes().as_slice()).bind(source.as_bytes().as_slice()).bind(f.user.as_bytes().as_slice()).execute(&f.pool).await.unwrap();
        assert_eq!(
            create(
                &f,
                credential,
                request(document, target, command, "View source", &h)
            )
            .await
            .unwrap()
            .unwrap(),
            DocumentTaskOutcome::Replayed(task)
        );
        // The same current source row now affiliates with an ungranted private
        // project. Old permissions/receipt cannot authorize its replay.
        let moved = create_project_fixture(&f, credential, "MOVED").await;
        sqlx::query("DELETE FROM project_members WHERE project_id=?1 AND user_id=?2")
            .bind(moved.as_bytes().as_slice())
            .bind(f.user.as_bytes().as_slice())
            .execute(&f.pool)
            .await
            .unwrap();
        sqlx::query(
            "UPDATE documents SET project_id=?2,number=17,parent_id=NULL,path=?3 WHERE id=?1",
        )
        .bind(document.as_bytes().as_slice())
        .bind(moved.as_bytes().as_slice())
        .bind(crate::db::documents::to_path_label(document))
        .execute(&f.pool)
        .await
        .unwrap();
        let after = state(&f, target).await;
        assert!(matches!(
            create(
                &f,
                credential,
                request(document, target, command, "View source", &h)
            )
            .await
            .unwrap(),
            Err(TaskOriginDbError::NotFound)
        ));
        assert_eq!(state(&f, target).await, after);
        f.close().await;
    }

    #[tokio::test]
    async fn wiki_aux_origin_create_personal_owner_self_assignment_payload_and_false_replay() {
        let (f, credential, project) = setup().await;
        let command = Uuid::now_v7();
        let title = "Self";
        let true_hash = command_hash(f.user, project, title, Some("literal-source-block"), true);
        let mut req = request(f.document, project, command, title, &true_hash);
        req.self_assign = true;
        let before = state(&f, project).await;
        assert!(matches!(
            create(&f, credential, req).await.unwrap(),
            Err(TaskOriginDbError::NotFound)
        ));
        assert_eq!(state(&f, project).await, before);
        sqlx::query("UPDATE workspaces SET kind='personal' WHERE id=?1")
            .bind(f.workspace.as_bytes().as_slice())
            .execute(&f.pool)
            .await
            .unwrap();
        sqlx::query("UPDATE users SET personal_workspace_id=?2 WHERE id=?1")
            .bind(f.user.as_bytes().as_slice())
            .bind(f.workspace.as_bytes().as_slice())
            .execute(&f.pool)
            .await
            .unwrap();
        let mut req = request(f.document, project, command, title, &true_hash);
        req.self_assign = true;
        let task = create(&f, credential, req)
            .await
            .unwrap()
            .unwrap()
            .task_id();
        let assignments: Vec<Vec<u8>> = sqlx::query_scalar(
            "SELECT user_id FROM task_assignees WHERE workspace_id=?1 AND task_id=?2",
        )
        .bind(f.workspace.as_bytes().as_slice())
        .bind(task.as_bytes().as_slice())
        .fetch_all(&f.pool)
        .await
        .unwrap();
        assert_eq!(assignments, vec![f.user.as_bytes().to_vec()]);
        let expected = json!({"taskId":task.to_string(),"projectId":project.to_string(),"assigneeIds":[f.user.to_string()],"addedAssigneeIds":[f.user.to_string()]});
        let update: (String, String) = sqlx::query_as(
            "SELECT channel,payload FROM events WHERE target_id=?1 AND verb='task.updated'",
        )
        .bind(task.as_bytes().as_slice())
        .fetch_one(&f.pool)
        .await
        .unwrap();
        assert_eq!(update.0, "web");
        assert_eq!(serde_json::from_str::<Value>(&update.1).unwrap(), expected);
        let audit: (String, String) = sqlx::query_as(
            "SELECT payload,ip FROM audit_log WHERE target_id=?1 AND verb='task.updated'",
        )
        .bind(task.as_bytes().as_slice())
        .fetch_one(&f.pool)
        .await
        .unwrap();
        assert_eq!(serde_json::from_str::<Value>(&audit.0).unwrap(), expected);
        assert_eq!(audit.1, "127.0.0.1");
        let after = state(&f, project).await;
        let mut replay = request(f.document, project, command, title, &true_hash);
        replay.self_assign = true;
        assert_eq!(
            create(&f, credential, replay).await.unwrap().unwrap(),
            DocumentTaskOutcome::Replayed(task)
        );
        assert_eq!(state(&f, project).await, after);
        let false_hash = command_hash(f.user, project, title, Some("literal-source-block"), false);
        assert_ne!(false_hash, true_hash);
        assert!(matches!(
            create(
                &f,
                credential,
                request(f.document, project, command, title, &false_hash)
            )
            .await
            .unwrap(),
            Err(TaskOriginDbError::RequestMismatch)
        ));
        assert_eq!(state(&f, project).await, after);
        let failure_command = Uuid::now_v7();
        let failure_hash = command_hash(
            f.user,
            project,
            "Self failure",
            Some("literal-source-block"),
            true,
        );
        for table in ["events", "audit_log"] {
            let before_failure = state(&f, project).await;
            sqlx::query(&format!("CREATE TRIGGER reject_self_assignment BEFORE INSERT ON {table} WHEN NEW.verb='task.updated' BEGIN SELECT RAISE(ABORT,'self assignment publication failure'); END;")).execute(&f.pool).await.unwrap();
            let mut failure = request(
                f.document,
                project,
                failure_command,
                "Self failure",
                &failure_hash,
            );
            failure.self_assign = true;
            assert!(create(&f, credential, failure).await.is_err());
            assert_eq!(state(&f, project).await, before_failure);
            let missing: (i64,) =
                sqlx::query_as("SELECT count(*) FROM task_origins WHERE request_id=?1")
                    .bind(failure_command.as_bytes().as_slice())
                    .fetch_one(&f.pool)
                    .await
                    .unwrap();
            assert_eq!(missing.0, 0);
            sqlx::query("DROP TRIGGER reject_self_assignment")
                .execute(&f.pool)
                .await
                .unwrap();
        }
        let mut healthy = request(
            f.document,
            project,
            failure_command,
            "Self failure",
            &failure_hash,
        );
        healthy.self_assign = true;
        let healthy = create(&f, credential, healthy)
            .await
            .unwrap()
            .unwrap()
            .task_id();
        let assignment: (Vec<u8>,) =
            sqlx::query_as("SELECT user_id FROM task_assignees WHERE task_id=?1")
                .bind(healthy.as_bytes().as_slice())
                .fetch_one(&f.pool)
                .await
                .unwrap();
        assert_eq!(assignment.0, f.user.as_bytes());
        let after = state(&f, project).await;
        sqlx::query("UPDATE users SET personal_workspace_id=NULL WHERE id=?1")
            .bind(f.user.as_bytes().as_slice())
            .execute(&f.pool)
            .await
            .unwrap();
        let mut replay = request(f.document, project, command, title, &true_hash);
        replay.self_assign = true;
        assert!(matches!(
            create(&f, credential, replay).await.unwrap(),
            Err(TaskOriginDbError::NotFound)
        ));
        assert_eq!(state(&f, project).await, after);
        // With false/omitted intent normalized by the actual route helper,
        // neither owner proof nor assignment is added on create or replay.
        let plain = Uuid::now_v7();
        let omitted_wire = json!({"projectId":project,"requestId":plain,"anchor":"literal-source-block","task":{"title":title}});
        let mut false_wire = omitted_wire.clone();
        false_wire["selfAssign"] = Value::Bool(false);
        let omitted: crate::api::tasks_dto::DocumentTaskCreateBody =
            serde_json::from_value(omitted_wire).unwrap();
        let explicit_false: crate::api::tasks_dto::DocumentTaskCreateBody =
            serde_json::from_value(false_wire).unwrap();
        assert!(!omitted.self_assign && !explicit_false.self_assign);
        let mut plain_request = request(
            f.document,
            omitted.project_id,
            omitted.request_id,
            title,
            &false_hash,
        );
        plain_request.self_assign = omitted.self_assign;
        assert_eq!(request_hash_for(f.user, &plain_request), false_hash);
        let first = create(&f, credential, plain_request)
            .await
            .unwrap()
            .unwrap()
            .task_id();
        let plain_after = state(&f, project).await;
        let mut false_request = request(
            f.document,
            explicit_false.project_id,
            explicit_false.request_id,
            title,
            &false_hash,
        );
        false_request.self_assign = explicit_false.self_assign;
        assert_eq!(request_hash_for(f.user, &false_request), false_hash);
        assert_eq!(
            create(&f, credential, false_request)
                .await
                .unwrap()
                .unwrap(),
            DocumentTaskOutcome::Replayed(first)
        );
        assert_eq!(state(&f, project).await, plain_after);
        assert_eq!(plain_after.6, after.6);
        f.close().await;
    }

    #[tokio::test]
    async fn wiki_aux_origin_create_hierarchy_status_milestone_recurrence_and_no_effects() {
        let (f, credential, project) = setup().await;
        let before = state(&f, project).await;
        let command = Uuid::now_v7();
        let h = command_hash(
            f.user,
            project,
            "Validation",
            Some("literal-source-block"),
            false,
        );
        let mut req = request(f.document, project, command, "Validation", &h);
        req.task.task_type = "subtask";
        assert!(matches!(
            create(&f, credential, req).await.unwrap(),
            Err(TaskOriginDbError::Task(ProjectDbError::Conflict))
        ));
        assert_eq!(state(&f, project).await, before);
        for kind in ["parent", "status", "milestone"] {
            let mut req = request(f.document, project, command, "Validation", &h);
            let missing = Uuid::now_v7();
            match kind {
                "parent" => req.task.parent_id = Some(missing),
                "status" => req.task.status_id = Some(missing),
                _ => req.task.milestone_id = Some(missing),
            }
            let input_hash = request_hash_for(f.user, &req);
            req.request_hash = &input_hash;
            let result = create(&f, credential, req).await.unwrap();
            assert!(matches!(
                (kind, result),
                (
                    "parent",
                    Err(TaskOriginDbError::Task(ProjectDbError::NotFound))
                ) | (
                    "status",
                    Err(TaskOriginDbError::Task(ProjectDbError::StatusNotInWorkflow))
                ) | (
                    "milestone",
                    Err(TaskOriginDbError::Task(ProjectDbError::MilestoneNotFound))
                )
            ));
            assert_eq!(state(&f, project).await, before);
        }
        let parent = create(
            &f,
            credential,
            request(f.document, project, command, "Validation", &h),
        )
        .await
        .unwrap()
        .unwrap()
        .task_id();
        let recurrence = json!({"kind":"daily"});
        let mut child = request(f.document, project, Uuid::now_v7(), "Child", &h);
        child.task.parent_id = Some(parent);
        child.task.task_type = "subtask";
        child.task.start_date = chrono::NaiveDate::from_ymd_opt(2026, 10, 5);
        child.task.due_date = chrono::NaiveDate::from_ymd_opt(2026, 10, 8);
        child.task.recurrence = Some(recurrence.clone());
        let child_hash = request_hash_for(f.user, &child);
        child.request_hash = &child_hash;
        let child = create(&f, credential, child)
            .await
            .unwrap()
            .unwrap()
            .task_id();
        let row: (Vec<u8>, String, String, String, String) = sqlx::query_as(
            "SELECT parent_id,type,start_date,due_date,recurrence FROM tasks WHERE id=?1",
        )
        .bind(child.as_bytes().as_slice())
        .fetch_one(&f.pool)
        .await
        .unwrap();
        assert_eq!(row.0, parent.as_bytes());
        assert_eq!(row.1, "subtask");
        assert_eq!(row.2, "2026-10-05");
        assert_eq!(row.3, "2026-10-08");
        assert_eq!(serde_json::from_str::<Value>(&row.4).unwrap(), recurrence);
        let after = state(&f, project).await;
        let mut forbidden = request(f.document, project, Uuid::now_v7(), "Invalid hierarchy", &h);
        forbidden.task.parent_id = Some(child);
        assert!(matches!(
            create(&f, credential, forbidden).await.unwrap(),
            Err(TaskOriginDbError::Task(ProjectDbError::Conflict))
        ));
        assert_eq!(state(&f, project).await, after);
        f.close().await;
    }

    #[tokio::test]
    async fn wiki_aux_origin_create_each_publication_failure_and_real_deferred_fk_then_healthy_retry(
    ) {
        let (f, credential, project) = setup().await;
        let command = Uuid::now_v7();
        let h = command_hash(
            f.user,
            project,
            "Atomic",
            Some("literal-source-block"),
            false,
        );
        for (table, predicate) in [
            ("events", "NEW.verb='task.created'"),
            ("audit_log", "NEW.verb='task.created'"),
            ("task_activity", "NEW.kind='created'"),
            ("task_origins", "1"),
        ] {
            let before = state(&f, project).await;
            sqlx::query(&format!("CREATE TRIGGER reject_origin_publish BEFORE INSERT ON {table} WHEN {predicate} BEGIN SELECT RAISE(ABORT,'origin publication failure'); END;"))
                .execute(&f.pool).await.unwrap();
            let error = create(
                &f,
                credential,
                request(f.document, project, command, "Atomic", &h),
            )
            .await
            .unwrap_err();
            assert!(error.to_string().contains("origin publication failure"));
            assert_eq!(state(&f, project).await, before);
            let missing:(i64,i64)=sqlx::query_as("SELECT (SELECT count(*) FROM tasks WHERE title='Atomic'),(SELECT count(*) FROM task_origins WHERE request_id=?1)").bind(command.as_bytes().as_slice()).fetch_one(&f.pool).await.unwrap();
            assert_eq!(missing, (0, 0));
            sqlx::query("DROP TRIGGER reject_origin_publish")
                .execute(&f.pool)
                .await
                .unwrap();
        }
        sqlx::query("CREATE TABLE origin_deferred_fk_probe(workspace_id BLOB REFERENCES workspaces(id) DEFERRABLE INITIALLY DEFERRED) STRICT").execute(&f.pool).await.unwrap();
        sqlx::query("CREATE TRIGGER reject_origin_commit AFTER INSERT ON task_origins BEGIN INSERT INTO origin_deferred_fk_probe(workspace_id) VALUES(zeroblob(16)); END;").execute(&f.pool).await.unwrap();
        let before = state(&f, project).await;
        let error = create(
            &f,
            credential,
            request(f.document, project, command, "Atomic", &h),
        )
        .await
        .unwrap_err();
        let sqlx::Error::AnyDriverError(inner) = &error else {
            panic!("typed original commit uncertainty required: {error}")
        };
        let unknown = inner
            .downcast_ref::<crate::db::backend::CommitCleanupUnknown>()
            .expect("original commit receipt retained");
        assert_eq!(
            unknown.settlement,
            crate::db::backend::CommitSettlement::LocalWriterReconcile
        );
        assert!(unknown
            .source
            .source
            .as_database_error()
            .is_some_and(|db| db.is_foreign_key_violation()));
        // A fresh local transaction observes actual state after queued local
        // cleanup; it does not reassemble/recreate or prove remote settlement.
        let mut observation = f.backend.begin_read().await.unwrap();
        let OperationTx::SqliteFamily(family) = observation.operation() else {
            unreachable!()
        };
        let rows=family.query("SELECT (SELECT count(*) FROM tasks WHERE title='Atomic'),(SELECT count(*) FROM task_origins WHERE request_id=?1),(SELECT count(*) FROM origin_deferred_fk_probe)",&[Cell::uuid(command)]).await.unwrap();
        for column in 0..3 {
            assert_eq!(rows[0].cell(column).unwrap().integer().unwrap(), 0);
        }
        observation.commit().await.unwrap();
        assert_eq!(state(&f, project).await, before);
        sqlx::query("DROP TRIGGER reject_origin_commit")
            .execute(&f.pool)
            .await
            .unwrap();
        let task = create(
            &f,
            credential,
            request(f.document, project, command, "Atomic", &h),
        )
        .await
        .unwrap()
        .unwrap()
        .task_id();
        assert_eq!(
            state(&f, project).await,
            (
                before.0 + 1,
                before.1 + 1,
                before.2 + 1,
                before.3 + 1,
                before.4 + 1,
                before.5 + 1,
                before.6
            )
        );
        let retry = create(
            &f,
            credential,
            request(f.document, project, command, "Atomic", &h),
        )
        .await
        .unwrap()
        .unwrap();
        assert_eq!(retry, DocumentTaskOutcome::Replayed(task));
        f.close().await;
    }

    #[tokio::test]
    async fn wiki_aux_origin_create_concurrent_command_current_revoke_wrong_tenant_and_healthy_retry(
    ) {
        let (mut f, credential, project) = setup().await;
        f.pool.close().await;
        f.pool = crate::db::pool::connect_sqlite_app(&f.path, 3)
            .await
            .unwrap();
        f.backend = Backend::Sqlite(f.pool.clone());
        let command = Uuid::now_v7();
        let h = command_hash(f.user, project, "Race", Some("literal-source-block"), false);
        let before = state(&f, project).await;
        let (left, right) = tokio::join!(
            create(
                &f,
                credential,
                request(f.document, project, command, "Race", &h)
            ),
            create(
                &f,
                credential,
                request(f.document, project, command, "Race", &h)
            )
        );
        let outcomes = [left.unwrap().unwrap(), right.unwrap().unwrap()];
        assert_eq!(outcomes[0].task_id(), outcomes[1].task_id());
        assert_eq!(
            outcomes
                .iter()
                .filter(|r| matches!(r, DocumentTaskOutcome::Created(_)))
                .count(),
            1
        );
        assert_eq!(
            outcomes
                .iter()
                .filter(|r| matches!(r, DocumentTaskOutcome::Replayed(_)))
                .count(),
            1
        );
        assert_eq!(
            state(&f, project).await,
            (
                before.0 + 1,
                before.1 + 1,
                before.2 + 1,
                before.3 + 1,
                before.4 + 1,
                before.5 + 1,
                before.6
            )
        );
        let committed = state(&f, project).await;
        assert!(matches!(
            create_document_task_backend(
                &f.backend,
                f.workspace,
                Uuid::now_v7(),
                credential,
                request(f.document, project, command, "Race", &h),
                None,
                "web"
            )
            .await
            .unwrap(),
            Err(TaskOriginDbError::Forbidden)
        ));
        assert_eq!(state(&f, project).await, committed);
        assert!(matches!(
            create_document_task_backend(
                &f.backend,
                Uuid::now_v7(),
                f.user,
                credential,
                request(f.document, project, command, "Race", &h),
                None,
                "web"
            )
            .await
            .unwrap(),
            Err(TaskOriginDbError::NotFound)
        ));
        assert_eq!(state(&f, project).await, committed);
        sqlx::query("UPDATE documents SET deleted_at=1 WHERE id=?1")
            .bind(f.document.as_bytes().as_slice())
            .execute(&f.pool)
            .await
            .unwrap();
        assert!(matches!(
            create(
                &f,
                credential,
                request(f.document, project, command, "Race", &h)
            )
            .await
            .unwrap(),
            Err(TaskOriginDbError::NotFound)
        ));
        assert_eq!(state(&f, project).await, committed);
        sqlx::query("UPDATE documents SET deleted_at=NULL WHERE id=?1")
            .bind(f.document.as_bytes().as_slice())
            .execute(&f.pool)
            .await
            .unwrap();
        for (deny, restore, id) in [
            (
                "UPDATE users SET suspended_at=1 WHERE id=?1",
                "UPDATE users SET suspended_at=NULL WHERE id=?1",
                f.user,
            ),
            (
                "UPDATE sessions SET expires_at=1 WHERE id=?1",
                "UPDATE sessions SET expires_at=unixepoch()*1000000+86400000000 WHERE id=?1",
                credential,
            ),
            (
                "UPDATE workspaces SET deleted_at=1 WHERE id=?1",
                "UPDATE workspaces SET deleted_at=NULL WHERE id=?1",
                f.workspace,
            ),
        ] {
            sqlx::query(deny)
                .bind(id.as_bytes().as_slice())
                .execute(&f.pool)
                .await
                .unwrap();
            let denial = create(
                &f,
                credential,
                request(f.document, project, command, "Race", &h),
            )
            .await
            .unwrap();
            if id == f.workspace {
                assert!(matches!(denial, Err(TaskOriginDbError::NotFound)))
            } else {
                assert!(matches!(denial, Err(TaskOriginDbError::Forbidden)))
            }
            assert_eq!(state(&f, project).await, committed);
            sqlx::query(restore)
                .bind(id.as_bytes().as_slice())
                .execute(&f.pool)
                .await
                .unwrap();
        }
        let mut revoker = f.backend.begin_write().await.unwrap();
        let OperationTx::SqliteFamily(family) = revoker.operation() else {
            unreachable!()
        };
        family
            .execute(
                "UPDATE sessions SET revoked_at=1 WHERE id=?1",
                &[Cell::uuid(credential)],
            )
            .await
            .unwrap();
        let backend = f.backend.clone();
        let workspace = f.workspace;
        let actor = f.user;
        let document = f.document;
        let saved_hash = h.clone();
        let (started, waiting) = tokio::sync::oneshot::channel();
        let pending = tokio::spawn(async move {
            started.send(()).unwrap();
            create_document_task_backend(
                &backend,
                workspace,
                actor,
                credential,
                request(document, project, command, "Race", &saved_hash),
                None,
                "web",
            )
            .await
        });
        waiting.await.unwrap();
        revoker.commit().await.unwrap();
        assert!(matches!(
            pending.await.unwrap().unwrap(),
            Err(TaskOriginDbError::Forbidden)
        ));
        assert_eq!(state(&f, project).await, committed);
        sqlx::query("UPDATE sessions SET revoked_at=NULL WHERE id=?1")
            .bind(credential.as_bytes().as_slice())
            .execute(&f.pool)
            .await
            .unwrap();
        assert_eq!(
            create(
                &f,
                credential,
                request(f.document, project, command, "Race", &h)
            )
            .await
            .unwrap()
            .unwrap()
            .task_id(),
            outcomes[0].task_id()
        );
        assert_eq!(state(&f, project).await, committed);
        f.close().await;
    }
}
