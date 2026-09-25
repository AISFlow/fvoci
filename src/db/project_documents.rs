#![allow(clippy::too_many_arguments, clippy::type_complexity)]

use chrono::{DateTime, Utc};
use serde_json::json;
use sqlx::{PgPool, Postgres, Transaction};
use uuid::Uuid;

use crate::db::context::{lock_tree, set_tenant};
use crate::db::documents::{
    assert_document_writable, between, empty_document_json, fetch_document_row, format_display_id,
    lock_document_rows, move_subtree, record_document_event_and_audit, row_to_meta, subtree_ids,
    CreateDocumentInput, DocumentDbError, DocumentMeta, TreeNode, UpdateDocumentMetaInput,
    DOCUMENT_SCHEMA_VERSION, MAX_TREE_DEPTH,
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
        return Ok(Err(DocumentDbError::AffiliationMismatch));
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
        match assert_project_document(&mut tx, workspace_id, project_id, parent_id, true).await? {
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
    match assert_project_document(&mut tx, workspace_id, project_id, document_id, true).await? {
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
    match assert_project_document(&mut tx, workspace_id, project_id, document_id, true).await? {
        Ok(()) => {}
        Err(err) => {
            tx.rollback().await?;
            return Ok(Err(err));
        }
    }
    match assert_document_writable(&mut tx, workspace_id, document_id).await? {
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

    record_document_event_and_audit(
        &mut tx,
        workspace_id,
        actor_user_id,
        "document.updated",
        document_id,
        json!({
            "documentId": document_id.to_string(),
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
    let Some((doc_project_id, deleted_at, _doc_path, parent_id)) = doc else {
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

    match assert_project_document(&mut tx, workspace_id, project_id, new_parent_id, true).await? {
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
    if depth_of(&parent_path) + 1 > MAX_TREE_DEPTH {
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

fn depth_of(path: &str) -> i32 {
    path.split('.').count() as i32
}
