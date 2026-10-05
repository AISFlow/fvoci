#![allow(clippy::too_many_arguments, clippy::type_complexity)]

use chrono::{DateTime, Utc};
use serde_json::json;
use sqlx::{PgPool, Postgres, Transaction};
use uuid::Uuid;

use crate::db::backend::{Backend, OperationTx};
use crate::db::codec::Cell;
use crate::db::context::{
    begin_read, lock_membership_users, lock_tree, recheck_session, session_is_live, set_tenant,
};
use crate::db::documents::{
    assert_document_writable, between, depth_of, empty_document_json, fetch_document_row,
    format_display_id, is_descendant, list_live_siblings_in, lock_document_rows, move_subtree,
    record_document_event_and_audit, resolve_reorder_sort_key, row_to_meta, subtree_ids,
    trash_document_row, trash_expired, CreateDocumentInput, DocumentDbError, DocumentMeta,
    TrashChildrenMode, TreeNode, UpdateDocumentMetaInput, DOCUMENT_SCHEMA_VERSION, MAX_TREE_DEPTH,
};
use crate::db::projects::{load_live_project, lock_project, project_permission};
use crate::db::workspace::workspace_is_live;
use crate::projects::ProjectPermission;

/// Live credential and workspace, then at least `min` on the live project.
/// `lock` takes the project row `FOR NO KEY UPDATE` for a caller that writes
/// in this transaction (archive, trash and member changes then serialize with
/// the write); a read passes `false` and runs in [`begin_read`], whose single
/// snapshot covers the check and the rows it returns.
pub(crate) async fn require_project_document_access(
    tx: &mut Transaction<'_, Postgres>,
    workspace_id: Uuid,
    actor_user_id: Uuid,
    session_id: Uuid,
    project_id: Uuid,
    min: ProjectPermission,
    lock: bool,
) -> Result<Result<(), DocumentDbError>, sqlx::Error> {
    if !session_is_live(tx, actor_user_id, session_id).await? {
        return Ok(Err(DocumentDbError::Forbidden));
    }
    if !workspace_is_live(tx, workspace_id).await? {
        return Ok(Err(DocumentDbError::NotFound));
    }
    let project = if lock {
        lock_project(tx, workspace_id, project_id).await?
    } else {
        load_live_project(tx, workspace_id, project_id).await?
    };
    let Some(project) = project else {
        return Ok(Err(DocumentDbError::NotFound));
    };
    if project.status == "archived" && min >= ProjectPermission::Edit {
        return Ok(Err(DocumentDbError::NotFound));
    }
    let permission = project_permission(tx, workspace_id, actor_user_id, &project).await?;
    if !permission.at_least(min) {
        return Ok(Err(DocumentDbError::NotFound));
    }
    Ok(Ok(()))
}

pub(crate) async fn assert_project_document(
    tx: &mut Transaction<'_, Postgres>,
    workspace_id: Uuid,
    project_id: Uuid,
    document_id: Uuid,
    require_live: bool,
    on_affiliation_mismatch: DocumentDbError,
) -> Result<Result<(), DocumentDbError>, sqlx::Error> {
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
    let Some((doc_project_id, deleted_at)) = row else {
        return Ok(Err(DocumentDbError::NotFound));
    };
    if doc_project_id != Some(project_id) {
        return Ok(Err(on_affiliation_mismatch));
    }
    if require_live && deleted_at.is_some() {
        return Ok(Err(DocumentDbError::NotFound));
    }
    Ok(Ok(()))
}

pub(crate) async fn project_key(
    tx: &mut Transaction<'_, Postgres>,
    workspace_id: Uuid,
    project_id: Uuid,
) -> Result<Option<String>, sqlx::Error> {
    let row: Option<(String,)> = sqlx::query_as(
        "SELECT key FROM fvoci.projects WHERE workspace_id = $1 AND id = $2 AND deleted_at IS NULL",
    )
    .bind(workspace_id)
    .bind(project_id)
    .fetch_optional(&mut **tx)
    .await?;
    Ok(row.map(|(key,)| key))
}

pub(crate) fn with_project_display_id(meta: DocumentMeta, project_key: &str) -> DocumentMeta {
    let mut meta = meta;
    meta.display_id = Some(format_display_id(project_key, meta.number));
    meta
}

fn map_tree_rows(
    rows: Vec<(
        Uuid,
        Uuid,
        Option<Uuid>,
        Option<Uuid>,
        String,
        Option<String>,
        String,
        String,
        i32,
        String,
    )>,
) -> Vec<TreeNode> {
    rows.into_iter()
        .map(
            |(
                id,
                workspace_id,
                parent_id,
                project_id,
                title,
                icon,
                path,
                sort_key,
                number,
                status,
            )| TreeNode {
                id,
                workspace_id,
                parent_id,
                project_id,
                title,
                icon,
                path,
                sort_key,
                number,
                status,
            },
        )
        .collect()
}

pub async fn list_project_document_tree(
    pool: &PgPool,
    workspace_id: Uuid,
    project_id: Uuid,
    actor_user_id: Uuid,
    session_id: Uuid,
) -> Result<Result<Vec<TreeNode>, DocumentDbError>, sqlx::Error> {
    let mut tx = begin_read(pool).await?;
    set_tenant(&mut tx, workspace_id).await?;
    match require_project_document_access(
        &mut tx,
        workspace_id,
        actor_user_id,
        session_id,
        project_id,
        ProjectPermission::View,
        false,
    )
    .await?
    {
        Ok(()) => {}
        Err(err) => {
            tx.rollback().await?;
            return Ok(Err(err));
        }
    }
    let rows = sqlx::query_as::<
        _,
        (
            Uuid,
            Uuid,
            Option<Uuid>,
            Option<Uuid>,
            String,
            Option<String>,
            String,
            String,
            i32,
            String,
        ),
    >(
        r#"
        SELECT id, workspace_id, parent_id, project_id, title, icon, path, sort_key, number, status
        FROM fvoci.documents
        WHERE workspace_id = $1 AND project_id = $2 AND deleted_at IS NULL
        ORDER BY sort_key COLLATE "C"
        "#,
    )
    .bind(workspace_id)
    .bind(project_id)
    .fetch_all(&mut *tx)
    .await?;
    tx.commit().await?;
    Ok(Ok(map_tree_rows(rows)))
}

/// Selected-backend twin of the existing project tree reader. Family reads
/// keep credentials, visibility, optional tag and rows in one snapshot. PG
/// retains its original reader/tag-helper contract. Archived projects remain
/// readable and never gain creation permission here.
pub async fn list_project_document_tree_backend(
    backend: &Backend,
    workspace: Uuid,
    project: Uuid,
    actor: Uuid,
    credential: Uuid,
    tag: Option<Uuid>,
) -> Result<Result<Vec<TreeNode>, DocumentDbError>, sqlx::Error> {
    if let Backend::Postgres(pool) = backend {
        let result =
            list_project_document_tree(pool, workspace, project, actor, credential).await?;
        return match (result, tag) {
            (Ok(nodes), Some(tag)) => {
                let tagged =
                    super::document_tags::tagged_document_id_set(pool, workspace, tag).await?;
                Ok(Ok(nodes
                    .into_iter()
                    .filter(|node| tagged.contains(&node.id))
                    .collect()))
            }
            (result, _) => Ok(result),
        };
    }
    let mut tx = backend.begin_read().await?;
    let result: Result<Result<Vec<TreeNode>, DocumentDbError>, sqlx::Error> = async {
        let mut op = tx.operation();
        op.set_tenant(workspace).await?;
        if !op.session_is_live(actor, credential).await? {
            return Ok(Err(DocumentDbError::Forbidden));
        }
        if !op.workspace_is_live(workspace).await? {
            return Ok(Err(DocumentDbError::NotFound));
        }
        if !op
            .project_permission_by_id(workspace, actor, project)
            .await?
            .is_some_and(|permission| permission.at_least(ProjectPermission::View))
        {
            return Ok(Err(DocumentDbError::NotFound));
        }
        let OperationTx::SqliteFamily(family) = op else {
            unreachable!("PostgreSQL uses its preserved public reader")
        };
        family.require_tenant(workspace)?;
        let rows = family.query(
            "SELECT id,workspace_id,parent_id,project_id,title,icon,path,sort_key,number,status FROM documents WHERE workspace_id=?1 AND project_id=?2 AND deleted_at IS NULL AND (?3 IS NULL OR EXISTS(SELECT 1 FROM document_tag_assignments a WHERE a.workspace_id=documents.workspace_id AND a.document_id=documents.id AND a.tag_id=?3)) ORDER BY sort_key COLLATE BINARY",
            &[Cell::uuid(workspace), Cell::uuid(project), Cell::optional_uuid(tag)],
        ).await?;
        let nodes = rows.iter().map(|row| {
            Ok(TreeNode {
                id: row.cell(0)?.id()?,
                workspace_id: row.cell(1)?.id()?,
                parent_id: row.cell(2)?.optional(Cell::id)?,
                project_id: row.cell(3)?.optional(Cell::id)?,
                title: row.cell(4)?.string()?,
                icon: row.cell(5)?.optional(Cell::string)?,
                path: row.cell(6)?.string()?,
                sort_key: row.cell(7)?.string()?,
                number: row.cell(8)?.int32()?,
                status: row.cell(9)?.string()?,
            })
        }).collect::<Result<Vec<_>,sqlx::Error>>()?;
        Ok(Ok(nodes))
    }.await;
    // This is read-only. Await this transaction's own release on both domain
    // denial and success; a dropped/failed remote release is not an ACK.
    tx.rollback().await?;
    result
}

pub async fn create_project_document(
    pool: &PgPool,
    workspace_id: Uuid,
    project_id: Uuid,
    actor_user_id: Uuid,
    session_id: Uuid,
    input: CreateDocumentInput<'_>,
    client_ip: Option<&str>,
) -> Result<Result<DocumentMeta, DocumentDbError>, sqlx::Error> {
    let document_id = Uuid::now_v7();
    let mut tx = pool.begin().await?;
    set_tenant(&mut tx, workspace_id).await?;
    lock_membership_users(&mut tx, &[actor_user_id]).await?;
    if !recheck_session(&mut tx, actor_user_id, session_id).await? {
        tx.rollback().await?;
        return Ok(Err(DocumentDbError::Forbidden));
    }
    lock_tree(&mut tx, workspace_id).await?;
    match require_project_document_access(
        &mut tx,
        workspace_id,
        actor_user_id,
        session_id,
        project_id,
        ProjectPermission::Edit,
        true,
    )
    .await?
    {
        Ok(()) => {}
        Err(err) => {
            tx.rollback().await?;
            return Ok(Err(err));
        }
    }

    let parent_id = input.parent_id;
    let parent_path = if let Some(parent_id) = parent_id {
        match assert_project_document(
            &mut tx,
            workspace_id,
            project_id,
            parent_id,
            true,
            DocumentDbError::AffiliationMismatch,
        )
        .await?
        {
            Ok(()) => {}
            Err(err) => {
                tx.rollback().await?;
                return Ok(Err(err));
            }
        }
        let parent: Option<(String,)> =
            sqlx::query_as("SELECT path FROM fvoci.documents WHERE workspace_id = $1 AND id = $2")
                .bind(workspace_id)
                .bind(parent_id)
                .fetch_optional(&mut *tx)
                .await?;
        let Some((path,)) = parent else {
            tx.rollback().await?;
            return Ok(Err(DocumentDbError::NotFound));
        };
        path
    } else {
        tx.rollback().await?;
        return Ok(Err(DocumentDbError::NotFound));
    };

    if depth_of(&parent_path) >= MAX_TREE_DEPTH {
        tx.rollback().await?;
        return Ok(Err(DocumentDbError::DepthLimit));
    }

    let last_sort: Option<(String,)> = sqlx::query_as(
        r#"
        SELECT sort_key
        FROM fvoci.documents
        WHERE workspace_id = $1 AND parent_id = $2 AND deleted_at IS NULL
        ORDER BY sort_key COLLATE "C" DESC
        LIMIT 1
        "#,
    )
    .bind(workspace_id)
    .bind(parent_id)
    .fetch_optional(&mut *tx)
    .await?;
    let sort_key = match between(last_sort.as_ref().map(|(k,)| k.as_str()), None) {
        Ok(key) => key,
        Err(_) => {
            tx.rollback().await?;
            return Ok(Err(DocumentDbError::InvalidSortKey));
        }
    };
    let path = format!(
        "{}.{}",
        parent_path,
        crate::db::documents::to_path_label(document_id)
    );
    let icon = match input.icon {
        Some(value) => value.map(str::to_string),
        None => None,
    };

    let number: (i32,) = sqlx::query_as(
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
            id, workspace_id, title, icon, path, parent_id, sort_key, project_id,
            number, status, schema_version, content_json, created_by, kind
        ) VALUES (
            $1, $2, $3, $4, $5, $6, $7, $8,
            $9, 'draft', $10, $11, $12, 'doc'
        )
        "#,
    )
    .bind(document_id)
    .bind(workspace_id)
    .bind(input.title)
    .bind(icon)
    .bind(&path)
    .bind(parent_id)
    .bind(&sort_key)
    .bind(project_id)
    .bind(number.0)
    .bind(DOCUMENT_SCHEMA_VERSION)
    .bind(empty_document_json())
    .bind(actor_user_id)
    .execute(&mut *tx)
    .await?;

    record_document_event_and_audit(
        &mut tx,
        workspace_id,
        actor_user_id,
        "document.created",
        document_id,
        json!({
            "documentId": document_id.to_string(),
            "parentId": parent_id.map(|id| id.to_string()),
            "title": input.title,
            "projectId": project_id.to_string(),
        }),
        client_ip,
    )
    .await?;

    let project_key = project_key(&mut tx, workspace_id, project_id).await?;
    let Some(project_key) = project_key else {
        tx.rollback().await?;
        return Ok(Err(DocumentDbError::NotFound));
    };
    let row = fetch_document_row(&mut tx, workspace_id, document_id).await?;
    tx.commit().await?;
    match row {
        Some(row) => Ok(Ok(with_project_display_id(
            row_to_meta(row, true),
            &project_key,
        ))),
        None => Ok(Err(DocumentDbError::NotFound)),
    }
}

pub async fn get_project_document(
    pool: &PgPool,
    workspace_id: Uuid,
    project_id: Uuid,
    document_id: Uuid,
    actor_user_id: Uuid,
    session_id: Uuid,
) -> Result<Result<DocumentMeta, DocumentDbError>, sqlx::Error> {
    let mut tx = begin_read(pool).await?;
    set_tenant(&mut tx, workspace_id).await?;
    match require_project_document_access(
        &mut tx,
        workspace_id,
        actor_user_id,
        session_id,
        project_id,
        ProjectPermission::View,
        false,
    )
    .await?
    {
        Ok(()) => {}
        Err(err) => {
            tx.rollback().await?;
            return Ok(Err(err));
        }
    }
    match assert_project_document(
        &mut tx,
        workspace_id,
        project_id,
        document_id,
        true,
        DocumentDbError::NotFound,
    )
    .await?
    {
        Ok(()) => {}
        Err(err) => {
            tx.rollback().await?;
            return Ok(Err(err));
        }
    }
    let project_key = project_key(&mut tx, workspace_id, project_id).await?;
    let Some(project_key) = project_key else {
        tx.rollback().await?;
        return Ok(Err(DocumentDbError::NotFound));
    };
    let row = fetch_document_row(&mut tx, workspace_id, document_id).await?;
    tx.commit().await?;
    match row {
        Some(row) => Ok(Ok(with_project_display_id(
            row_to_meta(row, true),
            &project_key,
        ))),
        None => Ok(Err(DocumentDbError::NotFound)),
    }
}

pub async fn update_project_document_meta(
    pool: &PgPool,
    workspace_id: Uuid,
    project_id: Uuid,
    document_id: Uuid,
    actor_user_id: Uuid,
    session_id: Uuid,
    input: UpdateDocumentMetaInput<'_>,
    client_ip: Option<&str>,
) -> Result<Result<DocumentMeta, DocumentDbError>, sqlx::Error> {
    let mut tx = pool.begin().await?;
    set_tenant(&mut tx, workspace_id).await?;
    lock_membership_users(&mut tx, &[actor_user_id]).await?;
    if !recheck_session(&mut tx, actor_user_id, session_id).await? {
        tx.rollback().await?;
        return Ok(Err(DocumentDbError::Forbidden));
    }
    match require_project_document_access(
        &mut tx,
        workspace_id,
        actor_user_id,
        session_id,
        project_id,
        ProjectPermission::Edit,
        true,
    )
    .await?
    {
        Ok(()) => {}
        Err(err) => {
            tx.rollback().await?;
            return Ok(Err(err));
        }
    }
    match assert_project_document(
        &mut tx,
        workspace_id,
        project_id,
        document_id,
        true,
        DocumentDbError::NotFound,
    )
    .await?
    {
        Ok(()) => {}
        Err(err) => {
            tx.rollback().await?;
            return Ok(Err(err));
        }
    }
    match assert_document_writable(&mut tx, workspace_id, document_id, Some(project_id)).await? {
        Ok(()) => {}
        Err(err) => {
            tx.rollback().await?;
            return Ok(Err(err));
        }
    }

    let current = fetch_document_row(&mut tx, workspace_id, document_id).await?;
    let Some(current) = current else {
        tx.rollback().await?;
        return Ok(Err(DocumentDbError::NotFound));
    };
    let title = input.title.unwrap_or(&current.2);
    let icon = match input.icon {
        Some(value) => value.map(str::to_string),
        None => current.4.clone(),
    };
    let status = input.status.unwrap_or(&current.9);

    sqlx::query(
        r#"
        UPDATE fvoci.documents
        SET title = $3, icon = $4, status = $5, version = version + 1, updated_at = now()
        WHERE workspace_id = $1 AND id = $2 AND deleted_at IS NULL
        "#,
    )
    .bind(workspace_id)
    .bind(document_id)
    .bind(title)
    .bind(icon)
    .bind(status)
    .execute(&mut *tx)
    .await?;

    let mut payload = json!({
        "documentId": document_id.to_string(),
        "projectId": project_id.to_string(),
    });
    if let Some(title) = input.title {
        payload["title"] = json!(title);
    }
    if let Some(icon) = input.icon {
        payload["icon"] = json!(icon);
    }
    if let Some(status) = input.status {
        payload["status"] = json!(status);
    }
    record_document_event_and_audit(
        &mut tx,
        workspace_id,
        actor_user_id,
        "document.updated",
        document_id,
        payload,
        client_ip,
    )
    .await?;

    let project_key = project_key(&mut tx, workspace_id, project_id).await?;
    let Some(project_key) = project_key else {
        tx.rollback().await?;
        return Ok(Err(DocumentDbError::NotFound));
    };
    let row = fetch_document_row(&mut tx, workspace_id, document_id).await?;
    tx.commit().await?;
    match row {
        Some(row) => Ok(Ok(with_project_display_id(
            row_to_meta(row, true),
            &project_key,
        ))),
        None => Ok(Err(DocumentDbError::NotFound)),
    }
}

pub async fn move_project_document(
    pool: &PgPool,
    workspace_id: Uuid,
    project_id: Uuid,
    document_id: Uuid,
    actor_user_id: Uuid,
    session_id: Uuid,
    new_parent_id: Uuid,
    client_ip: Option<&str>,
) -> Result<Result<DocumentMeta, DocumentDbError>, sqlx::Error> {
    let mut tx = pool.begin().await?;
    set_tenant(&mut tx, workspace_id).await?;
    lock_membership_users(&mut tx, &[actor_user_id]).await?;
    if !recheck_session(&mut tx, actor_user_id, session_id).await? {
        tx.rollback().await?;
        return Ok(Err(DocumentDbError::Forbidden));
    }
    lock_tree(&mut tx, workspace_id).await?;
    match require_project_document_access(
        &mut tx,
        workspace_id,
        actor_user_id,
        session_id,
        project_id,
        ProjectPermission::Edit,
        true,
    )
    .await?
    {
        Ok(()) => {}
        Err(err) => {
            tx.rollback().await?;
            return Ok(Err(err));
        }
    }

    let subtree = subtree_ids(&mut tx, workspace_id, document_id).await?;
    lock_document_rows(&mut tx, workspace_id, &subtree).await?;

    let doc: Option<(Option<Uuid>, Option<DateTime<Utc>>, String, Option<Uuid>)> = sqlx::query_as(
        r#"
            SELECT project_id, deleted_at, path, parent_id
            FROM fvoci.documents
            WHERE workspace_id = $1 AND id = $2
            "#,
    )
    .bind(workspace_id)
    .bind(document_id)
    .fetch_optional(&mut *tx)
    .await?;
    let Some((doc_project_id, deleted_at, doc_path, parent_id)) = doc else {
        tx.rollback().await?;
        return Ok(Err(DocumentDbError::NotFound));
    };
    if deleted_at.is_some() || doc_project_id != Some(project_id) {
        tx.rollback().await?;
        return Ok(Err(DocumentDbError::NotFound));
    }
    if parent_id.is_none() {
        tx.rollback().await?;
        return Ok(Err(DocumentDbError::RootDocumentMove));
    }

    match assert_project_document(
        &mut tx,
        workspace_id,
        project_id,
        new_parent_id,
        true,
        DocumentDbError::NotFound,
    )
    .await?
    {
        Ok(()) => {}
        Err(err) => {
            tx.rollback().await?;
            return Ok(Err(err));
        }
    }
    let parent: Option<(String,)> =
        sqlx::query_as("SELECT path FROM fvoci.documents WHERE workspace_id = $1 AND id = $2")
            .bind(workspace_id)
            .bind(new_parent_id)
            .fetch_optional(&mut *tx)
            .await?;
    let Some((parent_path,)) = parent else {
        tx.rollback().await?;
        return Ok(Err(DocumentDbError::NotFound));
    };
    if is_descendant(&mut tx, workspace_id, new_parent_id, document_id).await? {
        tx.rollback().await?;
        return Ok(Err(DocumentDbError::Cycle));
    }
    let own_depth = depth_of(&doc_path);
    let new_depth = depth_of(&parent_path) + 1;
    let mut max_relative_depth = 0i32;
    for id in &subtree {
        if *id == document_id {
            continue;
        }
        let row: Option<(String,)> =
            sqlx::query_as("SELECT path FROM fvoci.documents WHERE workspace_id = $1 AND id = $2")
                .bind(workspace_id)
                .bind(id)
                .fetch_optional(&mut *tx)
                .await?;
        if let Some((path,)) = row {
            max_relative_depth = max_relative_depth.max(depth_of(&path) - own_depth);
        }
    }
    if new_depth + max_relative_depth > MAX_TREE_DEPTH {
        tx.rollback().await?;
        return Ok(Err(DocumentDbError::DepthLimit));
    }
    let new_path = format!(
        "{}.{}",
        parent_path,
        crate::db::documents::to_path_label(document_id)
    );
    move_subtree(
        &mut tx,
        workspace_id,
        document_id,
        Some(new_parent_id),
        &new_path,
        Some(project_id),
    )
    .await?;

    record_document_event_and_audit(
        &mut tx,
        workspace_id,
        actor_user_id,
        "document.moved",
        document_id,
        json!({
            "documentId": document_id.to_string(),
            "oldProjectId": project_id.to_string(),
            "newProjectId": project_id.to_string(),
            "newParentId": new_parent_id.to_string(),
        }),
        client_ip,
    )
    .await?;

    let project_key = project_key(&mut tx, workspace_id, project_id).await?;
    let Some(project_key) = project_key else {
        tx.rollback().await?;
        return Ok(Err(DocumentDbError::NotFound));
    };
    let row = fetch_document_row(&mut tx, workspace_id, document_id).await?;
    tx.commit().await?;
    match row {
        Some(row) => Ok(Ok(with_project_display_id(
            row_to_meta(row, true),
            &project_key,
        ))),
        None => Ok(Err(DocumentDbError::NotFound)),
    }
}

async fn project_root_document_id(
    tx: &mut Transaction<'_, Postgres>,
    workspace_id: Uuid,
    project_id: Uuid,
) -> Result<Option<Uuid>, sqlx::Error> {
    let row: Option<(Option<Uuid>,)> = sqlx::query_as(
        "SELECT root_document_id FROM fvoci.projects WHERE workspace_id = $1 AND id = $2",
    )
    .bind(workspace_id)
    .bind(project_id)
    .fetch_optional(&mut **tx)
    .await?;
    Ok(row.and_then(|(root,)| root))
}

/// Shared prologue of project document tree mutations, in the project document
/// lock order: actor advisory lock → session recheck → workspace tree lock →
/// project row (`FOR NO KEY UPDATE`, writable) → document rows.
async fn begin_project_tree_write(
    tx: &mut Transaction<'_, Postgres>,
    workspace_id: Uuid,
    project_id: Uuid,
    actor_user_id: Uuid,
    session_id: Uuid,
) -> Result<Result<(), DocumentDbError>, sqlx::Error> {
    set_tenant(tx, workspace_id).await?;
    lock_membership_users(tx, &[actor_user_id]).await?;
    if !recheck_session(tx, actor_user_id, session_id).await? {
        return Ok(Err(DocumentDbError::Forbidden));
    }
    lock_tree(tx, workspace_id).await?;
    require_project_document_access(
        tx,
        workspace_id,
        actor_user_id,
        session_id,
        project_id,
        ProjectPermission::Edit,
        true,
    )
    .await
}

/// Source `trashDocument` for a project document: the project root cannot be
/// trashed; `Trash` trashes the live subtree, `Reparent` moves direct children
/// to the parent (always inside the same project) and trashes only the target.
pub async fn trash_project_document(
    pool: &PgPool,
    workspace_id: Uuid,
    project_id: Uuid,
    document_id: Uuid,
    actor_user_id: Uuid,
    session_id: Uuid,
    children: TrashChildrenMode,
    client_ip: Option<&str>,
) -> Result<Result<(), DocumentDbError>, sqlx::Error> {
    let now = Utc::now();
    let mut tx = pool.begin().await?;
    if let Err(err) =
        begin_project_tree_write(&mut tx, workspace_id, project_id, actor_user_id, session_id)
            .await?
    {
        tx.rollback().await?;
        return Ok(Err(err));
    }
    if let Err(err) = assert_project_document(
        &mut tx,
        workspace_id,
        project_id,
        document_id,
        true,
        DocumentDbError::NotFound,
    )
    .await?
    {
        tx.rollback().await?;
        return Ok(Err(err));
    }
    if project_root_document_id(&mut tx, workspace_id, project_id).await? == Some(document_id) {
        tx.rollback().await?;
        return Ok(Err(DocumentDbError::RootDocumentTrash));
    }

    if children == TrashChildrenMode::Reparent {
        let doc: Option<(Option<Uuid>,)> = sqlx::query_as(
            "SELECT parent_id FROM fvoci.documents WHERE workspace_id = $1 AND id = $2",
        )
        .bind(workspace_id)
        .bind(document_id)
        .fetch_optional(&mut *tx)
        .await?;
        let Some((Some(parent_id),)) = doc else {
            tx.rollback().await?;
            return Ok(Err(DocumentDbError::NotFound));
        };
        let parent: Option<(Option<Uuid>, Option<DateTime<Utc>>, String)> = sqlx::query_as(
            "SELECT project_id, deleted_at, path FROM fvoci.documents WHERE workspace_id = $1 AND id = $2",
        )
        .bind(workspace_id)
        .bind(parent_id)
        .fetch_optional(&mut *tx)
        .await?;
        let Some((parent_project_id, None, parent_path)) = parent else {
            tx.rollback().await?;
            return Ok(Err(DocumentDbError::NotFound));
        };
        if parent_project_id != Some(project_id) {
            tx.rollback().await?;
            return Ok(Err(DocumentDbError::NotFound));
        }
        let direct_children =
            list_live_siblings_in(&mut tx, workspace_id, Some(project_id), Some(document_id))
                .await?;
        let dest_siblings =
            list_live_siblings_in(&mut tx, workspace_id, Some(project_id), Some(parent_id)).await?;
        let mut last_key = dest_siblings
            .iter()
            .rev()
            .find(|node| node.id != document_id)
            .map(|node| node.sort_key.clone());
        for child in direct_children {
            let child_subtree = subtree_ids(&mut tx, workspace_id, child.id).await?;
            lock_document_rows(&mut tx, workspace_id, &child_subtree).await?;
            let old_path = child.path.clone();
            let new_path = format!(
                "{}.{}",
                parent_path,
                crate::db::documents::to_path_label(child.id)
            );
            move_subtree(
                &mut tx,
                workspace_id,
                child.id,
                Some(parent_id),
                &new_path,
                Some(project_id),
            )
            .await?;
            let Ok(sort_key) = between(last_key.as_deref(), None) else {
                tx.rollback().await?;
                return Ok(Err(DocumentDbError::InvalidSortKey));
            };
            sqlx::query(
                "UPDATE fvoci.documents SET sort_key = $3, updated_at = now() WHERE workspace_id = $1 AND id = $2",
            )
            .bind(workspace_id)
            .bind(child.id)
            .bind(&sort_key)
            .execute(&mut *tx)
            .await?;
            last_key = Some(sort_key);
            record_document_event_and_audit(
                &mut tx,
                workspace_id,
                actor_user_id,
                "document.moved",
                child.id,
                json!({
                    "documentId": child.id.to_string(),
                    "newParentId": parent_id.to_string(),
                    "newPath": new_path,
                    "oldParentId": document_id.to_string(),
                    "oldPath": old_path,
                    "oldProjectId": project_id.to_string(),
                    "newProjectId": project_id.to_string(),
                }),
                client_ip,
            )
            .await?;
        }
        lock_document_rows(&mut tx, workspace_id, &[document_id]).await?;
        if !trash_document_row(&mut tx, workspace_id, document_id, now).await? {
            tx.rollback().await?;
            return Ok(Err(DocumentDbError::NotFound));
        }
        record_document_event_and_audit(
            &mut tx,
            workspace_id,
            actor_user_id,
            "document.trashed",
            document_id,
            json!({ "documentId": document_id.to_string(), "projectId": project_id.to_string() }),
            client_ip,
        )
        .await?;
        tx.commit().await?;
        return Ok(Ok(()));
    }

    let subtree = subtree_ids(&mut tx, workspace_id, document_id).await?;
    lock_document_rows(&mut tx, workspace_id, &subtree).await?;
    for id in subtree {
        let live: Option<(Option<DateTime<Utc>>, Option<Uuid>)> = sqlx::query_as(
            "SELECT deleted_at, project_id FROM fvoci.documents WHERE workspace_id = $1 AND id = $2",
        )
        .bind(workspace_id)
        .bind(id)
        .fetch_optional(&mut *tx)
        .await?;
        let Some((None, row_project_id)) = live else {
            continue;
        };
        if row_project_id != Some(project_id) {
            continue;
        }
        if !trash_document_row(&mut tx, workspace_id, id, now).await? {
            continue;
        }
        record_document_event_and_audit(
            &mut tx,
            workspace_id,
            actor_user_id,
            "document.trashed",
            id,
            json!({ "documentId": id.to_string(), "projectId": project_id.to_string() }),
            client_ip,
        )
        .await?;
    }
    tx.commit().await?;
    Ok(Ok(()))
}

/// Source `restoreDocument` with project affiliation. A parent still in the
/// trash refuses with `TrashedParent`; a row past the trash retention is gone.
pub async fn restore_project_document(
    pool: &PgPool,
    workspace_id: Uuid,
    project_id: Uuid,
    document_id: Uuid,
    actor_user_id: Uuid,
    session_id: Uuid,
    client_ip: Option<&str>,
) -> Result<Result<(), DocumentDbError>, sqlx::Error> {
    let mut tx = pool.begin().await?;
    if let Err(err) =
        begin_project_tree_write(&mut tx, workspace_id, project_id, actor_user_id, session_id)
            .await?
    {
        tx.rollback().await?;
        return Ok(Err(err));
    }
    lock_document_rows(&mut tx, workspace_id, &[document_id]).await?;
    let doc: Option<(Option<Uuid>, Option<DateTime<Utc>>, Option<Uuid>)> = sqlx::query_as(
        "SELECT project_id, deleted_at, parent_id FROM fvoci.documents WHERE workspace_id = $1 AND id = $2",
    )
    .bind(workspace_id)
    .bind(document_id)
    .fetch_optional(&mut *tx)
    .await?;
    let Some((doc_project_id, Some(deleted_at), parent_id)) = doc else {
        tx.rollback().await?;
        return Ok(Err(DocumentDbError::NotFound));
    };
    if doc_project_id != Some(project_id) || trash_expired(&mut tx, deleted_at).await? {
        tx.rollback().await?;
        return Ok(Err(DocumentDbError::NotFound));
    }
    if let Some(parent_id) = parent_id {
        let parent: Option<(Option<DateTime<Utc>>,)> = sqlx::query_as(
            "SELECT deleted_at FROM fvoci.documents WHERE workspace_id = $1 AND id = $2",
        )
        .bind(workspace_id)
        .bind(parent_id)
        .fetch_optional(&mut *tx)
        .await?;
        if !matches!(parent, Some((None,))) {
            tx.rollback().await?;
            return Ok(Err(DocumentDbError::TrashedParent));
        }
    }
    sqlx::query(
        "UPDATE fvoci.documents SET deleted_at = NULL, updated_at = now() WHERE workspace_id = $1 AND id = $2",
    )
    .bind(workspace_id)
    .bind(document_id)
    .execute(&mut *tx)
    .await?;
    record_document_event_and_audit(
        &mut tx,
        workspace_id,
        actor_user_id,
        "document.restored",
        document_id,
        json!({ "documentId": document_id.to_string(), "projectId": project_id.to_string() }),
        client_ip,
    )
    .await?;
    tx.commit().await?;
    Ok(Ok(()))
}

/// Source `reorderDocument` for a project document (`afterId` null = first).
pub async fn reorder_project_document(
    pool: &PgPool,
    workspace_id: Uuid,
    project_id: Uuid,
    document_id: Uuid,
    actor_user_id: Uuid,
    session_id: Uuid,
    after_id: Option<Uuid>,
    client_ip: Option<&str>,
) -> Result<Result<DocumentMeta, DocumentDbError>, sqlx::Error> {
    let mut tx = pool.begin().await?;
    if let Err(err) =
        begin_project_tree_write(&mut tx, workspace_id, project_id, actor_user_id, session_id)
            .await?
    {
        tx.rollback().await?;
        return Ok(Err(err));
    }
    lock_document_rows(&mut tx, workspace_id, &[document_id]).await?;
    let doc: Option<(Option<Uuid>, Option<DateTime<Utc>>, Option<Uuid>)> = sqlx::query_as(
        "SELECT project_id, deleted_at, parent_id FROM fvoci.documents WHERE workspace_id = $1 AND id = $2",
    )
    .bind(workspace_id)
    .bind(document_id)
    .fetch_optional(&mut *tx)
    .await?;
    let Some((Some(doc_project_id), None, parent_id)) = doc else {
        tx.rollback().await?;
        return Ok(Err(DocumentDbError::NotFound));
    };
    if doc_project_id != project_id {
        tx.rollback().await?;
        return Ok(Err(DocumentDbError::NotFound));
    }
    let siblings =
        list_live_siblings_in(&mut tx, workspace_id, Some(project_id), parent_id).await?;
    let new_sort_key = match resolve_reorder_sort_key(&siblings, document_id, after_id) {
        Ok(key) => key,
        Err(err) => {
            tx.rollback().await?;
            return Ok(Err(err));
        }
    };
    sqlx::query(
        "UPDATE fvoci.documents SET sort_key = $3, updated_at = now() WHERE workspace_id = $1 AND id = $2 AND deleted_at IS NULL",
    )
    .bind(workspace_id)
    .bind(document_id)
    .bind(&new_sort_key)
    .execute(&mut *tx)
    .await?;
    record_document_event_and_audit(
        &mut tx,
        workspace_id,
        actor_user_id,
        "document.moved",
        document_id,
        json!({
            "documentId": document_id.to_string(),
            "kind": "reorder",
            "afterId": after_id.map(|id| id.to_string()),
            "newSortKey": new_sort_key,
        }),
        client_ip,
    )
    .await?;
    let project_key = project_key(&mut tx, workspace_id, project_id).await?;
    let Some(project_key) = project_key else {
        tx.rollback().await?;
        return Ok(Err(DocumentDbError::NotFound));
    };
    let row = fetch_document_row(&mut tx, workspace_id, document_id).await?;
    tx.commit().await?;
    match row {
        Some(row) => Ok(Ok(with_project_display_id(
            row_to_meta(row, true),
            &project_key,
        ))),
        None => Ok(Err(DocumentDbError::NotFound)),
    }
}

#[cfg(all(test, feature = "db-tests"))]
mod selected_tree_backend_tests {
    use super::*;
    use crate::db::attachment_preview::tests::Fixture;

    #[tokio::test]
    async fn selected_project_tree_keeps_tags_metadata_archived_guest_view_and_current_auth() {
        let f = Fixture::new().await;
        let credential = Uuid::now_v7();
        sqlx::query("INSERT INTO sessions(id,user_id,token_hash,expires_at) VALUES(?1,?2,?3,9223372036854775807)")
            .bind(credential.as_bytes().as_slice()).bind(f.user.as_bytes().as_slice()).bind(credential.to_string()).execute(&f.pool).await.unwrap();
        let (_, task) = f.task_attachment().await;
        let project: Vec<u8> = sqlx::query_scalar("SELECT project_id FROM tasks WHERE id=?1")
            .bind(task.as_bytes().as_slice())
            .fetch_one(&f.pool)
            .await
            .unwrap();
        let project = Uuid::from_slice(&project).unwrap();
        sqlx::query("UPDATE documents SET project_id=?1 WHERE id=?2")
            .bind(project.as_bytes().as_slice())
            .bind(f.document.as_bytes().as_slice())
            .execute(&f.pool)
            .await
            .unwrap();
        let child = Uuid::now_v7();
        let deleted = Uuid::now_v7();
        for (id, number, sort, trashed) in [(child, 2, "a0", false), (deleted, 3, "a1", true)] {
            sqlx::query("INSERT INTO documents(id,workspace_id,project_id,parent_id,title,icon,path,sort_key,number,status,schema_version,created_by,content_json,deleted_at) VALUES(?1,?2,?3,?4,'Existing child 😀','📄',?5,?6,?7,'draft',2,?8,?9,?10)")
                .bind(id.as_bytes().as_slice()).bind(f.workspace.as_bytes().as_slice()).bind(project.as_bytes().as_slice()).bind(f.document.as_bytes().as_slice()).bind(format!("{}.{}",f.document.simple(),id.simple())).bind(sort).bind(number).bind(f.user.as_bytes().as_slice()).bind(empty_document_json().to_string()).bind(trashed.then_some(1_i64)).execute(&f.pool).await.unwrap();
        }
        let tag = Uuid::now_v7();
        sqlx::query(
            "INSERT INTO document_tags(id,workspace_id,name,color) VALUES(?1,?2,'Selected','blue')",
        )
        .bind(tag.as_bytes().as_slice())
        .bind(f.workspace.as_bytes().as_slice())
        .execute(&f.pool)
        .await
        .unwrap();
        for id in [child, deleted] {
            sqlx::query("INSERT INTO document_tag_assignments(workspace_id,document_id,tag_id) VALUES(?1,?2,?3)")
                .bind(f.workspace.as_bytes().as_slice()).bind(id.as_bytes().as_slice()).bind(tag.as_bytes().as_slice()).execute(&f.pool).await.unwrap();
        }
        let foreign_workspace = Uuid::now_v7();
        let foreign_tag = Uuid::now_v7();
        sqlx::query("INSERT INTO workspaces(id,slug,name) VALUES(?1,'tree-foreign','Foreign')")
            .bind(foreign_workspace.as_bytes().as_slice())
            .execute(&f.pool)
            .await
            .unwrap();
        sqlx::query(
            "INSERT INTO document_tags(id,workspace_id,name,color) VALUES(?1,?2,'Selected','blue')",
        )
        .bind(foreign_tag.as_bytes().as_slice())
        .bind(foreign_workspace.as_bytes().as_slice())
        .execute(&f.pool)
        .await
        .unwrap();
        let foreign_document = Uuid::now_v7();
        sqlx::query("INSERT INTO documents(id,workspace_id,title,path,sort_key,number,status,schema_version,created_by,content_json) VALUES(?1,?2,'Foreign',?3,'V',1,'draft',2,?4,?5)")
            .bind(foreign_document.as_bytes().as_slice()).bind(foreign_workspace.as_bytes().as_slice()).bind(foreign_document.simple().to_string()).bind(f.user.as_bytes().as_slice()).bind(empty_document_json().to_string()).execute(&f.pool).await.unwrap();
        sqlx::query("INSERT INTO document_tag_assignments(workspace_id,document_id,tag_id) VALUES(?1,?2,?3)")
            .bind(foreign_workspace.as_bytes().as_slice()).bind(foreign_document.as_bytes().as_slice()).bind(foreign_tag.as_bytes().as_slice()).execute(&f.pool).await.unwrap();
        let before:(i64,i64,i64) = sqlx::query_as("SELECT (SELECT count(*) FROM documents),(SELECT count(*) FROM events),(SELECT count(*) FROM audit_log)").fetch_one(&f.pool).await.unwrap();
        let all = list_project_document_tree_backend(
            &f.backend,
            f.workspace,
            project,
            f.user,
            credential,
            None,
        )
        .await
        .unwrap()
        .unwrap();
        assert_eq!(
            all.iter().map(|node| node.id).collect::<Vec<_>>(),
            vec![f.document, child]
        );
        assert_eq!(all[0].parent_id, None);
        let node = &all[1];
        assert_eq!(
            (node.workspace_id, node.project_id, node.parent_id),
            (f.workspace, Some(project), Some(f.document))
        );
        assert_eq!(node.title, "Existing child 😀");
        assert_eq!(node.icon.as_deref(), Some("📄"));
        assert_eq!(
            node.path,
            format!("{}.{}", f.document.simple(), child.simple())
        );
        assert_eq!(node.sort_key, "a0");
        assert_eq!(node.number, 2);
        assert_eq!(node.status, "draft");
        let tagged = list_project_document_tree_backend(
            &f.backend,
            f.workspace,
            project,
            f.user,
            credential,
            Some(tag),
        )
        .await
        .unwrap()
        .unwrap();
        assert_eq!(tagged.iter().map(|node|node.id).collect::<Vec<_>>(),vec![child],"tag narrows the same live project rows, without restoring deleted documents or adding parents");
        for tag in [foreign_tag, Uuid::now_v7()] {
            assert!(list_project_document_tree_backend(
                &f.backend,
                f.workspace,
                project,
                f.user,
                credential,
                Some(tag)
            )
            .await
            .unwrap()
            .unwrap()
            .is_empty());
        }
        sqlx::query("UPDATE projects SET status='archived',visibility='private' WHERE id=?1")
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
            list_project_document_tree_backend(
                &f.backend,
                f.workspace,
                project,
                f.user,
                credential,
                Some(tag)
            )
            .await
            .unwrap(),
            Err(DocumentDbError::NotFound)
        ));
        sqlx::query("INSERT INTO project_members(id,workspace_id,project_id,user_id,role) VALUES(?1,?2,?3,?4,'viewer')")
            .bind(Uuid::now_v7().as_bytes().as_slice()).bind(f.workspace.as_bytes().as_slice()).bind(project.as_bytes().as_slice()).bind(f.user.as_bytes().as_slice()).execute(&f.pool).await.unwrap();
        assert_eq!(
            list_project_document_tree_backend(
                &f.backend,
                f.workspace,
                project,
                f.user,
                credential,
                Some(tag)
            )
            .await
            .unwrap()
            .unwrap()
            .len(),
            1,
            "archived project View remains sufficient"
        );
        assert!(matches!(
            list_project_document_tree_backend(
                &f.backend,
                foreign_workspace,
                project,
                f.user,
                credential,
                None
            )
            .await
            .unwrap(),
            Err(DocumentDbError::NotFound)
        ));
        sqlx::query("UPDATE sessions SET revoked_at=1 WHERE id=?1")
            .bind(credential.as_bytes().as_slice())
            .execute(&f.pool)
            .await
            .unwrap();
        assert!(matches!(
            list_project_document_tree_backend(
                &f.backend,
                f.workspace,
                project,
                f.user,
                credential,
                Some(tag)
            )
            .await
            .unwrap(),
            Err(DocumentDbError::Forbidden)
        ));
        sqlx::query("UPDATE sessions SET revoked_at=NULL WHERE id=?1")
            .bind(credential.as_bytes().as_slice())
            .execute(&f.pool)
            .await
            .unwrap();
        assert_eq!(
            list_project_document_tree_backend(
                &f.backend,
                f.workspace,
                project,
                f.user,
                credential,
                Some(tag)
            )
            .await
            .unwrap()
            .unwrap()
            .len(),
            1,
            "denied read releases its own snapshot and normal requests progress"
        );
        let after:(i64,i64,i64) = sqlx::query_as("SELECT (SELECT count(*) FROM documents),(SELECT count(*) FROM events),(SELECT count(*) FROM audit_log)").fetch_one(&f.pool).await.unwrap();
        assert_eq!(
            after, before,
            "tree/filter reads cannot create, audit or grant anything"
        );
        f.close().await;
    }
}
