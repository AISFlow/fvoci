use serde_json::Value;
use sqlx::PgPool;
use uuid::Uuid;

use crate::collab::derived_body::{prepare_derived_body, DOCUMENT_MAX_BODY_BYTES};
use crate::db::collab::{
    append_collab_update, claim_writer_and_load, project_derived_body, AppendCollabInput,
    CollabDbError, ProjectDerivedBodyInput,
};
use crate::db::documents::{create_wiki_document, CreateDocumentInput, DocumentDbError};
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
    #[error("import convert unavailable")]
    Unavailable,
    #[error("import failed")]
    Failed,
}

pub async fn create_imported_wiki_document(
    pool: &PgPool,
    workspace_id: Uuid,
    actor_user_id: Uuid,
    session_id: Uuid,
    title: &str,
    parent_id: Option<Uuid>,
    client_ip: Option<&str>,
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
        client_ip,
    )
    .await
    .map_err(|_| ImportBodyError::Failed)?;
    match created {
        Ok(meta) => Ok(meta.id),
        Err(DocumentDbError::NotFound | DocumentDbError::Forbidden) => {
            Err(ImportBodyError::Forbidden)
        }
        Err(_) => Err(ImportBodyError::Failed),
    }
}

#[allow(clippy::too_many_arguments)]
pub async fn apply_imported_markdown(
    pool: &PgPool,
    convert: &ConvertClient,
    workspace_id: Uuid,
    actor_user_id: Uuid,
    session_id: Uuid,
    document_id: Uuid,
    markdown: &str,
    client_ip: Option<&str>,
) -> Result<(), ImportBodyError> {
    if markdown.len() > DOCUMENT_MAX_BODY_BYTES {
        return Err(ImportBodyError::TooLarge);
    }
    let content_json = match convert.md_to_tiptap(markdown) {
        Ok(v) => v,
        Err(ConvertError::InvalidInput) => return Err(ImportBodyError::InvalidInput),
        Err(ConvertError::TooLarge) => return Err(ImportBodyError::TooLarge),
        Err(ConvertError::NotConfigured) => return Err(ImportBodyError::Unavailable),
        Err(_) => return Err(ImportBodyError::Failed),
    };
    apply_imported_tiptap(
        pool,
        convert,
        workspace_id,
        actor_user_id,
        session_id,
        document_id,
        &content_json,
        client_ip,
    )
    .await
}

#[allow(clippy::too_many_arguments)]
pub async fn apply_imported_tiptap(
    pool: &PgPool,
    convert: &ConvertClient,
    workspace_id: Uuid,
    actor_user_id: Uuid,
    session_id: Uuid,
    document_id: Uuid,
    content_json: &Value,
    client_ip: Option<&str>,
) -> Result<(), ImportBodyError> {
    let prepared = match prepare_derived_body(content_json.clone()) {
        Ok(v) => v,
        Err(crate::collab::derived_body::DerivedBodyError::TooLarge) => {
            return Err(ImportBodyError::TooLarge);
        }
        Err(_) => return Err(ImportBodyError::InvalidInput),
    };
    let update = match convert.tiptap_to_yjs_update(content_json) {
        Ok(v) => v,
        Err(ConvertError::InvalidInput) => return Err(ImportBodyError::InvalidInput),
        Err(ConvertError::TooLarge) => return Err(ImportBodyError::TooLarge),
        Err(ConvertError::NotConfigured) => return Err(ImportBodyError::Unavailable),
        Err(_) => return Err(ImportBodyError::Failed),
    };
    let claim = claim_writer_and_load(pool, workspace_id, actor_user_id, session_id, document_id)
        .await
        .map_err(|_| ImportBodyError::Failed)?;
    let claim = match claim {
        Ok(v) => v,
        Err(CollabDbError::NotFound | CollabDbError::Forbidden) => {
            return Err(ImportBodyError::NotFound);
        }
        Err(_) => return Err(ImportBodyError::Failed),
    };
    let op_id = Uuid::now_v7();
    let append = append_collab_update(
        pool,
        AppendCollabInput {
            workspace_id,
            actor_user_id,
            session_id,
            document_id,
            writer_generation: claim.writer_generation,
            expected_tail_seq: claim.load.tail_seq,
            op_id,
            payload: &update,
            client_ip,
        },
    )
    .await
    .map_err(|_| ImportBodyError::Failed)?;
    let seq = match append {
        Ok(crate::db::collab::AppendCollabResult::Committed { seq }) => seq,
        Ok(crate::db::collab::AppendCollabResult::DuplicateAck { seq }) => seq,
        Err(CollabDbError::NotFound | CollabDbError::Forbidden) => {
            return Err(ImportBodyError::NotFound);
        }
        Err(_) => return Err(ImportBodyError::Failed),
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
    .map_err(|_| ImportBodyError::Failed)?;
    match projected {
        Ok(_) => Ok(()),
        Err(CollabDbError::NotFound | CollabDbError::Forbidden) => Err(ImportBodyError::NotFound),
        Err(_) => Err(ImportBodyError::Failed),
    }
}
