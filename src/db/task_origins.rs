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
use crate::db::tasks::{create_task_tx, replace_task_assignees, CreateTaskInput};
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
#[error("task origin read refused: {0:?}")]
struct OriginReadRefusal(TaskOriginDbError);

async fn finish_origin_read<T>(
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
                    Some(Box::new(OriginReadRefusal(refusal))),
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
    let permission = if let Some(project) = row.project_id {
        op.project_permission_by_id(workspace, actor, project)
            .await?
            .unwrap_or(ProjectPermission::None)
    } else {
        op.document_permission(workspace, actor, document, true)
            .await?
    };
    Ok(permission
        .at_least(ProjectPermission::View)
        .then_some(row.project_id))
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
    finish_origin_read(tx, result).await
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
    finish_origin_read(tx, result).await
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
    let from_where = "FROM task_origins o
        JOIN tasks t ON t.workspace_id=o.workspace_id AND t.id=o.task_id AND t.deleted_at IS NULL
        JOIN projects tp ON tp.workspace_id=t.workspace_id AND tp.id=t.project_id AND tp.deleted_at IS NULL
        AND ((tp.visibility='workspace' AND ?2=0)
         OR EXISTS(SELECT 1 FROM project_members pm WHERE pm.workspace_id=tp.workspace_id AND pm.project_id=tp.id AND pm.user_id=?3)
         OR EXISTS(SELECT 1 FROM project_members pm JOIN group_members gm ON gm.workspace_id=pm.workspace_id AND gm.group_id=pm.group_id WHERE pm.workspace_id=tp.workspace_id AND pm.project_id=tp.id AND gm.user_id=?3 AND pm.group_id IS NOT NULL))
        JOIN documents d ON d.workspace_id=o.workspace_id AND d.id=o.document_id AND d.deleted_at IS NULL
        LEFT JOIN projects dp ON dp.workspace_id=d.workspace_id AND dp.id=d.project_id
        WHERE o.workspace_id=?1 AND o.document_id=?4";
    let args = [
        Cell::uuid(workspace),
        Cell::Integer(i64::from(guest)),
        Cell::uuid(actor),
        Cell::uuid(document),
    ];
    let count_rows = family
        .query(&format!("SELECT count(*) {from_where}"), &args)
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
    let rows = family.query(&format!("SELECT o.task_id,o.document_id,tp.key,t.number,t.title,dp.key,d.number,d.title,o.anchor {from_where} AND (?5 IS NULL OR o.task_id>?5) ORDER BY o.task_id LIMIT ?6"),&page_args).await?;
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
