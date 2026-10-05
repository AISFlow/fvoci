use chrono::{DateTime, Utc};
use serde_json::Value;
use sqlx::{PgPool, Postgres, Transaction};
use uuid::Uuid;

use crate::collab::derived_body::PreparedDerivedBody;
use crate::db::backend::{Backend, DbTx, OperationTx};
use crate::db::codec::{Cell, FamilyRow};
use crate::db::collab::{CollabKind, FamilyNativeConsumerProof, FamilyRoomFence};
use crate::db::context::{set_system, set_tenant};
use crate::db::projects::{load_live_project, project_permission};
use crate::db::workspace::workspace_is_live;
use crate::projects::ProjectPermission;

const SESSION_REVISION_HEAD_RETRIES: u32 = 2;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RevisionDbError {
    NotFound,
    /// Observed revision head changed between compare and INSERT.
    StaleRevisionHead,
    Forbidden,
    /// Task revision write on an archived task (409 `task_archived`).
    TaskArchived,
    /// Task revision write in an archived project (409 `project_archived`).
    ProjectArchived,
    /// A restore preview or correlation no longer describes this operation.
    RestoreConflict,
}

/// Revision owner (`revisions.target_kind` / `target_id`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RevisionTarget {
    Document(Uuid),
    Task(Uuid),
}

impl RevisionTarget {
    pub fn kind_str(self) -> &'static str {
        match self {
            Self::Document(_) => TARGET_DOCUMENT,
            Self::Task(_) => TARGET_TASK,
        }
    }

    pub fn id(self) -> Uuid {
        match self {
            Self::Document(id) | Self::Task(id) => id,
        }
    }

    fn matches(self, target_kind: &str, target_id: Uuid) -> bool {
        target_kind == self.kind_str() && target_id == self.id()
    }
}

/// Route a caller reached a revision target through. Wiki document routes use
/// the wiki document permission; project document routes (`project_id` set)
/// use the owning project's permission and require the document to belong to
/// that project. Stored revisions are keyed by the target alone.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RevisionScope {
    target: RevisionTarget,
    project_id: Option<Uuid>,
}

impl RevisionScope {
    pub fn project_document(project_id: Uuid, document_id: Uuid) -> Self {
        Self {
            target: RevisionTarget::Document(document_id),
            project_id: Some(project_id),
        }
    }

    pub fn target(self) -> RevisionTarget {
        self.target
    }
}

impl From<RevisionTarget> for RevisionScope {
    fn from(target: RevisionTarget) -> Self {
        Self {
            target,
            project_id: None,
        }
    }
}

#[derive(Debug, Clone)]
pub struct RevisionMeta {
    pub id: Uuid,
    pub target_kind: String,
    pub target_id: Uuid,
    pub reason: String,
    pub created_by: Option<Uuid>,
    pub created_at: DateTime<Utc>,
    pub restored_from_id: Option<Uuid>,
}

#[derive(Debug, Clone)]
pub struct RevisionDetail {
    pub meta: RevisionMeta,
    pub content_json: Value,
    pub y_snapshot: Vec<u8>,
}

#[derive(Debug, Clone)]
pub struct RevisionListPage {
    pub items: Vec<RevisionMeta>,
    pub next_cursor: Option<String>,
}

#[derive(Debug, Clone)]
pub struct CreateRevisionInput {
    pub y_snapshot: Vec<u8>,
    pub content_json: Value,
    pub text: String,
    pub reason: String,
}

/// Observed revision head before semantic compare; re-checked under row locks at INSERT.
#[derive(Debug, Clone)]
pub struct SystemRevisionHead {
    pub latest_revision_id: Option<Uuid>,
    pub latest_y_snapshot: Option<Vec<u8>>,
}

impl SystemRevisionHead {
    pub fn from_latest(row: Option<(Uuid, Vec<u8>)>) -> Self {
        match row {
            None => Self {
                latest_revision_id: None,
                latest_y_snapshot: None,
            },
            Some((id, snap)) => Self {
                latest_revision_id: Some(id),
                latest_y_snapshot: Some(snap),
            },
        }
    }

    fn matches_current(&self, current: Option<(Uuid, Vec<u8>)>) -> bool {
        match (self.latest_revision_id, current) {
            (None, None) => true,
            (Some(expected_id), Some((id, snap))) => {
                expected_id == id
                    && self
                        .latest_y_snapshot
                        .as_deref()
                        .is_some_and(|fenced| fenced == snap.as_slice())
            }
            _ => false,
        }
    }
}

pub const SYSTEM_REVISION_HEAD_RETRIES: u32 = SESSION_REVISION_HEAD_RETRIES;

#[derive(Debug, Clone)]
pub struct PersistedCollabSource {
    pub snapshot: Vec<u8>,
    pub tail: Vec<Vec<u8>>,
}

/// Durable collab snapshot + tail from DB (session revision capture; no user gate).
#[derive(Debug, Clone)]
pub struct DurableCollabSnapshot {
    pub snapshot: Vec<u8>,
    pub tail: Vec<Vec<u8>>,
    pub tail_seq: i64,
    pub snapshot_cutoff_seq: i64,
}

/// Load persisted collab bytes for automatic session snapshots (tenant-scoped, live target only).
pub async fn load_durable_collab_for_system(
    pool: &PgPool,
    workspace_id: Uuid,
    target: RevisionTarget,
) -> Result<Result<DurableCollabSnapshot, RevisionDbError>, sqlx::Error> {
    let mut tx = pool.begin().await?;
    set_tenant(&mut tx, workspace_id).await?;
    if !workspace_is_live(&mut tx, workspace_id).await? {
        tx.rollback().await?;
        return Ok(Err(RevisionDbError::NotFound));
    }
    let (state_sql, tail_sql) = match target {
        RevisionTarget::Document(_) => (
            "SELECT state, encoding, snapshot_cutoff_seq, tail_seq FROM fvoci.document_states WHERE workspace_id = $1 AND document_id = $2",
            "SELECT payload FROM fvoci.document_collab_updates WHERE workspace_id = $1 AND document_id = $2 AND seq > $3 ORDER BY seq ASC",
        ),
        RevisionTarget::Task(_) => (
            "SELECT state, encoding, snapshot_cutoff_seq, tail_seq FROM fvoci.task_states WHERE workspace_id = $1 AND task_id = $2",
            "SELECT payload FROM fvoci.task_collab_updates WHERE workspace_id = $1 AND task_id = $2 AND seq > $3 ORDER BY seq ASC",
        ),
    };
    let state: Option<(Vec<u8>, i16, i64, i64)> = sqlx::query_as(state_sql)
        .bind(workspace_id)
        .bind(target.id())
        .fetch_optional(&mut *tx)
        .await?;
    let Some((snapshot, encoding, snapshot_cutoff_seq, tail_seq)) = state else {
        tx.rollback().await?;
        return Ok(Err(RevisionDbError::NotFound));
    };
    if encoding != 1 {
        tx.rollback().await?;
        return Ok(Err(RevisionDbError::NotFound));
    }
    let tail: Vec<(Vec<u8>,)> = sqlx::query_as(tail_sql)
        .bind(workspace_id)
        .bind(target.id())
        .bind(snapshot_cutoff_seq)
        .fetch_all(&mut *tx)
        .await?;
    tx.commit().await?;
    Ok(Ok(DurableCollabSnapshot {
        snapshot,
        tail: tail.into_iter().map(|(payload,)| payload).collect(),
        tail_seq,
        snapshot_cutoff_seq,
    }))
}

#[derive(Debug, Clone, Copy)]
pub struct RevisionCursor {
    pub created_at: DateTime<Utc>,
    pub id: Uuid,
}

const MANUAL_REASON: &str = "manual";
const SESSION_REASON: &str = "session";
pub const SCHEDULED_REASON: &str = "scheduled";
const TARGET_DOCUMENT: &str = "document";
const TARGET_TASK: &str = "task";

type TaskRevisionLockRow = (Uuid, Option<DateTime<Utc>>);

fn is_automatic_revision_reason(reason: &str) -> bool {
    reason == SESSION_REASON || reason == SCHEDULED_REASON
}

type RevisionMetaRow = (
    Uuid,
    String,
    Uuid,
    String,
    Option<Uuid>,
    DateTime<Utc>,
    Option<Uuid>,
);
type RevisionDetailRow = (
    Uuid,
    String,
    Uuid,
    String,
    Option<Uuid>,
    DateTime<Utc>,
    Option<Uuid>,
    Value,
    Vec<u8>,
);

pub fn encode_revision_cursor(cursor: RevisionCursor) -> String {
    use base64::Engine;
    let payload = serde_json::json!({
        "ca": cursor.created_at.to_rfc3339(),
        "id": cursor.id.to_string(),
    });
    base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(payload.to_string())
}

pub fn decode_revision_cursor(raw: &str) -> Option<RevisionCursor> {
    use base64::Engine;
    if raw.len() > 1024 {
        return None;
    }
    let bytes = base64::engine::general_purpose::URL_SAFE_NO_PAD
        .decode(raw)
        .ok()?;
    let value: Value = serde_json::from_slice(&bytes).ok()?;
    let object = value.as_object()?;
    if object.keys().any(|key| key != "ca" && key != "id") {
        return None;
    }
    let created_at = DateTime::parse_from_rfc3339(object.get("ca")?.as_str()?)
        .ok()?
        .with_timezone(&Utc);
    let id = Uuid::parse_str(object.get("id")?.as_str()?).ok()?;
    Some(RevisionCursor { created_at, id })
}

impl OperationTx<'_, '_> {
    async fn revision_authorize_document(
        &mut self,
        workspace_id: Uuid,
        actor_user_id: Uuid,
        session_id: Uuid,
        document_id: Uuid,
        write: bool,
    ) -> Result<Result<(), RevisionDbError>, sqlx::Error> {
        if !self.session_is_live(actor_user_id, session_id).await? {
            return Ok(Err(RevisionDbError::Forbidden));
        }
        if !self.workspace_is_live(workspace_id).await? {
            return Ok(Err(RevisionDbError::NotFound));
        }
        let min = if write {
            crate::projects::ProjectPermission::Edit
        } else {
            crate::projects::ProjectPermission::View
        };
        let permission = self
            .document_permission(workspace_id, actor_user_id, document_id, true)
            .await?;
        if !permission.at_least(min) {
            return Ok(Err(RevisionDbError::NotFound));
        }
        Ok(Ok(()))
    }

    /// The actor's effective permission on a live project and whether it is
    /// archived. A write share-locks the project row so project mutations wait for
    /// it; a read (in [`begin_read`]) takes no row lock.
    async fn revision_project_access(
        &mut self,
        workspace_id: Uuid,
        actor_user_id: Uuid,
        project_id: Uuid,
        write: bool,
    ) -> Result<Option<(ProjectPermission, bool)>, sqlx::Error> {
        if write {
            return self
                .share_lock_project_permission(workspace_id, actor_user_id, project_id)
                .await;
        }
        self.revision_project_access_read(workspace_id, actor_user_id, project_id)
            .await
    }

    /// Project document revision access: the caller's effective project permission
    /// (members and project group grants; wiki document grants never apply), under
    /// a share lock on the project row for writes, and the live document must
    /// belong to the project named by the route. Writes refuse an archived
    /// project (source `assertProjectWritable`).
    async fn revision_authorize_project_document(
        &mut self,
        workspace_id: Uuid,
        actor_user_id: Uuid,
        session_id: Uuid,
        project_id: Uuid,
        document_id: Uuid,
        write: bool,
    ) -> Result<Result<(), RevisionDbError>, sqlx::Error> {
        if !self.workspace_is_live(workspace_id).await? {
            return Ok(Err(RevisionDbError::NotFound));
        }
        let Some((permission, project_archived)) = self
            .revision_project_access(workspace_id, actor_user_id, project_id, write)
            .await?
        else {
            return Ok(Err(RevisionDbError::NotFound));
        };
        // Checked after a write's project lock wait so a session revoked meanwhile
        // is refused.
        if !self.session_is_live(actor_user_id, session_id).await? {
            return Ok(Err(RevisionDbError::Forbidden));
        }
        let min = if write {
            crate::projects::ProjectPermission::Edit
        } else {
            crate::projects::ProjectPermission::View
        };
        if !permission.at_least(min) {
            return Ok(Err(RevisionDbError::NotFound));
        }
        let document = self
            .revision_live_document_project(workspace_id, document_id)
            .await?;
        if document.flatten() != Some(project_id) {
            return Ok(Err(RevisionDbError::NotFound));
        }
        if write && project_archived {
            return Ok(Err(RevisionDbError::ProjectArchived));
        }
        Ok(Ok(()))
    }

    /// Task revision access: a live task in a live project the caller can view
    /// (read) or edit (write). Writes also refuse an archived project or task
    /// (source `assertTaskWritable`) and share-lock the project row.
    async fn revision_authorize_task(
        &mut self,
        workspace_id: Uuid,
        actor_user_id: Uuid,
        session_id: Uuid,
        task_id: Uuid,
        write: bool,
    ) -> Result<Result<(), RevisionDbError>, sqlx::Error> {
        if !self.session_is_live(actor_user_id, session_id).await? {
            return Ok(Err(RevisionDbError::Forbidden));
        }
        if !self.workspace_is_live(workspace_id).await? {
            return Ok(Err(RevisionDbError::NotFound));
        }
        let task = self.revision_live_task(workspace_id, task_id).await?;
        let Some((project_id, archived_at)) = task else {
            return Ok(Err(RevisionDbError::NotFound));
        };
        let Some((permission, project_archived)) = self
            .revision_project_access(workspace_id, actor_user_id, project_id, write)
            .await?
        else {
            return Ok(Err(RevisionDbError::NotFound));
        };
        let min = if write {
            crate::projects::ProjectPermission::Edit
        } else {
            crate::projects::ProjectPermission::View
        };
        if !permission.at_least(min) {
            return Ok(Err(RevisionDbError::NotFound));
        }
        if write && project_archived {
            return Ok(Err(RevisionDbError::ProjectArchived));
        }
        if write && archived_at.is_some() {
            return Ok(Err(RevisionDbError::TaskArchived));
        }
        Ok(Ok(()))
    }

    pub(crate) async fn authorize_revision_scope(
        &mut self,
        workspace_id: Uuid,
        actor_user_id: Uuid,
        session_id: Uuid,
        scope: RevisionScope,
        write: bool,
    ) -> Result<Result<(), RevisionDbError>, sqlx::Error> {
        match (scope.target, scope.project_id) {
            (RevisionTarget::Document(id), None) => {
                self.revision_authorize_document(workspace_id, actor_user_id, session_id, id, write)
                    .await
            }
            (RevisionTarget::Document(id), Some(project_id)) => {
                self.revision_authorize_project_document(
                    workspace_id,
                    actor_user_id,
                    session_id,
                    project_id,
                    id,
                    write,
                )
                .await
            }
            (RevisionTarget::Task(id), _) => {
                self.revision_authorize_task(workspace_id, actor_user_id, session_id, id, write)
                    .await
            }
        }
    }

    async fn revision_project_access_read(
        &mut self,
        workspace: Uuid,
        actor: Uuid,
        project: Uuid,
    ) -> Result<Option<(ProjectPermission, bool)>, sqlx::Error> {
        match self {
            Self::Postgres(tx) => {
                let Some(row) = load_live_project(tx, workspace, project).await? else {
                    return Ok(None);
                };
                let permission = project_permission(tx, workspace, actor, &row).await?;
                Ok(Some((permission, row.status == "archived")))
            }
            Self::SqliteFamily(tx) => {
                tx.require_tenant(workspace)?;
                let rows=tx.query("SELECT status FROM projects WHERE workspace_id=?1 AND id=?2 AND deleted_at IS NULL",&[Cell::uuid(workspace),Cell::uuid(project)]).await?;
                let Some(row) = rows.first() else {
                    return Ok(None);
                };
                let archived = row.cell(0)?.string()? == "archived";
                let permission = self
                    .project_permission_by_id(workspace, actor, project)
                    .await?;
                Ok(permission.map(|permission| (permission, archived)))
            }
        }
    }
    async fn revision_live_document_project(
        &mut self,
        workspace: Uuid,
        document: Uuid,
    ) -> Result<Option<Option<Uuid>>, sqlx::Error> {
        match self {
            Self::Postgres(tx)=>sqlx::query_as::<_,(Option<Uuid>,)>("SELECT project_id FROM fvoci.documents WHERE workspace_id=$1 AND id=$2 AND deleted_at IS NULL")
                .bind(workspace).bind(document).fetch_optional(&mut ***tx).await.map(|row|row.map(|(project,)|project)),
            Self::SqliteFamily(tx)=>{
                tx.require_tenant(workspace)?;
                tx.query("SELECT project_id FROM documents WHERE workspace_id=?1 AND id=?2 AND deleted_at IS NULL",&[Cell::uuid(workspace),Cell::uuid(document)]).await?
                    .first().map(|row|row.cell(0)?.optional(Cell::id)).transpose()
            },
        }
    }
    async fn revision_live_task(
        &mut self,
        workspace: Uuid,
        task: Uuid,
    ) -> Result<Option<(Uuid, Option<DateTime<Utc>>)>, sqlx::Error> {
        match self {
            Self::Postgres(tx)=>sqlx::query_as("SELECT project_id,archived_at FROM fvoci.tasks WHERE workspace_id=$1 AND id=$2 AND deleted_at IS NULL")
                .bind(workspace).bind(task).fetch_optional(&mut ***tx).await,
            Self::SqliteFamily(tx)=>{
                tx.require_tenant(workspace)?;
                tx.query("SELECT project_id,archived_at FROM tasks WHERE workspace_id=?1 AND id=?2 AND deleted_at IS NULL",&[Cell::uuid(workspace),Cell::uuid(task)]).await?
                    .first().map(|row|Ok((row.cell(0)?.id()?,row.cell(1)?.optional(Cell::datetime)?))).transpose()
            },
        }
    }
}

async fn authorize_target(
    tx: &mut Transaction<'_, Postgres>,
    workspace: Uuid,
    actor: Uuid,
    credential: Uuid,
    scope: RevisionScope,
    write: bool,
) -> Result<Result<(), RevisionDbError>, sqlx::Error> {
    OperationTx::Postgres(tx)
        .authorize_revision_scope(workspace, actor, credential, scope, write)
        .await
}

/// Revision access check for a document or task target without other work.
pub async fn authorize_revision_target(
    pool: &PgPool,
    workspace: Uuid,
    actor: Uuid,
    credential: Uuid,
    scope: impl Into<RevisionScope>,
    write: bool,
) -> Result<Result<(), RevisionDbError>, sqlx::Error> {
    authorize_revision_target_backend(
        &Backend::Postgres(pool.clone()),
        workspace,
        actor,
        credential,
        scope,
        write,
    )
    .await
}

pub async fn authorize_revision_target_backend(
    backend: &Backend,
    workspace: Uuid,
    actor: Uuid,
    credential: Uuid,
    scope: impl Into<RevisionScope>,
    write: bool,
) -> Result<Result<(), RevisionDbError>, sqlx::Error> {
    let mut tx = if write {
        backend.begin_write().await?
    } else {
        backend.begin_read().await?
    };
    let result = async {
        let mut op = tx.operation();
        op.set_tenant(workspace).await?;
        op.authorize_revision_scope(workspace, actor, credential, scope.into(), write)
            .await
    }
    .await;
    finish_revision_authorization(tx, result).await
}

#[derive(Debug, thiserror::Error)]
#[error("revision authorization refused: {0:?}")]
struct RevisionAuthorizationRefusal(RevisionDbError);

async fn finish_revision_authorization(
    tx: DbTx,
    result: Result<Result<(), RevisionDbError>, sqlx::Error>,
) -> Result<Result<(), RevisionDbError>, sqlx::Error> {
    match result {
        Ok(Ok(())) => {
            tx.commit_with_cleanup()
                .await
                .map_err(|error| sqlx::Error::AnyDriverError(Box::new(error)))?;
            Ok(Ok(()))
        }
        Ok(Err(refusal)) => {
            if let Err(cleanup) = tx.rollback().await {
                return Err(crate::db::backend::rollback_cleanup_unknown(
                    Some(Box::new(RevisionAuthorizationRefusal(refusal))),
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

async fn collab_state_exists(
    tx: &mut Transaction<'_, Postgres>,
    workspace_id: Uuid,
    target: RevisionTarget,
) -> Result<bool, sqlx::Error> {
    let sql = match target {
        RevisionTarget::Document(_) => {
            "SELECT EXISTS (SELECT 1 FROM fvoci.document_states WHERE workspace_id = $1 AND document_id = $2)"
        }
        RevisionTarget::Task(_) => {
            "SELECT EXISTS (SELECT 1 FROM fvoci.task_states WHERE workspace_id = $1 AND task_id = $2)"
        }
    };
    let exists: bool = sqlx::query_scalar(sql)
        .bind(workspace_id)
        .bind(target.id())
        .fetch_one(&mut **tx)
        .await?;
    Ok(exists)
}

pub async fn list_revisions(
    pool: &PgPool,
    workspace: Uuid,
    actor: Uuid,
    credential: Uuid,
    scope: impl Into<RevisionScope>,
    limit: i64,
    before: Option<RevisionCursor>,
) -> Result<Result<RevisionListPage, RevisionDbError>, sqlx::Error> {
    list_revisions_backend(
        &Backend::Postgres(pool.clone()),
        workspace,
        actor,
        credential,
        scope,
        limit,
        before,
    )
    .await
}

pub async fn list_revisions_backend(
    backend: &Backend,
    workspace_id: Uuid,
    actor_user_id: Uuid,
    session_id: Uuid,
    scope: impl Into<RevisionScope>,
    limit: i64,
    before: Option<RevisionCursor>,
) -> Result<Result<RevisionListPage, RevisionDbError>, sqlx::Error> {
    let scope = scope.into();
    let target = scope.target;
    let mut tx = backend.begin_read().await?;
    tx.operation().set_tenant(workspace_id).await?;
    if let Err(error) = tx
        .operation()
        .authorize_revision_scope(workspace_id, actor_user_id, session_id, scope, false)
        .await?
    {
        tx.rollback().await?;
        return Ok(Err(error));
    }
    let rows = tx
        .operation()
        .revision_list_rows(workspace_id, target, limit.saturating_add(1), before)
        .await?;
    tx.commit().await.map_err(|unknown| unknown.source)?;
    let mut items: Vec<RevisionMeta> = rows
        .into_iter()
        .map(
            |(id, target_kind, target_id, reason, created_by, created_at, restored_from_id)| {
                RevisionMeta {
                    id,
                    target_kind,
                    target_id,
                    reason,
                    created_by,
                    created_at,
                    restored_from_id,
                }
            },
        )
        .collect();
    let next_cursor = if items.len() as i64 > limit {
        items.pop();
        items.last().map(|last| {
            encode_revision_cursor(RevisionCursor {
                created_at: last.created_at,
                id: last.id,
            })
        })
    } else {
        None
    };
    Ok(Ok(RevisionListPage { items, next_cursor }))
}

pub async fn get_revision(
    pool: &PgPool,
    workspace: Uuid,
    actor: Uuid,
    credential: Uuid,
    scope: impl Into<RevisionScope>,
    revision: Uuid,
) -> Result<Result<RevisionDetail, RevisionDbError>, sqlx::Error> {
    get_revision_backend(
        &Backend::Postgres(pool.clone()),
        workspace,
        actor,
        credential,
        scope,
        revision,
    )
    .await
}

pub async fn get_revision_backend(
    backend: &Backend,
    workspace: Uuid,
    actor: Uuid,
    credential: Uuid,
    scope: impl Into<RevisionScope>,
    revision: Uuid,
) -> Result<Result<RevisionDetail, RevisionDbError>, sqlx::Error> {
    let scope = scope.into();
    let target = scope.target;
    let mut tx = backend.begin_read().await?;
    tx.operation().set_tenant(workspace).await?;
    if let Err(error) = tx
        .operation()
        .authorize_revision_scope(workspace, actor, credential, scope, false)
        .await?
    {
        tx.rollback().await?;
        return Ok(Err(error));
    }
    let row = tx
        .operation()
        .revision_detail_row(workspace, revision)
        .await?;
    tx.commit().await.map_err(|unknown| unknown.source)?;
    match row {
        Some((
            id,
            target_kind,
            target_id,
            reason,
            created_by,
            created_at,
            restored_from_id,
            content_json,
            y_snapshot,
        )) if target.matches(&target_kind, target_id) => Ok(Ok(RevisionDetail {
            meta: RevisionMeta {
                id,
                target_kind,
                target_id,
                reason,
                created_by,
                created_at,
                restored_from_id,
            },
            content_json,
            y_snapshot,
        })),
        _ => Ok(Err(RevisionDbError::NotFound)),
    }
}

pub async fn create_manual_revision(
    pool: &PgPool,
    workspace: Uuid,
    actor: Uuid,
    credential: Uuid,
    scope: impl Into<RevisionScope>,
    input: CreateRevisionInput,
) -> Result<Result<Uuid, RevisionDbError>, sqlx::Error> {
    create_manual_revision_backend(
        &Backend::Postgres(pool.clone()),
        workspace,
        actor,
        credential,
        scope,
        input,
    )
    .await
}

pub async fn create_manual_revision_backend(
    backend: &Backend,
    workspace: Uuid,
    actor: Uuid,
    credential: Uuid,
    scope: impl Into<RevisionScope>,
    input: CreateRevisionInput,
) -> Result<Result<Uuid, RevisionDbError>, sqlx::Error> {
    create_manual_revision_with_room_proof(
        backend,
        workspace,
        actor,
        credential,
        scope.into(),
        input,
        None,
    )
    .await
}

pub(crate) async fn create_manual_revision_with_room_proof(
    backend: &Backend,
    workspace: Uuid,
    actor: Uuid,
    credential: Uuid,
    scope: RevisionScope,
    input: CreateRevisionInput,
    proof: Option<FamilyNativeConsumerProof>,
) -> Result<Result<Uuid, RevisionDbError>, sqlx::Error> {
    let mut tx = backend.begin_write().await?;
    let result = if proof.is_some() {
        tx.operation()
            .create_manual_revision_with_room_proof(
                workspace, actor, credential, scope, input, proof,
            )
            .await?
    } else {
        tx.operation()
            .create_manual_revision(workspace, actor, credential, scope, input)
            .await?
    };
    if result.is_ok() {
        tx.commit().await.map_err(|unknown| unknown.source)?;
    } else {
        tx.rollback().await?;
    }
    Ok(result)
}

fn family_revision_meta(row: &FamilyRow) -> Result<RevisionMetaRow, sqlx::Error> {
    Ok((
        row.cell(0)?.id()?,
        row.cell(1)?.string()?,
        row.cell(2)?.id()?,
        row.cell(3)?.string()?,
        row.cell(4)?.optional(Cell::id)?,
        row.cell(5)?.datetime()?,
        row.cell(6)?.optional(Cell::id)?,
    ))
}

impl OperationTx<'_, '_> {
    async fn revision_list_rows(
        &mut self,
        workspace_id: Uuid,
        target: RevisionTarget,
        fetch_limit: i64,
        before: Option<RevisionCursor>,
    ) -> Result<Vec<RevisionMetaRow>, sqlx::Error> {
        match self {
            Self::Postgres(tx) => match before {
                Some(cursor) => {
                    sqlx::query_as(
                        r#"
                SELECT id, target_kind, target_id, reason, created_by, created_at, restored_from_id
                FROM fvoci.revisions
                WHERE workspace_id = $1
                  AND target_kind = $2
                  AND target_id = $3
                  AND (created_at, id) < ($4, $5)
                ORDER BY created_at DESC, id DESC
                LIMIT $6
                "#,
                    )
                    .bind(workspace_id)
                    .bind(target.kind_str())
                    .bind(target.id())
                    .bind(cursor.created_at)
                    .bind(cursor.id)
                    .bind(fetch_limit)
                    .fetch_all(&mut ***tx)
                    .await
                }
                None => {
                    sqlx::query_as(
                        r#"
                SELECT id, target_kind, target_id, reason, created_by, created_at, restored_from_id
                FROM fvoci.revisions
                WHERE workspace_id = $1
                  AND target_kind = $2
                  AND target_id = $3
                ORDER BY created_at DESC, id DESC
                LIMIT $4
                "#,
                    )
                    .bind(workspace_id)
                    .bind(target.kind_str())
                    .bind(target.id())
                    .bind(fetch_limit)
                    .fetch_all(&mut ***tx)
                    .await
                }
            },
            Self::SqliteFamily(tx) => {
                tx.require_tenant(workspace_id)?;
                let (instant, id) = match before {
                    Some(cursor) => (Cell::instant(cursor.created_at)?, Cell::uuid(cursor.id)),
                    None => (Cell::Null, Cell::Null),
                };
                tx.query("SELECT id,target_kind,target_id,reason,created_by,created_at,restored_from_id FROM revisions WHERE workspace_id=?1 AND target_kind=?2 AND target_id=?3 AND (?4 IS NULL OR (created_at,id)<(?4,?5)) ORDER BY created_at DESC,id DESC LIMIT ?6",
                    &[Cell::uuid(workspace_id),Cell::text(target.kind_str()),Cell::uuid(target.id()),instant,id,Cell::Integer(fetch_limit)]).await?.iter().map(family_revision_meta).collect()
            }
        }
    }
    async fn revision_detail_row(
        &mut self,
        workspace: Uuid,
        revision: Uuid,
    ) -> Result<Option<RevisionDetailRow>, sqlx::Error> {
        match self {
            Self::Postgres(tx)=>sqlx::query_as("SELECT id,target_kind,target_id,reason,created_by,created_at,restored_from_id,content_json,y_snapshot FROM fvoci.revisions WHERE workspace_id=$1 AND id=$2")
                .bind(workspace).bind(revision).fetch_optional(&mut ***tx).await,
            Self::SqliteFamily(tx)=>{
                tx.require_tenant(workspace)?;
                tx.query("SELECT id,target_kind,target_id,reason,created_by,created_at,restored_from_id,content_json,y_snapshot FROM revisions WHERE workspace_id=?1 AND id=?2",&[Cell::uuid(workspace),Cell::uuid(revision)]).await?
                    .first().map(|row| {let (id,kind,target,reason,actor,created,source)=family_revision_meta(row)?;Ok((id,kind,target,reason,actor,created,source,row.cell(7)?.value()?,row.cell(8)?.bytes()?))}).transpose()
            },
        }
    }
    /// Current writer authorization and manual revision effect without finishing
    /// the caller's native/body/command transaction.
    pub(crate) async fn create_manual_revision(
        &mut self,
        workspace: Uuid,
        actor: Uuid,
        credential: Uuid,
        scope: RevisionScope,
        input: CreateRevisionInput,
    ) -> Result<Result<Uuid, RevisionDbError>, sqlx::Error> {
        self.create_manual_revision_with_room_proof(
            workspace, actor, credential, scope, input, None,
        )
        .await
    }

    async fn create_manual_revision_with_room_proof(
        &mut self,
        workspace: Uuid,
        actor: Uuid,
        credential: Uuid,
        scope: RevisionScope,
        input: CreateRevisionInput,
        proof: Option<FamilyNativeConsumerProof>,
    ) -> Result<Result<Uuid, RevisionDbError>, sqlx::Error> {
        self.set_tenant(workspace).await?;
        self.lock_membership_users(&[actor]).await?;
        if !self.recheck_session(actor, credential).await? {
            return Ok(Err(RevisionDbError::Forbidden));
        }
        if let Err(error) = self
            .authorize_revision_scope(workspace, actor, credential, scope, true)
            .await?
        {
            return Ok(Err(error));
        }
        if let Some(proof) = proof {
            if proof.room.workspace_id != workspace
                || scope.target != RevisionTarget::Document(proof.room.document_id)
                || !self.verify_family_native_consumer_proof(proof).await?
            {
                return Ok(Err(RevisionDbError::NotFound));
            }
        }
        let recent = self
            .revision_latest_snapshot(workspace, scope.target)
            .await?;
        if let Some((id, snapshot, reason)) = recent {
            if snapshot == input.y_snapshot {
                if is_automatic_revision_reason(&reason) {
                    self.promote_manual_revision(workspace, id, actor).await?;
                }
                if let Some(proof) = proof {
                    if !self.verify_family_native_consumer_proof(proof).await? {
                        return Ok(Err(RevisionDbError::NotFound));
                    }
                }
                return Ok(Ok(id));
            }
        }
        let id = Uuid::now_v7();
        self.insert_revision(
            workspace,
            scope.target,
            id,
            Some(actor),
            &input,
            MANUAL_REASON,
        )
        .await?;
        if let Some(proof) = proof {
            if !self.verify_family_native_consumer_proof(proof).await? {
                return Ok(Err(RevisionDbError::NotFound));
            }
        }
        Ok(Ok(id))
    }
    async fn revision_latest_snapshot(
        &mut self,
        workspace: Uuid,
        target: RevisionTarget,
    ) -> Result<Option<(Uuid, Vec<u8>, String)>, sqlx::Error> {
        match self {
            Self::Postgres(tx)=>sqlx::query_as("SELECT id,y_snapshot,reason FROM fvoci.revisions WHERE workspace_id=$1 AND target_kind=$2 AND target_id=$3 ORDER BY created_at DESC,id DESC LIMIT 1 FOR UPDATE")
                .bind(workspace).bind(target.kind_str()).bind(target.id()).fetch_optional(&mut ***tx).await,
            Self::SqliteFamily(tx)=>{
                tx.require_writer()?;tx.require_tenant(workspace)?;
                tx.query("SELECT id,y_snapshot,reason FROM revisions WHERE workspace_id=?1 AND target_kind=?2 AND target_id=?3 ORDER BY created_at DESC,id DESC LIMIT 1",&[Cell::uuid(workspace),Cell::text(target.kind_str()),Cell::uuid(target.id())]).await?
                    .first().map(|row|Ok((row.cell(0)?.id()?,row.cell(1)?.bytes()?,row.cell(2)?.string()?))).transpose()
            },
        }
    }
    async fn promote_manual_revision(
        &mut self,
        workspace: Uuid,
        revision: Uuid,
        actor: Uuid,
    ) -> Result<(), sqlx::Error> {
        match self {
            Self::Postgres(tx) => {
                sqlx::query("UPDATE fvoci.revisions SET reason=$3,created_by=$4 WHERE workspace_id=$1 AND id=$2")
                .bind(workspace).bind(revision).bind(MANUAL_REASON).bind(actor).execute(&mut ***tx).await?;
            }
            Self::SqliteFamily(tx) => {
                tx.require_writer()?;
                tx.require_tenant(workspace)?;
                tx.execute(
                    "UPDATE revisions SET reason=?3,created_by=?4 WHERE workspace_id=?1 AND id=?2",
                    &[
                        Cell::uuid(workspace),
                        Cell::uuid(revision),
                        Cell::text(MANUAL_REASON),
                        Cell::uuid(actor),
                    ],
                )
                .await?;
            }
        }
        Ok(())
    }
    async fn insert_revision(
        &mut self,
        workspace: Uuid,
        target: RevisionTarget,
        id: Uuid,
        actor: Option<Uuid>,
        input: &CreateRevisionInput,
        default_reason: &str,
    ) -> Result<(), sqlx::Error> {
        let reason = if input.reason.is_empty() {
            default_reason
        } else {
            &input.reason
        };
        match self {
            Self::Postgres(tx) => {
                sqlx::query("INSERT INTO fvoci.revisions(id,workspace_id,target_kind,target_id,y_snapshot,encoding,content_json,text,reason,created_by) VALUES($1,$2,$3,$4,$5,1,$6,$7,$8,$9)")
                .bind(id).bind(workspace).bind(target.kind_str()).bind(target.id()).bind(&input.y_snapshot).bind(&input.content_json).bind(&input.text).bind(reason).bind(actor).execute(&mut ***tx).await?;
            }
            Self::SqliteFamily(tx) => {
                tx.require_writer()?;
                tx.require_tenant(workspace)?;
                tx.execute("INSERT INTO revisions(id,workspace_id,target_kind,target_id,y_snapshot,encoding,content_json,text,reason,created_by) VALUES(?1,?2,?3,?4,?5,1,?6,?7,?8,?9)",
                    &[Cell::uuid(id),Cell::uuid(workspace),Cell::text(target.kind_str()),Cell::uuid(target.id()),Cell::Blob(input.y_snapshot.clone()),Cell::json(&input.content_json)?,Cell::text(&input.text),Cell::text(reason),Cell::optional_uuid(actor)]).await?;
            }
        }
        Ok(())
    }
}

async fn load_system_revision_generation_pg(
    tx: &mut Transaction<'_, Postgres>,
    workspace_id: Uuid,
    target: RevisionTarget,
) -> Result<Result<i64, RevisionDbError>, sqlx::Error> {
    if !workspace_is_live(&mut *tx, workspace_id).await? {
        return Ok(Err(RevisionDbError::NotFound));
    }
    let writer_generation = match target {
        RevisionTarget::Document(document_id) => {
            let affiliation: Option<(Option<Uuid>,)> = sqlx::query_as(
                "SELECT project_id FROM fvoci.documents WHERE workspace_id = $1 AND id = $2",
            )
            .bind(workspace_id)
            .bind(document_id)
            .fetch_optional(&mut **tx)
            .await?;
            let Some((expected_project_id,)) = affiliation else {
                return Ok(Err(RevisionDbError::NotFound));
            };
            if let Some(project_id) = expected_project_id {
                let project: Option<(Uuid,)> = sqlx::query_as(
                    r#"
                    SELECT id FROM fvoci.projects
                    WHERE workspace_id = $1 AND id = $2 AND deleted_at IS NULL
                    FOR SHARE
                    "#,
                )
                .bind(workspace_id)
                .bind(project_id)
                .fetch_optional(&mut **tx)
                .await?;
                if project.is_none() {
                    return Ok(Err(RevisionDbError::NotFound));
                }
            }
            let row: Option<(Option<Uuid>, Option<DateTime<Utc>>)> = sqlx::query_as(
                r#"
                SELECT project_id, deleted_at
                FROM fvoci.documents
                WHERE workspace_id = $1 AND id = $2
                FOR UPDATE
                "#,
            )
            .bind(workspace_id)
            .bind(document_id)
            .fetch_optional(&mut **tx)
            .await?;
            let Some((project_id, deleted_at)) = row else {
                return Ok(Err(RevisionDbError::NotFound));
            };
            if deleted_at.is_some() || project_id != expected_project_id {
                return Ok(Err(RevisionDbError::NotFound));
            }
            let state: Option<(i64,)> = sqlx::query_as(
                r#"
                SELECT writer_generation
                FROM fvoci.document_states
                WHERE workspace_id = $1 AND document_id = $2
                FOR UPDATE
                "#,
            )
            .bind(workspace_id)
            .bind(document_id)
            .fetch_optional(&mut **tx)
            .await?;
            state.map(|(g,)| g)
        }
        RevisionTarget::Task(task_id) => {
            let expected: Option<(Uuid,)> = sqlx::query_as(
                r#"
                SELECT project_id FROM fvoci.tasks
                WHERE workspace_id = $1 AND id = $2 AND deleted_at IS NULL
                "#,
            )
            .bind(workspace_id)
            .bind(task_id)
            .fetch_optional(&mut **tx)
            .await?;
            let Some((expected_project_id,)) = expected else {
                return Ok(Err(RevisionDbError::NotFound));
            };
            let project: Option<(Uuid,)> = sqlx::query_as(
                r#"
                SELECT id FROM fvoci.projects
                WHERE workspace_id = $1 AND id = $2 AND deleted_at IS NULL
                FOR SHARE
                "#,
            )
            .bind(workspace_id)
            .bind(expected_project_id)
            .fetch_optional(&mut **tx)
            .await?;
            if project.is_none() {
                return Ok(Err(RevisionDbError::NotFound));
            }
            let row: Option<TaskRevisionLockRow> = sqlx::query_as(
                r#"
                SELECT project_id, deleted_at
                FROM fvoci.tasks
                WHERE workspace_id = $1 AND id = $2
                FOR NO KEY UPDATE
                "#,
            )
            .bind(workspace_id)
            .bind(task_id)
            .fetch_optional(&mut **tx)
            .await?;
            let Some((project_id, deleted_at)) = row else {
                return Ok(Err(RevisionDbError::NotFound));
            };
            if deleted_at.is_some() || project_id != expected_project_id {
                return Ok(Err(RevisionDbError::NotFound));
            }
            let state: Option<(i64,)> = sqlx::query_as(
                r#"
                SELECT writer_generation
                FROM fvoci.task_states
                WHERE workspace_id = $1 AND task_id = $2
                FOR UPDATE
                "#,
            )
            .bind(workspace_id)
            .bind(task_id)
            .fetch_optional(&mut **tx)
            .await?;
            state.map(|(g,)| g)
        }
    };
    let Some(writer_generation) = writer_generation else {
        return Ok(Err(RevisionDbError::NotFound));
    };
    Ok(Ok(writer_generation))
}

impl OperationTx<'_, '_> {
    async fn lock_system_revision_target(
        &mut self,
        workspace: Uuid,
        target: RevisionTarget,
        expected: Option<i64>,
    ) -> Result<Result<(), RevisionDbError>, sqlx::Error> {
        let generation = match self {
            Self::Postgres(tx) => load_system_revision_generation_pg(tx, workspace, target).await?,
            Self::SqliteFamily(tx) => {
                tx.require_writer()?;
                tx.require_tenant(workspace)?;
                let statement = match target {
                    RevisionTarget::Document(_) => "SELECT s.writer_generation FROM documents d INNER JOIN workspaces w ON w.id=d.workspace_id AND w.deleted_at IS NULL INNER JOIN document_states s ON s.workspace_id=d.workspace_id AND s.document_id=d.id LEFT JOIN projects p ON p.workspace_id=d.workspace_id AND p.id=d.project_id WHERE d.workspace_id=?1 AND d.id=?2 AND d.deleted_at IS NULL AND (d.project_id IS NULL OR (p.id IS NOT NULL AND p.deleted_at IS NULL))",
                    RevisionTarget::Task(_) => "SELECT s.writer_generation FROM tasks t INNER JOIN workspaces w ON w.id=t.workspace_id AND w.deleted_at IS NULL INNER JOIN task_states s ON s.workspace_id=t.workspace_id AND s.task_id=t.id INNER JOIN projects p ON p.workspace_id=t.workspace_id AND p.id=t.project_id AND p.deleted_at IS NULL WHERE t.workspace_id=?1 AND t.id=?2 AND t.deleted_at IS NULL",
                };
                match tx
                    .query(statement, &[Cell::uuid(workspace), Cell::uuid(target.id())])
                    .await?
                    .first()
                {
                    Some(row) => Ok(row.cell(0)?.integer()?),
                    None => Err(RevisionDbError::NotFound),
                }
            }
        };
        let generation = match generation {
            Ok(generation) => generation,
            Err(error) => return Ok(Err(error)),
        };
        if expected.is_some_and(|expected| expected != generation) {
            return Ok(Err(RevisionDbError::NotFound));
        }
        Ok(Ok(()))
    }

    async fn latest_revision_head(
        &mut self,
        workspace: Uuid,
        target: RevisionTarget,
    ) -> Result<Option<(Uuid, Vec<u8>)>, sqlx::Error> {
        match self {
            Self::Postgres(tx) => latest_revision_in_tx(tx, workspace, target).await,
            Self::SqliteFamily(tx) => {
                tx.require_writer()?;
                tx.require_tenant(workspace)?;
                tx.query("SELECT id,y_snapshot FROM revisions WHERE workspace_id=?1 AND target_kind=?2 AND target_id=?3 ORDER BY created_at DESC,id DESC LIMIT 1",&[Cell::uuid(workspace),Cell::text(target.kind_str()),Cell::uuid(target.id())]).await?.first()
                    .map(|row| Ok((row.cell(0)?.id()?,row.cell(1)?.bytes()?))).transpose()
            }
        }
    }

    async fn current_room_revision_scope(
        &mut self,
        workspace: Uuid,
        target: RevisionTarget,
        fence: Option<FamilyRoomFence>,
        expected: Option<i64>,
    ) -> Result<bool, sqlx::Error> {
        let Some(fence) = fence else {
            return Ok(false);
        };
        if target != RevisionTarget::Document(fence.document_id) || workspace != fence.workspace_id
        {
            return Ok(false);
        }
        if !self.verify_family_room_fence(fence).await? {
            return Ok(false);
        }
        Ok(self
            .lock_system_revision_target(workspace, target, expected)
            .await?
            .is_ok())
    }
}

async fn latest_revision_in_tx(
    tx: &mut Transaction<'_, Postgres>,
    workspace_id: Uuid,
    target: RevisionTarget,
) -> Result<Option<(Uuid, Vec<u8>)>, sqlx::Error> {
    sqlx::query_as(
        r#"
        SELECT id, y_snapshot
        FROM fvoci.revisions
        WHERE workspace_id = $1 AND target_kind = $2 AND target_id = $3
        ORDER BY created_at DESC, id DESC
        LIMIT 1
        "#,
    )
    .bind(workspace_id)
    .bind(target.kind_str())
    .bind(target.id())
    .fetch_optional(&mut **tx)
    .await
}

/// System-authored revision (`created_by` null). No user permission gate.
pub async fn latest_revision_y_snapshot(
    pool: &PgPool,
    workspace_id: Uuid,
    target: RevisionTarget,
) -> Result<Option<(Uuid, Vec<u8>)>, sqlx::Error> {
    let mut tx = pool.begin().await?;
    set_tenant(&mut tx, workspace_id).await?;
    let row: Option<(Uuid, Vec<u8>)> = sqlx::query_as(
        r#"
        SELECT id, y_snapshot
        FROM fvoci.revisions
        WHERE workspace_id = $1 AND target_kind = $2 AND target_id = $3
        ORDER BY created_at DESC, id DESC
        LIMIT 1
        "#,
    )
    .bind(workspace_id)
    .bind(target.kind_str())
    .bind(target.id())
    .fetch_optional(&mut *tx)
    .await?;
    tx.commit().await?;
    Ok(row)
}

pub async fn create_system_revision(
    pool: &PgPool,
    workspace_id: Uuid,
    target: RevisionTarget,
    input: CreateRevisionInput,
    expected_writer_generation: i64,
    head_fence: SystemRevisionHead,
) -> Result<Result<Uuid, RevisionDbError>, sqlx::Error> {
    create_system_revision_for_room_backend(
        &Backend::Postgres(pool.clone()),
        workspace_id,
        target,
        input,
        expected_writer_generation,
        head_fence,
        None,
    )
    .await
}

/// Room-owned system consumers keep the family lease around the complete
/// operation. PG callers retain their original SQL, role and lock order.
pub(crate) async fn load_durable_collab_for_room_backend(
    backend: &Backend,
    workspace: Uuid,
    target: RevisionTarget,
    fence: Option<FamilyRoomFence>,
) -> Result<Result<DurableCollabSnapshot, RevisionDbError>, sqlx::Error> {
    if let Backend::Postgres(pool) = backend {
        return load_durable_collab_for_system(pool, workspace, target).await;
    }
    let mut tx = backend.begin_write().await?;
    tx.operation().set_tenant(workspace).await?;
    if !tx
        .operation()
        .current_room_revision_scope(workspace, target, fence, None)
        .await?
    {
        tx.rollback().await?;
        return Ok(Err(RevisionDbError::NotFound));
    }
    let load = tx
        .operation()
        .load_durable_native_source(CollabKind::Document, workspace, target.id())
        .await?;
    let load = match load {
        Ok(load) => load,
        Err(_) => {
            tx.rollback().await?;
            return Ok(Err(RevisionDbError::NotFound));
        }
    };
    if !tx
        .operation()
        .current_room_revision_scope(workspace, target, fence, Some(load.writer_generation))
        .await?
    {
        tx.rollback().await?;
        return Ok(Err(RevisionDbError::NotFound));
    }
    tx.commit().await.map_err(|unknown| unknown.source)?;
    Ok(Ok(DurableCollabSnapshot {
        snapshot: load.snapshot,
        tail: load.tail.into_iter().map(|row| row.payload).collect(),
        tail_seq: load.tail_seq,
        snapshot_cutoff_seq: load.snapshot_cutoff_seq,
    }))
}

pub(crate) async fn latest_revision_y_snapshot_for_room_backend(
    backend: &Backend,
    workspace: Uuid,
    target: RevisionTarget,
    fence: Option<FamilyRoomFence>,
) -> Result<Option<(Uuid, Vec<u8>)>, sqlx::Error> {
    if let Backend::Postgres(pool) = backend {
        return latest_revision_y_snapshot(pool, workspace, target).await;
    }
    let mut tx = backend.begin_write().await?;
    tx.operation().set_tenant(workspace).await?;
    if !tx
        .operation()
        .current_room_revision_scope(workspace, target, fence, None)
        .await?
    {
        tx.rollback().await?;
        return Err(sqlx::Error::Protocol("room revision scope lost".into()));
    }
    let head = tx
        .operation()
        .latest_revision_head(workspace, target)
        .await?;
    if !tx
        .operation()
        .current_room_revision_scope(workspace, target, fence, None)
        .await?
    {
        tx.rollback().await?;
        return Err(sqlx::Error::Protocol("room revision scope lost".into()));
    }
    tx.commit().await.map_err(|unknown| unknown.source)?;
    Ok(head)
}

pub(crate) async fn create_system_revision_for_room_backend(
    backend: &Backend,
    workspace: Uuid,
    target: RevisionTarget,
    input: CreateRevisionInput,
    expected_generation: i64,
    head_fence: SystemRevisionHead,
    room_fence: Option<FamilyRoomFence>,
) -> Result<Result<Uuid, RevisionDbError>, sqlx::Error> {
    let mut tx = backend.begin_write().await?;
    tx.operation().set_tenant(workspace).await?;
    let current = if matches!(backend, Backend::Postgres(_)) {
        tx.operation()
            .lock_system_revision_target(workspace, target, Some(expected_generation))
            .await?
            .is_ok()
    } else {
        tx.operation()
            .current_room_revision_scope(workspace, target, room_fence, Some(expected_generation))
            .await?
    };
    if !current {
        tx.rollback().await?;
        return Ok(Err(RevisionDbError::NotFound));
    }
    let recent = tx
        .operation()
        .latest_revision_head(workspace, target)
        .await?;
    if !head_fence.matches_current(recent.clone()) {
        tx.rollback().await?;
        return Ok(Err(RevisionDbError::StaleRevisionHead));
    }
    let id = if let Some((id, _)) = recent.filter(|(_, bytes)| bytes == &input.y_snapshot) {
        id
    } else {
        let id = Uuid::now_v7();
        tx.operation()
            .insert_revision(workspace, target, id, None, &input, SESSION_REASON)
            .await?;
        id
    };
    if !matches!(backend, Backend::Postgres(_))
        && !tx
            .operation()
            .current_room_revision_scope(workspace, target, room_fence, Some(expected_generation))
            .await?
    {
        tx.rollback().await?;
        return Ok(Err(RevisionDbError::NotFound));
    }
    tx.commit().await.map_err(|unknown| unknown.source)?;
    Ok(Ok(id))
}

pub async fn resolve_restore(
    pool: &PgPool,
    workspace_id: Uuid,
    actor_user_id: Uuid,
    session_id: Uuid,
    scope: impl Into<RevisionScope>,
    revision_id: Uuid,
    client_ip: Option<&str>,
) -> Result<Result<Vec<u8>, RevisionDbError>, sqlx::Error> {
    let scope = scope.into();
    let target = scope.target;
    let _ = client_ip;
    let mut tx = pool.begin().await?;
    set_tenant(&mut tx, workspace_id).await?;
    match authorize_target(
        &mut tx,
        workspace_id,
        actor_user_id,
        session_id,
        scope,
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
    if !collab_state_exists(&mut tx, workspace_id, target).await? {
        tx.rollback().await?;
        return Ok(Err(RevisionDbError::NotFound));
    }
    let row: Option<(String, Uuid, Vec<u8>)> = sqlx::query_as(
        r#"
        SELECT target_kind, target_id, y_snapshot
        FROM fvoci.revisions
        WHERE workspace_id = $1 AND id = $2
        "#,
    )
    .bind(workspace_id)
    .bind(revision_id)
    .fetch_optional(&mut *tx)
    .await?;
    let Some((target_kind, target_id, y_snapshot)) = row else {
        tx.rollback().await?;
        return Ok(Err(RevisionDbError::NotFound));
    };
    if !target.matches(&target_kind, target_id) {
        tx.rollback().await?;
        return Ok(Err(RevisionDbError::NotFound));
    }
    // Successful restore provenance is committed with the forward update.
    // Resolving a source is not a restore and must not announce success.
    tx.commit().await?;
    Ok(Ok(y_snapshot))
}

/// Captured restore intent; the tail is an opaque decimal string at HTTP only.
#[derive(Debug, Clone, Copy)]
pub struct RestoreRevisionInput {
    pub scope: RevisionScope,
    pub source_revision_id: Uuid,
    pub correlation_id: Uuid,
    pub expected_tail_seq: i64,
}

/// Prepared from the exact committed snapshot/tail plus restore payload.
pub struct RestoreRevisionAppend {
    pub intent: RestoreRevisionInput,
    pub revision_id: Uuid,
    pub y_snapshot: Vec<u8>,
    pub prepared_body: PreparedDerivedBody,
}

#[derive(Debug, Clone, Copy)]
pub struct RestoredRevision {
    pub revision_id: Uuid,
    pub committed_tail_seq: i64,
}

type RestoreReceiptRow = (
    Uuid,
    String,
    Uuid,
    Option<Uuid>,
    Option<Uuid>,
    Option<i64>,
    Option<i64>,
);

/// Revalidate the route's project/document/task boundary in the append tx.
pub(crate) async fn authorize_restore_in_tx(
    tx: &mut Transaction<'_, Postgres>,
    workspace_id: Uuid,
    actor_user_id: Uuid,
    session_id: Uuid,
    input: RestoreRevisionInput,
) -> Result<Result<(), RevisionDbError>, sqlx::Error> {
    authorize_target(
        tx,
        workspace_id,
        actor_user_id,
        session_id,
        input.scope,
        true,
    )
    .await
}

/// Current authorization precedes response-loss replay recovery. Workspace-wide
/// correlation uniqueness also catches accidental actor/target reuse.
pub async fn lookup_restored_revision(
    pool: &PgPool,
    workspace_id: Uuid,
    actor_user_id: Uuid,
    session_id: Uuid,
    input: RestoreRevisionInput,
) -> Result<Result<Option<RestoredRevision>, RevisionDbError>, sqlx::Error> {
    let mut tx = pool.begin().await?;
    set_tenant(&mut tx, workspace_id).await?;
    if let Err(err) =
        authorize_restore_in_tx(&mut tx, workspace_id, actor_user_id, session_id, input).await?
    {
        tx.rollback().await?;
        return Ok(Err(err));
    }
    let result =
        lookup_restored_revision_in_tx(&mut tx, workspace_id, actor_user_id, input).await?;
    tx.commit().await?;
    Ok(result)
}

pub(crate) async fn lookup_restored_revision_in_tx(
    tx: &mut Transaction<'_, Postgres>,
    workspace_id: Uuid,
    actor_user_id: Uuid,
    input: RestoreRevisionInput,
) -> Result<Result<Option<RestoredRevision>, RevisionDbError>, sqlx::Error> {
    let row: Option<RestoreReceiptRow> = sqlx::query_as(
        "SELECT id, target_kind, target_id, created_by, restored_from_id, restore_base_tail_seq, restore_committed_tail_seq FROM fvoci.revisions WHERE workspace_id = $1 AND restore_correlation_id = $2",
    )
    .bind(workspace_id).bind(input.correlation_id).fetch_optional(&mut **tx).await?;
    let Some((id, kind, target_id, actor, source, base, committed)) = row else {
        return Ok(Ok(None));
    };
    if !input.scope.target().matches(&kind, target_id)
        || actor != Some(actor_user_id)
        || source != Some(input.source_revision_id)
        || base != Some(input.expected_tail_seq)
    {
        return Ok(Err(RevisionDbError::RestoreConflict));
    }
    let Some(committed_tail_seq) = committed else {
        return Ok(Err(RevisionDbError::RestoreConflict));
    };
    Ok(Ok(Some(RestoredRevision {
        revision_id: id,
        committed_tail_seq,
    })))
}

pub async fn load_persisted_target_source(
    pool: &PgPool,
    workspace: Uuid,
    actor: Uuid,
    credential: Uuid,
    scope: impl Into<RevisionScope>,
) -> Result<Result<PersistedCollabSource, RevisionDbError>, sqlx::Error> {
    load_persisted_target_source_backend(
        &Backend::Postgres(pool.clone()),
        workspace,
        actor,
        credential,
        scope,
    )
    .await
}

pub async fn load_persisted_target_source_backend(
    backend: &Backend,
    workspace: Uuid,
    actor: Uuid,
    credential: Uuid,
    scope: impl Into<RevisionScope>,
) -> Result<Result<PersistedCollabSource, RevisionDbError>, sqlx::Error> {
    let scope = scope.into();
    let target = scope.target;
    let mut tx = backend.begin_write().await?;
    tx.operation().set_tenant(workspace).await?;
    if let Err(error) = tx
        .operation()
        .authorize_revision_scope(workspace, actor, credential, scope, true)
        .await?
    {
        tx.rollback().await?;
        return Ok(Err(error));
    }
    let kind = match target {
        RevisionTarget::Document(_) => crate::db::collab::CollabKind::Document,
        RevisionTarget::Task(_) => crate::db::collab::CollabKind::Task,
    };
    let native = tx
        .operation()
        .load_durable_native_source(kind, workspace, target.id())
        .await?;
    let native = match native {
        Ok(native) => native,
        Err(crate::db::collab::CollabDbError::NotFound) => {
            tx.rollback().await?;
            return Ok(Err(RevisionDbError::NotFound));
        }
        Err(_) => {
            tx.rollback().await?;
            return Err(sqlx::Error::Protocol(
                "persisted revision source exceeds native load bounds".into(),
            ));
        }
    };
    tx.commit().await.map_err(|unknown| unknown.source)?;
    Ok(Ok(PersistedCollabSource {
        snapshot: native.snapshot,
        tail: native.tail.into_iter().map(|row| row.payload).collect(),
    }))
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ScheduledRevisionCursor {
    pub workspace_id: Uuid,
    /// `0` = document, `1` = task.
    pub target_kind: u8,
    pub target_id: Uuid,
}

#[derive(Debug, Clone)]
pub struct ScheduledRevisionCandidate {
    pub workspace_id: Uuid,
    pub target: RevisionTarget,
    pub writer_generation: i64,
    pub state_updated_at: DateTime<Utc>,
    pub anchor_at: DateTime<Utc>,
}

/// Exact source retained while the maintenance consumer owns its writer and
/// awaits native capture/comparison. The current head cannot be fabricated by
/// the caller from a candidate or a cached room fence.
pub(crate) struct ScheduledRevisionSource {
    pub(crate) durable: DurableCollabSnapshot,
    workspace: Uuid,
    target: RevisionTarget,
    generation: i64,
    cutoff: i64,
    tail: i64,
}

pub(crate) fn archive_native_instant(raw: &str) -> Result<DateTime<Utc>, sqlx::Error> {
    let instant = DateTime::parse_from_rfc3339(raw)
        .map_err(|error| sqlx::Error::Decode(Box::new(error)))?
        .with_timezone(&Utc);
    Cell::instant(instant)?;
    Ok(instant)
}

impl OperationTx<'_, '_> {
    pub(crate) async fn install_archived_revisions(
        &mut self,
        claim: &crate::db::import_jobs::ImportClaim,
        archive: &crate::native_archive::Archive,
    ) -> Result<(), crate::db::native_archive::NativeDbError> {
        for row in &archive.graph.revisions {
            let snapshot = archive.bytes(&row.snapshot_entry)?;
            let created = archive_native_instant(&row.created_at)?;
            if !self.hold_import_claim(claim).await? {
                return Err(crate::db::native_archive::NativeDbError::Fenced);
            }
            match self {
                Self::Postgres(tx) => {
                    sqlx::query("INSERT INTO fvoci.revisions(id,workspace_id,target_kind,target_id,y_snapshot,encoding,content_json,text,reason,created_by,created_at,restored_from_id,restore_correlation_id,restore_base_tail_seq,restore_committed_tail_seq) VALUES($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11,$12,$13,$14,$15)")
                        .bind(row.id).bind(claim.workspace_id).bind(&row.target_kind).bind(row.target_id).bind(snapshot).bind(row.encoding).bind(&row.content_json).bind(&row.text).bind(&row.reason).bind(row.created_by.map(|_|claim.created_by)).bind(created).bind(row.restored_from_id).bind(row.restore_correlation_id).bind(row.restore_base_tail_seq).bind(row.restore_committed_tail_seq).execute(&mut ***tx).await?;
                }
                Self::SqliteFamily(writer) => {
                    writer.require_writer()?;
                    writer.require_tenant(claim.workspace_id)?;
                    writer.execute("INSERT INTO revisions(id,workspace_id,target_kind,target_id,y_snapshot,encoding,content_json,text,reason,created_by,created_at,restored_from_id,restore_correlation_id,restore_base_tail_seq,restore_committed_tail_seq) VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13,?14,?15)",&[
                        Cell::uuid(row.id),Cell::uuid(claim.workspace_id),Cell::text(&row.target_kind),Cell::uuid(row.target_id),Cell::Blob(snapshot),Cell::Integer(i64::from(row.encoding)),Cell::json(&row.content_json)?,Cell::text(&row.text),Cell::text(&row.reason),Cell::optional_uuid(row.created_by.map(|_|claim.created_by)),Cell::instant(created)?,Cell::optional_uuid(row.restored_from_id),Cell::optional_uuid(row.restore_correlation_id),row.restore_base_tail_seq.map(Cell::Integer).unwrap_or(Cell::Null),row.restore_committed_tail_seq.map(Cell::Integer).unwrap_or(Cell::Null)
                    ]).await?;
                }
            }
        }
        Ok(())
    }

    /// Borrowed maintenance operations never acquire/finish transactions or
    /// grant business context. The Revisions consumer checks/renews its actual
    /// claim on this writer before work and checks it again before COMMIT.
    pub(crate) async fn list_revision_live_workspace_ids(
        &mut self,
        after: Option<Uuid>,
        inclusive_after: bool,
        limit: i64,
    ) -> Result<Vec<Uuid>, sqlx::Error> {
        if limit <= 0 {
            return Err(sqlx::Error::Protocol(
                "revision page limit must be positive".into(),
            ));
        }
        match self {
            Self::Postgres(tx) => {
                let rows: Vec<(Uuid,)> = match after {
                    Some(after) => {
                        let statement = if inclusive_after {
                            "SELECT id FROM fvoci.workspaces WHERE deleted_at IS NULL AND id >= $1 ORDER BY id LIMIT $2"
                        } else {
                            "SELECT id FROM fvoci.workspaces WHERE deleted_at IS NULL AND id > $1 ORDER BY id LIMIT $2"
                        };
                        sqlx::query_as(statement).bind(after).bind(limit).fetch_all(&mut ***tx).await?
                    }
                    None => sqlx::query_as("SELECT id FROM fvoci.workspaces WHERE deleted_at IS NULL ORDER BY id LIMIT $1")
                        .bind(limit).fetch_all(&mut ***tx).await?,
                };
                Ok(rows.into_iter().map(|(id,)| id).collect())
            }
            Self::SqliteFamily(tx) => {
                tx.require_writer()?;
                tx.require_system_context()?;
                let (statement, args) = match after {
                    Some(after) => (
                        if inclusive_after {
                            "SELECT id FROM workspaces WHERE deleted_at IS NULL AND id >= ?1 ORDER BY id LIMIT ?2"
                        } else {
                            "SELECT id FROM workspaces WHERE deleted_at IS NULL AND id > ?1 ORDER BY id LIMIT ?2"
                        },
                        vec![Cell::uuid(after), Cell::Integer(limit)],
                    ),
                    None => (
                        "SELECT id FROM workspaces WHERE deleted_at IS NULL ORDER BY id LIMIT ?1",
                        vec![Cell::Integer(limit)],
                    ),
                };
                tx.query(statement, &args)
                    .await?
                    .iter()
                    .map(|row| row.cell(0)?.id())
                    .collect()
            }
        }
    }

    pub(crate) async fn list_scheduled_revision_candidates(
        &mut self,
        workspace: Uuid,
        after: Option<ScheduledRevisionCursor>,
        limit: i64,
    ) -> Result<Vec<ScheduledRevisionCandidate>, sqlx::Error> {
        if limit <= 0 || after.is_some_and(|c| c.workspace_id != workspace || c.target_kind > 1) {
            return Err(sqlx::Error::Protocol(
                "invalid revision cursor/page scope".into(),
            ));
        }
        if let Self::SqliteFamily(tx) = self {
            tx.require_writer()?;
            tx.require_tenant(workspace)?;
        }
        let mut out = Vec::new();
        for (ordinal, target_kind) in [(0, CollabKind::Document), (1, CollabKind::Task)] {
            if after.is_some_and(|cursor| cursor.target_kind > ordinal) {
                continue;
            }
            let target_after = after
                .filter(|cursor| cursor.target_kind == ordinal)
                .map(|cursor| cursor.target_id);
            let remaining = limit
                - i64::try_from(out.len())
                    .map_err(|_| sqlx::Error::Protocol("revision page overflow".into()))?;
            if remaining == 0 {
                break;
            }
            let rows: Vec<ScheduledRevisionListingRow> = match self {
                Self::Postgres(tx) => {
                    let statement = match target_kind {
                        CollabKind::Document => "SELECT s.document_id,s.updated_at,s.writer_generation,s.created_at,(SELECT r.created_at FROM fvoci.revisions r WHERE r.workspace_id=s.workspace_id AND r.target_kind='document' AND r.target_id=s.document_id ORDER BY r.created_at DESC,r.id DESC LIMIT 1) FROM fvoci.document_states s INNER JOIN fvoci.documents d ON d.workspace_id=s.workspace_id AND d.id=s.document_id AND d.deleted_at IS NULL INNER JOIN fvoci.workspaces w ON w.id=s.workspace_id AND w.deleted_at IS NULL WHERE s.workspace_id=$1 AND ($2::uuid IS NULL OR s.document_id>$2) ORDER BY s.document_id LIMIT $3",
                        CollabKind::Task => "SELECT s.task_id,s.updated_at,s.writer_generation,s.created_at,(SELECT r.created_at FROM fvoci.revisions r WHERE r.workspace_id=s.workspace_id AND r.target_kind='task' AND r.target_id=s.task_id ORDER BY r.created_at DESC,r.id DESC LIMIT 1) FROM fvoci.task_states s INNER JOIN fvoci.tasks t ON t.workspace_id=s.workspace_id AND t.id=s.task_id AND t.deleted_at IS NULL INNER JOIN fvoci.workspaces w ON w.id=s.workspace_id AND w.deleted_at IS NULL WHERE s.workspace_id=$1 AND ($2::uuid IS NULL OR s.task_id>$2) ORDER BY s.task_id LIMIT $3",
                    };
                    sqlx::query_as(statement)
                        .bind(workspace)
                        .bind(target_after)
                        .bind(remaining)
                        .fetch_all(&mut ***tx)
                        .await?
                }
                Self::SqliteFamily(tx) => {
                    let statement = match target_kind {
                        CollabKind::Document => "SELECT s.document_id,s.updated_at,s.writer_generation,s.created_at,(SELECT r.created_at FROM revisions r WHERE r.workspace_id=s.workspace_id AND r.target_kind='document' AND r.target_id=s.document_id ORDER BY r.created_at DESC,r.id DESC LIMIT 1) FROM document_states s INNER JOIN documents d ON d.workspace_id=s.workspace_id AND d.id=s.document_id AND d.deleted_at IS NULL INNER JOIN workspaces w ON w.id=s.workspace_id AND w.deleted_at IS NULL WHERE s.workspace_id=?1 AND (?2 IS NULL OR s.document_id>?2) ORDER BY s.document_id LIMIT ?3",
                        CollabKind::Task => "SELECT s.task_id,s.updated_at,s.writer_generation,s.created_at,(SELECT r.created_at FROM revisions r WHERE r.workspace_id=s.workspace_id AND r.target_kind='task' AND r.target_id=s.task_id ORDER BY r.created_at DESC,r.id DESC LIMIT 1) FROM task_states s INNER JOIN tasks t ON t.workspace_id=s.workspace_id AND t.id=s.task_id AND t.deleted_at IS NULL INNER JOIN workspaces w ON w.id=s.workspace_id AND w.deleted_at IS NULL WHERE s.workspace_id=?1 AND (?2 IS NULL OR s.task_id>?2) ORDER BY s.task_id LIMIT ?3",
                    };
                    tx.query(
                        statement,
                        &[
                            Cell::uuid(workspace),
                            Cell::optional_uuid(target_after),
                            Cell::Integer(remaining),
                        ],
                    )
                    .await?
                    .iter()
                    .map(|row| {
                        Ok((
                            row.cell(0)?.id()?,
                            row.cell(1)?.datetime()?,
                            row.cell(2)?.integer()?,
                            row.cell(3)?.datetime()?,
                            row.cell(4)?.optional(Cell::datetime)?,
                        ))
                    })
                    .collect::<Result<_, sqlx::Error>>()?
                }
            };
            out.extend(rows.into_iter().map(
                |(id, state_updated_at, writer_generation, created_at, last_rev_at)| {
                    ScheduledRevisionCandidate {
                        workspace_id: workspace,
                        target: match target_kind {
                            CollabKind::Document => RevisionTarget::Document(id),
                            CollabKind::Task => RevisionTarget::Task(id),
                        },
                        writer_generation,
                        state_updated_at,
                        anchor_at: last_rev_at.unwrap_or(created_at),
                    }
                },
            ));
        }
        Ok(out)
    }

    pub(crate) async fn load_scheduled_revision_source(
        &mut self,
        workspace: Uuid,
        target: RevisionTarget,
        expected_generation: i64,
    ) -> Result<Result<ScheduledRevisionSource, RevisionDbError>, sqlx::Error> {
        if let Err(error) = self
            .lock_system_revision_target(workspace, target, Some(expected_generation))
            .await?
        {
            return Ok(Err(error));
        }
        let kind = match target {
            RevisionTarget::Document(_) => CollabKind::Document,
            RevisionTarget::Task(_) => CollabKind::Task,
        };
        let load = match self
            .load_durable_native_source(kind, workspace, target.id())
            .await?
        {
            Ok(load) => load,
            Err(_) => return Ok(Err(RevisionDbError::NotFound)),
        };
        if load.writer_generation != expected_generation {
            return Ok(Err(RevisionDbError::NotFound));
        }
        Ok(Ok(ScheduledRevisionSource {
            workspace,
            target,
            generation: load.writer_generation,
            cutoff: load.snapshot_cutoff_seq,
            tail: load.tail_seq,
            durable: DurableCollabSnapshot {
                snapshot: load.snapshot,
                tail: load.tail.into_iter().map(|row| row.payload).collect(),
                tail_seq: load.tail_seq,
                snapshot_cutoff_seq: load.snapshot_cutoff_seq,
            },
        }))
    }

    pub(crate) async fn latest_scheduled_revision_head(
        &mut self,
        workspace: Uuid,
        target: RevisionTarget,
    ) -> Result<Option<(Uuid, Vec<u8>)>, sqlx::Error> {
        self.latest_revision_head(workspace, target).await
    }

    /// Required after native capture AND semantic-equal comparison. Returning
    /// a dedupe result still requires the caller's final current claim check.
    pub(crate) async fn check_scheduled_revision_source(
        &mut self,
        source: &ScheduledRevisionSource,
    ) -> Result<Result<(), RevisionDbError>, sqlx::Error> {
        if let Err(error) = self
            .lock_system_revision_target(source.workspace, source.target, Some(source.generation))
            .await?
        {
            return Ok(Err(error));
        }
        let kind = match source.target {
            RevisionTarget::Document(_) => CollabKind::Document,
            RevisionTarget::Task(_) => CollabKind::Task,
        };
        if self
            .durable_native_head(kind, source.workspace, source.target.id())
            .await?
            != Some((source.generation, source.cutoff, source.tail))
        {
            return Ok(Err(RevisionDbError::NotFound));
        }
        Ok(Ok(()))
    }

    pub(crate) async fn create_scheduled_revision(
        &mut self,
        source: &ScheduledRevisionSource,
        input: &CreateRevisionInput,
        head_fence: &SystemRevisionHead,
    ) -> Result<Result<Uuid, RevisionDbError>, sqlx::Error> {
        if let Err(error) = self.check_scheduled_revision_source(source).await? {
            return Ok(Err(error));
        }
        if input.reason != SCHEDULED_REASON {
            return Err(sqlx::Error::Protocol(
                "scheduled revision reason required".into(),
            ));
        }
        let recent = self
            .latest_revision_head(source.workspace, source.target)
            .await?;
        if !head_fence.matches_current(recent.clone()) {
            return Ok(Err(RevisionDbError::StaleRevisionHead));
        }
        let id = match recent.filter(|(_, bytes)| bytes == &input.y_snapshot) {
            Some((id, _)) => id,
            None => {
                let id = Uuid::now_v7();
                self.insert_revision(
                    source.workspace,
                    source.target,
                    id,
                    None,
                    input,
                    SCHEDULED_REASON,
                )
                .await?;
                id
            }
        };
        if let Err(error) = self.check_scheduled_revision_source(source).await? {
            return Ok(Err(error));
        }
        Ok(Ok(id))
    }

    pub(crate) async fn gc_revision_automatic_rows(
        &mut self,
        workspace: Uuid,
        keep: u32,
        batch: i32,
    ) -> Result<u32, sqlx::Error> {
        if batch <= 0 {
            return Err(sqlx::Error::Protocol(
                "revision GC batch must be positive".into(),
            ));
        }
        let deleted = match self {
            Self::Postgres(tx) => sqlx::query("WITH ranked AS (SELECT id,created_at,row_number() OVER (PARTITION BY target_kind,target_id ORDER BY created_at DESC,id DESC) AS rn FROM fvoci.revisions WHERE workspace_id=$1 AND reason IN ('session','scheduled')),doomed AS (SELECT id FROM ranked WHERE rn>$2 ORDER BY created_at,id LIMIT $3),locked AS (SELECT r.id,r.reason FROM fvoci.revisions r INNER JOIN doomed d ON d.id=r.id WHERE r.workspace_id=$1 FOR UPDATE OF r) DELETE FROM fvoci.revisions r USING locked l WHERE r.workspace_id=$1 AND r.id=l.id AND l.reason IN ('session','scheduled')")
                .bind(workspace).bind(i64::from(keep)).bind(batch).execute(&mut ***tx).await?.rows_affected(),
            Self::SqliteFamily(tx) => {
                tx.require_writer()?;
                tx.require_tenant(workspace)?;
                tx.execute("WITH ranked AS (SELECT id,created_at,row_number() OVER (PARTITION BY target_kind,target_id ORDER BY created_at DESC,id DESC) AS rn FROM revisions WHERE workspace_id=?1 AND reason IN ('session','scheduled')),doomed AS (SELECT id FROM ranked WHERE rn>?2 ORDER BY created_at,id LIMIT ?3) DELETE FROM revisions WHERE workspace_id=?1 AND reason IN ('session','scheduled') AND id IN (SELECT id FROM doomed)", &[Cell::uuid(workspace),Cell::Integer(i64::from(keep)),Cell::Integer(i64::from(batch))]).await?
            }
        };
        u32::try_from(deleted)
            .map_err(|_| sqlx::Error::Protocol("revision GC count exceeds u32".into()))
    }
}

type ScheduledRevisionListingRow = (
    Uuid,
    DateTime<Utc>,
    i64,
    DateTime<Utc>,
    Option<DateTime<Utc>>,
);

pub async fn list_live_workspace_ids_batch(
    pool: &PgPool,
    after: Option<Uuid>,
    inclusive_after: bool,
    limit: i64,
) -> Result<Vec<Uuid>, sqlx::Error> {
    let mut tx = pool.begin().await?;
    set_system(&mut tx).await?;
    let rows: Vec<(Uuid,)> = if let Some(after) = after {
        if inclusive_after {
            sqlx::query_as(
                r#"
                SELECT id
                FROM fvoci.workspaces
                WHERE deleted_at IS NULL AND id >= $1
                ORDER BY id ASC
                LIMIT $2
                "#,
            )
            .bind(after)
            .bind(limit)
            .fetch_all(&mut *tx)
            .await?
        } else {
            sqlx::query_as(
                r#"
                SELECT id
                FROM fvoci.workspaces
                WHERE deleted_at IS NULL AND id > $1
                ORDER BY id ASC
                LIMIT $2
                "#,
            )
            .bind(after)
            .bind(limit)
            .fetch_all(&mut *tx)
            .await?
        }
    } else {
        sqlx::query_as(
            r#"
            SELECT id
            FROM fvoci.workspaces
            WHERE deleted_at IS NULL
            ORDER BY id ASC
            LIMIT $1
            "#,
        )
        .bind(limit)
        .fetch_all(&mut *tx)
        .await?
    };
    tx.commit().await?;
    Ok(rows.into_iter().map(|(id,)| id).collect())
}

/// Collab targets in one workspace that may need a scheduled snapshot (predicate
/// applied in Rust: `anchor_at < cutoff` and `state_updated_at > anchor_at`).
pub async fn list_scheduled_revision_candidates_for_workspace(
    pool: &PgPool,
    workspace_id: Uuid,
    after: Option<ScheduledRevisionCursor>,
    limit: i64,
) -> Result<Vec<ScheduledRevisionCandidate>, sqlx::Error> {
    let mut tx = pool.begin().await?;
    set_tenant(&mut tx, workspace_id).await?;
    let mut out = Vec::new();
    let doc_after = match after {
        None => None,
        Some(ScheduledRevisionCursor {
            target_kind: 0,
            target_id,
            ..
        }) => Some(target_id),
        Some(ScheduledRevisionCursor { .. }) => None,
    };
    let doc_rows: Vec<ScheduledRevisionListingRow> = if after.is_none()
        || after.is_some_and(|c| c.target_kind == 0)
    {
        let sql = if doc_after.is_some() {
            r#"
                SELECT ds.document_id, ds.updated_at, ds.writer_generation, ds.created_at, lr.created_at
                FROM fvoci.document_states ds
                INNER JOIN fvoci.documents d
                    ON d.workspace_id = ds.workspace_id
                    AND d.id = ds.document_id
                    AND d.deleted_at IS NULL
                LEFT JOIN LATERAL (
                    SELECT r.created_at
                    FROM fvoci.revisions r
                    WHERE r.workspace_id = ds.workspace_id
                        AND r.target_kind = 'document'
                        AND r.target_id = ds.document_id
                    ORDER BY r.created_at DESC, r.id DESC
                    LIMIT 1
                ) lr ON TRUE
                WHERE ds.workspace_id = $1 AND ds.document_id > $2
                ORDER BY ds.document_id ASC
                LIMIT $3
                "#
        } else {
            r#"
                SELECT ds.document_id, ds.updated_at, ds.writer_generation, ds.created_at, lr.created_at
                FROM fvoci.document_states ds
                INNER JOIN fvoci.documents d
                    ON d.workspace_id = ds.workspace_id
                    AND d.id = ds.document_id
                    AND d.deleted_at IS NULL
                LEFT JOIN LATERAL (
                    SELECT r.created_at
                    FROM fvoci.revisions r
                    WHERE r.workspace_id = ds.workspace_id
                        AND r.target_kind = 'document'
                        AND r.target_id = ds.document_id
                    ORDER BY r.created_at DESC, r.id DESC
                    LIMIT 1
                ) lr ON TRUE
                WHERE ds.workspace_id = $1
                ORDER BY ds.document_id ASC
                LIMIT $2
                "#
        };
        if let Some(after_id) = doc_after {
            sqlx::query_as(sql)
                .bind(workspace_id)
                .bind(after_id)
                .bind(limit)
                .fetch_all(&mut *tx)
                .await?
        } else {
            sqlx::query_as(sql)
                .bind(workspace_id)
                .bind(limit)
                .fetch_all(&mut *tx)
                .await?
        }
    } else {
        Vec::new()
    };
    for (document_id, state_updated_at, writer_generation, created_at, last_rev_at) in doc_rows {
        let anchor_at = last_rev_at.unwrap_or(created_at);
        out.push(ScheduledRevisionCandidate {
            workspace_id,
            target: RevisionTarget::Document(document_id),
            writer_generation,
            state_updated_at,
            anchor_at,
        });
        if out.len() as i64 >= limit {
            tx.commit().await?;
            return Ok(out);
        }
    }

    let remaining = limit - out.len() as i64;
    if remaining > 0 {
        let task_after = match after {
            Some(ScheduledRevisionCursor {
                target_kind: 1,
                target_id,
                ..
            }) => Some(target_id),
            _ => None,
        };
        let task_sql = if task_after.is_some() {
            r#"
            SELECT ts.task_id, ts.updated_at, ts.writer_generation, ts.created_at, lr.created_at
            FROM fvoci.task_states ts
            INNER JOIN fvoci.tasks t
                ON t.workspace_id = ts.workspace_id
                AND t.id = ts.task_id
                AND t.deleted_at IS NULL
            LEFT JOIN LATERAL (
                SELECT r.created_at
                FROM fvoci.revisions r
                WHERE r.workspace_id = ts.workspace_id
                    AND r.target_kind = 'task'
                    AND r.target_id = ts.task_id
                ORDER BY r.created_at DESC, r.id DESC
                LIMIT 1
            ) lr ON TRUE
            WHERE ts.workspace_id = $1 AND ts.task_id > $2
            ORDER BY ts.task_id ASC
            LIMIT $3
            "#
        } else {
            r#"
            SELECT ts.task_id, ts.updated_at, ts.writer_generation, ts.created_at, lr.created_at
            FROM fvoci.task_states ts
            INNER JOIN fvoci.tasks t
                ON t.workspace_id = ts.workspace_id
                AND t.id = ts.task_id
                AND t.deleted_at IS NULL
            LEFT JOIN LATERAL (
                SELECT r.created_at
                FROM fvoci.revisions r
                WHERE r.workspace_id = ts.workspace_id
                    AND r.target_kind = 'task'
                    AND r.target_id = ts.task_id
                ORDER BY r.created_at DESC, r.id DESC
                LIMIT 1
            ) lr ON TRUE
            WHERE ts.workspace_id = $1
            ORDER BY ts.task_id ASC
            LIMIT $2
            "#
        };
        let task_rows: Vec<ScheduledRevisionListingRow> = if let Some(after_id) = task_after {
            sqlx::query_as(task_sql)
                .bind(workspace_id)
                .bind(after_id)
                .bind(remaining)
                .fetch_all(&mut *tx)
                .await?
        } else {
            sqlx::query_as(task_sql)
                .bind(workspace_id)
                .bind(remaining)
                .fetch_all(&mut *tx)
                .await?
        };
        for (task_id, state_updated_at, writer_generation, created_at, last_rev_at) in task_rows {
            let anchor_at = last_rev_at.unwrap_or(created_at);
            out.push(ScheduledRevisionCandidate {
                workspace_id,
                target: RevisionTarget::Task(task_id),
                writer_generation,
                state_updated_at,
                anchor_at,
            });
        }
    }
    tx.commit().await?;
    Ok(out)
}

pub fn scheduled_revision_cursor(
    candidate: &ScheduledRevisionCandidate,
) -> ScheduledRevisionCursor {
    ScheduledRevisionCursor {
        workspace_id: candidate.workspace_id,
        target_kind: match candidate.target {
            RevisionTarget::Document(_) => 0,
            RevisionTarget::Task(_) => 1,
        },
        target_id: candidate.target.id(),
    }
}

/// Delete oldest automatic revision rows beyond `keep` per target (manual never deleted).
pub async fn gc_automatic_revisions_batch(
    pool: &PgPool,
    workspace_id: Uuid,
    keep: u32,
    batch: i32,
) -> Result<u32, sqlx::Error> {
    let mut tx = pool.begin().await?;
    set_tenant(&mut tx, workspace_id).await?;
    let deleted = sqlx::query(
        r#"
        WITH ranked AS (
            SELECT id,
                created_at,
                row_number() OVER (
                    PARTITION BY target_kind, target_id
                    ORDER BY created_at DESC, id DESC
                ) AS rn
            FROM fvoci.revisions
            WHERE workspace_id = $1 AND reason IN ('session', 'scheduled')
        ),
        doomed AS (
            SELECT id
            FROM ranked
            WHERE rn > $2
            ORDER BY created_at ASC, id ASC
            LIMIT $3
        ),
        locked AS (
            SELECT r.id, r.reason
            FROM fvoci.revisions r
            INNER JOIN doomed d ON d.id = r.id
            WHERE r.workspace_id = $1
            FOR UPDATE OF r
        )
        DELETE FROM fvoci.revisions r
        USING locked l
        WHERE r.workspace_id = $1
          AND r.id = l.id
          AND l.reason IN ('session', 'scheduled')
        "#,
    )
    .bind(workspace_id)
    .bind(i64::from(keep))
    .bind(batch)
    .execute(&mut *tx)
    .await?
    .rows_affected();
    tx.commit().await?;
    Ok(deleted as u32)
}

#[cfg(all(test, feature = "db-tests"))]
mod selected_revision_preflight_finish_tests {
    use super::*;
    use crate::db::attachment_preview::tests::Fixture;
    use crate::db::backend::{CommitCleanupUnknown, CommitSettlement, RollbackCleanupUnknown};
    use crate::db::tasks::selected_task_detail_tests::setup;

    async fn authorize(
        f: &Fixture,
        credential: Uuid,
        task: Uuid,
        write: bool,
    ) -> Result<Result<(), RevisionDbError>, sqlx::Error> {
        authorize_revision_target_backend(
            &f.backend,
            f.workspace,
            f.user,
            credential,
            RevisionTarget::Task(task),
            write,
        )
        .await
    }

    // Observe actual persisted body/version and publication tables, rather than
    // treating the authorization return value as proof of no publication.
    async fn publication(f: &Fixture, task: Uuid) -> (String, i64, i64, i64, i64, i64) {
        sqlx::query_as("SELECT content_json,version,(SELECT count(*) FROM task_collab_updates WHERE task_id=?1),(SELECT count(*) FROM revisions WHERE target_kind='task' AND target_id=?1),(SELECT count(*) FROM events),(SELECT count(*) FROM audit_log) FROM tasks WHERE id=?1")
            .bind(task.as_bytes().as_slice())
            .fetch_one(&f.pool)
            .await
            .unwrap()
    }

    #[tokio::test]
    async fn wiki_aux_revision_preflight_current_refusals_decode_rollback_then_healthy() {
        let (f, credential, _, project, task) = setup().await;
        let before = publication(&f, task).await;
        assert_eq!(
            serde_json::from_str::<Value>(&before.0).unwrap(),
            crate::db::documents::empty_document_json()
        );
        assert_eq!(before.1, 1);
        assert_eq!(authorize(&f, credential, task, true).await.unwrap(), Ok(()));
        sqlx::query("UPDATE project_members SET role='viewer' WHERE project_id=?1 AND user_id=?2")
            .bind(project.as_bytes().as_slice())
            .bind(f.user.as_bytes().as_slice())
            .execute(&f.pool)
            .await
            .unwrap();
        assert_eq!(
            authorize(&f, credential, task, false).await.unwrap(),
            Ok(())
        );
        assert_eq!(
            authorize(&f, credential, task, true).await.unwrap(),
            Err(RevisionDbError::NotFound)
        );
        sqlx::query("UPDATE project_members SET role='lead' WHERE project_id=?1 AND user_id=?2")
            .bind(project.as_bytes().as_slice())
            .bind(f.user.as_bytes().as_slice())
            .execute(&f.pool)
            .await
            .unwrap();
        assert_eq!(
            authorize(&f, Uuid::now_v7(), task, true).await.unwrap(),
            Err(RevisionDbError::Forbidden)
        );
        sqlx::query("UPDATE sessions SET revoked_at=1 WHERE id=?1")
            .bind(credential.as_bytes().as_slice())
            .execute(&f.pool)
            .await
            .unwrap();
        assert_eq!(
            authorize(&f, credential, task, true).await.unwrap(),
            Err(RevisionDbError::Forbidden)
        );
        sqlx::query("UPDATE sessions SET revoked_at=NULL WHERE id=?1")
            .bind(credential.as_bytes().as_slice())
            .execute(&f.pool)
            .await
            .unwrap();
        assert_eq!(
            authorize_revision_target_backend(
                &f.backend,
                Uuid::now_v7(),
                f.user,
                credential,
                RevisionTarget::Task(task),
                true
            )
            .await
            .unwrap(),
            Err(RevisionDbError::NotFound)
        );
        sqlx::query("UPDATE tasks SET archived_at=1 WHERE id=?1")
            .bind(task.as_bytes().as_slice())
            .execute(&f.pool)
            .await
            .unwrap();
        assert_eq!(
            authorize(&f, credential, task, true).await.unwrap(),
            Err(RevisionDbError::TaskArchived)
        );
        assert_eq!(
            authorize(&f, credential, task, false).await.unwrap(),
            Ok(())
        );
        sqlx::query("UPDATE tasks SET archived_at=NULL WHERE id=?1")
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
            authorize(&f, credential, task, true).await.unwrap(),
            Err(RevisionDbError::ProjectArchived)
        );
        sqlx::query("UPDATE projects SET status='active' WHERE id=?1")
            .bind(project.as_bytes().as_slice())
            .execute(&f.pool)
            .await
            .unwrap();
        // A legal SQLite INTEGER outside Chrono's range causes the real public
        // authorizer to fail after BEGIN/set_tenant, before any native capture.
        sqlx::query("UPDATE tasks SET archived_at=?1 WHERE id=?2")
            .bind(i64::MAX)
            .bind(task.as_bytes().as_slice())
            .execute(&f.pool)
            .await
            .unwrap();
        let error = authorize(&f, credential, task, true).await.unwrap_err();
        assert!(
            matches!(error, sqlx::Error::Protocol(ref message) if message == "SQLite instant out of range")
        );
        // The actual repair writer must become usable after the returned rollback.
        let mut repair = f.backend.begin_write().await.unwrap();
        repair.operation().set_tenant(f.workspace).await.unwrap();
        let OperationTx::SqliteFamily(writer) = repair.operation() else {
            panic!("actual SQLite writer")
        };
        assert_eq!(
            writer
                .execute(
                    "UPDATE tasks SET archived_at=NULL WHERE id=?1",
                    &[Cell::uuid(task)]
                )
                .await
                .unwrap(),
            1
        );
        repair.commit_with_cleanup().await.unwrap();
        assert_eq!(authorize(&f, credential, task, true).await.unwrap(), Ok(()));
        assert_eq!(publication(&f, task).await, before);
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
    async fn wiki_aux_revision_preflight_actual_deferred_fk_commit_metadata_no_publish() {
        let (f, credential, _, _, task) = setup().await;
        let before = publication(&f, task).await;
        // Fixture-local fault table; the maintained product schema/FK policy is
        // unchanged. This fails the actual COMMIT, not an injected return value.
        sqlx::query("CREATE TABLE revision_finish_probe (workspace_id BLOB REFERENCES workspaces(id) DEFERRABLE INITIALLY DEFERRED) STRICT")
            .execute(&f.pool).await.unwrap();
        let mut tx = f.backend.begin_write().await.unwrap();
        let result = {
            let mut op = tx.operation();
            op.set_tenant(f.workspace).await.unwrap();
            op.authorize_revision_scope(
                f.workspace,
                f.user,
                credential,
                RevisionTarget::Task(task).into(),
                true,
            )
            .await
            .unwrap()
        };
        assert_eq!(result, Ok(()));
        let OperationTx::SqliteFamily(writer) = tx.operation() else {
            panic!("actual SQLite writer")
        };
        writer
            .execute(
                "INSERT INTO revision_finish_probe(workspace_id) VALUES(?1)",
                &[Cell::uuid(Uuid::now_v7())],
            )
            .await
            .unwrap();
        writer.execute("UPDATE tasks SET content_json='{\"type\":\"doc\",\"content\":[]}',version=99 WHERE id=?1", &[Cell::uuid(task)]).await.unwrap();
        let error = finish_revision_authorization(tx, Ok(result))
            .await
            .unwrap_err();
        let sqlx::Error::AnyDriverError(source) = error else {
            panic!("typed original commit uncertainty")
        };
        let unknown = source.downcast_ref::<CommitCleanupUnknown>().unwrap();
        assert!(unknown
            .source
            .source
            .as_database_error()
            .unwrap()
            .is_foreign_key_violation());
        assert_eq!(unknown.settlement, CommitSettlement::LocalWriterReconcile);
        assert!(unknown.cleanup_error.is_none());
        assert!(unknown.permits_reconciliation());
        // Local SQLx FIFO/Drop behavior is observed here. It is not proof of
        // original-stream cleanup on the pinned remote SDK or Turso.
        assert_eq!(publication(&f, task).await, before);
        assert_eq!(
            sqlx::query_scalar::<_, i64>("SELECT count(*) FROM revision_finish_probe")
                .fetch_one(&f.pool)
                .await
                .unwrap(),
            0
        );
        assert_eq!(authorize(&f, credential, task, true).await.unwrap(), Ok(()));
        f.close().await;
    }

    #[tokio::test]
    async fn wiki_aux_revision_preflight_actual_failed_rollback_retains_refusal_and_driver() {
        let (f, credential, _, _, task) = setup().await;
        let before = publication(&f, task).await;
        for driver_failure in [false, true] {
            // A separate real app connection confines the deliberately disturbed
            // SQLx transaction bookkeeping to this fault control.
            let pool = crate::db::pool::connect_sqlite_app(&f.path, 1)
                .await
                .unwrap();
            let backend = Backend::Sqlite(pool);
            let mut tx = backend.begin_write().await.unwrap();
            tx.operation().set_tenant(f.workspace).await.unwrap();
            let result = if driver_failure {
                let OperationTx::SqliteFamily(writer) = tx.operation() else {
                    panic!("actual SQLite writer")
                };
                Err(writer
                    .query("SELECT * FROM revision_finish_missing_table", &[])
                    .await
                    .unwrap_err())
            } else {
                let result = tx
                    .operation()
                    .authorize_revision_scope(
                        f.workspace,
                        f.user,
                        credential,
                        RevisionTarget::Task(Uuid::now_v7()).into(),
                        true,
                    )
                    .await
                    .unwrap();
                assert_eq!(result, Err(RevisionDbError::NotFound));
                Ok(result)
            };
            let OperationTx::SqliteFamily(writer) = tx.operation() else {
                panic!("actual SQLite writer")
            };
            // The driver still owns a Transaction. Ending its underlying SQL
            // transaction makes its awaited rollback fail with real SQLite data.
            writer.execute("ROLLBACK", &[]).await.unwrap();
            let error = finish_revision_authorization(tx, result).await.unwrap_err();
            let sqlx::Error::AnyDriverError(source) = error else {
                panic!("typed rollback uncertainty")
            };
            let unknown = source.downcast_ref::<RollbackCleanupUnknown>().unwrap();
            assert!(unknown
                .cleanup
                .as_database_error()
                .unwrap()
                .message()
                .contains("no transaction is active"));
            let original = unknown.original.as_ref().unwrap();
            if driver_failure {
                assert!(original
                    .downcast_ref::<sqlx::Error>()
                    .unwrap()
                    .as_database_error()
                    .unwrap()
                    .message()
                    .contains("revision_finish_missing_table"));
            } else {
                assert_eq!(
                    original
                        .downcast_ref::<RevisionAuthorizationRefusal>()
                        .unwrap()
                        .0,
                    RevisionDbError::NotFound
                );
            }
            // Do not reuse bookkeeping deliberately invalidated by this oracle.
            // Closing it and observing another local connection is not a remote
            // settlement receipt, and the returned result remains unknown.
            backend.close().await.unwrap();
            assert_eq!(authorize(&f, credential, task, true).await.unwrap(), Ok(()));
            assert_eq!(publication(&f, task).await, before);
        }
        f.close().await;
    }
}

#[cfg(test)]
mod maintenance_operation_tests {
    use super::*;
    use crate::db::maintenance_claim::{
        FamilyClaimAcquisition, FamilyMaintenanceClaimRequest, FamilyMaintenanceLeasePolicy,
        MaintenanceJobKey,
    };
    use std::time::Duration;
    use tokio_util::sync::CancellationToken;

    /// Actual app-connection/DDL checks for the borrowed operations. Native
    /// capture/semantic comparison and scheduler finish belong to the separate
    /// real consumer fixtures, not to this SQL source/retention oracle.
    #[tokio::test]
    async fn selected_revision_borrowed_writer_controls() {
        let root = std::env::temp_dir().join(format!("fvoci-revision-producer-{}", Uuid::now_v7()));
        std::fs::create_dir(&root).unwrap();
        let file = root.join("app.sqlite");
        crate::db::migrate::run_sqlite_migrations(&file)
            .await
            .unwrap();
        let pool = crate::db::pool::connect_sqlite_app(&file, 1).await.unwrap();
        let backend = Backend::Sqlite(pool.clone());
        crate::db::migrate::assert_sqlite_schema_current(&backend)
            .await
            .unwrap();
        assert_eq!(
            sqlx::query_scalar::<_, i64>("PRAGMA foreign_keys")
                .fetch_one(&pool)
                .await
                .unwrap(),
            1
        );
        let workspace = Uuid::from_u128(100);
        let other_workspace = Uuid::from_u128(200);
        let actor = Uuid::now_v7();
        sqlx::query("INSERT INTO users(id,email,given_name) VALUES(?1,'revision@fixture.invalid','Revision')")
            .bind(actor.as_bytes().as_slice()).execute(&pool).await.unwrap();
        for (id, slug) in [
            (workspace, "revision-one"),
            (other_workspace, "revision-two"),
        ] {
            sqlx::query("INSERT INTO workspaces(id,slug,name) VALUES(?1,?2,'Revision fixture')")
                .bind(id.as_bytes().as_slice())
                .bind(slug)
                .execute(&pool)
                .await
                .unwrap();
        }
        let documents = [Uuid::from_u128(20), Uuid::from_u128(10)];
        for (number, id) in documents.iter().enumerate() {
            sqlx::query("INSERT INTO documents(id,workspace_id,title,path,sort_key,number,status,schema_version,created_by,content_json) VALUES(?1,?2,'Revision document',?3,'V',?4,'draft',2,?5,?6)")
                .bind(id.as_bytes().as_slice()).bind(workspace.as_bytes().as_slice()).bind(id.simple().to_string()).bind(i64::try_from(number+1).unwrap()).bind(actor.as_bytes().as_slice()).bind(crate::db::documents::empty_document_json().to_string()).execute(&pool).await.unwrap();
            sqlx::query("INSERT INTO document_states(workspace_id,document_id,state,encoding,writer_generation,created_at,updated_at) VALUES(?1,?2,?3,1,7,1000000,2000000)")
                .bind(workspace.as_bytes().as_slice()).bind(id.as_bytes().as_slice()).bind([0_u8,0].as_slice()).execute(&pool).await.unwrap();
        }
        let project = Uuid::now_v7();
        let other_target = Uuid::now_v7();
        sqlx::query("INSERT INTO documents(id,workspace_id,title,path,sort_key,number,status,schema_version,created_by,content_json) VALUES(?1,?2,'Other tenant',?3,'V',1,'draft',2,?4,?5)")
            .bind(other_target.as_bytes().as_slice()).bind(other_workspace.as_bytes().as_slice()).bind(other_target.simple().to_string()).bind(actor.as_bytes().as_slice()).bind(crate::db::documents::empty_document_json().to_string()).execute(&pool).await.unwrap();
        sqlx::query("INSERT INTO document_states(workspace_id,document_id,state,encoding,writer_generation,created_at,updated_at) VALUES(?1,?2,?3,1,7,1000000,2000000)")
            .bind(other_workspace.as_bytes().as_slice()).bind(other_target.as_bytes().as_slice()).bind([0_u8,0].as_slice()).execute(&pool).await.unwrap();
        let workflow = Uuid::now_v7();
        let status = Uuid::now_v7();
        let task = Uuid::from_u128(1);
        sqlx::query("INSERT INTO projects(id,workspace_id,key,name,visibility,created_by) VALUES(?1,?2,'REV','Revision project','workspace',?3)")
            .bind(project.as_bytes().as_slice()).bind(workspace.as_bytes().as_slice()).bind(actor.as_bytes().as_slice()).execute(&pool).await.unwrap();
        sqlx::query("INSERT INTO workflows(id,workspace_id,project_id) VALUES(?1,?2,?3)")
            .bind(workflow.as_bytes().as_slice())
            .bind(workspace.as_bytes().as_slice())
            .bind(project.as_bytes().as_slice())
            .execute(&pool)
            .await
            .unwrap();
        sqlx::query("INSERT INTO statuses(id,workspace_id,project_id,workflow_id,name,category,sort_key) VALUES(?1,?2,?3,?4,'Todo','todo','V')")
            .bind(status.as_bytes().as_slice()).bind(workspace.as_bytes().as_slice()).bind(project.as_bytes().as_slice()).bind(workflow.as_bytes().as_slice()).execute(&pool).await.unwrap();
        sqlx::query("INSERT INTO tasks(id,workspace_id,project_id,number,title,status_id,content_json,created_by) VALUES(?1,?2,?3,1,'Revision task',?4,?5,?6)")
            .bind(task.as_bytes().as_slice()).bind(workspace.as_bytes().as_slice()).bind(project.as_bytes().as_slice()).bind(status.as_bytes().as_slice()).bind(crate::db::documents::empty_document_json().to_string()).bind(actor.as_bytes().as_slice()).execute(&pool).await.unwrap();
        sqlx::query("INSERT INTO task_states(workspace_id,task_id,state,writer_generation,created_at,updated_at) VALUES(?1,?2,?3,7,1000000,2000000)")
            .bind(workspace.as_bytes().as_slice()).bind(task.as_bytes().as_slice()).bind([0_u8,0].as_slice()).execute(&pool).await.unwrap();

        let cancel = CancellationToken::new();
        let policy =
            FamilyMaintenanceLeasePolicy::new(Duration::from_secs(300), Duration::from_secs(60))
                .unwrap();
        let request = FamilyMaintenanceClaimRequest::new(MaintenanceJobKey::Revisions);
        let FamilyClaimAcquisition::Acquired(claim) = request
            .try_acquire(&backend, policy, &cancel)
            .await
            .unwrap()
        else {
            panic!("current Revisions claim")
        };
        let mut read = backend.begin_read().await.unwrap();
        read.operation().set_tenant(workspace).await.unwrap();
        assert!(read
            .operation()
            .list_scheduled_revision_candidates(workspace, None, 16)
            .await
            .is_err());
        read.rollback().await.unwrap();
        let mut tx = backend.begin_write().await.unwrap();
        assert!(!tx
            .operation()
            .check_family_maintenance_claim(claim.proof(), MaintenanceJobKey::Daily)
            .await
            .unwrap());
        assert!(tx
            .operation()
            .list_revision_live_workspace_ids(None, false, 8)
            .await
            .is_err());
        let previous = tx.operation().set_system().await.unwrap();
        assert_eq!(
            tx.operation()
                .list_revision_live_workspace_ids(None, false, 8)
                .await
                .unwrap(),
            vec![workspace, other_workspace]
        );
        assert_eq!(
            tx.operation()
                .list_revision_live_workspace_ids(Some(workspace), true, 8)
                .await
                .unwrap(),
            vec![workspace, other_workspace]
        );
        assert_eq!(
            tx.operation()
                .list_revision_live_workspace_ids(Some(workspace), false, 8)
                .await
                .unwrap(),
            vec![other_workspace]
        );
        tx.operation().restore_system(previous).await.unwrap();
        tx.operation().set_tenant(workspace).await.unwrap();
        assert!(tx
            .operation()
            .check_family_maintenance_claim(claim.proof(), MaintenanceJobKey::Revisions)
            .await
            .unwrap());
        let page = tx
            .operation()
            .list_scheduled_revision_candidates(workspace, None, 2)
            .await
            .unwrap();
        assert_eq!(
            page.iter().map(|c| c.target).collect::<Vec<_>>(),
            vec![
                RevisionTarget::Document(documents[1]),
                RevisionTarget::Document(documents[0])
            ]
        );
        assert!(page.iter().all(|c| c.writer_generation == 7
            && c.anchor_at.timestamp_micros() == 1000000
            && c.state_updated_at.timestamp_micros() == 2000000));
        let after = scheduled_revision_cursor(page.last().unwrap());
        let tasks = tx
            .operation()
            .list_scheduled_revision_candidates(workspace, Some(after), 2)
            .await
            .unwrap();
        assert_eq!(tasks.len(), 1);
        assert_eq!(tasks[0].target, RevisionTarget::Task(task));
        assert!(tx
            .operation()
            .list_scheduled_revision_candidates(other_workspace, Some(after), 2)
            .await
            .is_err());
        assert!(tx
            .operation()
            .list_scheduled_revision_candidates(other_workspace, None, 2)
            .await
            .is_err());
        assert!(matches!(
            tx.operation()
                .load_scheduled_revision_source(workspace, page[0].target, 8)
                .await
                .unwrap(),
            Err(RevisionDbError::NotFound)
        ));
        let source = tx
            .operation()
            .load_scheduled_revision_source(workspace, page[0].target, 7)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(source.durable.snapshot, vec![0, 0]);
        assert!(source.durable.tail.is_empty());
        let input = CreateRevisionInput {
            y_snapshot: source.durable.snapshot.clone(),
            content_json: crate::db::documents::empty_document_json(),
            text: String::new(),
            reason: SCHEDULED_REASON.into(),
        };
        let original_head = SystemRevisionHead::from_latest(
            tx.operation()
                .latest_scheduled_revision_head(workspace, page[0].target)
                .await
                .unwrap(),
        );
        let revision = tx
            .operation()
            .create_scheduled_revision(&source, &input, &original_head)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(
            tx.operation()
                .create_scheduled_revision(&source, &input, &original_head)
                .await
                .unwrap(),
            Err(RevisionDbError::StaleRevisionHead)
        );
        let current_head = SystemRevisionHead::from_latest(
            tx.operation()
                .latest_scheduled_revision_head(workspace, page[0].target)
                .await
                .unwrap(),
        );
        assert_eq!(
            tx.operation()
                .create_scheduled_revision(&source, &input, &current_head)
                .await
                .unwrap(),
            Ok(revision)
        );
        let task_source = tx
            .operation()
            .load_scheduled_revision_source(workspace, RevisionTarget::Task(task), 7)
            .await
            .unwrap()
            .unwrap();
        let task_revision = tx
            .operation()
            .create_scheduled_revision(&task_source, &input, &SystemRevisionHead::from_latest(None))
            .await
            .unwrap()
            .unwrap();
        assert!(tx
            .operation()
            .check_family_maintenance_claim(claim.proof(), MaintenanceJobKey::Revisions)
            .await
            .unwrap());
        tx.commit_with_cleanup().await.unwrap();
        // Otherwise identical valid arguments reach the actual tenant guard
        // above; a fresh writer with the matching tenant succeeds.
        let mut other_tx = backend.begin_write().await.unwrap();
        other_tx
            .operation()
            .set_tenant(other_workspace)
            .await
            .unwrap();
        let other_candidates = other_tx
            .operation()
            .list_scheduled_revision_candidates(other_workspace, None, 2)
            .await
            .unwrap();
        assert_eq!(other_candidates.len(), 1);
        assert_eq!(
            other_candidates[0].target,
            RevisionTarget::Document(other_target)
        );
        other_tx.rollback().await.unwrap();
        let stored: Vec<(Vec<u8>, String, Option<Vec<u8>>)> =
            sqlx::query_as("SELECT id,reason,created_by FROM revisions ORDER BY id")
                .fetch_all(&pool)
                .await
                .unwrap();
        assert_eq!(stored.len(), 2);
        assert!(stored
            .iter()
            .all(|(_, reason, author)| reason == SCHEDULED_REASON && author.is_none()));
        assert!(stored.iter().any(|(id, _, _)| id == revision.as_bytes()));
        assert!(stored
            .iter()
            .any(|(id, _, _)| id == task_revision.as_bytes()));

        // Mutate each native proof component on the same actual reserved
        // writer. The source's immutable proof must reject publication even
        // though its bytes and revision head still compare equal.
        for statement in [
            "UPDATE document_states SET writer_generation=writer_generation+1 WHERE workspace_id=?1 AND document_id=?2",
            "UPDATE document_states SET tail_seq=tail_seq+1 WHERE workspace_id=?1 AND document_id=?2",
            "UPDATE document_states SET snapshot_cutoff_seq=snapshot_cutoff_seq+1 WHERE workspace_id=?1 AND document_id=?2",
        ] {
            let mut tx=backend.begin_write().await.unwrap();
            tx.operation().set_tenant(workspace).await.unwrap();
            let OperationTx::SqliteFamily(writer)=tx.operation() else {panic!("family writer")};
            writer.execute(statement,&[Cell::uuid(workspace),Cell::uuid(documents[1])]).await.unwrap();
            assert_eq!(tx.operation().check_scheduled_revision_source(&source).await.unwrap(),Err(RevisionDbError::NotFound));
            assert_eq!(tx.operation().create_scheduled_revision(&source,&input,&current_head).await.unwrap(),Err(RevisionDbError::NotFound));
            tx.rollback().await.unwrap();
        }
        let manual = Uuid::now_v7();
        let task_manual = Uuid::now_v7();
        let other_parent_revision = Uuid::now_v7();
        let other_tenant_revisions = [Uuid::now_v7(), Uuid::now_v7()];
        for (id, row_workspace, target_kind, target_id, reason, at) in [
            (manual, workspace, "document", documents[1], "manual", 1_i64),
            (
                Uuid::now_v7(),
                workspace,
                "document",
                documents[1],
                "session",
                2,
            ),
            (
                Uuid::now_v7(),
                workspace,
                "document",
                documents[1],
                "scheduled",
                3,
            ),
            (task_manual, workspace, "task", task, "manual", 1),
            (Uuid::now_v7(), workspace, "task", task, "session", 2),
            (Uuid::now_v7(), workspace, "task", task, "scheduled", 3),
            (
                other_parent_revision,
                workspace,
                "document",
                documents[0],
                "session",
                4,
            ),
            (
                other_tenant_revisions[0],
                other_workspace,
                "document",
                other_target,
                "session",
                2,
            ),
            (
                other_tenant_revisions[1],
                other_workspace,
                "document",
                other_target,
                "scheduled",
                3,
            ),
        ] {
            sqlx::query("INSERT INTO revisions(id,workspace_id,target_kind,target_id,y_snapshot,encoding,content_json,text,reason,created_at,created_by) VALUES(?1,?2,?3,?4,?5,1,?6,'',?7,?8,?9)")
                .bind(id.as_bytes().as_slice()).bind(row_workspace.as_bytes().as_slice()).bind(target_kind).bind(target_id.as_bytes().as_slice()).bind([0_u8,0].as_slice()).bind(crate::db::documents::empty_document_json().to_string()).bind(reason).bind(at).bind((reason=="manual").then(||actor.as_bytes().to_vec())).execute(&pool).await.unwrap();
        }
        let mut tx = backend.begin_write().await.unwrap();
        tx.operation().set_tenant(workspace).await.unwrap();
        assert!(tx
            .operation()
            .check_family_maintenance_claim(claim.proof(), MaintenanceJobKey::Revisions)
            .await
            .unwrap());
        assert_eq!(
            tx.operation()
                .gc_revision_automatic_rows(workspace, 1, 1)
                .await
                .unwrap(),
            1
        );
        tx.rollback().await.unwrap();
        assert_eq!(
            sqlx::query_scalar::<_, i64>("SELECT count(*) FROM revisions")
                .fetch_one(&pool)
                .await
                .unwrap(),
            11
        );
        let mut tx = backend.begin_write().await.unwrap();
        tx.operation().set_tenant(workspace).await.unwrap();
        assert_eq!(
            tx.operation()
                .gc_revision_automatic_rows(workspace, 1, 1)
                .await
                .unwrap(),
            1
        );
        assert_eq!(
            tx.operation()
                .gc_revision_automatic_rows(workspace, 1, 5000)
                .await
                .unwrap(),
            3
        );
        assert_eq!(
            tx.operation()
                .gc_revision_automatic_rows(workspace, 1, 5000)
                .await
                .unwrap(),
            0
        );
        assert!(tx
            .operation()
            .check_family_maintenance_claim(claim.proof(), MaintenanceJobKey::Revisions)
            .await
            .unwrap());
        tx.commit_with_cleanup().await.unwrap();
        let mut fresh = backend.begin_write().await.unwrap();
        fresh.operation().set_tenant(workspace).await.unwrap();
        assert_eq!(
            fresh
                .operation()
                .latest_scheduled_revision_head(workspace, RevisionTarget::Task(task))
                .await
                .unwrap()
                .unwrap()
                .0,
            task_revision
        );
        let OperationTx::SqliteFamily(writer) = fresh.operation() else {
            panic!("fresh actual writer")
        };
        let retained: Vec<Vec<u8>> = writer
            .query("SELECT id FROM revisions ORDER BY id", &[])
            .await
            .unwrap()
            .iter()
            .map(|row| row.cell(0).unwrap().bytes().unwrap())
            .collect();
        assert_eq!(retained.len(), 7);
        for id in [
            manual,
            revision,
            task_manual,
            task_revision,
            other_parent_revision,
            other_tenant_revisions[0],
            other_tenant_revisions[1],
        ] {
            assert!(retained.iter().any(|bytes| bytes == id.as_bytes()));
        }
        fresh.rollback().await.unwrap();
        claim.release().await.unwrap();
        backend.close().await.unwrap();
        std::fs::remove_dir_all(root).unwrap();
    }
}
