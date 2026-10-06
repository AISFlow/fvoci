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

/// Ordinary creation owns its writer; the OFF draft caller continues to own
/// its borrowed creator, command receipt and native publication separately.
pub async fn create_project_document_backend(
    backend: &Backend,
    workspace_id: Uuid,
    project_id: Uuid,
    actor_user_id: Uuid,
    session_id: Uuid,
    input: CreateDocumentInput<'_>,
    client_ip: Option<&str>,
) -> Result<Result<DocumentMeta, DocumentDbError>, sqlx::Error> {
    if let Backend::Postgres(pool) = backend {
        return create_project_document(
            pool,
            workspace_id,
            project_id,
            actor_user_id,
            session_id,
            input,
            client_ip,
        )
        .await;
    }
    let mut tx = backend.begin_write().await?;
    let result = create_project_document_operation(
        &mut tx.operation(),
        workspace_id,
        project_id,
        actor_user_id,
        session_id,
        input,
        client_ip,
    )
    .await;
    if let Ok(Ok(meta)) = result {
        // An uncertain finish is not a creation ACK or permission to retry.
        // Retain this original writer's typed receipt; do not open an observer.
        tx.commit_with_cleanup()
            .await
            .map_err(|error| sqlx::Error::AnyDriverError(Box::new(error)))?;
        Ok(Ok(meta))
    } else {
        project_create_after_rollback(result, tx.rollback().await)
    }
}

#[derive(Debug, thiserror::Error)]
#[error("project document creation refused: {0:?}")]
struct ProjectCreateRefusal(DocumentDbError);

fn project_create_after_rollback(
    result: Result<Result<DocumentMeta, DocumentDbError>, sqlx::Error>,
    cleanup: Result<(), sqlx::Error>,
) -> Result<Result<DocumentMeta, DocumentDbError>, sqlx::Error> {
    match cleanup {
        Ok(()) => result,
        Err(cleanup) => {
            let original: Option<Box<dyn std::error::Error + Send + Sync>> = match result {
                Err(driver) => Some(Box::new(driver)),
                Ok(Err(refusal)) => Some(Box::new(ProjectCreateRefusal(refusal))),
                Ok(Ok(_)) => None,
            };
            Err(crate::db::backend::rollback_cleanup_unknown(
                original, cleanup,
            ))
        }
    }
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

/// The exact existing project-create authority/parent policy, borrowed for both
/// first creation and a stable draft receipt replay; never starts or finishes a tx.
pub(crate) async fn authorize_project_draft_parent(
    op: &mut OperationTx<'_, '_>,
    workspace: Uuid,
    project: Uuid,
    actor: Uuid,
    credential: Uuid,
    parent: Option<Uuid>,
) -> Result<Result<String, DocumentDbError>, sqlx::Error> {
    op.set_tenant(workspace).await?;
    op.lock_membership_users(&[actor]).await?;
    if !op.recheck_session(actor, credential).await? {
        return Ok(Err(DocumentDbError::Forbidden));
    }
    op.lock_tree(workspace).await?;
    if !op.workspace_is_live(workspace).await? {
        return Ok(Err(DocumentDbError::NotFound));
    }
    match op {
        OperationTx::Postgres(tx) => {
            if let Err(error) = require_project_document_access(
                tx,
                workspace,
                actor,
                credential,
                project,
                ProjectPermission::Edit,
                true,
            )
            .await?
            {
                return Ok(Err(error));
            }
        }
        OperationTx::SqliteFamily(_) => {
            let Some((permission, archived)) = op
                .share_lock_project_permission(workspace, actor, project)
                .await?
            else {
                return Ok(Err(DocumentDbError::NotFound));
            };
            if archived || !permission.at_least(ProjectPermission::Edit) {
                return Ok(Err(DocumentDbError::NotFound));
            }
            if !op.recheck_session(actor, credential).await? {
                return Ok(Err(DocumentDbError::Forbidden));
            }
        }
    }
    let Some(parent) = parent else {
        return Ok(Err(DocumentDbError::NotFound));
    };
    let Some((affiliation, parent_path, deleted)) = op.wiki_parent(workspace, parent).await? else {
        return Ok(Err(DocumentDbError::NotFound));
    };
    if affiliation != Some(project) {
        return Ok(Err(DocumentDbError::AffiliationMismatch));
    }
    if deleted.is_some() {
        return Ok(Err(DocumentDbError::NotFound));
    }
    if depth_of(&parent_path) >= MAX_TREE_DEPTH {
        return Ok(Err(DocumentDbError::DepthLimit));
    }
    Ok(Ok(parent_path))
}

/// Dedicated borrowed creator for the stable OFF draft command. The caller
/// owns the command ledger, native body/revision effects and single finish.
/// Ordinary project create keeps its original public body and PG wrapper.
pub(crate) async fn create_project_document_operation(
    op: &mut OperationTx<'_, '_>,
    workspace: Uuid,
    project: Uuid,
    actor: Uuid,
    credential: Uuid,
    input: CreateDocumentInput<'_>,
    ip: Option<&str>,
) -> Result<Result<DocumentMeta, DocumentDbError>, sqlx::Error> {
    let parent_path = match authorize_project_draft_parent(
        op,
        workspace,
        project,
        actor,
        credential,
        input.parent_id,
    )
    .await?
    {
        Ok(path) => path,
        Err(error) => return Ok(Err(error)),
    };
    let parent = input.parent_id.ok_or(sqlx::Error::Protocol(
        "authorized project parent is absent".into(),
    ))?;
    let last = op.last_wiki_sort_key(workspace, Some(parent)).await?;
    let sort = match between(last.as_deref(), None) {
        Ok(value) => value,
        Err(_) => return Ok(Err(DocumentDbError::InvalidSortKey)),
    };
    let document = Uuid::now_v7();
    let path = format!(
        "{}.{}",
        parent_path,
        crate::db::documents::to_path_label(document)
    );
    let icon = input.icon.flatten();
    let body = empty_document_json();
    let key = match op {
        OperationTx::Postgres(tx) => {
            let number:i32 = sqlx::query_scalar("UPDATE fvoci.projects SET next_number=next_number+1,updated_at=now() WHERE workspace_id=$1 AND id=$2 RETURNING next_number-1")
                .bind(workspace).bind(project).fetch_one(&mut ***tx).await?;
            sqlx::query("INSERT INTO fvoci.documents(id,workspace_id,title,icon,path,parent_id,sort_key,project_id,number,status,schema_version,content_json,created_by,kind) VALUES($1,$2,$3,$4,$5,$6,$7,$8,$9,'draft',$10,$11,$12,'doc')")
                .bind(document).bind(workspace).bind(input.title).bind(icon).bind(&path).bind(parent).bind(&sort).bind(project).bind(number).bind(DOCUMENT_SCHEMA_VERSION).bind(&body).bind(actor).execute(&mut ***tx).await?;
            project_key(tx, workspace, project)
                .await?
                .ok_or(sqlx::Error::RowNotFound)?
        }
        OperationTx::SqliteFamily(tx) => {
            tx.require_writer()?;
            tx.require_tenant(workspace)?;
            let rows = tx.query("UPDATE projects SET next_number=next_number+1,updated_at=(unixepoch()*1000000+CAST(substr(strftime('%f','now'),4,3) AS INTEGER)*1000) WHERE workspace_id=?1 AND id=?2 AND next_number<2147483647 RETURNING next_number-1,key", &[Cell::uuid(workspace),Cell::uuid(project)]).await?;
            let row = rows.first().ok_or(sqlx::Error::RowNotFound)?;
            let number = row.cell(0)?.int32()?;
            let key = row.cell(1)?.string()?;
            tx.execute("INSERT INTO documents(id,workspace_id,title,icon,path,parent_id,sort_key,project_id,number,status,schema_version,content_json,created_by,kind) VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,'draft',?10,?11,?12,'doc')",
                &[Cell::uuid(document),Cell::uuid(workspace),Cell::text(input.title),Cell::optional_text(icon),Cell::text(path),Cell::uuid(parent),Cell::text(sort),Cell::uuid(project),Cell::Integer(i64::from(number)),Cell::Integer(i64::from(DOCUMENT_SCHEMA_VERSION)),Cell::json(&body)?,Cell::uuid(actor)]).await?;
            key
        }
    };
    op.record_document_event_and_audit(
        workspace,
        actor,
        "document.created",
        document,
        json!({"documentId":document,"parentId":parent,"title":input.title,"projectId":project}),
        ip,
    )
    .await?;
    let row = op
        .document_row(workspace, document)
        .await?
        .ok_or(sqlx::Error::RowNotFound)?;
    Ok(Ok(with_project_display_id(row_to_meta(row, true), &key)))
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
mod off_draft_borrowed_create_tests {
    use super::*;
    use crate::db::attachment_preview::tests::Fixture;

    #[tokio::test]
    async fn project_draft_creator_keeps_current_parent_policy_and_rolls_back_same_fk_failure() {
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
        // A populated project parent; keep its existing number namespace.
        sqlx::query("UPDATE documents SET project_id=?1 WHERE id=?2")
            .bind(project.as_bytes().as_slice())
            .bind(f.document.as_bytes().as_slice())
            .execute(&f.pool)
            .await
            .unwrap();
        sqlx::query("UPDATE projects SET next_number=2 WHERE id=?1")
            .bind(project.as_bytes().as_slice())
            .execute(&f.pool)
            .await
            .unwrap();
        let before: (i64, i64) = sqlx::query_as(
            "SELECT (SELECT count(*) FROM events), (SELECT count(*) FROM audit_log)",
        )
        .fetch_one(&f.pool)
        .await
        .unwrap();
        let input = CreateDocumentInput {
            parent_id: Some(f.document),
            title: "independent 😀",
            icon: Some(Some("📄")),
        };
        let mut tx = f.backend.begin_off_body().await.unwrap();
        let staged = create_project_document_operation(
            &mut tx.operation(),
            f.workspace,
            project,
            f.user,
            credential,
            input,
            None,
        )
        .await
        .unwrap()
        .unwrap();
        assert_eq!(staged.project_id, Some(project));
        assert_eq!(staged.parent_id, Some(f.document));
        assert_eq!(staged.number, 2);
        let OperationTx::SqliteFamily(writer) = tx.operation() else {
            panic!("actual family writer")
        };
        let error = writer
            .execute(
                "UPDATE documents SET parent_id=?1 WHERE workspace_id=?2 AND id=?3",
                &[
                    Cell::uuid(Uuid::now_v7()),
                    Cell::uuid(f.workspace),
                    Cell::uuid(staged.id),
                ],
            )
            .await
            .unwrap_err();
        assert!(
            error.to_string().contains("FOREIGN KEY"),
            "actual same-writer FK refusal: {error}"
        );
        tx.rollback().await.unwrap();
        let absent: i64 = sqlx::query_scalar("SELECT count(*) FROM documents WHERE id=?1")
            .bind(staged.id.as_bytes().as_slice())
            .fetch_one(&f.pool)
            .await
            .unwrap();
        assert_eq!(absent, 0);
        let number: i64 = sqlx::query_scalar("SELECT next_number FROM projects WHERE id=?1")
            .bind(project.as_bytes().as_slice())
            .fetch_one(&f.pool)
            .await
            .unwrap();
        assert_eq!(number, 2);
        let after_rollback: (i64, i64) = sqlx::query_as(
            "SELECT (SELECT count(*) FROM events), (SELECT count(*) FROM audit_log)",
        )
        .fetch_one(&f.pool)
        .await
        .unwrap();
        assert_eq!(
            after_rollback, before,
            "same FK refusal rolls back the creator outbox and audit too"
        );
        let mut tx = f.backend.begin_off_body().await.unwrap();
        let healthy = create_project_document_operation(
            &mut tx.operation(),
            f.workspace,
            project,
            f.user,
            credential,
            CreateDocumentInput {
                parent_id: Some(f.document),
                title: "independent 😀",
                icon: Some(Some("📄")),
            },
            None,
        )
        .await
        .unwrap()
        .unwrap();
        assert_eq!(healthy.number, 2);
        assert!(healthy
            .display_id
            .as_ref()
            .is_some_and(|id| id.ends_with("-2")));
        assert_eq!(healthy.title, "independent 😀");
        assert_eq!(healthy.icon.as_deref(), Some("📄"));
        tx.commit_with_cleanup().await.unwrap();
        let after_healthy: (i64, i64) = sqlx::query_as(
            "SELECT (SELECT count(*) FROM events), (SELECT count(*) FROM audit_log)",
        )
        .fetch_one(&f.pool)
        .await
        .unwrap();
        assert_eq!(after_healthy, (before.0 + 1, before.1 + 1));
        let mut denied = f.backend.begin_off_body().await.unwrap();
        assert!(matches!(
            create_project_document_operation(
                &mut denied.operation(),
                f.workspace,
                project,
                f.user,
                credential,
                CreateDocumentInput {
                    parent_id: None,
                    title: "must not create",
                    icon: None
                },
                None
            )
            .await
            .unwrap(),
            Err(DocumentDbError::NotFound)
        ));
        denied.rollback().await.unwrap();
        sqlx::query("UPDATE projects SET status='archived' WHERE id=?1")
            .bind(project.as_bytes().as_slice())
            .execute(&f.pool)
            .await
            .unwrap();
        let mut denied = f.backend.begin_off_body().await.unwrap();
        assert!(matches!(
            create_project_document_operation(
                &mut denied.operation(),
                f.workspace,
                project,
                f.user,
                credential,
                CreateDocumentInput {
                    parent_id: Some(f.document),
                    title: "must not create",
                    icon: None
                },
                None
            )
            .await
            .unwrap(),
            Err(DocumentDbError::NotFound)
        ));
        denied.rollback().await.unwrap();
        let after_denials: (i64, i64) = sqlx::query_as(
            "SELECT (SELECT count(*) FROM events), (SELECT count(*) FROM audit_log)",
        )
        .fetch_one(&f.pool)
        .await
        .unwrap();
        assert_eq!(after_denials, after_healthy);
        let nodes = list_project_document_tree_backend(
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
        assert_eq!(nodes.len(), 2, "archived project remains readable");
        assert!(nodes
            .windows(2)
            .all(|pair| pair[0].sort_key <= pair[1].sort_key));
        let child = nodes.iter().find(|node| node.id == healthy.id).unwrap();
        assert_eq!(child.workspace_id, f.workspace);
        assert_eq!(child.project_id, Some(project));
        assert_eq!(child.parent_id, Some(f.document));
        assert_eq!(child.title, healthy.title);
        assert_eq!(child.number, healthy.number);
        sqlx::query("UPDATE projects SET visibility='private' WHERE id=?1")
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
                None
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
                None
            )
            .await
            .unwrap()
            .unwrap()
            .len(),
            2,
            "explicit current guest View is sufficient for the archived tree"
        );
        sqlx::query("UPDATE projects SET status='active' WHERE id=?1")
            .bind(project.as_bytes().as_slice())
            .execute(&f.pool)
            .await
            .unwrap();
        let mut view_only = f.backend.begin_off_body().await.unwrap();
        assert!(matches!(
            create_project_document_operation(
                &mut view_only.operation(),
                f.workspace,
                project,
                f.user,
                credential,
                CreateDocumentInput {
                    parent_id: Some(f.document),
                    title: "view must not grant creation",
                    icon: None
                },
                None,
            )
            .await
            .unwrap(),
            Err(DocumentDbError::NotFound)
        ));
        view_only.rollback().await.unwrap();
        assert!(matches!(
            list_project_document_tree_backend(
                &f.backend,
                Uuid::now_v7(),
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
                None
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
                None
            )
            .await
            .unwrap()
            .unwrap()
            .len(),
            2,
            "denied read releases its own transaction; healthy reads still work"
        );
        let after_reads: (i64, i64) = sqlx::query_as(
            "SELECT (SELECT count(*) FROM events), (SELECT count(*) FROM audit_log)",
        )
        .fetch_one(&f.pool)
        .await
        .unwrap();
        assert_eq!(
            after_reads, after_healthy,
            "tree visibility never grants or writes events/audit"
        );
        f.close().await;
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

#[cfg(test)]
mod selected_create_finish_tests {
    use super::*;

    #[test]
    fn project_create_rollback_retains_domain_and_driver_causes() {
        assert!(matches!(
            project_create_after_rollback(Ok(Err(DocumentDbError::NotFound)), Ok(())),
            Ok(Err(DocumentDbError::NotFound))
        ));
        let driver = project_create_after_rollback(
            Err(sqlx::Error::Protocol("original publication fault".into())),
            Ok(()),
        )
        .unwrap_err();
        assert!(driver.to_string().contains("original publication fault"));
        for original in [
            Ok(Err(DocumentDbError::Forbidden)),
            Err(sqlx::Error::Protocol("original publication fault".into())),
        ] {
            let error = project_create_after_rollback(
                original,
                Err(sqlx::Error::Protocol("original rollback fault".into())),
            )
            .unwrap_err();
            let sqlx::Error::AnyDriverError(inner) = error else {
                panic!("typed rollback uncertainty required")
            };
            let unknown = inner
                .downcast_ref::<crate::db::backend::RollbackCleanupUnknown>()
                .unwrap();
            assert!(unknown
                .cleanup
                .to_string()
                .contains("original rollback fault"));
            let original = unknown.original.as_ref().unwrap();
            if let Some(refusal) = original.downcast_ref::<ProjectCreateRefusal>() {
                assert!(matches!(refusal.0, DocumentDbError::Forbidden));
            } else {
                assert!(original
                    .downcast_ref::<sqlx::Error>()
                    .unwrap()
                    .to_string()
                    .contains("original publication fault"));
            }
        }
    }
}

#[cfg(all(test, feature = "db-tests"))]
pub(crate) mod selected_create_backend_tests {
    use super::*;
    use crate::db::attachment_preview::tests::Fixture;
    use std::future::Future;

    pub(crate) async fn setup() -> (Fixture, Uuid, Uuid) {
        let f = Fixture::new().await;
        let credential = Uuid::now_v7();
        sqlx::query("INSERT INTO sessions(id,user_id,token_hash,expires_at) VALUES(?1,?2,?3,9223372036854775807)")
            .bind(credential.as_bytes().as_slice()).bind(f.user.as_bytes().as_slice()).bind(credential.to_string()).execute(&f.pool).await.unwrap();
        let project = Uuid::now_v7();
        sqlx::query("INSERT INTO projects(id,workspace_id,key,name,visibility,created_by,next_number) VALUES(?1,?2,'OFF','OFF selected project','workspace',?3,2)")
            .bind(project.as_bytes().as_slice()).bind(f.workspace.as_bytes().as_slice()).bind(f.user.as_bytes().as_slice()).execute(&f.pool).await.unwrap();
        sqlx::query("UPDATE documents SET project_id=?1 WHERE id=?2")
            .bind(project.as_bytes().as_slice())
            .bind(f.document.as_bytes().as_slice())
            .execute(&f.pool)
            .await
            .unwrap();
        sqlx::query("UPDATE projects SET root_document_id=?1 WHERE id=?2")
            .bind(f.document.as_bytes().as_slice())
            .bind(project.as_bytes().as_slice())
            .execute(&f.pool)
            .await
            .unwrap();
        (f, credential, project)
    }

    pub(crate) async fn counts(f: &Fixture, project: Uuid) -> (i64, i64, i64, i64) {
        sqlx::query_as("SELECT (SELECT count(*) FROM documents WHERE project_id=?1),(SELECT next_number FROM projects WHERE id=?1),(SELECT count(*) FROM events WHERE verb='document.created'),(SELECT count(*) FROM audit_log WHERE verb='document.created')")
            .bind(project.as_bytes().as_slice()).fetch_one(&f.pool).await.unwrap()
    }

    async fn create(
        f: &Fixture,
        credential: Uuid,
        project: Uuid,
        parent: Option<Uuid>,
    ) -> Result<Result<DocumentMeta, DocumentDbError>, sqlx::Error> {
        create_project_document_backend(
            &f.backend,
            f.workspace,
            project,
            f.user,
            credential,
            CreateDocumentInput {
                parent_id: parent,
                title: "OFF project body",
                icon: None,
            },
            None,
        )
        .await
    }

    #[tokio::test]
    async fn sqlite_project_create_current_authority_and_parent_denials() {
        let (f, credential, project) = setup().await;
        let before = counts(&f, project).await;
        for parent in [None, Some(Uuid::now_v7())] {
            assert!(matches!(
                create(&f, credential, project, parent).await.unwrap(),
                Err(DocumentDbError::NotFound)
            ));
            assert_eq!(counts(&f, project).await, before);
        }
        for (query, expected) in [
            (
                "UPDATE memberships SET role='guest' WHERE user_id=?1",
                "missing",
            ),
            (
                "UPDATE projects SET status='archived' WHERE id=?1",
                "missing",
            ),
            (
                "UPDATE documents SET project_id=NULL WHERE id=?1",
                "affiliation",
            ),
            ("UPDATE documents SET deleted_at=1 WHERE id=?1", "missing"),
            ("UPDATE workspaces SET deleted_at=1 WHERE id=?1", "missing"),
            ("UPDATE sessions SET expires_at=1 WHERE id=?1", "forbidden"),
        ] {
            let id = if query.contains("memberships") {
                f.user
            } else if query.contains("projects") {
                project
            } else if query.contains("workspaces") {
                f.workspace
            } else if query.contains("sessions") {
                credential
            } else {
                f.document
            };
            let mut blocker = f.pool.begin().await.unwrap();
            sqlx::query(query)
                .bind(id.as_bytes().as_slice())
                .execute(&mut *blocker)
                .await
                .unwrap();
            blocker.commit().await.unwrap();
            let refusal = create(&f, credential, project, Some(f.document))
                .await
                .unwrap()
                .unwrap_err();
            match expected {
                "affiliation" => assert!(matches!(refusal, DocumentDbError::AffiliationMismatch)),
                "forbidden" => assert!(matches!(refusal, DocumentDbError::Forbidden)),
                _ => assert!(matches!(refusal, DocumentDbError::NotFound)),
            }
            assert_eq!(counts(&f, project).await, before);
            for (restore, id) in [
                (
                    "UPDATE memberships SET role='owner' WHERE user_id=?1",
                    f.user,
                ),
                ("UPDATE projects SET status='active' WHERE id=?1", project),
                (
                    "UPDATE documents SET deleted_at=NULL WHERE id=?1",
                    f.document,
                ),
                (
                    "UPDATE workspaces SET deleted_at=NULL WHERE id=?1",
                    f.workspace,
                ),
                (
                    "UPDATE sessions SET expires_at=9223372036854775807 WHERE id=?1",
                    credential,
                ),
            ] {
                sqlx::query(restore)
                    .bind(id.as_bytes().as_slice())
                    .execute(&f.pool)
                    .await
                    .unwrap();
            }
            sqlx::query("UPDATE documents SET project_id=?1 WHERE id=?2")
                .bind(project.as_bytes().as_slice())
                .bind(f.document.as_bytes().as_slice())
                .execute(&f.pool)
                .await
                .unwrap();
        }
        let deep = std::iter::repeat_n(
            f.document.simple().to_string(),
            usize::try_from(MAX_TREE_DEPTH).unwrap(),
        )
        .collect::<Vec<_>>()
        .join(".");
        sqlx::query("UPDATE documents SET path=?1 WHERE id=?2")
            .bind(deep)
            .bind(f.document.as_bytes().as_slice())
            .execute(&f.pool)
            .await
            .unwrap();
        assert!(matches!(
            create(&f, credential, project, Some(f.document))
                .await
                .unwrap(),
            Err(DocumentDbError::DepthLimit)
        ));
        assert_eq!(counts(&f, project).await, before);
        sqlx::query("UPDATE documents SET path=?1 WHERE id=?2")
            .bind(f.document.simple().to_string())
            .bind(f.document.as_bytes().as_slice())
            .execute(&f.pool)
            .await
            .unwrap();
        sqlx::query("UPDATE projects SET next_number=2147483647 WHERE id=?1")
            .bind(project.as_bytes().as_slice())
            .execute(&f.pool)
            .await
            .unwrap();
        let exhausted = counts(&f, project).await;
        assert!(matches!(
            create(&f, credential, project, Some(f.document))
                .await
                .unwrap_err(),
            sqlx::Error::RowNotFound
        ));
        assert_eq!(
            counts(&f, project).await,
            exhausted,
            "number namespace cannot overflow"
        );
        sqlx::query("UPDATE projects SET next_number=2 WHERE id=?1")
            .bind(project.as_bytes().as_slice())
            .execute(&f.pool)
            .await
            .unwrap();
        // An active credential still cannot create into a different tenant.
        assert!(matches!(
            create_project_document_backend(
                &f.backend,
                Uuid::now_v7(),
                project,
                f.user,
                credential,
                CreateDocumentInput {
                    parent_id: Some(f.document),
                    title: "foreign",
                    icon: None
                },
                None
            )
            .await
            .unwrap(),
            Err(DocumentDbError::NotFound)
        ));
        assert_eq!(counts(&f, project).await, before);
        assert_eq!(
            create(&f, credential, project, Some(f.document))
                .await
                .unwrap()
                .unwrap()
                .number,
            2
        );
        f.close().await;
    }

    #[tokio::test]
    async fn sqlite_project_create_queued_writer_rechecks_credential_and_permission() {
        let (f, credential, project) = setup().await;
        let before = counts(&f, project).await;
        for revoke_credential in [true, false] {
            // Fixture pool has one connection: a polled Pending creator is
            // demonstrably waiting for this holder, without sleeps or hooks.
            let mut holder = f.pool.begin_with("BEGIN IMMEDIATE").await.unwrap();
            let backend = f.backend.clone();
            let (workspace, actor, parent) = (f.workspace, f.user, f.document);
            let (ready, observed) = tokio::sync::oneshot::channel();
            let pending = tokio::spawn(async move {
                let mut creation = Box::pin(create_project_document_backend(
                    &backend,
                    workspace,
                    project,
                    actor,
                    credential,
                    CreateDocumentInput {
                        parent_id: Some(parent),
                        title: "queued",
                        icon: None,
                    },
                    None,
                ));
                let mut ready = Some(ready);
                std::future::poll_fn(|cx| {
                    let result = creation.as_mut().poll(cx);
                    if result.is_pending() {
                        if let Some(ready) = ready.take() {
                            let _ = ready.send(());
                        }
                    }
                    result
                })
                .await
            });
            observed.await.unwrap();
            let query = if revoke_credential {
                "UPDATE sessions SET expires_at=1 WHERE id=?1"
            } else {
                "UPDATE memberships SET role='guest' WHERE user_id=?1"
            };
            let id = if revoke_credential {
                credential
            } else {
                f.user
            };
            sqlx::query(query)
                .bind(id.as_bytes().as_slice())
                .execute(&mut *holder)
                .await
                .unwrap();
            holder.commit().await.unwrap();
            let refusal = pending.await.unwrap().unwrap().unwrap_err();
            if revoke_credential {
                assert!(matches!(refusal, DocumentDbError::Forbidden));
            } else {
                assert!(matches!(refusal, DocumentDbError::NotFound));
            }
            assert_eq!(counts(&f, project).await, before);
            sqlx::query("UPDATE sessions SET expires_at=9223372036854775807 WHERE id=?1")
                .bind(credential.as_bytes().as_slice())
                .execute(&f.pool)
                .await
                .unwrap();
            sqlx::query("UPDATE memberships SET role='owner' WHERE user_id=?1")
                .bind(f.user.as_bytes().as_slice())
                .execute(&f.pool)
                .await
                .unwrap();
        }
        assert_eq!(
            create(&f, credential, project, Some(f.document))
                .await
                .unwrap()
                .unwrap()
                .number,
            2
        );
        f.close().await;
    }

    #[tokio::test]
    async fn sqlite_project_create_fk_publication_and_commit_failures_then_healthy_create() {
        let (f, credential, project) = setup().await;
        assert_eq!(
            sqlx::query_scalar::<_, i64>("PRAGMA foreign_keys")
                .fetch_one(&f.pool)
                .await
                .unwrap(),
            1
        );
        let before = counts(&f, project).await;
        sqlx::query(
            "CREATE TABLE project_create_fk_probe(id BLOB REFERENCES documents(id)) STRICT",
        )
        .execute(&f.pool)
        .await
        .unwrap();
        for table in ["events", "audit_log"] {
            sqlx::query(&format!("CREATE TRIGGER project_create_refuse AFTER INSERT ON {table} WHEN NEW.verb='document.created' BEGIN INSERT INTO project_create_fk_probe(id) VALUES(zeroblob(16)); END;")).execute(&f.pool).await.unwrap();
            let error = create(&f, credential, project, Some(f.document))
                .await
                .unwrap_err();
            assert!(
                error
                    .as_database_error()
                    .is_some_and(|error| error.is_foreign_key_violation()),
                "original FK cause retained: {error}"
            );
            assert_eq!(
                counts(&f, project).await,
                before,
                "document, number, event and audit roll back together"
            );
            sqlx::query("DROP TRIGGER project_create_refuse")
                .execute(&f.pool)
                .await
                .unwrap();
        }
        sqlx::query("CREATE TABLE project_create_deferred_probe(id BLOB REFERENCES documents(id) DEFERRABLE INITIALLY DEFERRED) STRICT").execute(&f.pool).await.unwrap();
        sqlx::query("CREATE TRIGGER project_create_commit_refuse AFTER INSERT ON audit_log WHEN NEW.verb='document.created' BEGIN INSERT INTO project_create_deferred_probe(id) VALUES(zeroblob(16)); END;").execute(&f.pool).await.unwrap();
        let error = create(&f, credential, project, Some(f.document))
            .await
            .unwrap_err();
        let sqlx::Error::AnyDriverError(inner) = &error else {
            panic!("typed original commit uncertainty required: {error}")
        };
        let unknown = inner
            .downcast_ref::<crate::db::backend::CommitCleanupUnknown>()
            .unwrap();
        assert_eq!(
            unknown.settlement,
            crate::db::backend::CommitSettlement::LocalWriterReconcile
        );
        assert!(unknown
            .source
            .source
            .as_database_error()
            .is_some_and(|error| error.is_foreign_key_violation()));
        // Fresh local SELECT observes queued rollback; it is not a remote
        // settlement proof and never re-creates the uncertain result.
        assert_eq!(counts(&f, project).await, before);
        assert_eq!(
            sqlx::query_scalar::<_, i64>("SELECT count(*) FROM project_create_deferred_probe")
                .fetch_one(&f.pool)
                .await
                .unwrap(),
            0
        );
        sqlx::query("DROP TRIGGER project_create_commit_refuse")
            .execute(&f.pool)
            .await
            .unwrap();
        let healthy = create(&f, credential, project, Some(f.document))
            .await
            .unwrap()
            .unwrap();
        assert_eq!(healthy.number, 2);
        assert_eq!(healthy.display_id.as_deref(), Some("OFF-2"));
        assert_eq!(
            counts(&f, project).await,
            (before.0 + 1, 3, before.2 + 1, before.3 + 1)
        );
        f.close().await;
    }
}
