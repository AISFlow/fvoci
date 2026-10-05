//! Boot-OFF wiki saves: native history, current authority, CAS, revision,
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
    pub document: Uuid,
    pub actor: Uuid,
    pub credential: Uuid,
    pub command: Uuid,
    pub expected_tail: i64,
    pub update: Vec<u8>,
    pub client_ip: Option<String>,
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

pub async fn read_off_wiki_body(
    backend: &Backend,
    mode: RealtimeMode,
    engine: CollabConfig,
    workspace: Uuid,
    document: Uuid,
    actor: Uuid,
    credential: Uuid,
) -> Result<OffBodySource, BodySaveError> {
    let mut tx = backend.begin_off_body().await?;
    let result = async {
        let mut op = tx.operation();
        op.set_tenant(workspace).await?;
        op.authorize_revision_scope(
            workspace,
            actor,
            credential,
            RevisionTarget::Document(document).into(),
            false,
        )
        .await?
        .map_err(BodySaveError::Revision)?;
        let native = op
            .load_off_body_read(
                mode,
                CollabKind::Document,
                workspace,
                actor,
                credential,
                document,
            )
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
        op.authorize_revision_scope(
            workspace,
            actor,
            credential,
            RevisionTarget::Document(document).into(),
            false,
        )
        .await?
        .map_err(BodySaveError::Revision)?;
        if !op.recheck_session(actor, credential).await? {
            return Err(BodySaveError::Native(CollabDbError::Forbidden));
        }
        let writable = op
            .authorize_revision_scope(
                workspace,
                actor,
                credential,
                RevisionTarget::Document(document).into(),
                true,
            )
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

pub async fn save_off_wiki_body(
    backend: &Backend,
    mode: RealtimeMode,
    engine: CollabConfig,
    request: OffBodyRequest,
) -> Result<SavedBody, BodySaveError> {
    if mode != RealtimeMode::Off
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
    hash.update(b"fvoci:off-wiki-body:v1\0");
    for id in [
        request.workspace,
        request.document,
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
        document,
        actor,
        credential,
        command,
        expected_tail,
        update,
        client_ip,
    } = request;
    op.set_tenant(*workspace).await?;
    let (proof, load) = op
        .load_off_body_writer(
            mode,
            CollabKind::Document,
            *workspace,
            *actor,
            *credential,
            *document,
        )
        .await?
        .map_err(BodySaveError::Native)?;
    // Wiki-only route cannot name a project document, even if collab access is allowed.
    op.authorize_revision_scope(
        *workspace,
        *actor,
        *credential,
        RevisionTarget::Document(*document).into(),
        true,
    )
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
            || saved.target_id != *document
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
    let seq = op
        .append_off_body(
            proof,
            AppendCollabInput {
                workspace_id: *workspace,
                actor_user_id: *actor,
                session_id: *credential,
                document_id: *document,
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
    op.project_off_body(
        proof,
        ProjectDerivedBodyInput::new(
            *workspace,
            *actor,
            *credential,
            *document,
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
            RevisionScope::from(RevisionTarget::Document(*document)),
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
        target_id: *document,
        tail_seq: seq.to_string(),
        revision_id: revision,
    };
    if !op.insert_body_save_receipt(request, &hash, &saved).await? {
        return Err(BodySaveError::RequestMismatch);
    }
    // Current credential/target authorization is still locked and rechecked
    // after all native awaits, including immediately before caller-owned finish.
    op.authorize_revision_scope(
        *workspace,
        *actor,
        *credential,
        RevisionTarget::Document(*document).into(),
        true,
    )
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
        match self {
            Self::Postgres(tx) => {
                let count=sqlx::query("INSERT INTO fvoci.body_save_commands(workspace_id,command_id,actor_user_id,credential_id,target_kind,target_id,document_id,expected_tail_seq,committed_tail_seq,request_hash,payload_hash,result_json) VALUES($1,$2,$3,$4,'document',$5,$5,$6,$7,$8,$9,$10) ON CONFLICT(workspace_id,command_id) DO NOTHING")
                    .bind(request.workspace).bind(request.command).bind(request.actor).bind(request.credential).bind(request.document)
                    .bind(request.expected_tail).bind(committed).bind(hash).bind(payload_hash).bind(result).execute(&mut ***tx).await?.rows_affected();
                Ok(count == 1)
            }
            Self::SqliteFamily(tx) => {
                tx.require_writer()?;
                tx.require_tenant(request.workspace)?;
                let count=tx.execute("INSERT INTO body_save_commands(workspace_id,command_id,actor_user_id,credential_id,target_kind,target_id,document_id,expected_tail_seq,committed_tail_seq,request_hash,payload_hash,result_json) VALUES(?1,?2,?3,?4,'document',?5,?5,?6,?7,?8,?9,?10) ON CONFLICT(workspace_id,command_id) DO NOTHING",
                    &[Cell::uuid(request.workspace),Cell::uuid(request.command),Cell::uuid(request.actor),Cell::uuid(request.credential),Cell::uuid(request.document),Cell::Integer(request.expected_tail),Cell::Integer(committed),Cell::text(hash),Cell::Blob(payload_hash),Cell::json(&result)?]).await?;
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
            document: Uuid::now_v7(),
            actor: Uuid::now_v7(),
            credential: Uuid::now_v7(),
            command: Uuid::now_v7(),
            expected_tail: 7,
            update: vec![0, 0],
            client_ip: None,
        };
        let original = command_hash(&request);
        let mutations: [fn(&mut OffBodyRequest); 6] = [
            |r| r.workspace = Uuid::now_v7(),
            |r| r.document = Uuid::now_v7(),
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
            document: f.document,
            actor: f.user,
            credential,
            command: Uuid::now_v7(),
            expected_tail: 0,
            update,
            client_ip: Some("127.0.0.1".into()),
        }
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
        let saved = save_off_wiki_body(&f.backend, RealtimeMode::Off, engine(), one.clone())
            .await
            .unwrap();
        assert_eq!(saved.command_id, one.command);
        assert_eq!(saved.tail_seq, "1");
        assert!(!saved.revision_id.is_nil());
        let before = counts(&f).await;
        assert_eq!(before, (1, 1, 1, 1));
        assert!(matches!(
            save_off_wiki_body(&f.backend, RealtimeMode::Off, engine(), two).await,
            Err(BodySaveError::Conflict)
        ));
        assert_eq!(counts(&f).await, before);
        let replay = save_off_wiki_body(&f.backend, RealtimeMode::Off, engine(), one.clone())
            .await
            .unwrap();
        assert_eq!(replay.revision_id, saved.revision_id);
        assert_eq!(counts(&f).await, before);
        let mut changed = one.clone();
        changed.update.push(0);
        assert!(matches!(
            save_off_wiki_body(&f.backend, RealtimeMode::Off, engine(), changed).await,
            Err(BodySaveError::RequestMismatch)
        ));
        assert_eq!(counts(&f).await, before);
        let reader = read_off_wiki_body(
            &f.backend,
            RealtimeMode::Off,
            engine(),
            f.workspace,
            f.document,
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
            save_off_wiki_body(&f.backend, RealtimeMode::Off, engine(), one).await,
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
            save_off_wiki_body(&f.backend, RealtimeMode::Off, engine(), input)
                .await
                .unwrap()
                .tail_seq,
            "1"
        );
        f.close().await;
    }
}
