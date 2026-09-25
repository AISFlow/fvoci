use serde_json::Value;
use sqlx::PgPool;
use uuid::Uuid;

use crate::collab::derived_body::{prepare_derived_body, DOCUMENT_MAX_BODY_BYTES};
use crate::db::collab::{
    append_collab_update, claim_writer_and_load, project_derived_body, AppendCollabInput,
    CollabDbError, ProjectDerivedBodyInput,
};
use crate::db::documents::{
    create_wiki_document, create_wiki_document_for_import, CreateDocumentInput, DocumentDbError,
    ImportFence,
};
use crate::documents::convert::{ConvertClient, ConvertError};

#[derive(Debug, thiserror::Error)]
pub enum ImportBodyError {
    #[error("not found")]
    NotFound,
    #[error("forbidden")]
    Forbidden,
    #[error("invalid input")]
    InvalidInput,
    #[error("document too large")]
    TooLarge,
    #[error("import fenced")]
    Fenced,
    #[error("import failed: {0}")]
    Failed(String),
}

fn map_create_error(err: DocumentDbError) -> ImportBodyError {
    match err {
        DocumentDbError::NotFound => ImportBodyError::NotFound,
        DocumentDbError::Forbidden => ImportBodyError::Forbidden,
        other => ImportBodyError::Failed(format!("create document: {other:?}")),
    }
}

/// Request-driven create (markdown-zip): the request's own session and role.
pub async fn create_imported_wiki_document(
    pool: &PgPool,
    workspace_id: Uuid,
    actor_user_id: Uuid,
    session_id: Uuid,
    title: &str,
    parent_id: Option<Uuid>,
) -> Result<Uuid, ImportBodyError> {
    let created = create_wiki_document(
        pool,
        workspace_id,
        actor_user_id,
        session_id,
        CreateDocumentInput {
            parent_id,
            title,
            icon: None,
        },
        None,
    )
    .await
    .map_err(|e| ImportBodyError::Failed(e.to_string()))?;
    created.map(|meta| meta.id).map_err(map_create_error)
}

/// Async-job create: admin rechecked and the id recorded under the job lease
/// in the same transaction.
pub async fn create_fenced_wiki_document(
    pool: &PgPool,
    workspace_id: Uuid,
    actor_user_id: Uuid,
    session_id: Uuid,
    title: &str,
    parent_id: Option<Uuid>,
    fence: ImportFence,
) -> Result<Uuid, ImportBodyError> {
    let created = create_wiki_document_for_import(
        pool,
        workspace_id,
        actor_user_id,
        session_id,
        CreateDocumentInput {
            parent_id,
            title,
            icon: None,
        },
        fence,
    )
    .await
    .map_err(|e| ImportBodyError::Failed(e.to_string()))?;
    match created {
        Ok(Some(meta)) => Ok(meta.id),
        Ok(None) => Err(ImportBodyError::Fenced),
        Err(err) => Err(map_create_error(err)),
    }
}

fn map_convert_error(err: ConvertError) -> ImportBodyError {
    match err {
        ConvertError::InvalidInput => ImportBodyError::InvalidInput,
        ConvertError::TooLarge => ImportBodyError::TooLarge,
        other => ImportBodyError::Failed(other.to_string()),
    }
}

/// Converts Markdown with the editor's parser and writes it through the
/// collaboration path (Yjs update + derived body), like an editor save.
pub async fn apply_imported_markdown(
    pool: &PgPool,
    convert: &ConvertClient,
    workspace_id: Uuid,
    actor_user_id: Uuid,
    session_id: Uuid,
    document_id: Uuid,
    markdown: &str,
) -> Result<(), ImportBodyError> {
    if markdown.len() > DOCUMENT_MAX_BODY_BYTES {
        return Err(ImportBodyError::TooLarge);
    }
    let content_json = convert
        .md_to_tiptap(markdown)
        .await
        .map_err(map_convert_error)?;
    apply_imported_tiptap(
        pool,
        convert,
        workspace_id,
        actor_user_id,
        session_id,
        document_id,
        &content_json,
    )
    .await
}

pub async fn apply_imported_tiptap(
    pool: &PgPool,
    convert: &ConvertClient,
    workspace_id: Uuid,
    actor_user_id: Uuid,
    session_id: Uuid,
    document_id: Uuid,
    content_json: &Value,
) -> Result<(), ImportBodyError> {
    let prepared = match prepare_derived_body(content_json.clone()) {
        Ok(v) => v,
        Err(crate::collab::derived_body::DerivedBodyError::TooLarge) => {
            return Err(ImportBodyError::TooLarge);
        }
        Err(_) => return Err(ImportBodyError::InvalidInput),
    };
    let update = convert
        .tiptap_to_yjs_update(content_json)
        .await
        .map_err(map_convert_error)?;
    let failed = |e: sqlx::Error| ImportBodyError::Failed(e.to_string());
    let claim = claim_writer_and_load(pool, workspace_id, actor_user_id, session_id, document_id)
        .await
        .map_err(failed)?;
    let claim = match claim {
        Ok(v) => v,
        Err(CollabDbError::NotFound | CollabDbError::Forbidden) => {
            return Err(ImportBodyError::NotFound);
        }
        Err(other) => return Err(ImportBodyError::Failed(format!("claim writer: {other:?}"))),
    };
    let append = append_collab_update(
        pool,
        AppendCollabInput {
            workspace_id,
            actor_user_id,
            session_id,
            document_id,
            writer_generation: claim.writer_generation,
            expected_tail_seq: claim.load.tail_seq,
            op_id: Uuid::now_v7(),
            payload: &update,
            client_ip: None,
        },
    )
    .await
    .map_err(failed)?;
    let seq = match append {
        Ok(crate::db::collab::AppendCollabResult::Committed { seq }) => seq,
        Ok(crate::db::collab::AppendCollabResult::DuplicateAck { seq }) => seq,
        Err(CollabDbError::NotFound | CollabDbError::Forbidden) => {
            return Err(ImportBodyError::NotFound);
        }
        Err(other) => return Err(ImportBodyError::Failed(format!("append update: {other:?}"))),
    };
    let projected = project_derived_body(
        pool,
        ProjectDerivedBodyInput::new(
            workspace_id,
            actor_user_id,
            session_id,
            document_id,
            claim.writer_generation,
            seq,
            prepared,
        ),
    )
    .await
    .map_err(failed)?;
    match projected {
        Ok(_) => Ok(()),
        Err(CollabDbError::NotFound | CollabDbError::Forbidden) => Err(ImportBodyError::NotFound),
        Err(other) => Err(ImportBodyError::Failed(format!("project body: {other:?}"))),
    }
}
