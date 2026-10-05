//! Boot-OFF wiki/project/task saves: native history, current authority, CAS, revision,
//! outbox and stable command result share one reserved writer and one finish.
use crate::collab::{
    revision::{capture_revision_offline, prepare_off_body, OffBodyPrepareError},
    CollabConfig,
};
use crate::config::RealtimeMode;
use crate::db::{
    backend::{Backend, CommitCleanupUnknown, OperationTx},
    codec::Cell,
    collab::{
        AppendCollabInput, CollabDbError, CollabKind, CollabLoadState, ProjectDerivedBodyInput,
    },
    revisions::{CreateRevisionInput, RevisionDbError, RevisionScope, RevisionTarget},
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
    if mode != RealtimeMode::Off
        || (matches!(request.target, RevisionTarget::Task(_)) && request.project.is_some())
        || request.expected_tail < 0
        || request.expected_tail == i64::MAX
        || request.command.is_nil()
        || request.update.is_empty()
        || request.update.len() > super::collab::MAX_COLLAB_UPDATE_BYTES
    {
        return Err(BodySaveError::Invalid);
    }
    let mut tx = backend.begin_off_body().await?;
    let cancelled = Arc::new(AtomicBool::new(false));
    let _cancel_on_drop = NativeCancel(cancelled.clone());
    let result = save_in_writer(&mut tx.operation(), mode, engine, &request, cancelled).await;
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

async fn save_in_writer(
    op: &mut OperationTx<'_, '_>,
    mode: RealtimeMode,
    engine: CollabConfig,
    request: &OffBodyRequest,
    cancelled: Arc<AtomicBool>,
) -> Result<SavedBody, BodySaveError> {
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
    let hash = command_hash(request);
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
        return Ok(saved);
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
            + update.len() as i64
            > super::collab::MAX_COLLAB_LOAD_BYTES;
    let generation = load.writer_generation;
    let snapshot = load.snapshot;
    let tail = load.tail.into_iter().map(|row| row.payload).collect();
    let payload = update.clone();
    let native = tokio::task::spawn_blocking(move || {
        prepare_off_body(
            engine.engine_bin,
            engine.limits,
            snapshot,
            tail,
            payload,
            compact_start,
            &cancelled,
        )
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
                payload: update,
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
    let revision = op
        .create_manual_revision(
            *workspace,
            *actor,
            *credential,
            request.scope(),
            CreateRevisionInput {
                y_snapshot: native.captured.y_snapshot,
                content_json: native.captured.content_json,
                text,
                reason: "manual".into(),
            },
        )
        .await?
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
    if !op.insert_body_save_receipt(request, &hash, &saved).await? {
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
    Ok(saved)
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
