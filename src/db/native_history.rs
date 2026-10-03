//! Locked, read-only inventory of one document/task's retained native history
//! inside a caller's transaction (no begin, no writes).
use serde_json::Value;
use sqlx::{Postgres, Row, Transaction};
use uuid::Uuid;

use crate::native_history::*;

#[derive(Debug, thiserror::Error)]
pub enum NativeDbError {
    #[error("native archive access denied")]
    Forbidden,
    #[error("native archive conflict")]
    Conflict,
    #[error("native archive lease lost")]
    Fenced,
    #[error(transparent)]
    Archive(#[from] ArchiveError),
    #[error(transparent)]
    Sql(#[from] sqlx::Error),
}

/// Canonical retained native history of one document/task as read under the
/// caller's locks, with the isolated engine's bound retained-reference report.
#[derive(Debug, Clone)]
pub struct NativeHistoryInventory {
    pub kind: crate::collab::wire::CollabKind,
    pub target_id: Uuid,
    pub snapshot_cutoff_seq: i64,
    pub tail_seq: i64,
    /// Retained tail (seq, op_id), contiguous after the cutoff.
    pub tail: Vec<(i64, Uuid)>,
    /// Every receipt (op_id, seq, actor), including compacted operations.
    pub receipts: Vec<(Uuid, i64, Uuid)>,
    /// Revision IDs replayed against this state, oldest first.
    pub revisions: Vec<Uuid>,
    /// Stable digest of the stored body, state, tail, receipts and revision
    /// snapshots/JSON (no workspace, actor or time): equal only for equal
    /// content, so a preview and its commit compare this.
    pub content_digest: String,
    /// Capture association of these bytes, actor and this TX's now(). It
    /// differs per transaction; association only, never a preview digest.
    pub binding: String,
    pub report: collab_engine::archive_history::NativeArchiveInventory,
}

impl NativeHistoryInventory {
    /// The caller's fixed-kind closure decides presence; everything the
    /// retained history cannot prove is a typed blocker before any effect.
    pub fn closure_blocker(
        &self,
        present: impl Fn(collab_engine::archive_history::ReferenceKind, Uuid) -> bool,
    ) -> Option<RetainedHistoryBlocker> {
        retained_closure_blocker(&self.report, present)
    }
}

/// Inventory the target's canonical state, tail, receipts and revisions
/// INSIDE the caller's transaction, after the caller has authorized the actor
/// and taken its tree/owner/session locks. The state row is locked here so no
/// writer can move the cut before the caller commits. No row is written.
///
/// `Ok(None)` means the target has never had native state, updates, receipts
/// or revisions (the stored body is then the whole body). The isolated child
/// runs on a blocking thread under `cancel` with the engine's own limits.
#[allow(clippy::too_many_arguments)]
pub async fn native_history_inventory(
    tx: &mut Transaction<'_, Postgres>,
    workspace: Uuid,
    actor: Uuid,
    kind: crate::collab::wire::CollabKind,
    target: Uuid,
    engine_bin: &std::path::Path,
    limits: collab_engine::Limits,
    cancel: &tokio_util::sync::CancellationToken,
) -> Result<Option<NativeHistoryInventory>, NativeDbError> {
    use crate::collab::wire::CollabKind;
    let (name, table, parent) = match kind {
        CollabKind::Document => ("document", "document", "documents"),
        CollabKind::Task => ("task", "task", "tasks"),
    };
    // Existence only: the body is read after the state and revision locks.
    let exists: bool = sqlx::query_scalar(&format!(
        "SELECT EXISTS(SELECT 1 FROM fvoci.{parent} WHERE workspace_id=$1 AND id=$2)"
    ))
    .bind(workspace)
    .bind(target)
    .fetch_one(&mut **tx)
    .await?;
    if !exists {
        return Err(NativeDbError::Forbidden);
    }
    let state = sqlx::query(&format!(
        "SELECT encoding,snapshot_cutoff_seq,tail_seq,octet_length(state)::bigint AS n FROM fvoci.{table}_states WHERE workspace_id=$1 AND {table}_id=$2 FOR UPDATE"
    ))
    .bind(workspace)
    .bind(target)
    .fetch_optional(&mut **tx)
    .await?;
    let Some(state) = state else {
        let orphaned: bool = sqlx::query_scalar(&format!(
            "SELECT EXISTS(SELECT 1 FROM fvoci.{table}_collab_updates WHERE workspace_id=$1 AND {table}_id=$2) OR EXISTS(SELECT 1 FROM fvoci.{table}_collab_op_receipts WHERE workspace_id=$1 AND {table}_id=$2) OR EXISTS(SELECT 1 FROM fvoci.revisions WHERE workspace_id=$1 AND target_kind=$3 AND target_id=$2)"
        ))
        .bind(workspace)
        .bind(target)
        .bind(name)
        .fetch_one(&mut **tx)
        .await?;
        if orphaned {
            return Err(
                ArchiveError::Unsupported("missing native state for captured body".into()).into(),
            );
        }
        return Ok(None);
    };
    let encoding: i16 = state.get("encoding");
    let cutoff: i64 = state.get("snapshot_cutoff_seq");
    let tail_seq: i64 = state.get("tail_seq");
    // Revision rows are locked before any size or payload read: automatic
    // revision GC deletes rows under its own row locks without the state/tree
    // lock, so only this locked set is measured, replayed and later moved.
    let revision_meta = sqlx::query(
        "SELECT id,octet_length(y_snapshot)::bigint AS n,(octet_length(content_json::text)+octet_length(text))::bigint AS j FROM fvoci.revisions WHERE workspace_id=$1 AND target_kind=$2 AND target_id=$3 ORDER BY created_at,id LIMIT 10001 FOR UPDATE",
    )
    .bind(workspace)
    .bind(name)
    .bind(target)
    .fetch_all(&mut **tx)
    .await?;
    // Sizes before bytes: the engine's load bound covers state plus tail, each
    // revision snapshot is loaded over the same state, and the parent holds the
    // stored body plus every revision JSON and text at once.
    let tail_bytes: i64 = sqlx::query_scalar(&format!(
        "SELECT COALESCE(sum(octet_length(payload)),0)::bigint FROM fvoci.{table}_collab_updates WHERE workspace_id=$1 AND {table}_id=$2 AND seq>$3 AND seq<=$4"
    ))
    .bind(workspace)
    .bind(target)
    .bind(cutoff)
    .bind(tail_seq)
    .fetch_one(&mut **tx)
    .await?;
    let body_json_bytes: i64 = sqlx::query_scalar(&format!(
        "SELECT octet_length(content_json::text)::bigint FROM fvoci.{parent} WHERE workspace_id=$1 AND id=$2"
    ))
    .bind(workspace)
    .bind(target)
    .fetch_one(&mut **tx)
    .await?;
    let revision_bytes: Vec<i64> = revision_meta.iter().map(|r| r.get("n")).collect();
    let json_bytes = revision_meta
        .iter()
        .map(|r| r.get::<i64, _>("j"))
        .fold(body_json_bytes, i64::saturating_add);
    let load_bytes = state.get::<i64, _>("n").saturating_add(tail_bytes);
    if load_bytes as u64 > limits.max_load_bytes
        || revision_meta.len() > MAX_ENTRIES
        || revision_bytes
            .iter()
            .any(|n| *n as u64 > limits.max_load_bytes)
        || revision_bytes
            .iter()
            .sum::<i64>()
            .saturating_add(load_bytes)
            > MAX_BYTES as i64
        || json_bytes > MAX_GRAPH_BYTES as i64
    {
        return Err(ArchiveError::Limit.into());
    }
    let revision_ids: Vec<Uuid> = revision_meta.iter().map(|r| r.get("id")).collect();
    let body: Value = sqlx::query_scalar(&format!(
        "SELECT content_json FROM fvoci.{parent} WHERE workspace_id=$1 AND id=$2"
    ))
    .bind(workspace)
    .bind(target)
    .fetch_one(&mut **tx)
    .await?;
    let snapshot: Vec<u8> = sqlx::query_scalar(&format!(
        "SELECT state FROM fvoci.{table}_states WHERE workspace_id=$1 AND {table}_id=$2"
    ))
    .bind(workspace)
    .bind(target)
    .fetch_one(&mut **tx)
    .await?;
    let tail_rows = sqlx::query(&format!(
        "SELECT seq,op_id,payload FROM fvoci.{table}_collab_updates WHERE workspace_id=$1 AND {table}_id=$2 AND seq>$3 AND seq<=$4 ORDER BY seq LIMIT 10001"
    ))
    .bind(workspace)
    .bind(target)
    .bind(cutoff)
    .bind(tail_seq)
    .fetch_all(&mut **tx)
    .await?;
    let receipt_rows = sqlx::query(&format!(
        "SELECT op_id,seq,payload_len,payload_sha256,actor_user_id FROM fvoci.{table}_collab_op_receipts WHERE workspace_id=$1 AND {table}_id=$2 ORDER BY seq,op_id LIMIT 10001"
    ))
    .bind(workspace)
    .bind(target)
    .fetch_all(&mut **tx)
    .await?;
    if tail_rows.len() > MAX_ENTRIES || receipt_rows.len() > MAX_ENTRIES {
        return Err(ArchiveError::Limit.into());
    }
    let tail: Vec<(i64, Uuid, Vec<u8>)> = tail_rows
        .iter()
        .map(|r| (r.get("seq"), r.get("op_id"), r.get("payload")))
        .collect();
    let receipts: Vec<(Uuid, i64, i64, Vec<u8>, Uuid)> = receipt_rows
        .iter()
        .map(|r| {
            (
                r.get("op_id"),
                r.get("seq"),
                r.get("payload_len"),
                r.get("payload_sha256"),
                r.get("actor_user_id"),
            )
        })
        .collect();
    check_canonical_history(cutoff, tail_seq, &tail, &receipts)?;
    let mut revision_rows = sqlx::query(
        "SELECT id,encoding,y_snapshot,content_json,reason,created_by,created_at,text,restored_from_id,restore_correlation_id,restore_base_tail_seq,restore_committed_tail_seq FROM fvoci.revisions WHERE workspace_id=$1 AND id=ANY($2)",
    )
    .bind(workspace)
    .bind(&revision_ids)
    .fetch_all(&mut **tx)
    .await?;
    if revision_rows.len() != revision_ids.len() {
        return Err(ArchiveError::Invalid("native history continuity".into()).into());
    }
    revision_rows.sort_by_key(|r| {
        let id: Uuid = r.get("id");
        revision_ids.iter().position(|locked| *locked == id)
    });
    let captured_at = sqlx::query_scalar::<_, chrono::DateTime<chrono::Utc>>("SELECT now()")
        .fetch_one(&mut **tx)
        .await?
        .to_rfc3339();
    let input = NativeTargetInput {
        kind: name.to_owned(),
        id: target,
        encoding,
        cutoff,
        tail_seq,
        snapshot,
        tail,
        body,
        revisions: revision_rows
            .iter()
            .map(|r| {
                (
                    r.get("id"),
                    r.get("encoding"),
                    r.get("y_snapshot"),
                    r.get("content_json"),
                )
            })
            .collect(),
    };
    let binding = target_binding(workspace, actor, &captured_at, &input);
    let revision_meta: Vec<RetainedRevisionMeta> = revision_rows
        .iter()
        .map(|r| RetainedRevisionMeta {
            reason: r.get("reason"),
            created_by: r.get("created_by"),
            created_at: r
                .get::<chrono::DateTime<chrono::Utc>, _>("created_at")
                .to_rfc3339_opts(chrono::SecondsFormat::Micros, true),
            text: r.get("text"),
            restored_from_id: r.get("restored_from_id"),
            restore_correlation_id: r.get("restore_correlation_id"),
            restore_base_tail_seq: r.get("restore_base_tail_seq"),
            restore_committed_tail_seq: r.get("restore_committed_tail_seq"),
        })
        .collect();
    let content_digest = target_content_digest(&input, &receipts, &revision_meta);
    let tail_ids: Vec<(i64, Uuid)> = input.tail.iter().map(|(seq, op, _)| (*seq, *op)).collect();
    let revision_ids: Vec<Uuid> = input.revisions.iter().map(|r| r.0).collect();
    let cancelled = cancel.child_token();
    let _stop_on_drop = cancelled.clone().drop_guard();
    let stop = cancelled.clone();
    let engine_bin = engine_bin.to_path_buf();
    let mut worker = tokio::task::spawn_blocking(move || {
        inspect_native_target(
            &input,
            &binding,
            &engine_bin,
            limits,
            &stop,
            &mut |report| check_report_bounds(report, &binding, &limits),
        )
    });
    let report = tokio::select! {
        result = &mut worker => result.map_err(|_| ArchiveError::Worker)??,
        () = cancel.cancelled() => {
            cancelled.cancel();
            let _ = worker.await;
            return Err(ArchiveError::Cancelled.into());
        }
    };
    Ok(Some(NativeHistoryInventory {
        kind,
        target_id: target,
        snapshot_cutoff_seq: cutoff,
        tail_seq,
        tail: tail_ids,
        receipts: receipts
            .iter()
            .map(|(op, seq, _, _, actor)| (*op, *seq, *actor))
            .collect(),
        revisions: revision_ids,
        content_digest,
        binding: report.binding.clone(),
        report,
    }))
}
