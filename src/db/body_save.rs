//! Boot-OFF wiki/project/task saves: native history, current authority, CAS, revision,
//! outbox and stable command result share one reserved writer and one finish.
use crate::collab::{
    revision::{
        capture_revision_offline, prepare_off_body, prepare_off_restore, OffBodyPrepareError,
    },
    CollabConfig,
};
use crate::config::RealtimeMode;
use crate::db::{
    backend::{Backend, CommitCleanupUnknown, OperationTx},
    codec::Cell,
    collab::{
        AppendCollabInput, CollabDbError, CollabKind, CollabLoadState, ProjectDerivedBodyInput,
    },
    revisions::{
        CreateRevisionInput, RestoreRevisionInput, RevisionDbError, RevisionDetail, RevisionScope,
        RevisionTarget,
    },
};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc,
};
use uuid::Uuid;

#[derive(Debug, thiserror::Error)]
pub enum BodySaveError {
    #[error(transparent)]
    Database(#[from] sqlx::Error),
    #[error("native body access refused: {0:?}")]
    Native(CollabDbError),
    #[error("revision access refused: {0:?}")]
    Revision(RevisionDbError),
    #[error("body version conflict")]
    Conflict,
    #[error("body command binding mismatch")]
    RequestMismatch,
    #[error("invalid native body")]
    Invalid,
    #[error("isolated native engine unavailable")]
    Unavailable,
    #[error("body request cancelled")]
    Cancelled,
    #[error("body commit is unconfirmed")]
    CommitUnconfirmed(#[source] Box<CommitCleanupUnknown>),
    #[error("body rollback is unconfirmed; original={original}")]
    RollbackUnconfirmed {
        original: Box<BodySaveError>,
        #[source]
        cleanup: sqlx::Error,
    },
}

#[derive(Clone)]
pub struct OffBodyRequest {
    pub workspace: Uuid,
    pub target: RevisionTarget,
    pub project: Option<Uuid>,
    pub actor: Uuid,
    pub credential: Uuid,
    pub command: Uuid,
    pub expected_tail: i64,
    pub update: Vec<u8>,
    pub client_ip: Option<String>,
}
impl OffBodyRequest {
    fn scope(&self) -> RevisionScope {
        match (self.target, self.project) {
            (RevisionTarget::Document(document), Some(project)) => {
                RevisionScope::project_document(project, document)
            }
            _ => self.target.into(),
        }
    }
    fn kind(&self) -> CollabKind {
        match self.target {
            RevisionTarget::Document(_) => CollabKind::Document,
            RevisionTarget::Task(_) => CollabKind::Task,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SavedBody {
    pub command_id: Uuid,
    pub target_id: Uuid,
    pub tail_seq: String,
    pub revision_id: Uuid,
}

/// One frozen logical draft publication. Source view and destination creation
/// are independent authorities; this never changes the source native history.
#[derive(Clone)]
pub struct OffDraftCreateRequest {
    pub workspace: Uuid,
    pub source: RevisionScope,
    pub destination_project: Option<Uuid>,
    pub parent: Option<Uuid>,
    pub actor: Uuid,
    pub credential: Uuid,
    pub command: Uuid,
    pub title: String,
    pub icon: Option<String>,
    pub content_json: Value,
    pub client_ip: Option<String>,
}

impl OffDraftCreateRequest {
    fn validate(&self) -> Result<(), OffDraftCreateError> {
        if self.command.is_nil()
            || self.workspace.is_nil()
            || self.source.target().id().is_nil()
            || (matches!(self.source.target(), RevisionTarget::Task(_))
                && self.source.project_id().is_some())
            || self.destination_project.is_some_and(|id| id.is_nil())
            || self.parent.is_some_and(|id| id.is_nil())
            || !super::documents::title_is_valid(&self.title)
            || self
                .icon
                .as_ref()
                .is_some_and(|icon| !super::documents::icon_is_valid(icon))
        {
            return Err(BodySaveError::Invalid.into());
        }
        crate::collab::derived_body::extract_stored_attachment_refs(&self.content_json)
            .map_err(|_| BodySaveError::Invalid)?;
        Ok(())
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OffDraftCreated {
    pub document: super::documents::DocumentMeta,
    pub command_id: Uuid,
    pub tail_seq: String,
    pub revision_id: Uuid,
}

#[derive(Debug, thiserror::Error)]
pub enum OffDraftCreateError {
    #[error(transparent)]
    Body(#[from] BodySaveError),
    #[error("draft destination access refused: {0:?}")]
    Document(super::documents::DocumentDbError),
    #[error("draft source attachment access refused: {0:?}")]
    Attachment(super::attachments::AttachmentDbError),
    #[error("draft rollback is unconfirmed; original={original}")]
    RollbackUnconfirmed {
        original: Box<OffDraftCreateError>,
        #[source]
        cleanup: sqlx::Error,
    },
}
impl From<sqlx::Error> for OffDraftCreateError {
    fn from(error: sqlx::Error) -> Self {
        Self::Body(BodySaveError::Database(error))
    }
}

fn draft_create_hash(request: &OffDraftCreateRequest) -> Result<String, sqlx::Error> {
    let bytes = serde_json::to_vec(&(
        "fvoci:off-independent-draft:v1",
        request.workspace,
        request.actor,
        request.credential,
        request.source.target().kind_str(),
        request.source.target().id(),
        request.source.project_id(),
        request.destination_project,
        request.parent,
        &request.title,
        &request.icon,
        &request.content_json,
    ))
    .map_err(|error| sqlx::Error::Encode(Box::new(error)))?;
    Ok(hex::encode(Sha256::digest(bytes)))
}

/// Dedicated OFF publication: receipt lookup precedes the randomized seed,
/// creation/native body/history/outbox/result share the caller-owned writer.
pub async fn create_off_draft(
    backend: &Backend,
    mode: RealtimeMode,
    engine: CollabConfig,
    request: OffDraftCreateRequest,
) -> Result<OffDraftCreated, OffDraftCreateError> {
    if mode != RealtimeMode::Off {
        return Err(BodySaveError::Invalid.into());
    }
    request.validate()?;
    let mut tx = backend.begin_off_body().await?;
    let cancelled = Arc::new(AtomicBool::new(false));
    let _cancel_on_drop = NativeCancel(cancelled.clone());
    let result =
        create_draft_in_writer(&mut tx.operation(), mode, engine, &request, cancelled).await;
    match result {
        Ok(created) => {
            tx.commit_with_cleanup()
                .await
                .map_err(|error| BodySaveError::CommitUnconfirmed(Box::new(error)))?;
            Ok(created)
        }
        Err(original) => {
            if let Err(cleanup) = tx.rollback().await {
                return Err(OffDraftCreateError::RollbackUnconfirmed {
                    original: Box::new(original),
                    cleanup,
                });
            }
            Err(original)
        }
    }
}

async fn create_draft_in_writer(
    op: &mut OperationTx<'_, '_>,
    mode: RealtimeMode,
    engine: CollabConfig,
    request: &OffDraftCreateRequest,
    cancelled: Arc<AtomicBool>,
) -> Result<OffDraftCreated, OffDraftCreateError> {
    if mode != RealtimeMode::Off {
        return Err(BodySaveError::Invalid.into());
    }
    request.validate()?;
    authorize_draft_source(op, request).await?;
    authorize_draft_destination(op, request).await?;
    authorize_draft_references(op, request).await?;
    let hash = draft_create_hash(request)?;
    if let Some((actor, stored_hash, target, result)) = op
        .wiki_create_receipt(request.workspace, request.command)
        .await?
    {
        if actor != request.actor || stored_hash != hash {
            return Err(BodySaveError::RequestMismatch.into());
        }
        let target = target.ok_or(BodySaveError::Native(CollabDbError::NotFound))?;
        let created: OffDraftCreated =
            serde_json::from_value(result).map_err(|_| BodySaveError::Invalid)?;
        if created.command_id != request.command
            || created.document.id != target
            || target == request.source.target().id()
            || created.document.workspace_id != request.workspace
            || created.document.project_id != request.destination_project
            || created.document.parent_id != request.parent
            || created.tail_seq != "1"
            || created.revision_id.is_nil()
        {
            return Err(BodySaveError::Invalid.into());
        }
        let (_, body_result) = op
            .body_save_receipt(request.workspace, request.command)
            .await?
            .ok_or(BodySaveError::Invalid)?;
        let body: SavedBody =
            serde_json::from_value(body_result).map_err(|_| BodySaveError::Invalid)?;
        if body.command_id != request.command
            || body.target_id != target
            || body.tail_seq != created.tail_seq
            || body.revision_id != created.revision_id
        {
            return Err(BodySaveError::Invalid.into());
        }
        let scope = match request.destination_project {
            Some(project) => RevisionScope::project_document(project, target),
            None => RevisionTarget::Document(target).into(),
        };
        op.authorize_collab_read(
            CollabKind::Document,
            request.workspace,
            request.actor,
            request.credential,
            target,
            &mut super::collab::CollabDbStageTimings::default(),
        )
        .await?
        .map_err(BodySaveError::Native)?;
        op.authorize_revision_scope(
            request.workspace,
            request.actor,
            request.credential,
            scope,
            false,
        )
        .await?
        .map_err(BodySaveError::Revision)?;
        if cancelled.load(Ordering::Acquire) {
            return Err(BodySaveError::Cancelled.into());
        }
        if !op
            .recheck_session(request.actor, request.credential)
            .await?
        {
            return Err(BodySaveError::Native(CollabDbError::Forbidden).into());
        }
        return Ok(created);
    }
    if cancelled.load(Ordering::Acquire) {
        return Err(BodySaveError::Cancelled.into());
    }
    // This exact maintained producer is owned/reviewed separately. It seeds a
    // new document with server-assigned block IDs; it never reseeds the source.
    let seed = crate::collab::seed::SeedEngine::new(engine.engine_bin.clone(), engine.limits)
        .tiptap_to_independent_yjs_update(&request.content_json)
        .await
        .map_err(|error| match error {
            crate::collab::seed::SeedError::InvalidInput(_) => BodySaveError::Invalid,
            crate::collab::seed::SeedError::TooLarge(_) => {
                BodySaveError::Native(CollabDbError::PayloadTooLarge)
            }
            crate::collab::seed::SeedError::Unavailable
            | crate::collab::seed::SeedError::Failed(_) => BodySaveError::Unavailable,
        })?;
    if cancelled.load(Ordering::Acquire) {
        return Err(BodySaveError::Cancelled.into());
    }
    // Current source/destination/reference checks are repeated after the seed
    // await before the first creation effect; authority is never lease metadata.
    authorize_draft_source(op, request).await?;
    authorize_draft_destination(op, request).await?;
    authorize_draft_references(op, request).await?;
    let input = super::documents::CreateDocumentInput {
        parent_id: request.parent,
        title: &request.title,
        icon: Some(request.icon.as_deref()),
    };
    let created = match request.destination_project {
        Some(project) => super::project_documents::create_project_document_operation(
            op,
            request.workspace,
            project,
            request.actor,
            request.credential,
            input,
            request.client_ip.as_deref(),
        )
        .await?
        .map_err(OffDraftCreateError::Document)?,
        None => super::documents::create_wiki_document_operation(
            op,
            request.workspace,
            request.actor,
            request.credential,
            input,
            request.client_ip.as_deref(),
            None,
        )
        .await?
        .map_err(OffDraftCreateError::Document)?
        .ok_or(BodySaveError::Invalid)?,
    };
    if created.id == request.source.target().id() {
        return Err(BodySaveError::Invalid.into());
    }
    let (saved, proof) = save_in_writer_with_proof(
        op,
        mode,
        engine,
        &OffBodyRequest {
            workspace: request.workspace,
            target: RevisionTarget::Document(created.id),
            project: request.destination_project,
            actor: request.actor,
            credential: request.credential,
            command: request.command,
            expected_tail: 0,
            update: seed,
            client_ip: request.client_ip.clone(),
        },
        cancelled.clone(),
        None,
    )
    .await?;
    let row = op
        .document_row(request.workspace, created.id)
        .await?
        .ok_or(BodySaveError::Invalid)?;
    let mut document = super::documents::row_to_meta(row, true);
    document.display_id = created.display_id;
    let result = OffDraftCreated {
        document,
        command_id: saved.command_id,
        tail_seq: saved.tail_seq,
        revision_id: saved.revision_id,
    };
    let json =
        serde_json::to_value(&result).map_err(|error| sqlx::Error::Encode(Box::new(error)))?;
    op.insert_wiki_create_receipt(
        request.workspace,
        request.command,
        request.actor,
        &hash,
        result.document.id,
        &json,
    )
    .await?;
    authorize_draft_source(op, request).await?;
    authorize_draft_destination(op, request).await?;
    authorize_draft_references(op, request).await?;
    if cancelled.load(Ordering::Acquire) {
        return Err(BodySaveError::Cancelled.into());
    }
    if !op
        .recheck_session(request.actor, request.credential)
        .await?
        || !op.verify_off_body_writer(proof).await?
    {
        return Err(BodySaveError::Native(CollabDbError::Forbidden).into());
    }
    Ok(result)
}

/// Same current actor/session and maintained source View locks, including an
/// archived readable source. This capability does not grant destination rights.
async fn authorize_draft_source(
    op: &mut OperationTx<'_, '_>,
    request: &OffDraftCreateRequest,
) -> Result<(), OffDraftCreateError> {
    op.set_tenant(request.workspace).await?;
    op.lock_membership_users(&[request.actor]).await?;
    op.lock_tree(request.workspace).await?;
    let kind = match request.source.target() {
        RevisionTarget::Document(_) => CollabKind::Document,
        RevisionTarget::Task(_) => CollabKind::Task,
    };
    op.authorize_collab_read(
        kind,
        request.workspace,
        request.actor,
        request.credential,
        request.source.target().id(),
        &mut super::collab::CollabDbStageTimings::default(),
    )
    .await?
    .map_err(BodySaveError::Native)?;
    op.authorize_revision_scope(
        request.workspace,
        request.actor,
        request.credential,
        request.source,
        false,
    )
    .await?
    .map_err(BodySaveError::Revision)?;
    Ok(())
}

/// Native document/task reference access is the maintained View policy on the
/// same outer writer, with project/resource locks and current credential.
async fn authorize_internal_draft_refs(
    op: &mut OperationTx<'_, '_>,
    request: &OffDraftCreateRequest,
) -> Result<(), OffDraftCreateError> {
    for reference in crate::collab::derived_body::extract_internal_refs(&request.content_json) {
        let id = Uuid::parse_str(&reference.id).map_err(|_| BodySaveError::Invalid)?;
        let kind = match reference.kind {
            crate::collab::derived_body::InternalRefKind::Document => CollabKind::Document,
            crate::collab::derived_body::InternalRefKind::Task => CollabKind::Task,
        };
        op.authorize_collab_read(
            kind,
            request.workspace,
            request.actor,
            request.credential,
            id,
            &mut super::collab::CollabDbStageTimings::default(),
        )
        .await?
        .map_err(BodySaveError::Native)?;
    }
    Ok(())
}

/// Stored file references retain original IDs/parent/ownership. Admission is
/// current source View on the borrowed writer, never a recipient access grant.
async fn authorize_draft_references(
    op: &mut OperationTx<'_, '_>,
    request: &OffDraftCreateRequest,
) -> Result<(), OffDraftCreateError> {
    let attachments =
        crate::collab::derived_body::extract_stored_attachment_refs(&request.content_json)
            .map_err(|_| BodySaveError::Invalid)?;
    authorize_internal_draft_refs(op, request).await?;
    for id in attachments {
        op.authorize_stored_attachment_reference(
            request.workspace,
            id,
            request.actor,
            request.credential,
        )
        .await?
        .map_err(OffDraftCreateError::Attachment)?;
    }
    Ok(())
}

/// Repeat the current ordinary create policy before even a stored result is
/// returned. Receipt replay is not a right to a retired destination or parent.
async fn authorize_draft_destination(
    op: &mut OperationTx<'_, '_>,
    request: &OffDraftCreateRequest,
) -> Result<(), OffDraftCreateError> {
    op.set_tenant(request.workspace).await?;
    if let Some(project) = request.destination_project {
        super::project_documents::authorize_project_draft_parent(
            op,
            request.workspace,
            project,
            request.actor,
            request.credential,
            request.parent,
        )
        .await?
        .map_err(OffDraftCreateError::Document)?;
        return Ok(());
    }
    op.lock_membership_users(&[request.actor]).await?;
    if !op
        .recheck_session(request.actor, request.credential)
        .await?
    {
        return Err(OffDraftCreateError::Document(
            super::documents::DocumentDbError::Forbidden,
        ));
    }
    op.lock_tree(request.workspace).await?;
    if !op.workspace_is_live(request.workspace).await? {
        return Err(OffDraftCreateError::Document(
            super::documents::DocumentDbError::NotFound,
        ));
    }
    if !super::documents::wiki_can_edit(
        op.membership_role(request.workspace, request.actor, true)
            .await?,
    ) {
        return Err(OffDraftCreateError::Document(
            super::documents::DocumentDbError::Forbidden,
        ));
    }
    if let Some(parent) = request.parent {
        let Some((project, path, deleted)) = op.wiki_parent(request.workspace, parent).await?
        else {
            return Err(OffDraftCreateError::Document(
                super::documents::DocumentDbError::NotFound,
            ));
        };
        if deleted.is_some() {
            return Err(OffDraftCreateError::Document(
                super::documents::DocumentDbError::NotFound,
            ));
        }
        if project.is_some() {
            return Err(OffDraftCreateError::Document(
                super::documents::DocumentDbError::AffiliationMismatch,
            ));
        }
        if super::documents::depth_of(&path) >= super::documents::MAX_TREE_DEPTH {
            return Err(OffDraftCreateError::Document(
                super::documents::DocumentDbError::DepthLimit,
            ));
        }
    }
    Ok(())
}

pub struct OffBodySource {
    pub native: CollabLoadState,
    pub content_json: Value,
    pub writable: bool,
}

struct NativeCancel(Arc<AtomicBool>);
impl Drop for NativeCancel {
    fn drop(&mut self) {
        self.0.store(true, Ordering::Release);
    }
}

pub struct OffRestorePreview {
    pub source: RevisionDetail,
    pub current_content_json: Value,
    pub current_tail: i64,
}

pub async fn create_off_revision(
    backend: &Backend,
    mode: RealtimeMode,
    engine: CollabConfig,
    workspace: Uuid,
    scope: RevisionScope,
    actor: Uuid,
    credential: Uuid,
) -> Result<Uuid, BodySaveError> {
    match capture_off_revision(
        backend, mode, engine, workspace, scope, actor, credential, None,
    )
    .await?
    {
        OffRevisionCapture::Created(id) => Ok(id),
        OffRevisionCapture::Preview(_) => Err(BodySaveError::Invalid),
    }
}

#[expect(
    clippy::too_many_arguments,
    reason = "restore preview binds one immutable source revision to the current native writer scope"
)]
pub async fn preview_off_restore(
    backend: &Backend,
    mode: RealtimeMode,
    engine: CollabConfig,
    workspace: Uuid,
    scope: RevisionScope,
    actor: Uuid,
    credential: Uuid,
    source: Uuid,
) -> Result<OffRestorePreview, BodySaveError> {
    match capture_off_revision(
        backend,
        mode,
        engine,
        workspace,
        scope,
        actor,
        credential,
        Some(source),
    )
    .await?
    {
        OffRevisionCapture::Preview(preview) => Ok(preview),
        OffRevisionCapture::Created(_) => Err(BodySaveError::Invalid),
    }
}

enum OffRevisionCapture {
    Created(Uuid),
    Preview(OffRestorePreview),
}

#[expect(
    clippy::too_many_arguments,
    reason = "manual history and restore preview use the same current native writer scope"
)]
async fn capture_off_revision(
    backend: &Backend,
    mode: RealtimeMode,
    engine: CollabConfig,
    workspace: Uuid,
    scope: RevisionScope,
    actor: Uuid,
    credential: Uuid,
    source: Option<Uuid>,
) -> Result<OffRevisionCapture, BodySaveError> {
    if mode != RealtimeMode::Off {
        return Err(BodySaveError::Invalid);
    }
    let kind = match scope.target() {
        RevisionTarget::Document(_) => CollabKind::Document,
        RevisionTarget::Task(_) => CollabKind::Task,
    };
    let mut tx = backend.begin_off_body().await?;
    let cancelled = Arc::new(AtomicBool::new(false));
    let _cancel_on_drop = NativeCancel(cancelled.clone());
    let result = async {
        let mut op = tx.operation();
        op.set_tenant(workspace).await?;
        let (proof, native) = op
            .load_off_body_writer(
                mode,
                kind,
                workspace,
                actor,
                credential,
                scope.target().id(),
            )
            .await?
            .map_err(BodySaveError::Native)?;
        op.authorize_revision_scope(workspace, actor, credential, scope, true)
            .await?
            .map_err(BodySaveError::Revision)?;
        let detail = match source {
            Some(id) => Some(
                op.off_revision_source(workspace, actor, credential, scope, id)
                    .await?
                    .map_err(BodySaveError::Revision)?,
            ),
            None => None,
        };
        let snapshot = native.snapshot;
        let tail = native.tail.into_iter().map(|row| row.payload).collect();
        let check_cancel = cancelled.clone();
        let captured = tokio::task::spawn_blocking(move || {
            if check_cancel.load(Ordering::Acquire) {
                return Err(BodySaveError::Cancelled);
            }
            let captured =
                capture_revision_offline(engine.engine_bin, engine.limits, snapshot, tail)
                    .map_err(|_| BodySaveError::Unavailable)?;
            if check_cancel.load(Ordering::Acquire) {
                return Err(BodySaveError::Cancelled);
            }
            Ok(captured)
        })
        .await
        .map_err(|_| BodySaveError::Unavailable)??;
        let result = if let Some(source) = detail {
            OffRevisionCapture::Preview(OffRestorePreview {
                source,
                current_content_json: captured.content_json,
                current_tail: native.tail_seq,
            })
        } else {
            let text = crate::collab::revision::prepare_revision_text(&captured.content_json)
                .map_err(|_| BodySaveError::Invalid)?;
            let id = op
                .create_manual_revision(
                    workspace,
                    actor,
                    credential,
                    scope,
                    CreateRevisionInput {
                        y_snapshot: captured.y_snapshot,
                        content_json: captured.content_json,
                        text,
                        reason: "manual".into(),
                    },
                )
                .await?
                .map_err(BodySaveError::Revision)?;
            OffRevisionCapture::Created(id)
        };
        op.authorize_revision_scope(workspace, actor, credential, scope, true)
            .await?
            .map_err(BodySaveError::Revision)?;
        if !op.recheck_session(actor, credential).await?
            || !op.verify_off_body_writer(proof).await?
        {
            return Err(BodySaveError::Native(CollabDbError::Forbidden));
        }
        Ok(result)
    }
    .await;
    match result {
        Ok(value) => {
            tx.commit_with_cleanup()
                .await
                .map_err(|e| BodySaveError::CommitUnconfirmed(Box::new(e)))?;
            Ok(value)
        }
        Err(original) => {
            if let Err(cleanup) = tx.rollback().await {
                return Err(BodySaveError::RollbackUnconfirmed {
                    original: Box::new(original),
                    cleanup,
                });
            }
            Err(original)
        }
    }
}

pub async fn read_off_body(
    backend: &Backend,
    mode: RealtimeMode,
    engine: CollabConfig,
    workspace: Uuid,
    scope: RevisionScope,
    actor: Uuid,
    credential: Uuid,
) -> Result<OffBodySource, BodySaveError> {
    let document = scope.target().id();
    let kind = match scope.target() {
        RevisionTarget::Document(_) => CollabKind::Document,
        RevisionTarget::Task(_) => CollabKind::Task,
    };
    let mut tx = backend.begin_off_body().await?;
    let result = async {
        let mut op = tx.operation();
        op.set_tenant(workspace).await?;
        op.authorize_revision_scope(workspace, actor, credential, scope, false)
            .await?
            .map_err(BodySaveError::Revision)?;
        let native = op
            .load_off_body_read(mode, kind, workspace, actor, credential, document)
            .await?
            .map_err(BodySaveError::Native)?;
        let snapshot = native.snapshot.clone();
        let tail = native.tail.iter().map(|row| row.payload.clone()).collect();
        let captured = tokio::task::spawn_blocking(move || {
            capture_revision_offline(engine.engine_bin, engine.limits, snapshot, tail)
        })
        .await
        .map_err(|_| BodySaveError::Unavailable)?
        .map_err(|_| BodySaveError::Unavailable)?;
        op.authorize_revision_scope(workspace, actor, credential, scope, false)
            .await?
            .map_err(BodySaveError::Revision)?;
        if !op.recheck_session(actor, credential).await? {
            return Err(BodySaveError::Native(CollabDbError::Forbidden));
        }
        let writable = op
            .authorize_collab_write(
                kind,
                workspace,
                actor,
                credential,
                document,
                &mut super::collab::CollabDbStageTimings::default(),
            )
            .await?
            .is_ok()
            && op
                .authorize_revision_scope(workspace, actor, credential, scope, true)
                .await?
                .is_ok();
        Ok(OffBodySource {
            native,
            content_json: captured.content_json,
            writable,
        })
    }
    .await;
    match result {
        Ok(source) => {
            tx.commit_with_cleanup()
                .await
                .map_err(|e| BodySaveError::CommitUnconfirmed(Box::new(e)))?;
            Ok(source)
        }
        Err(original) => {
            if let Err(cleanup) = tx.rollback().await {
                return Err(BodySaveError::RollbackUnconfirmed {
                    original: Box::new(original),
                    cleanup,
                });
            }
            Err(original)
        }
    }
}

pub async fn save_off_body(
    backend: &Backend,
    mode: RealtimeMode,
    engine: CollabConfig,
    request: OffBodyRequest,
) -> Result<SavedBody, BodySaveError> {
    finish_off_save(backend, mode, engine, request, None).await
}

pub async fn restore_off_body(
    backend: &Backend,
    mode: RealtimeMode,
    engine: CollabConfig,
    request: OffBodyRequest,
    source_revision: Uuid,
) -> Result<SavedBody, BodySaveError> {
    if source_revision.is_nil() || !request.update.is_empty() {
        return Err(BodySaveError::Invalid);
    }
    finish_off_save(backend, mode, engine, request, Some(source_revision)).await
}

async fn finish_off_save(
    backend: &Backend,
    mode: RealtimeMode,
    engine: CollabConfig,
    request: OffBodyRequest,
    source_revision: Option<Uuid>,
) -> Result<SavedBody, BodySaveError> {
    if mode != RealtimeMode::Off
        || (matches!(request.target, RevisionTarget::Task(_)) && request.project.is_some())
        || request.expected_tail < 0
        || request.expected_tail == i64::MAX
        || request.command.is_nil()
        || (source_revision.is_none() && request.update.is_empty())
        || request.update.len() > super::collab::MAX_COLLAB_UPDATE_BYTES
    {
        return Err(BodySaveError::Invalid);
    }
    let mut tx = backend.begin_off_body().await?;
    let cancelled = Arc::new(AtomicBool::new(false));
    let _cancel_on_drop = NativeCancel(cancelled.clone());
    let result = save_in_writer(
        &mut tx.operation(),
        mode,
        engine,
        &request,
        cancelled,
        source_revision,
    )
    .await;
    match result {
        Ok(saved) => {
            tx.commit_with_cleanup()
                .await
                .map_err(|e| BodySaveError::CommitUnconfirmed(Box::new(e)))?;
            Ok(saved)
        }
        Err(original) => {
            if let Err(cleanup) = tx.rollback().await {
                return Err(BodySaveError::RollbackUnconfirmed {
                    original: Box::new(original),
                    cleanup,
                });
            }
            Err(original)
        }
    }
}

fn command_hash(request: &OffBodyRequest) -> String {
    let mut hash = Sha256::new();
    match (request.target, request.project) {
        (RevisionTarget::Document(_), Some(project)) => {
            hash.update(b"fvoci:off-project-body:v1\0");
            hash.update(project.as_bytes());
        }
        (RevisionTarget::Task(_), _) => hash.update(b"fvoci:off-task-body:v1\0"),
        (RevisionTarget::Document(_), None) => hash.update(b"fvoci:off-wiki-body:v1\0"),
    }
    for id in [
        request.workspace,
        request.target.id(),
        request.actor,
        request.credential,
    ] {
        hash.update(id.as_bytes());
    }
    hash.update(request.expected_tail.to_be_bytes());
    hash.update((request.update.len() as u64).to_be_bytes());
    hash.update(&request.update);
    hex::encode(hash.finalize())
}

fn restore_command_hash(request: &OffBodyRequest, source: &RevisionDetail) -> String {
    let mut hash = Sha256::new();
    hash.update(b"fvoci:off-forward-restore:v1\0");
    hash.update(command_hash(request).as_bytes());
    hash.update(source.meta.id.as_bytes());
    hash.update((source.y_snapshot.len() as u64).to_be_bytes());
    hash.update(&source.y_snapshot);
    hex::encode(hash.finalize())
}

async fn save_in_writer(
    op: &mut OperationTx<'_, '_>,
    mode: RealtimeMode,
    engine: CollabConfig,
    request: &OffBodyRequest,
    cancelled: Arc<AtomicBool>,
    source_revision: Option<Uuid>,
) -> Result<SavedBody, BodySaveError> {
    save_in_writer_with_proof(op, mode, engine, request, cancelled, source_revision)
        .await
        .map(|(saved, _)| saved)
}

async fn save_in_writer_with_proof(
    op: &mut OperationTx<'_, '_>,
    mode: RealtimeMode,
    engine: CollabConfig,
    request: &OffBodyRequest,
    cancelled: Arc<AtomicBool>,
    source_revision: Option<Uuid>,
) -> Result<(SavedBody, super::collab::OffBodyWriter), BodySaveError> {
    let OffBodyRequest {
        workspace,
        target,
        project: _,
        actor,
        credential,
        command,
        expected_tail,
        update,
        client_ip,
    } = request;
    let document = target.id();
    op.set_tenant(*workspace).await?;
    let (proof, load) = op
        .load_off_body_writer(
            mode,
            request.kind(),
            *workspace,
            *actor,
            *credential,
            document,
        )
        .await?
        .map_err(BodySaveError::Native)?;
    // Reuse the exact route's current document/task affiliation and authority.
    op.authorize_revision_scope(*workspace, *actor, *credential, request.scope(), true)
        .await?
        .map_err(BodySaveError::Revision)?;
    let source = match source_revision {
        Some(id) => Some(
            op.off_revision_source(*workspace, *actor, *credential, request.scope(), id)
                .await?
                .map_err(BodySaveError::Revision)?,
        ),
        None => None,
    };
    // Restores bind the immutable source, not a newly randomized forward
    // update generated after a lost response. Current source auth precedes replay.
    let hash = match &source {
        Some(detail) => restore_command_hash(request, detail),
        None => command_hash(request),
    };
    if let Some((stored, result)) = op.body_save_receipt(*workspace, *command).await? {
        if stored != hash {
            return Err(BodySaveError::RequestMismatch);
        }
        let saved: SavedBody =
            serde_json::from_value(result).map_err(|_| BodySaveError::Invalid)?;
        if saved.command_id != *command
            || saved.target_id != document
            || saved.tail_seq
                != expected_tail
                    .checked_add(1)
                    .ok_or(BodySaveError::Invalid)?
                    .to_string()
        {
            return Err(BodySaveError::Invalid);
        }
        if !op.recheck_session(*actor, *credential).await?
            || !op.verify_off_body_writer(proof).await?
        {
            return Err(BodySaveError::Native(CollabDbError::Forbidden));
        }
        return Ok((saved, proof));
    }
    if load.tail_seq != *expected_tail {
        return Err(BodySaveError::Conflict);
    }
    let compact_start = load.tail.len() as i64 >= super::collab::MAX_COLLAB_TAIL_UPDATES
        || load.snapshot.len() as i64
            + load
                .tail
                .iter()
                .map(|row| row.payload.len() as i64)
                .sum::<i64>()
            + if source.is_some() {
                super::collab::MAX_COLLAB_UPDATE_BYTES as i64
            } else {
                update.len() as i64
            }
            > super::collab::MAX_COLLAB_LOAD_BYTES;
    let generation = load.writer_generation;
    let snapshot = load.snapshot;
    let tail = load.tail.into_iter().map(|row| row.payload).collect();
    let payload = update.clone();
    let restore_snapshot = source.as_ref().map(|detail| detail.y_snapshot.clone());
    let native = tokio::task::spawn_blocking(move || match restore_snapshot {
        Some(source) => prepare_off_restore(
            engine.engine_bin,
            engine.limits,
            snapshot,
            tail,
            source,
            compact_start,
            &cancelled,
        ),
        None => prepare_off_body(
            engine.engine_bin,
            engine.limits,
            snapshot,
            tail,
            payload,
            compact_start,
            &cancelled,
        ),
    })
    .await
    .map_err(|_| BodySaveError::Unavailable)?
    .map_err(|e| match e {
        OffBodyPrepareError::Invalid => BodySaveError::Invalid,
        OffBodyPrepareError::Unavailable => BodySaveError::Unavailable,
        OffBodyPrepareError::Cancelled => BodySaveError::Cancelled,
    })?;
    let prepared =
        crate::collab::derived_body::prepare_derived_body(native.captured.content_json.clone())
            .map_err(|_| BodySaveError::Invalid)?;
    if let Some(start) = &native.start_complete_v1 {
        op.compact_off_body(proof, *expected_tail, start, client_ip.as_deref())
            .await?
            .map_err(BodySaveError::Native)?;
    }
    let seq = op
        .append_off_body(
            proof,
            AppendCollabInput {
                workspace_id: *workspace,
                actor_user_id: *actor,
                session_id: *credential,
                document_id: document,
                writer_generation: generation,
                expected_tail_seq: *expected_tail,
                op_id: *command,
                payload: &native.update,
                client_ip: client_ip.as_deref(),
            },
        )
        .await?
        .map_err(BodySaveError::Native)?
        .seq();
    // The command namespace can collide with an older ON/native operation.
    // A duplicate native ACK is not a newly committed expected+1 body save.
    if seq != expected_tail.checked_add(1).ok_or(BodySaveError::Invalid)? {
        return Err(BodySaveError::RequestMismatch);
    }
    op.project_off_body(
        proof,
        ProjectDerivedBodyInput::new(
            *workspace,
            *actor,
            *credential,
            document,
            generation,
            seq,
            prepared,
        ),
    )
    .await?
    .map_err(BodySaveError::Native)?;
    let text = crate::collab::revision::prepare_revision_text(&native.captured.content_json)
        .map_err(|_| BodySaveError::Invalid)?;
    let revision_input = CreateRevisionInput {
        y_snapshot: native.captured.y_snapshot,
        content_json: native.captured.content_json,
        text,
        reason: if source.is_some() {
            "restore"
        } else {
            "manual"
        }
        .into(),
    };
    let revision = if let Some(source) = source {
        op.record_off_restored_revision(
            *workspace,
            *actor,
            *credential,
            RestoreRevisionInput {
                scope: request.scope(),
                source_revision_id: source.meta.id,
                correlation_id: *command,
                expected_tail_seq: *expected_tail,
            },
            seq,
            revision_input,
        )
        .await?
    } else {
        op.create_manual_revision(
            *workspace,
            *actor,
            *credential,
            request.scope(),
            revision_input,
        )
        .await?
    }
    .map_err(BodySaveError::Revision)?;
    op.compact_off_body(proof, seq, &native.complete_v1, client_ip.as_deref())
        .await?
        .map_err(BodySaveError::Native)?;
    let saved = SavedBody {
        command_id: *command,
        target_id: document,
        tail_seq: seq.to_string(),
        revision_id: revision,
    };
    let mut committed_request = request.clone();
    committed_request.update = native.update;
    if !op
        .insert_body_save_receipt(&committed_request, &hash, &saved)
        .await?
    {
        return Err(BodySaveError::RequestMismatch);
    }
    // Current credential/target authorization is still locked and rechecked
    // after all native awaits, including immediately before caller-owned finish.
    op.authorize_revision_scope(*workspace, *actor, *credential, request.scope(), true)
        .await?
        .map_err(BodySaveError::Revision)?;
    if !op.recheck_session(*actor, *credential).await? || !op.verify_off_body_writer(proof).await? {
        return Err(BodySaveError::Native(CollabDbError::Forbidden));
    }
    Ok((saved, proof))
}

impl OperationTx<'_, '_> {
    async fn body_save_receipt(
        &mut self,
        workspace: Uuid,
        command: Uuid,
    ) -> Result<Option<(String, Value)>, sqlx::Error> {
        match self {
            Self::Postgres(tx)=>sqlx::query_as("SELECT request_hash,result_json FROM fvoci.body_save_commands WHERE workspace_id=$1 AND command_id=$2")
                .bind(workspace).bind(command).fetch_optional(&mut ***tx).await,
            Self::SqliteFamily(tx)=>{
                tx.require_writer()?;tx.require_tenant(workspace)?;
                tx.query("SELECT request_hash,result_json FROM body_save_commands WHERE workspace_id=?1 AND command_id=?2",&[Cell::uuid(workspace),Cell::uuid(command)]).await?
                    .first().map(|row|Ok::<_,sqlx::Error>((row.cell(0)?.string()?,row.cell(1)?.value()?))).transpose()
            }
        }
    }
    async fn insert_body_save_receipt(
        &mut self,
        request: &OffBodyRequest,
        hash: &str,
        saved: &SavedBody,
    ) -> Result<bool, sqlx::Error> {
        let result =
            serde_json::to_value(saved).map_err(|e| sqlx::Error::Protocol(e.to_string()))?;
        let payload_hash = Sha256::digest(&request.update).to_vec();
        let committed = request.expected_tail + 1;
        let target = request.target.id();
        let kind = request.target.kind_str();
        let (document, task) = match request.target {
            RevisionTarget::Document(id) => (Some(id), None),
            RevisionTarget::Task(id) => (None, Some(id)),
        };
        match self {
            Self::Postgres(tx) => {
                let count=sqlx::query("INSERT INTO fvoci.body_save_commands(workspace_id,command_id,actor_user_id,credential_id,target_kind,target_id,document_id,task_id,expected_tail_seq,committed_tail_seq,request_hash,payload_hash,result_json) VALUES($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11,$12,$13) ON CONFLICT(workspace_id,command_id) DO NOTHING")
                    .bind(request.workspace).bind(request.command).bind(request.actor).bind(request.credential)
                    .bind(kind).bind(target).bind(document).bind(task).bind(request.expected_tail).bind(committed)
                    .bind(hash).bind(payload_hash).bind(result).execute(&mut ***tx).await?.rows_affected();
                Ok(count == 1)
            }
            Self::SqliteFamily(tx) => {
                tx.require_writer()?;
                tx.require_tenant(request.workspace)?;
                let count = tx.execute("INSERT INTO body_save_commands(workspace_id,command_id,actor_user_id,credential_id,target_kind,target_id,document_id,task_id,expected_tail_seq,committed_tail_seq,request_hash,payload_hash,result_json) VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13) ON CONFLICT(workspace_id,command_id) DO NOTHING",
                    &[Cell::uuid(request.workspace),Cell::uuid(request.command),Cell::uuid(request.actor),Cell::uuid(request.credential),
                    Cell::text(kind),Cell::uuid(target),Cell::optional_uuid(document),Cell::optional_uuid(task),Cell::Integer(request.expected_tail),Cell::Integer(committed),
                    Cell::text(hash),Cell::Blob(payload_hash),Cell::json(&result)?]).await?;
                Ok(count == 1)
            }
        }
    }
}

#[cfg(test)]
mod policy_tests {
    use super::*;
    #[test]
    fn draft_command_hash_binds_source_destination_current_identity_and_exact_private_body() {
        let request = OffDraftCreateRequest {
            workspace: Uuid::now_v7(),
            source: RevisionTarget::Document(Uuid::now_v7()).into(),
            destination_project: None,
            parent: None,
            actor: Uuid::now_v7(),
            credential: Uuid::now_v7(),
            command: Uuid::now_v7(),
            title: "private 😀".into(),
            icon: None,
            content_json: serde_json::json!({"type":"doc","content":[]}),
            client_ip: None,
        };
        request.validate().unwrap();
        let hash = draft_create_hash(&request).unwrap();
        let mutations: [fn(&mut OffDraftCreateRequest); 9] = [
            |r| r.workspace = Uuid::now_v7(),
            |r| r.actor = Uuid::now_v7(),
            |r| r.credential = Uuid::now_v7(),
            |r| r.source = RevisionTarget::Task(r.source.target().id()).into(),
            |r| r.source = RevisionScope::project_document(Uuid::now_v7(), r.source.target().id()),
            |r| r.destination_project = Some(Uuid::now_v7()),
            |r| r.parent = Some(Uuid::now_v7()),
            |r| r.title.push('!'),
            |r| {
                r.content_json =
                    serde_json::json!({"type":"doc","content":[{"type":"paragraph","content":[]}]})
            },
        ];
        for mutate in mutations {
            let mut next = request.clone();
            mutate(&mut next);
            assert_ne!(draft_create_hash(&next).unwrap(), hash);
        }
        let mut retry = request.clone();
        retry.client_ip = Some("127.0.0.1".into());
        assert_eq!(draft_create_hash(&retry).unwrap(), hash);
        retry.command = Uuid::nil();
        assert!(retry.validate().is_err());
        retry.command = request.command;
        retry.content_json = serde_json::json!({"type":"doc","content":[{"type":"attachment","attrs":{"id":"bad-ref"}}]});
        assert!(
            retry.validate().is_err(),
            "malformed supported references must fail before effects"
        );
    }

    #[test]
    fn command_binding_covers_content_version_target_and_current_identity() {
        let request = OffBodyRequest {
            workspace: Uuid::now_v7(),
            target: RevisionTarget::Document(Uuid::now_v7()),
            project: None,
            actor: Uuid::now_v7(),
            credential: Uuid::now_v7(),
            command: Uuid::now_v7(),
            expected_tail: 7,
            update: vec![0, 0],
            client_ip: None,
        };
        let original = command_hash(&request);
        let mutations: [fn(&mut OffBodyRequest); 8] = [
            |r| r.workspace = Uuid::now_v7(),
            |r| r.target = RevisionTarget::Document(Uuid::now_v7()),
            |r| r.project = Some(Uuid::now_v7()),
            |r| r.target = RevisionTarget::Task(r.target.id()),
            |r| r.actor = Uuid::now_v7(),
            |r| r.credential = Uuid::now_v7(),
            |r| r.expected_tail += 1,
            |r| r.update.push(0),
        ];
        for mutate in mutations {
            let mut changed = request.clone();
            mutate(&mut changed);
            assert_ne!(command_hash(&changed), original);
        }
        let mut retransmission = request.clone();
        retransmission.client_ip = Some("127.0.0.1".into());
        assert_eq!(command_hash(&retransmission), original);
    }
    #[test]
    fn request_drop_cancels_native_preparation_before_spawn() {
        let cancelled = Arc::new(AtomicBool::new(false));
        drop(NativeCancel(cancelled.clone()));
        assert!(matches!(
            prepare_off_body(
                "must-not-be-spawned".into(),
                collab_engine::limits::Limits::default(),
                vec![0, 0],
                vec![],
                vec![0, 0],
                false,
                &cancelled
            ),
            Err(OffBodyPrepareError::Cancelled)
        ));
    }
}

#[cfg(all(test, feature = "db-tests"))]
mod sqlite_native_tests {
    use super::*;
    use crate::collab::seed::SeedEngine;
    use crate::db::attachment_preview::tests::Fixture;
    use serde_json::json;

    async fn draft_observable(f: &Fixture) -> (i64, i64, i64, i64, i64, i64, i64) {
        sqlx::query_as("SELECT (SELECT count(*) FROM documents WHERE workspace_id=?1),(SELECT count(*) FROM wiki_create_commands WHERE workspace_id=?1),(SELECT count(*) FROM body_save_commands WHERE workspace_id=?1),(SELECT count(*) FROM revisions WHERE workspace_id=?1),(SELECT count(*) FROM events WHERE workspace_id=?1),(SELECT count(*) FROM audit_log WHERE workspace_id=?1),(SELECT next_document_number FROM workspaces WHERE id=?1)")
            .bind(f.workspace.as_bytes().as_slice()).fetch_one(&f.pool).await.unwrap()
    }
    async fn original_history(f: &Fixture) -> (i64, Vec<u8>, String, i64, i64) {
        sqlx::query_as("SELECT ds.tail_seq,ds.state,d.content_json,(SELECT count(*) FROM revisions WHERE workspace_id=?1 AND target_kind='document' AND target_id=?2),(SELECT count(*) FROM document_collab_op_receipts WHERE workspace_id=?1 AND document_id=?2) FROM document_states ds JOIN documents d ON d.workspace_id=ds.workspace_id AND d.id=ds.document_id WHERE ds.workspace_id=?1 AND ds.document_id=?2")
            .bind(f.workspace.as_bytes().as_slice()).bind(f.document.as_bytes().as_slice()).fetch_one(&f.pool).await.unwrap()
    }
    #[tokio::test]
    async fn off_draft_copy_has_independent_ids_same_fk_rollback_exact_receipt_and_authorized_replay(
    ) {
        let f = Fixture::new().await;
        let credential = session(&f).await;
        sqlx::query("UPDATE workspaces SET next_document_number=1 WHERE id=?1")
            .bind(f.workspace.as_bytes().as_slice())
            .execute(&f.pool)
            .await
            .unwrap();
        let original = request(&f, credential, "server original 😀").await;
        save_off_body(&f.backend, RealtimeMode::Off, engine(), original)
            .await
            .unwrap();
        let (attachment, _) = f.attachment(10, "text/plain").await;
        let source = original_history(&f).await;
        let request = OffDraftCreateRequest {
            workspace: f.workspace,
            source: RevisionTarget::Document(f.document).into(),
            destination_project: None,
            parent: None,
            actor: f.user,
            credential,
            command: Uuid::now_v7(),
            title: "private independent copy".into(),
            icon: Some("📄".into()),
            content_json: json!({"type":"doc","content":[
                {"type":"paragraph","attrs":{"id":"off-stable-block"},"content":[{"type":"text","text":"unsaved private 😀","marks":[{"type":"bold"}]}]},
                {"type":"attachment","attrs":{"id":attachment,"name":"literal.txt","image":false,"width":64,"align":null,"caption":"private caption","previewWidth":128,"previewHeight":256}},
                {"type":"embed","attrs":{"id":"client-embed-block","entity":"document","ref":f.document}},
            ]}),
            client_ip: Some("127.0.0.1".into()),
        };
        let input = request.content_json.clone();
        let before = draft_observable(&f).await;
        let mut cancelled_writer = f.backend.begin_off_body().await.unwrap();
        assert!(matches!(
            create_draft_in_writer(
                &mut cancelled_writer.operation(),
                RealtimeMode::Off,
                engine(),
                &request,
                Arc::new(AtomicBool::new(true)),
            )
            .await,
            Err(OffDraftCreateError::Body(BodySaveError::Cancelled))
        ));
        cancelled_writer.rollback().await.unwrap();
        assert_eq!(draft_observable(&f).await, before);
        assert_eq!(original_history(&f).await, source);
        // Cancellation did not consume the stable logical command. The exact
        // same binding below must still reach a real writer and make progress.
        let mut outer = f.backend.begin_off_body().await.unwrap();
        let staged = create_draft_in_writer(
            &mut outer.operation(),
            RealtimeMode::Off,
            engine(),
            &request,
            Arc::new(AtomicBool::new(false)),
        )
        .await
        .unwrap();
        let OperationTx::SqliteFamily(writer) = outer.operation() else {
            panic!("actual family writer")
        };
        let fk=writer.execute("UPDATE wiki_create_commands SET document_id=?1 WHERE workspace_id=?2 AND command_id=?3",&[Cell::uuid(Uuid::now_v7()),Cell::uuid(f.workspace),Cell::uuid(request.command)]).await.unwrap_err();
        assert!(
            fk.to_string().contains("FOREIGN KEY"),
            "real same-writer FK failure after ALL copy effects: {fk}"
        );
        outer.rollback().await.unwrap();
        assert_eq!(draft_observable(&f).await, before);
        assert_eq!(original_history(&f).await, source);
        let absent: i64 = sqlx::query_scalar("SELECT count(*) FROM documents WHERE id=?1")
            .bind(staged.document.id.as_bytes().as_slice())
            .fetch_one(&f.pool)
            .await
            .unwrap();
        assert_eq!(absent, 0);
        let created = create_off_draft(&f.backend, RealtimeMode::Off, engine(), request.clone())
            .await
            .unwrap();
        assert_ne!(created.document.id, f.document);
        assert_ne!(created.document.id, staged.document.id);
        assert_eq!(created.tail_seq, "1");
        assert_eq!(created.document.number, 2);
        assert!(!created.revision_id.is_nil());
        assert_eq!(
            request.content_json, input,
            "producer/copy must not mutate private input"
        );
        let fresh = read_off_body(
            &f.backend,
            RealtimeMode::Off,
            engine(),
            f.workspace,
            RevisionTarget::Document(created.document.id).into(),
            f.user,
            credential,
        )
        .await
        .unwrap();
        assert_eq!(fresh.native.tail_seq, 1);
        assert_eq!(fresh.content_json, created.document.content_json);
        let blocks = &fresh.content_json["content"];
        let paragraph = blocks[0]["attrs"]["id"].as_str().unwrap();
        let embed = blocks[2]["attrs"]["id"].as_str().unwrap();
        assert!(Uuid::parse_str(paragraph).is_ok() && Uuid::parse_str(embed).is_ok());
        assert_ne!(paragraph, embed);
        assert_ne!(paragraph, "off-stable-block");
        assert_ne!(embed, "client-embed-block");
        assert_eq!(blocks[1]["attrs"]["id"], json!(attachment));
        assert_eq!(blocks[1]["attrs"]["caption"], json!("private caption"));
        assert_eq!(blocks[1]["attrs"]["width"], json!(64));
        assert_eq!(blocks[1]["attrs"]["previewWidth"], json!(128));
        assert_eq!(blocks[2]["attrs"]["ref"], json!(f.document));
        assert!(
            fresh
                .content_json
                .to_string()
                .contains("unsaved private 😀")
                && fresh.content_json.to_string().contains("bold")
        );
        assert_eq!(
            original_history(&f).await,
            source,
            "source native snapshot/body/revision/receipts remain immutable"
        );
        let after = draft_observable(&f).await;
        let replay = create_off_draft(&f.backend, RealtimeMode::Off, engine(), request.clone())
            .await
            .unwrap();
        assert_eq!(
            serde_json::to_value(&replay).unwrap(),
            serde_json::to_value(&created).unwrap()
        );
        assert_eq!(draft_observable(&f).await, after);
        let mut mismatch = request.clone();
        mismatch.title.push('!');
        assert!(matches!(
            create_off_draft(&f.backend, RealtimeMode::Off, engine(), mismatch).await,
            Err(OffDraftCreateError::Body(BodySaveError::RequestMismatch))
        ));
        assert_eq!(draft_observable(&f).await, after);
        // A fabricated/mismatched command result cannot become a copy ACK.
        let original_result: String = sqlx::query_scalar(
            "SELECT result_json FROM wiki_create_commands WHERE workspace_id=?1 AND command_id=?2",
        )
        .bind(f.workspace.as_bytes().as_slice())
        .bind(request.command.as_bytes().as_slice())
        .fetch_one(&f.pool)
        .await
        .unwrap();
        let mut wrong: Value = serde_json::from_str(&original_result).unwrap();
        wrong["revision_id"] = json!(Uuid::now_v7());
        sqlx::query("UPDATE wiki_create_commands SET result_json=?1 WHERE workspace_id=?2 AND command_id=?3").bind(wrong.to_string()).bind(f.workspace.as_bytes().as_slice()).bind(request.command.as_bytes().as_slice()).execute(&f.pool).await.unwrap();
        assert!(matches!(
            create_off_draft(&f.backend, RealtimeMode::Off, engine(), request.clone()).await,
            Err(OffDraftCreateError::Body(BodySaveError::Invalid))
        ));
        sqlx::query("UPDATE wiki_create_commands SET result_json=?1 WHERE workspace_id=?2 AND command_id=?3").bind(&original_result).bind(f.workspace.as_bytes().as_slice()).bind(request.command.as_bytes().as_slice()).execute(&f.pool).await.unwrap();
        sqlx::query("UPDATE sessions SET revoked_at=1 WHERE id=?1")
            .bind(credential.as_bytes().as_slice())
            .execute(&f.pool)
            .await
            .unwrap();
        assert!(matches!(
            create_off_draft(&f.backend, RealtimeMode::Off, engine(), request.clone()).await,
            Err(OffDraftCreateError::Body(BodySaveError::Native(
                CollabDbError::Forbidden
            )))
        ));
        assert_eq!(draft_observable(&f).await, after);
        assert_eq!(original_history(&f).await, source);
        sqlx::query("UPDATE sessions SET revoked_at=NULL WHERE id=?1")
            .bind(credential.as_bytes().as_slice())
            .execute(&f.pool)
            .await
            .unwrap();
        let healthy = create_off_draft(&f.backend, RealtimeMode::Off, engine(), request)
            .await
            .unwrap();
        assert_eq!(healthy.document.id, created.document.id);
        let file: (Vec<u8>, Vec<u8>) =
            sqlx::query_as("SELECT document_id,uploader_id FROM attachments WHERE id=?1")
                .bind(attachment.as_bytes().as_slice())
                .fetch_one(&f.pool)
                .await
                .unwrap();
        assert_eq!(
            file,
            (f.document.as_bytes().to_vec(), f.user.as_bytes().to_vec())
        );
        f.close().await;
    }

    #[tokio::test]
    async fn off_draft_source_view_is_not_destination_create_or_foreign_reference_authority() {
        let f = Fixture::new().await;
        let credential = session(&f).await;
        let task = task_target(&f).await;
        let project: Vec<u8> = sqlx::query_scalar("SELECT project_id FROM tasks WHERE id=?1")
            .bind(task.as_bytes().as_slice())
            .fetch_one(&f.pool)
            .await
            .unwrap();
        let project = Uuid::from_slice(&project).unwrap();
        let attachment: Vec<u8> = sqlx::query_scalar("SELECT id FROM attachments WHERE task_id=?1")
            .bind(task.as_bytes().as_slice())
            .fetch_one(&f.pool)
            .await
            .unwrap();
        let attachment = Uuid::from_slice(&attachment).unwrap();
        sqlx::query("UPDATE projects SET status='archived' WHERE id=?1")
            .bind(project.as_bytes().as_slice())
            .execute(&f.pool)
            .await
            .unwrap();
        sqlx::query("UPDATE tasks SET archived_at=1 WHERE id=?1")
            .bind(task.as_bytes().as_slice())
            .execute(&f.pool)
            .await
            .unwrap();
        let request = OffDraftCreateRequest {
            workspace: f.workspace,
            source: RevisionTarget::Task(task).into(),
            destination_project: None,
            parent: None,
            actor: f.user,
            credential,
            command: Uuid::now_v7(),
            title: "preserved private draft".into(),
            icon: None,
            content_json: json!({"type":"doc","content":[{"type":"embed","attrs":{"entity":"document","ref":f.document,"id":"draft-block"}},{"type":"attachment","attrs":{"id":attachment,"name":"original file"}}]}),
            client_ip: None,
        };
        request.validate().unwrap();
        let before:(i64,i64,i64,i64) = sqlx::query_as("SELECT (SELECT count(*) FROM documents),(SELECT count(*) FROM wiki_create_commands),(SELECT count(*) FROM events),(SELECT count(*) FROM audit_log)").fetch_one(&f.pool).await.unwrap();
        let mut outer = f.backend.begin_off_body().await.unwrap();
        authorize_draft_source(&mut outer.operation(), &request)
            .await
            .unwrap();
        authorize_draft_destination(&mut outer.operation(), &request)
            .await
            .unwrap();
        authorize_draft_references(&mut outer.operation(), &request)
            .await
            .unwrap();
        outer.rollback().await.unwrap();
        let mut archived_destination = request.clone();
        archived_destination.destination_project = Some(project);
        archived_destination.parent = Some(f.document);
        let mut outer = f.backend.begin_off_body().await.unwrap();
        authorize_draft_source(&mut outer.operation(), &archived_destination)
            .await
            .unwrap();
        assert!(matches!(
            authorize_draft_destination(&mut outer.operation(), &archived_destination).await,
            Err(OffDraftCreateError::Document(
                super::super::documents::DocumentDbError::NotFound
            ))
        ));
        outer.rollback().await.unwrap();
        let foreign_workspace = Uuid::now_v7();
        let foreign_document = Uuid::now_v7();
        sqlx::query("INSERT INTO workspaces(id,slug,name) VALUES(?1,'draft-foreign','Foreign')")
            .bind(foreign_workspace.as_bytes().as_slice())
            .execute(&f.pool)
            .await
            .unwrap();
        sqlx::query("INSERT INTO documents(id,workspace_id,title,path,sort_key,number,status,schema_version,content_json,created_by) VALUES(?1,?2,'Foreign',?3,'V',1,'draft',2,?4,?5)")
            .bind(foreign_document.as_bytes().as_slice()).bind(foreign_workspace.as_bytes().as_slice()).bind(foreign_document.simple().to_string()).bind(super::super::documents::empty_document_json().to_string()).bind(f.user.as_bytes().as_slice()).execute(&f.pool).await.unwrap();
        let mut wrong_reference = request.clone();
        wrong_reference.content_json["content"][0]["attrs"]["ref"] = json!(foreign_document);
        let mut outer = f.backend.begin_off_body().await.unwrap();
        authorize_draft_source(&mut outer.operation(), &wrong_reference)
            .await
            .unwrap();
        assert!(matches!(
            authorize_draft_references(&mut outer.operation(), &wrong_reference).await,
            Err(OffDraftCreateError::Body(BodySaveError::Native(
                CollabDbError::NotFound
            )))
        ));
        outer.rollback().await.unwrap();
        sqlx::query("UPDATE attachments SET scan_status='infected' WHERE id=?1")
            .bind(attachment.as_bytes().as_slice())
            .execute(&f.pool)
            .await
            .unwrap();
        let mut outer = f.backend.begin_off_body().await.unwrap();
        authorize_draft_source(&mut outer.operation(), &request)
            .await
            .unwrap();
        assert!(matches!(
            authorize_draft_references(&mut outer.operation(), &request).await,
            Err(OffDraftCreateError::Attachment(
                super::super::attachments::AttachmentDbError::Infected
            ))
        ));
        outer.rollback().await.unwrap();
        sqlx::query("UPDATE attachments SET scan_status='clean' WHERE id=?1")
            .bind(attachment.as_bytes().as_slice())
            .execute(&f.pool)
            .await
            .unwrap();
        sqlx::query("UPDATE sessions SET revoked_at=1 WHERE id=?1")
            .bind(credential.as_bytes().as_slice())
            .execute(&f.pool)
            .await
            .unwrap();
        let mut outer = f.backend.begin_off_body().await.unwrap();
        assert!(matches!(
            authorize_draft_source(&mut outer.operation(), &request).await,
            Err(OffDraftCreateError::Body(BodySaveError::Native(
                CollabDbError::Forbidden
            )))
        ));
        outer.rollback().await.unwrap();
        let after:(i64,i64,i64,i64) = sqlx::query_as("SELECT (SELECT count(*) FROM documents WHERE workspace_id=?1),(SELECT count(*) FROM wiki_create_commands),(SELECT count(*) FROM events),(SELECT count(*) FROM audit_log)")
            .bind(f.workspace.as_bytes().as_slice()).fetch_one(&f.pool).await.unwrap();
        assert_eq!(after,before,"current source/destination/reference refusals have no command/document/outbox/audit effects");
        sqlx::query("UPDATE sessions SET revoked_at=NULL WHERE id=?1")
            .bind(credential.as_bytes().as_slice())
            .execute(&f.pool)
            .await
            .unwrap();
        let mut healthy = f.backend.begin_off_body().await.unwrap();
        authorize_draft_source(&mut healthy.operation(), &request)
            .await
            .unwrap();
        authorize_draft_destination(&mut healthy.operation(), &request)
            .await
            .unwrap();
        authorize_draft_references(&mut healthy.operation(), &request)
            .await
            .unwrap();
        healthy.rollback().await.unwrap();
        let original_file: (Option<Vec<u8>>, Option<Vec<u8>>, Vec<u8>, String) = sqlx::query_as(
            "SELECT document_id,task_id,uploader_id,status FROM attachments WHERE id=?1",
        )
        .bind(attachment.as_bytes().as_slice())
        .fetch_one(&f.pool)
        .await
        .unwrap();
        assert_eq!(original_file,(None,Some(task.as_bytes().to_vec()),f.user.as_bytes().to_vec(),"stored".into()), "source reference admission never rebinds ownership/parent or grants destination file access");
        f.close().await;
    }

    async fn session(f: &Fixture) -> Uuid {
        // This fixture was built for attachment previews. A normal newly
        // created wiki has the existing canonical empty body before lazy native
        // initialization; do not change the shared fixture or seed live history.
        sqlx::query("UPDATE documents SET content_json=?1 WHERE id=?2")
            .bind(crate::db::documents::empty_document_json().to_string())
            .bind(f.document.as_bytes().as_slice())
            .execute(&f.pool)
            .await
            .unwrap();
        let credential = Uuid::now_v7();
        sqlx::query("INSERT INTO sessions(id,user_id,token_hash,expires_at) VALUES(?1,?2,?3,9223372036854775807)")
            .bind(credential.as_bytes().as_slice()).bind(f.user.as_bytes().as_slice()).bind(credential.to_string())
            .execute(&f.pool).await.unwrap();
        credential
    }
    fn engine() -> CollabConfig {
        // Missing allocation is a failure, never a skip or another worker's binary.
        CollabConfig::from_env()
            .expect("root allocation must provide the current FVOCI_COLLAB_ENGINE")
    }
    async fn request(f: &Fixture, credential: Uuid, text: &str) -> OffBodyRequest {
        let config = engine();
        let update = SeedEngine::new(config.engine_bin, config.limits)
            .tiptap_to_yjs_update(&json!({"type":"doc","content":[{"type":"paragraph","attrs":{"id":"off-stable-block"},
                "content":[{"type":"text","text":text,"marks":[{"type":"bold"}]}]}]})).await.unwrap();
        OffBodyRequest {
            workspace: f.workspace,
            target: RevisionTarget::Document(f.document),
            project: None,
            actor: f.user,
            credential,
            command: Uuid::now_v7(),
            expected_tail: 0,
            update,
            client_ip: Some("127.0.0.1".into()),
        }
    }
    async fn task_target(f: &Fixture) -> Uuid {
        let (_, task) = f.task_attachment().await;
        // This is a newly created fixture target, not an existing history reseed.
        sqlx::query("UPDATE tasks SET content_json=?1 WHERE workspace_id=?2 AND id=?3")
            .bind(crate::db::documents::empty_document_json().to_string())
            .bind(f.workspace.as_bytes().as_slice())
            .bind(task.as_bytes().as_slice())
            .execute(&f.pool)
            .await
            .unwrap();
        task
    }
    async fn task_counts(f: &Fixture, task: Uuid) -> (i64, i64, i64, i64) {
        sqlx::query_as("SELECT (SELECT tail_seq FROM task_states WHERE workspace_id=?1 AND task_id=?2),
            (SELECT count(*) FROM body_save_commands WHERE workspace_id=?1 AND target_kind='task' AND target_id=?2),
            (SELECT count(*) FROM revisions WHERE workspace_id=?1 AND target_kind='task' AND target_id=?2),
            (SELECT count(*) FROM task_collab_op_receipts WHERE workspace_id=?1 AND task_id=?2)")
            .bind(f.workspace.as_bytes().as_slice()).bind(task.as_bytes().as_slice()).fetch_one(&f.pool).await.unwrap()
    }

    async fn counts(f: &Fixture) -> (i64, i64, i64, i64) {
        sqlx::query_as("SELECT (SELECT tail_seq FROM document_states WHERE workspace_id=?1 AND document_id=?2),
            (SELECT count(*) FROM body_save_commands WHERE workspace_id=?1),
            (SELECT count(*) FROM revisions WHERE workspace_id=?1 AND target_kind='document' AND target_id=?2),
            (SELECT count(*) FROM document_collab_op_receipts WHERE workspace_id=?1 AND document_id=?2)")
            .bind(f.workspace.as_bytes().as_slice()).bind(f.document.as_bytes().as_slice()).fetch_one(&f.pool).await.unwrap()
    }
    #[tokio::test]
    async fn off_restore_is_one_forward_cas_with_exact_provenance_replay_and_fk_rollback() {
        let f = Fixture::new().await;
        let credential = session(&f).await;
        let original = request(&f, credential, "original text 😀").await;
        let first = save_off_body(&f.backend, RealtimeMode::Off, engine(), original.clone())
            .await
            .unwrap();
        assert_eq!(
            create_off_revision(
                &f.backend,
                RealtimeMode::Off,
                engine(),
                f.workspace,
                original.scope(),
                f.user,
                credential
            )
            .await
            .unwrap(),
            first.revision_id,
            "explicit current manual history preserves snapshot deduplication"
        );
        let mut second = original.clone();
        second.command = Uuid::now_v7();
        second.expected_tail = 1;
        let config = engine();
        second.update = SeedEngine::new(config.engine_bin, config.limits).tiptap_to_yjs_update(
            &json!({"type":"doc","content":[{"type":"paragraph","attrs":{"id":"added-block"},"content":[{"type":"text","text":"later text"}]}]}),
        ).await.unwrap();
        let second_saved = save_off_body(&f.backend, RealtimeMode::Off, engine(), second.clone())
            .await
            .unwrap();
        let preview = preview_off_restore(
            &f.backend,
            RealtimeMode::Off,
            engine(),
            f.workspace,
            original.scope(),
            f.user,
            credential,
            first.revision_id,
        )
        .await
        .unwrap();
        assert_eq!(preview.current_tail, 2);
        assert!(preview
            .current_content_json
            .to_string()
            .contains("later text"));
        assert!(!preview
            .source
            .content_json
            .to_string()
            .contains("later text"));
        let mut restore = second.clone();
        restore.command = Uuid::now_v7();
        restore.expected_tail = 2;
        restore.update.clear();
        let before = counts(&f).await;
        assert_eq!(before, (2, 2, 2, 2));
        // Every restore effect is prepared on this actual reserved writer.
        let mut tx = f.backend.begin_off_body().await.unwrap();
        let staged = save_in_writer(
            &mut tx.operation(),
            RealtimeMode::Off,
            engine(),
            &restore,
            Arc::new(AtomicBool::new(false)),
            Some(first.revision_id),
        )
        .await
        .unwrap();
        let OperationTx::SqliteFamily(writer) = tx.operation() else {
            panic!("actual SQLite writer")
        };
        let error = writer.execute("UPDATE body_save_commands SET document_id=?1,target_id=?1 WHERE workspace_id=?2 AND command_id=?3",
            &[Cell::uuid(Uuid::now_v7()),Cell::uuid(f.workspace),Cell::uuid(restore.command)]).await.unwrap_err();
        assert!(
            error.to_string().contains("FOREIGN KEY"),
            "same real FK failure: {error}"
        );
        tx.rollback().await.unwrap();
        assert_eq!(counts(&f).await, before);
        let absent: i64 = sqlx::query_scalar("SELECT count(*) FROM revisions WHERE id=?1")
            .bind(staged.revision_id.as_bytes().as_slice())
            .fetch_one(&f.pool)
            .await
            .unwrap();
        assert_eq!(absent, 0);
        let saved = restore_off_body(
            &f.backend,
            RealtimeMode::Off,
            engine(),
            restore.clone(),
            first.revision_id,
        )
        .await
        .unwrap();
        assert_eq!(saved.tail_seq, "3");
        assert_ne!(saved.revision_id, first.revision_id);
        let provenance:(String,Vec<u8>,Vec<u8>,i64,i64) = sqlx::query_as("SELECT reason,restored_from_id,restore_correlation_id,restore_base_tail_seq,restore_committed_tail_seq FROM revisions WHERE id=?1")
            .bind(saved.revision_id.as_bytes().as_slice()).fetch_one(&f.pool).await.unwrap();
        assert_eq!(
            provenance,
            (
                "restore".into(),
                first.revision_id.as_bytes().to_vec(),
                restore.command.as_bytes().to_vec(),
                2,
                3
            )
        );
        let after = counts(&f).await;
        assert_eq!(after, (3, 3, 3, 3));
        let replay = restore_off_body(
            &f.backend,
            RealtimeMode::Off,
            engine(),
            restore.clone(),
            first.revision_id,
        )
        .await
        .unwrap();
        assert_eq!(replay.revision_id, saved.revision_id);
        assert_eq!(counts(&f).await, after);
        assert!(matches!(
            restore_off_body(
                &f.backend,
                RealtimeMode::Off,
                engine(),
                restore.clone(),
                second_saved.revision_id
            )
            .await,
            Err(BodySaveError::RequestMismatch)
        ));
        let mut stale = restore.clone();
        stale.command = Uuid::now_v7();
        assert!(matches!(
            restore_off_body(
                &f.backend,
                RealtimeMode::Off,
                engine(),
                stale,
                first.revision_id
            )
            .await,
            Err(BodySaveError::Conflict)
        ));
        assert_eq!(counts(&f).await, after);
        let fresh = read_off_body(
            &f.backend,
            RealtimeMode::Off,
            engine(),
            f.workspace,
            original.scope(),
            f.user,
            credential,
        )
        .await
        .unwrap();
        let text = fresh.content_json.to_string();
        assert!(
            text.contains("original text 😀")
                && text.contains("off-stable-block")
                && text.contains("bold")
        );
        assert!(!text.contains("later text") && !text.contains("added-block"));
        sqlx::query("UPDATE sessions SET revoked_at=1 WHERE id=?1")
            .bind(credential.as_bytes().as_slice())
            .execute(&f.pool)
            .await
            .unwrap();
        assert!(matches!(
            restore_off_body(
                &f.backend,
                RealtimeMode::Off,
                engine(),
                restore,
                first.revision_id
            )
            .await,
            Err(BodySaveError::Native(CollabDbError::Forbidden))
        ));
        assert_eq!(counts(&f).await, after);
        f.close().await;
    }
    #[tokio::test]
    async fn off_wiki_native_cas_receipt_replay_conflict_and_fresh_readback() {
        let f = Fixture::new().await;
        let credential = session(&f).await;
        let one = request(&f, credential, "one actual writer 😀").await;
        let two = request(&f, credential, "second conflicting writer").await;
        let saved = save_off_body(&f.backend, RealtimeMode::Off, engine(), one.clone())
            .await
            .unwrap();
        assert_eq!(saved.command_id, one.command);
        assert_eq!(saved.tail_seq, "1");
        assert!(!saved.revision_id.is_nil());
        let before = counts(&f).await;
        assert_eq!(before, (1, 1, 1, 1));
        assert!(matches!(
            save_off_body(&f.backend, RealtimeMode::Off, engine(), two).await,
            Err(BodySaveError::Conflict)
        ));
        assert_eq!(counts(&f).await, before);
        let replay = save_off_body(&f.backend, RealtimeMode::Off, engine(), one.clone())
            .await
            .unwrap();
        assert_eq!(replay.revision_id, saved.revision_id);
        assert_eq!(counts(&f).await, before);
        let mut changed = one.clone();
        changed.update.push(0);
        assert!(matches!(
            save_off_body(&f.backend, RealtimeMode::Off, engine(), changed).await,
            Err(BodySaveError::RequestMismatch)
        ));
        assert_eq!(counts(&f).await, before);
        let reader = read_off_body(
            &f.backend,
            RealtimeMode::Off,
            engine(),
            f.workspace,
            RevisionTarget::Document(f.document).into(),
            f.user,
            credential,
        )
        .await
        .unwrap();
        assert_eq!(reader.native.tail_seq, 1);
        assert_eq!(reader.native.snapshot_cutoff_seq, 1);
        assert!(reader.native.tail.is_empty());
        let encoded = serde_json::to_string(&reader.content_json).unwrap();
        assert!(encoded.contains("one actual writer 😀"));
        assert!(encoded.contains("off-stable-block"));
        assert!(encoded.contains("bold"));
        sqlx::query("DELETE FROM sessions WHERE id=?1")
            .bind(credential.as_bytes().as_slice())
            .execute(&f.pool)
            .await
            .unwrap();
        assert!(matches!(
            save_off_body(&f.backend, RealtimeMode::Off, engine(), one).await,
            Err(BodySaveError::Native(_))
        ));
        assert_eq!(counts(&f).await, before);
        let fk: i64 = sqlx::query_scalar("PRAGMA foreign_keys")
            .fetch_one(&f.pool)
            .await
            .unwrap();
        assert_eq!(fk, 1);
        f.close().await;
    }
    #[tokio::test]
    async fn off_task_current_native_cas_receipt_history_and_room_expiry() {
        let f = Fixture::new().await;
        let credential = session(&f).await;
        let task = task_target(&f).await;
        let owner = Uuid::now_v7();
        let lease = std::time::Duration::from_secs(60);
        let mut tx = f.backend.begin_off_body().await.unwrap();
        let claim = tx
            .operation()
            .prepare_family_task_room_writer(
                (f.workspace, task),
                (f.user, credential),
                owner,
                lease,
            )
            .await
            .unwrap()
            .unwrap();
        assert!(claim.native.writer_generation > 0);
        tx.commit_with_cleanup().await.unwrap();
        let before = task_counts(&f, task).await;
        assert_eq!(before, (0, 0, 0, 0));
        let mut command = request(&f, credential, "actual OFF task native 😀").await;
        command.target = RevisionTarget::Task(task);
        assert!(matches!(
            save_off_body(&f.backend, RealtimeMode::Off, engine(), command.clone()).await,
            Err(BodySaveError::Native(CollabDbError::StaleWriter))
        ));
        assert_eq!(task_counts(&f, task).await, before);
        let mut tx = f.backend.begin_off_body().await.unwrap();
        let retry = tx
            .operation()
            .prepare_family_task_room_writer(
                (f.workspace, task),
                (f.user, credential),
                owner,
                lease,
            )
            .await
            .unwrap()
            .unwrap();
        assert_eq!(retry.fence, claim.fence);
        assert_eq!(
            retry.native.writer_generation,
            claim.native.writer_generation
        );
        tx.commit_with_cleanup().await.unwrap();
        // Expiry is DB metadata controlled only in this fixture, no sleeps or
        // fabricated successful remote acknowledgements.
        sqlx::query(
            "UPDATE task_collab_room_fences SET expires_at=0 WHERE workspace_id=?1 AND task_id=?2",
        )
        .bind(f.workspace.as_bytes().as_slice())
        .bind(task.as_bytes().as_slice())
        .execute(&f.pool)
        .await
        .unwrap();
        let mut tx = f.backend.begin_off_body().await.unwrap();
        assert!(!tx
            .operation()
            .renew_family_task_room_fence(claim.fence, lease)
            .await
            .unwrap());
        assert!(!tx
            .operation()
            .verify_family_task_room_fence(claim.fence)
            .await
            .unwrap());
        assert!(matches!(
            tx.operation()
                .prepare_family_task_room_writer(
                    (f.workspace, task),
                    (f.user, credential),
                    owner,
                    lease
                )
                .await
                .unwrap(),
            Err(CollabDbError::StaleWriter)
        ));
        tx.rollback().await.unwrap();
        let saved = save_off_body(&f.backend, RealtimeMode::Off, engine(), command.clone())
            .await
            .unwrap();
        assert_eq!(saved.command_id, command.command);
        assert_eq!(saved.tail_seq, "1");
        assert_eq!(task_counts(&f, task).await, (1, 1, 1, 1));
        let fresh = read_off_body(
            &f.backend,
            RealtimeMode::Off,
            engine(),
            f.workspace,
            RevisionTarget::Task(task).into(),
            f.user,
            credential,
        )
        .await
        .unwrap();
        assert!(fresh.writable);
        assert_eq!(
            fresh.native.writer_generation,
            claim.native.writer_generation
        );
        assert!(fresh
            .content_json
            .to_string()
            .contains("actual OFF task native 😀"));
        assert!(fresh.content_json.to_string().contains("off-stable-block"));
        let replay = save_off_body(&f.backend, RealtimeMode::Off, engine(), command.clone())
            .await
            .unwrap();
        assert_eq!(replay.revision_id, saved.revision_id);
        assert_eq!(task_counts(&f, task).await, (1, 1, 1, 1));
        let mut conflicting = command.clone();
        conflicting.command = Uuid::now_v7();
        assert!(matches!(
            save_off_body(&f.backend, RealtimeMode::Off, engine(), conflicting).await,
            Err(BodySaveError::Conflict)
        ));
        assert_eq!(task_counts(&f, task).await, (1, 1, 1, 1));
        let mut tx = f.backend.begin_off_body().await.unwrap();
        let successor = tx
            .operation()
            .prepare_family_task_room_writer(
                (f.workspace, task),
                (f.user, credential),
                Uuid::now_v7(),
                lease,
            )
            .await
            .unwrap()
            .unwrap();
        assert_ne!(successor.fence, claim.fence);
        assert!(successor.native.writer_generation > claim.native.writer_generation);
        assert!(!tx
            .operation()
            .release_family_task_room_fence(claim.fence)
            .await
            .unwrap());
        assert!(tx
            .operation()
            .verify_family_task_room_fence(successor.fence)
            .await
            .unwrap());
        tx.commit_with_cleanup().await.unwrap();
        assert!(matches!(
            save_off_body(&f.backend, RealtimeMode::Off, engine(), command).await,
            Err(BodySaveError::Native(CollabDbError::StaleWriter))
        ));
        assert_eq!(task_counts(&f, task).await, (1, 1, 1, 1));
        f.close().await;
    }

    #[tokio::test]
    async fn older_native_operation_receipt_cannot_ack_a_new_body_command_version() {
        let f = Fixture::new().await;
        let credential = session(&f).await;
        let mut command = request(&f, credential, "actual older native operation").await;
        let mut tx = f.backend.begin_off_body().await.unwrap();
        let mut op = tx.operation();
        let (proof, load) = op
            .load_off_body_writer(
                RealtimeMode::Off,
                CollabKind::Document,
                f.workspace,
                f.user,
                credential,
                f.document,
            )
            .await
            .unwrap()
            .unwrap();
        let seq = op
            .append_off_body(
                proof,
                AppendCollabInput {
                    workspace_id: f.workspace,
                    actor_user_id: f.user,
                    session_id: credential,
                    document_id: f.document,
                    writer_generation: load.writer_generation,
                    expected_tail_seq: 0,
                    op_id: command.command,
                    payload: &command.update,
                    client_ip: None,
                },
            )
            .await
            .unwrap()
            .unwrap()
            .seq();
        assert_eq!(seq, 1);
        tx.commit_with_cleanup().await.unwrap();
        command.expected_tail = 1;
        let before = counts(&f).await;
        assert_eq!(before, (1, 0, 0, 1));
        assert!(matches!(
            save_off_body(&f.backend, RealtimeMode::Off, engine(), command.clone()).await,
            Err(BodySaveError::RequestMismatch)
        ));
        assert_eq!(counts(&f).await, before);
        command.command = Uuid::now_v7();
        let saved = save_off_body(&f.backend, RealtimeMode::Off, engine(), command)
            .await
            .unwrap();
        assert_eq!(saved.tail_seq, "2");
        assert_eq!(counts(&f).await, (2, 1, 1, 2));
        f.close().await;
    }

    #[tokio::test]
    async fn two_actual_sqlite_writers_at_one_version_commit_exactly_one_result() {
        let f = Fixture::new().await;
        let credential = session(&f).await;
        let one = request(&f, credential, "first competing native client").await;
        let two = request(&f, credential, "second competing native client").await;
        let pool = crate::db::pool::connect_sqlite_app(&f.path, 2)
            .await
            .unwrap();
        let backend = Backend::Sqlite(pool.clone());
        let (first, second) = tokio::join!(
            save_off_body(&backend, RealtimeMode::Off, engine(), one.clone()),
            save_off_body(&backend, RealtimeMode::Off, engine(), two.clone()),
        );
        let (saved, winner, loser, expected_text, excluded_text) = match (first, second) {
            (Ok(saved), Err(BodySaveError::Conflict)) => (
                saved,
                one,
                two,
                "first competing native client",
                "second competing native client",
            ),
            (Err(BodySaveError::Conflict), Ok(saved)) => (
                saved,
                two,
                one,
                "second competing native client",
                "first competing native client",
            ),
            results => panic!("one CAS commit and one conflict required, got {results:?}"),
        };
        assert_eq!(saved.command_id, winner.command);
        assert_eq!(saved.tail_seq, "1");
        let before = counts(&f).await;
        assert_eq!(before, (1, 1, 1, 1));
        let reader = read_off_body(
            &backend,
            RealtimeMode::Off,
            engine(),
            f.workspace,
            RevisionTarget::Document(f.document).into(),
            f.user,
            credential,
        )
        .await
        .unwrap();
        assert_eq!(reader.native.tail_seq, 1);
        assert_eq!(reader.native.snapshot_cutoff_seq, 1);
        let projection = reader.content_json.to_string();
        assert!(projection.contains(expected_text));
        assert!(!projection.contains(excluded_text));
        let committed: Vec<u8> = sqlx::query_scalar("SELECT payload_sha256 FROM document_collab_op_receipts WHERE workspace_id=?1 AND document_id=?2 AND op_id=?3")
            .bind(f.workspace.as_bytes().as_slice()).bind(f.document.as_bytes().as_slice()).bind(winner.command.as_bytes().as_slice())
            .fetch_one(&f.pool).await.unwrap();
        assert_eq!(committed, Sha256::digest(&winner.update).to_vec());
        let excluded: i64 = sqlx::query_scalar(
            "SELECT count(*) FROM body_save_commands WHERE workspace_id=?1 AND command_id=?2",
        )
        .bind(f.workspace.as_bytes().as_slice())
        .bind(loser.command.as_bytes().as_slice())
        .fetch_one(&f.pool)
        .await
        .unwrap();
        assert_eq!(excluded, 0);
        let replay = save_off_body(&backend, RealtimeMode::Off, engine(), winner)
            .await
            .unwrap();
        assert_eq!(replay.revision_id, saved.revision_id);
        assert_eq!(counts(&f).await, before);
        pool.close().await;
        f.close().await;
    }

    #[tokio::test]
    async fn archived_wiki_history_is_readable_but_not_a_write_grant_or_receipt_oracle() {
        let f = Fixture::new().await;
        let credential = session(&f).await;
        let command = request(&f, credential, "retained archived wiki history").await;
        save_off_body(&f.backend, RealtimeMode::Off, engine(), command.clone())
            .await
            .unwrap();
        let before = counts(&f).await;
        sqlx::query("UPDATE documents SET status='archived' WHERE workspace_id=?1 AND id=?2")
            .bind(f.workspace.as_bytes().as_slice())
            .bind(f.document.as_bytes().as_slice())
            .execute(&f.pool)
            .await
            .unwrap();
        let source = read_off_body(
            &f.backend,
            RealtimeMode::Off,
            engine(),
            f.workspace,
            RevisionTarget::Document(f.document).into(),
            f.user,
            credential,
        )
        .await
        .unwrap();
        assert!(!source.writable);
        assert!(source
            .content_json
            .to_string()
            .contains("retained archived wiki history"));
        assert!(matches!(
            save_off_body(&f.backend, RealtimeMode::Off, engine(), command).await,
            Err(BodySaveError::Native(CollabDbError::Forbidden))
        ));
        assert_eq!(counts(&f).await, before);
        f.close().await;
    }

    #[tokio::test]
    async fn off_save_at_existing_native_tail_limit_compacts_only_validated_history() {
        let f = Fixture::new().await;
        let credential = session(&f).await;
        let initial = request(&f, credential, "retained native history at full tail").await;
        let mut tx = f.backend.begin_off_body().await.unwrap();
        let mut op = tx.operation();
        let (proof, load) = op
            .load_off_body_writer(
                RealtimeMode::Off,
                CollabKind::Document,
                f.workspace,
                f.user,
                credential,
                f.document,
            )
            .await
            .unwrap()
            .unwrap();
        assert_eq!(load.tail_seq, 0);
        for seq in 0..super::super::collab::MAX_COLLAB_TAIL_UPDATES {
            let appended = op
                .append_off_body(
                    proof,
                    AppendCollabInput {
                        workspace_id: f.workspace,
                        actor_user_id: f.user,
                        session_id: credential,
                        document_id: f.document,
                        writer_generation: load.writer_generation,
                        expected_tail_seq: seq,
                        op_id: Uuid::now_v7(),
                        payload: &initial.update,
                        client_ip: None,
                    },
                )
                .await
                .unwrap()
                .unwrap();
            assert_eq!(appended.seq(), seq + 1);
        }
        tx.commit_with_cleanup().await.unwrap();
        assert_eq!(counts(&f).await, (64, 0, 0, 64));
        let config = engine();
        let incoming = SeedEngine::new(config.engine_bin, config.limits)
            .tiptap_to_yjs_update(&json!({"type":"doc","content":[{"type":"paragraph","attrs":{"id":"new-forward-block"},
                "content":[{"type":"text","text":"new forward edit at full tail"}]}]})).await.unwrap();
        let mut command = initial;
        command.command = Uuid::now_v7();
        command.expected_tail = 64;
        command.update = incoming;
        let saved = save_off_body(&f.backend, RealtimeMode::Off, engine(), command.clone())
            .await
            .unwrap();
        assert_eq!(saved.tail_seq, "65");
        assert_eq!(counts(&f).await, (65, 1, 1, 65));
        let fresh = read_off_body(
            &f.backend,
            RealtimeMode::Off,
            engine(),
            f.workspace,
            RevisionTarget::Document(f.document).into(),
            f.user,
            credential,
        )
        .await
        .unwrap();
        assert_eq!(fresh.native.snapshot_cutoff_seq, 65);
        assert!(fresh.native.tail.is_empty());
        let projected = fresh.content_json.to_string();
        for kept in [
            "retained native history at full tail",
            "off-stable-block",
            "bold",
            "new forward edit at full tail",
            "new-forward-block",
        ] {
            assert!(projected.contains(kept), "missing {kept}");
        }
        let before = counts(&f).await;
        let replay = save_off_body(&f.backend, RealtimeMode::Off, engine(), command)
            .await
            .unwrap();
        assert_eq!(replay.revision_id, saved.revision_id);
        assert_eq!(counts(&f).await, before);
        f.close().await;
    }

    #[tokio::test]
    async fn off_project_native_save_binds_affiliation_and_rechecks_archived_replay() {
        let f = Fixture::new().await;
        let credential = session(&f).await;
        let project = Uuid::now_v7();
        sqlx::query("INSERT INTO projects(id,workspace_id,key,name,visibility,created_by) VALUES(?1,?2,'OFF','OFF','workspace',?3)")
            .bind(project.as_bytes().as_slice()).bind(f.workspace.as_bytes().as_slice()).bind(f.user.as_bytes().as_slice())
            .execute(&f.pool).await.unwrap();
        sqlx::query("UPDATE documents SET project_id=?1 WHERE workspace_id=?2 AND id=?3")
            .bind(project.as_bytes().as_slice())
            .bind(f.workspace.as_bytes().as_slice())
            .bind(f.document.as_bytes().as_slice())
            .execute(&f.pool)
            .await
            .unwrap();
        let mut command = request(&f, credential, "project native history").await;
        command.project = Some(project);
        let mut wrong = command.clone();
        wrong.project = Some(Uuid::now_v7());
        assert!(matches!(
            save_off_body(&f.backend, RealtimeMode::Off, engine(), wrong).await,
            Err(BodySaveError::Revision(RevisionDbError::NotFound))
        ));
        assert_eq!(counts(&f).await, (0, 0, 0, 0));
        let saved = save_off_body(&f.backend, RealtimeMode::Off, engine(), command.clone())
            .await
            .unwrap();
        assert_eq!(saved.tail_seq, "1");
        let before = counts(&f).await;
        let scope = RevisionScope::project_document(project, f.document);
        let fresh = read_off_body(
            &f.backend,
            RealtimeMode::Off,
            engine(),
            f.workspace,
            scope,
            f.user,
            credential,
        )
        .await
        .unwrap();
        assert!(fresh.writable);
        assert_eq!(fresh.native.tail_seq, 1);
        assert!(fresh
            .content_json
            .to_string()
            .contains("project native history"));
        let replay = save_off_body(&f.backend, RealtimeMode::Off, engine(), command.clone())
            .await
            .unwrap();
        assert_eq!(replay.revision_id, saved.revision_id);
        assert_eq!(counts(&f).await, before);
        sqlx::query("UPDATE projects SET archived_at=1 WHERE workspace_id=?1 AND id=?2")
            .bind(f.workspace.as_bytes().as_slice())
            .bind(project.as_bytes().as_slice())
            .execute(&f.pool)
            .await
            .unwrap();
        assert!(matches!(
            save_off_body(&f.backend, RealtimeMode::Off, engine(), command).await,
            Err(BodySaveError::Native(CollabDbError::Forbidden))
        ));
        assert_eq!(counts(&f).await, before);
        let fresh = read_off_body(
            &f.backend,
            RealtimeMode::Off,
            engine(),
            f.workspace,
            scope,
            f.user,
            credential,
        )
        .await
        .unwrap();
        assert!(!fresh.writable);
        assert_eq!(fresh.native.tail_seq, 1);
        f.close().await;
    }

    #[tokio::test]
    async fn off_wiki_real_foreign_key_failure_rolls_back_native_revision_and_command() {
        let f = Fixture::new().await;
        let credential = session(&f).await;
        let input = request(&f, credential, "must be rolled back").await;
        // A real invalid FK inside the same reserved writer after every prepared
        // product effect proves caller rollback, not a fabricated commit error.
        let mut tx = f.backend.begin_off_body().await.unwrap();
        let saved = save_in_writer(
            &mut tx.operation(),
            RealtimeMode::Off,
            engine(),
            &input,
            Arc::new(AtomicBool::new(false)),
            None,
        )
        .await
        .unwrap();
        assert!(!saved.revision_id.is_nil());
        let error = match tx.operation() {
            OperationTx::SqliteFamily(writer) => writer.execute(
                "UPDATE body_save_commands SET document_id=?1,target_id=?1 WHERE workspace_id=?2 AND command_id=?3",
                &[Cell::uuid(Uuid::now_v7()), Cell::uuid(f.workspace), Cell::uuid(input.command)],
            ).await.unwrap_err(),
            OperationTx::Postgres(_) => panic!("fixture must retain actual SQLite writer"),
        };
        assert!(error.to_string().contains("FOREIGN KEY"));
        tx.rollback().await.unwrap();
        let count: i64 = sqlx::query_scalar("SELECT count(*) FROM body_save_commands")
            .fetch_one(&f.pool)
            .await
            .unwrap();
        assert_eq!(count, 0);
        let revisions: i64 = sqlx::query_scalar("SELECT count(*) FROM revisions")
            .fetch_one(&f.pool)
            .await
            .unwrap();
        assert_eq!(revisions, 0);
        let native: i64 = sqlx::query_scalar("SELECT count(*) FROM document_states")
            .fetch_one(&f.pool)
            .await
            .unwrap();
        assert_eq!(native, 0);
        let events: i64 = sqlx::query_scalar(
            "SELECT count(*) FROM events WHERE workspace_id=?1 AND target_id=?2",
        )
        .bind(f.workspace.as_bytes().as_slice())
        .bind(f.document.as_bytes().as_slice())
        .fetch_one(&f.pool)
        .await
        .unwrap();
        assert_eq!(events, 0);
        let audits: i64 = sqlx::query_scalar(
            "SELECT count(*) FROM audit_log WHERE workspace_id=?1 AND target_id=?2",
        )
        .bind(f.workspace.as_bytes().as_slice())
        .bind(f.document.as_bytes().as_slice())
        .fetch_one(&f.pool)
        .await
        .unwrap();
        assert_eq!(audits, 0);
        // The same bytes must make normal progress after the rejected writer.
        assert_eq!(
            save_off_body(&f.backend, RealtimeMode::Off, engine(), input)
                .await
                .unwrap()
                .tail_seq,
            "1"
        );
        f.close().await;
    }
}

#[cfg(all(test, feature = "db-tests"))]
mod pg_draft_native_tests {
    use super::*;
    use serde_json::json;
    use sqlx::postgres::PgPoolOptions;

    async fn observable(pool: &sqlx::PgPool, workspace: Uuid) -> Vec<i64> {
        let row: (i64, i64, i64, i64, i64, i64, i64, i64) = sqlx::query_as("SELECT (SELECT count(*) FROM fvoci.documents WHERE workspace_id=$1),(SELECT count(*) FROM fvoci.wiki_create_commands WHERE workspace_id=$1),(SELECT count(*) FROM fvoci.body_save_commands WHERE workspace_id=$1),(SELECT count(*) FROM fvoci.revisions WHERE workspace_id=$1),(SELECT count(*) FROM fvoci.events WHERE workspace_id=$1),(SELECT count(*) FROM fvoci.audit_log WHERE workspace_id=$1),(SELECT count(*) FROM fvoci.document_collab_op_receipts WHERE workspace_id=$1),(SELECT next_document_number::bigint FROM fvoci.workspaces WHERE id=$1)")
            .bind(workspace).fetch_one(pool).await.unwrap();
        vec![row.0, row.1, row.2, row.3, row.4, row.5, row.6, row.7]
    }

    #[tokio::test]
    async fn off_draft_pg_restricted_writer_same_fk_rollback_replay_and_current_auth() {
        // Provisioning follows the maintained attachment/reference PG fixture:
        // current migrations/grants, isolated LOGIN role, no missing-URL skip.
        let provisioner = std::env::var("TEST_DATABASE_URL")
            .or_else(|_| std::env::var("FVOCI_TEST_DATABASE_URL"))
            .expect("root allocation must provide isolated PG provisioner URL");
        let engine = CollabConfig::from_env()
            .expect("root allocation must provide current FVOCI_COLLAB_ENGINE");
        let mut server = url::Url::parse(&provisioner).unwrap();
        server.set_path("/postgres");
        let admin_server = PgPoolOptions::new()
            .max_connections(1)
            .connect(server.as_str())
            .await
            .unwrap();
        let database = format!("fvoci_off_copy_{}", Uuid::now_v7().simple());
        let role = format!("fvoci_off_app_{}", Uuid::now_v7().simple());
        let password = crate::auth::token::new_token().hash;
        sqlx::query(&format!("CREATE DATABASE \"{database}\""))
            .execute(&admin_server)
            .await
            .unwrap();
        let mut database_url = server.clone();
        database_url.set_path(&format!("/{database}"));
        crate::db::migrate::run_migrations(database_url.as_str())
            .await
            .unwrap();
        let admin = PgPoolOptions::new()
            .max_connections(1)
            .connect(database_url.as_str())
            .await
            .unwrap();
        sqlx::query(&format!(
            "CREATE ROLE \"{role}\" LOGIN PASSWORD '{password}' NOSUPERUSER NOBYPASSRLS"
        ))
        .execute(&admin)
        .await
        .unwrap();
        crate::db::migrate::apply_app_role_grants(&admin, &role)
            .await
            .unwrap();
        let mut app_url = database_url;
        app_url.set_username(&role).unwrap();
        app_url.set_password(Some(&password)).unwrap();
        let app = crate::db::pool::connect_app_with_max(app_url.as_str(), 2)
            .await
            .unwrap();
        let flags: (bool, bool, bool) = sqlx::query_as(
            "SELECT rolcanlogin,rolsuper,rolbypassrls FROM pg_roles WHERE rolname=current_user",
        )
        .fetch_one(&app)
        .await
        .unwrap();
        assert_eq!(flags, (true, false, false));
        for table in [
            "documents",
            "document_states",
            "wiki_create_commands",
            "body_save_commands",
            "revisions",
        ] {
            let secured: bool =
                sqlx::query_scalar("SELECT relrowsecurity FROM pg_class WHERE oid=to_regclass($1)")
                    .bind(format!("fvoci.{table}"))
                    .fetch_one(&app)
                    .await
                    .unwrap();
            assert!(secured, "actual app operation must use RLS on {table}");
        }
        let workspace = Uuid::now_v7();
        let actor = Uuid::now_v7();
        let source = Uuid::now_v7();
        let credential = Uuid::now_v7();
        let session = crate::auth::token::new_token();
        sqlx::query(
            "INSERT INTO fvoci.users(id,email,given_name) VALUES($1,'off-copy@example.test','OFF')",
        )
        .bind(actor)
        .execute(&admin)
        .await
        .unwrap();
        sqlx::query("INSERT INTO fvoci.workspaces(id,slug,name,next_document_number) VALUES($1,'off-copy','OFF',1)")
            .bind(workspace).execute(&admin).await.unwrap();
        sqlx::query(
            "INSERT INTO fvoci.memberships(workspace_id,user_id,role) VALUES($1,$2,'owner')",
        )
        .bind(workspace)
        .bind(actor)
        .execute(&admin)
        .await
        .unwrap();
        let original = super::super::documents::empty_document_json();
        sqlx::query("INSERT INTO fvoci.documents(id,workspace_id,title,path,sort_key,number,status,schema_version,created_by,content_json) VALUES($1,$2,'Original',$3,'V',1,'published',2,$4,$5)")
            .bind(source).bind(workspace).bind(source.simple().to_string()).bind(actor).bind(&original).execute(&admin).await.unwrap();
        sqlx::query("INSERT INTO fvoci.sessions(id,user_id,token_hash,expires_at) VALUES($1,$2,$3,now()+interval '30 days')")
            .bind(credential).bind(actor).bind(&session.hash).execute(&admin).await.unwrap();
        let backend = Backend::Postgres(app.clone());
        let request = OffDraftCreateRequest {
            workspace,
            source: RevisionTarget::Document(source).into(),
            destination_project: None,
            parent: None,
            actor,
            credential,
            command: Uuid::now_v7(),
            title: "private native copy 😀".into(),
            icon: None,
            content_json: json!({"type":"doc","content":[{"type":"paragraph","attrs":{"id":"client-source-block"},"content":[{"type":"text","text":"unsaved private PG 😀","marks":[{"type":"bold"}]}]}]}),
            client_ip: None,
        };
        let before = observable(&admin, workspace).await;
        let mut outer = backend.begin_off_body().await.unwrap();
        let staged = create_draft_in_writer(
            &mut outer.operation(),
            RealtimeMode::Off,
            engine.clone(),
            &request,
            Arc::new(AtomicBool::new(false)),
        )
        .await
        .unwrap();
        let OperationTx::Postgres(writer) = outer.operation() else {
            panic!("real PG writer")
        };
        let fk = sqlx::query("UPDATE fvoci.wiki_create_commands SET document_id=$1 WHERE workspace_id=$2 AND command_id=$3")
            .bind(Uuid::now_v7()).bind(workspace).bind(request.command).execute(&mut **writer).await.unwrap_err();
        assert_eq!(
            fk.as_database_error().unwrap().code().as_deref(),
            Some("23503"),
            "same real restricted writer after all copy effects"
        );
        outer.rollback().await.unwrap();
        assert_eq!(observable(&admin, workspace).await, before);
        let staged_exists: bool =
            sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM fvoci.documents WHERE id=$1)")
                .bind(staged.document.id)
                .fetch_one(&admin)
                .await
                .unwrap();
        assert!(!staged_exists);
        let created =
            create_off_draft(&backend, RealtimeMode::Off, engine.clone(), request.clone())
                .await
                .unwrap();
        assert_ne!(created.document.id, source);
        assert_ne!(created.document.id, staged.document.id);
        assert_eq!(created.document.number, 2);
        assert_eq!(created.tail_seq, "1");
        let fresh = read_off_body(
            &backend,
            RealtimeMode::Off,
            engine.clone(),
            workspace,
            RevisionTarget::Document(created.document.id).into(),
            actor,
            credential,
        )
        .await
        .unwrap();
        assert_eq!(fresh.native.tail_seq, 1);
        assert_eq!(fresh.content_json, created.document.content_json);
        assert!(fresh
            .content_json
            .to_string()
            .contains("unsaved private PG 😀"));
        let block = fresh.content_json["content"][0]["attrs"]["id"]
            .as_str()
            .unwrap();
        assert!(Uuid::parse_str(block).is_ok());
        assert_ne!(block, "client-source-block");
        assert!(fresh.content_json.to_string().contains("bold"));
        let after = observable(&admin, workspace).await;
        assert_eq!(
            (after[0], after[1], after[2], after[3], after[6], after[7]),
            (
                before[0] + 1,
                before[1] + 1,
                before[2] + 1,
                before[3] + 1,
                before[6] + 1,
                before[7] + 1
            )
        );
        let replay = create_off_draft(&backend, RealtimeMode::Off, engine.clone(), request.clone())
            .await
            .unwrap();
        assert_eq!(
            serde_json::to_value(replay).unwrap(),
            serde_json::to_value(&created).unwrap()
        );
        assert_eq!(observable(&admin, workspace).await, after);
        let mut mismatch = request.clone();
        mismatch.content_json["content"][0]["content"][0]["text"] = json!("different private body");
        assert!(matches!(
            create_off_draft(&backend, RealtimeMode::Off, engine.clone(), mismatch).await,
            Err(OffDraftCreateError::Body(BodySaveError::RequestMismatch))
        ));
        sqlx::query("UPDATE fvoci.sessions SET revoked_at=now() WHERE id=$1")
            .bind(credential)
            .execute(&admin)
            .await
            .unwrap();
        assert!(matches!(
            create_off_draft(&backend, RealtimeMode::Off, engine.clone(), request.clone()).await,
            Err(OffDraftCreateError::Body(BodySaveError::Native(
                CollabDbError::Forbidden
            )))
        ));
        assert_eq!(observable(&admin, workspace).await, after);
        sqlx::query("UPDATE fvoci.sessions SET revoked_at=NULL WHERE id=$1")
            .bind(credential)
            .execute(&admin)
            .await
            .unwrap();
        assert_eq!(
            create_off_draft(&backend, RealtimeMode::Off, engine, request)
                .await
                .unwrap()
                .document
                .id,
            created.document.id
        );
        let source_state:(Value,i64,i64) = sqlx::query_as("SELECT content_json,(SELECT count(*) FROM fvoci.document_states WHERE workspace_id=$1 AND document_id=$2),(SELECT count(*) FROM fvoci.revisions WHERE workspace_id=$1 AND target_kind='document' AND target_id=$2) FROM fvoci.documents WHERE workspace_id=$1 AND id=$2")
            .bind(workspace).bind(source).fetch_one(&admin).await.unwrap();
        assert_eq!(
            source_state,
            (original, 0, 0),
            "copy must not initialize, reseed or write original history"
        );
        app.close().await;
        admin.close().await;
        sqlx::query(&format!("DROP DATABASE \"{database}\""))
            .execute(&admin_server)
            .await
            .unwrap();
        sqlx::query(&format!("DROP ROLE \"{role}\""))
            .execute(&admin_server)
            .await
            .unwrap();
        admin_server.close().await;
    }
}
