#![allow(clippy::too_many_arguments, clippy::type_complexity)]

use chrono::{DateTime, Utc};
use serde_json::json;
use sqlx::{PgPool, Postgres, Transaction};
use uuid::Uuid;

use crate::db::context::{lock_tree, set_tenant};
use crate::db::documents::{
    assert_document_writable, between, depth_of, empty_document_json, fetch_document_row,
    format_display_id, is_descendant, list_live_siblings_in, lock_document_rows, move_subtree,
    record_document_event_and_audit, resolve_reorder_sort_key, row_to_meta, subtree_ids,
    trash_document_row, trash_expired, CreateDocumentInput, DocumentDbError, DocumentMeta,
    TrashChildrenMode, TreeNode, UpdateDocumentMetaInput, DOCUMENT_SCHEMA_VERSION, MAX_TREE_DEPTH,
};
use crate::db::documents::{
    lock_membership_users, recheck_session, session_is_live, workspace_is_live,
};
use crate::db::projects::{lock_project, project_permission};
use crate::projects::ProjectPermission;

async fn require_project_document_access(
    tx: &mut Transaction<'_, Postgres>,
    workspace_id: Uuid,
    actor_user_id: Uuid,
    session_id: Uuid,
    project_id: Uuid,
    min: ProjectPermission,
) -> Result<Result<(), DocumentDbError>, sqlx::Error> {
    if !session_is_live(tx, actor_user_id, session_id).await? {
        return Ok(Err(DocumentDbError::Forbidden));
    }
    if !workspace_is_live(tx, workspace_id).await? {
        return Ok(Err(DocumentDbError::NotFound));
    }
    let Some(locked) = lock_project(tx, workspace_id, project_id).await? else {
        return Ok(Err(DocumentDbError::NotFound));
    };
    if locked.status == "archived" && min >= ProjectPermission::Edit {
        return Ok(Err(DocumentDbError::NotFound));
    }
    let permission = project_permission(tx, workspace_id, actor_user_id, &locked).await?;
    if !permission.at_least(min) {
        return Ok(Err(DocumentDbError::NotFound));
    }
    Ok(Ok(()))
}

async fn assert_project_document(
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

async fn project_key(
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

fn with_project_display_id(meta: DocumentMeta, project_key: &str) -> DocumentMeta {
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
    let mut tx = pool.begin().await?;
    set_tenant(&mut tx, workspace_id).await?;
    match require_project_document_access(
        &mut tx,
        workspace_id,
        actor_user_id,
        session_id,
        project_id,
        ProjectPermission::View,
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
    let mut tx = pool.begin().await?;
    set_tenant(&mut tx, workspace_id).await?;
    match require_project_document_access(
        &mut tx,
        workspace_id,
        actor_user_id,
        session_id,
        project_id,
        ProjectPermission::View,
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
