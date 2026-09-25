//! Collaboration DB admission boundary.
//!
//! ## Caps (source-grounded final)
//! - `MAX_COLLAB_SNAPSHOT_BYTES` / `MAX_COLLAB_UPDATE_BYTES`: 8 MiB each (supersedes 1 MiB).
//! - `MAX_COLLAB_TAIL_UPDATES`: 64 tail rows.
//! - `MAX_COLLAB_LOAD_BYTES`: snapshot + tail combined 32 MiB (engine reload budget).
//! - Engine Load framed JSON cap 48 MiB (documented; DB refuses tails engine cannot reload).
//! - Op receipts are append-only identity rows (seq/actor/len/digest); history grows without
//!   automatic retention or a per-document receipt count cap. Memory/recovery stays bounded by
//!   the 32 MiB / 64-row tail load budget above.
//!
//! ## Public API (room actor consumes later; DB remains ACL/durable/fence authority)
//! - `claim_writer_and_load(pool, workspace_id, actor_user_id, session_id, document_id)`
//! - `load_collab_document(pool, workspace_id, actor_user_id, session_id, document_id)`
//! - `append_collab_update(pool, AppendCollabInput)` → `AppendCollabResult` | `CollabDbError`
//! - `lookup_collab_operation(pool, workspace_id, actor_user_id, session_id, document_id, op_id)`
//! - `verify_collab_operation(pool, VerifyCollabInput)` → length+digest+actor mismatch → `OpIdConflict`
//!
//! ## Op receipts (immutable identity, no raw payload)
//! Receipts retain `seq`, `actor_user_id`, `payload_len`, and `payload_sha256` only.
//! They do not store historical update bytes; callers cannot recover raw payload from a
//! receipt alone and must load the canonical snapshot plus tail for payload bytes.
//! - `compact_collab_snapshot(pool, CompactCollabInput)` — exact cutoff fence; receipts retained
//!
//! ## Lock order within one transaction (sorted when multiple user ids):
//! 1. `pg_advisory_xact_lock(1907006, lockKeyFromUuid(userId))` for each actor user
//! 2. `users` + `sessions` `FOR UPDATE` via `recheck_session`
//! 3. `memberships` `FOR UPDATE` via `membership_role_for_update`
//! 4. `documents` `FOR UPDATE` for the wiki document row
//! 5. `document_states` `FOR UPDATE`
//!
//! Empty-state seed only: `pg_advisory_xact_lock(1907004, lockKeyFromUuid(documentId))`.
//! Reserved for a future room-manager connection lifetime lock:
//! `pg_advisory_lock(1907007, lockKeyFromUuid(documentId))` — not acquired here.

use std::time::Instant;

use chrono::{DateTime, Utc};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use sqlx::{Connection, PgConnection, PgPool, Postgres, Transaction};
use uuid::Uuid;

use crate::collab::derived_body::PreparedDerivedBody;
use crate::db::context::{lock_key_from_uuid, set_tenant};
use crate::db::documents::{
    empty_document_json, lock_membership_users, membership_role_for_update, recheck_session,
    wiki_can_edit, workspace_is_live,
};
use crate::db::identity::{append_audit, append_event, AuditAppend, EventAppend};

pub use crate::collab::derived_body::DOCUMENT_MAX_BODY_BYTES;

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct CollabDbStageTimings {
    pub pool_wait_us: u64,
    pub advisory_lock_us: u64,
    pub row_lock_us: u64,
    pub stmt_us: u64,
    pub commit_us: u64,
}

pub const COLLAB_INIT_LOCK_NAMESPACE: i32 = 1_907_004;
/// Reserved for a future room-manager session lock held for the connection lifetime.
/// Init paths must not reuse this namespace/key pair.
pub const COLLAB_ROOM_SESSION_LOCK_NAMESPACE: i32 = 1_907_007;
pub const COLLAB_STATE_ENCODING_V1: i16 = 1;
pub const MAX_COLLAB_SNAPSHOT_BYTES: usize = 8 * 1024 * 1024;
pub const MAX_COLLAB_UPDATE_BYTES: usize = 8 * 1024 * 1024;
pub const MAX_COLLAB_TAIL_UPDATES: i64 = 64;
pub const MAX_COLLAB_LOAD_BYTES: i64 = 32 * 1024 * 1024;

/// Minimal fixed empty Yjs updateV1 bytes for the canonical empty Tiptap seed only.
/// This layer does not parse CRDT payloads.
const EMPTY_YJS_STATE_V1: &[u8] = &[0, 0];

fn payload_sha256(payload: &[u8]) -> Vec<u8> {
    Sha256::digest(payload).to_vec()
}

async fn tail_budget_allows_append(
    tx: &mut Transaction<'_, Postgres>,
    workspace_id: Uuid,
    document_id: Uuid,
    snapshot_cutoff_seq: i64,
    snapshot_len: i64,
    incoming_len: i64,
) -> Result<Result<(), CollabDbError>, sqlx::Error> {
    let stats: (i64, i64) = sqlx::query_as(
        r#"
        SELECT count(*)::bigint, coalesce(sum(octet_length(payload)), 0)::bigint
        FROM fvoci.document_collab_updates
        WHERE workspace_id = $1 AND document_id = $2 AND seq > $3
        "#,
    )
    .bind(workspace_id)
    .bind(document_id)
    .bind(snapshot_cutoff_seq)
    .fetch_one(&mut **tx)
    .await?;
    if stats.0 + 1 > MAX_COLLAB_TAIL_UPDATES {
        return Ok(Err(CollabDbError::StateBudgetExceeded));
    }
    if snapshot_len + stats.1 + incoming_len > MAX_COLLAB_LOAD_BYTES {
        return Ok(Err(CollabDbError::StateBudgetExceeded));
    }
    Ok(Ok(()))
}

fn load_budget_allows(
    snapshot_len: i64,
    tail_count: i64,
    tail_bytes: i64,
) -> Result<(), CollabDbError> {
    if tail_count > MAX_COLLAB_TAIL_UPDATES {
        return Err(CollabDbError::StateBudgetExceeded);
    }
    if snapshot_len + tail_bytes > MAX_COLLAB_LOAD_BYTES {
        return Err(CollabDbError::StateBudgetExceeded);
    }
    Ok(())
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CollabDbError {
    NotFound,
    Forbidden,
    StaleWriter,
    PayloadTooLarge,
    OpIdConflict,
    StaleCutoff,
    InvalidCutoff,
    StateBudgetExceeded,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CollabUpdateRow {
    pub seq: i64,
    pub op_id: Uuid,
    pub payload: Vec<u8>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CollabLoadState {
    pub snapshot: Vec<u8>,
    pub tail: Vec<CollabUpdateRow>,
    pub writer_generation: i64,
    pub snapshot_cutoff_seq: i64,
    pub tail_seq: i64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ClaimWriterResult {
    pub writer_generation: i64,
    pub load: CollabLoadState,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AppendCollabResult {
    Committed { seq: i64 },
    DuplicateAck { seq: i64 },
}

pub struct AppendCollabInput<'a> {
    pub workspace_id: Uuid,
    pub actor_user_id: Uuid,
    pub session_id: Uuid,
    pub document_id: Uuid,
    pub writer_generation: i64,
    pub expected_tail_seq: i64,
    pub op_id: Uuid,
    pub payload: &'a [u8],
    pub client_ip: Option<&'a str>,
}

pub struct CompactCollabInput<'a> {
    pub workspace_id: Uuid,
    pub actor_user_id: Uuid,
    pub session_id: Uuid,
    pub document_id: Uuid,
    pub writer_generation: i64,
    pub cutoff_seq: i64,
    pub expected_tail_seq: i64,
    pub new_snapshot: &'a [u8],
    pub client_ip: Option<&'a str>,
}

struct CollabAuditRecord<'a> {
    workspace_id: Uuid,
    actor_user_id: Uuid,
    document_id: Uuid,
    op_id: Uuid,
    seq: i64,
    writer_generation: i64,
    client_ip: Option<&'a str>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CollabOperationLookup {
    pub seq: i64,
    pub payload_len: i64,
    pub payload_sha256: Vec<u8>,
    pub actor_user_id: Uuid,
}

pub struct VerifyCollabInput<'a> {
    pub workspace_id: Uuid,
    pub actor_user_id: Uuid,
    pub session_id: Uuid,
    pub document_id: Uuid,
    pub op_id: Uuid,
    pub expected_payload_len: i64,
    pub expected_payload_sha256: &'a [u8],
    pub expected_actor_user_id: Uuid,
}

pub struct ProjectDerivedBodyInput {
    pub workspace_id: Uuid,
    pub actor_user_id: Uuid,
    pub session_id: Uuid,
    pub document_id: Uuid,
    pub writer_generation: i64,
    pub expected_tail_seq: i64,
    prepared: PreparedDerivedBody,
}

impl ProjectDerivedBodyInput {
    pub fn new(
        workspace_id: Uuid,
        actor_user_id: Uuid,
        session_id: Uuid,
        document_id: Uuid,
        writer_generation: i64,
        expected_tail_seq: i64,
        prepared: PreparedDerivedBody,
    ) -> Self {
        Self {
            workspace_id,
            actor_user_id,
            session_id,
            document_id,
            writer_generation,
            expected_tail_seq,
            prepared,
        }
    }

    pub fn prepared(&self) -> &PreparedDerivedBody {
        &self.prepared
    }

    pub fn into_parts(self) -> (Uuid, Uuid, Uuid, Uuid, i64, i64, PreparedDerivedBody) {
        (
            self.workspace_id,
            self.actor_user_id,
            self.session_id,
            self.document_id,
            self.writer_generation,
            self.expected_tail_seq,
            self.prepared,
        )
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ProjectDerivedBodyResult {
    Updated,
    Unchanged,
    /// Empty Yjs seed at `tail_seq == 0` must not overwrite the paragraph seed JSON.
    SkippedSeed,
}

type StateRow = (Vec<u8>, i16, i64, i64, i64, DateTime<Utc>);

async fn lock_collab_init(
    tx: &mut Transaction<'_, Postgres>,
    document_id: Uuid,
) -> Result<(), sqlx::Error> {
    sqlx::query("SELECT pg_advisory_xact_lock($1, $2)")
        .bind(COLLAB_INIT_LOCK_NAMESPACE)
        .bind(lock_key_from_uuid(document_id))
        .execute(&mut **tx)
        .await?;
    Ok(())
}

async fn lock_wiki_document_for_update(
    tx: &mut Transaction<'_, Postgres>,
    workspace_id: Uuid,
    document_id: Uuid,
) -> Result<Option<(Option<Uuid>, String, Option<DateTime<Utc>>)>, sqlx::Error> {
    sqlx::query_as(
        r#"
        SELECT project_id, status, deleted_at
        FROM fvoci.documents
        WHERE workspace_id = $1 AND id = $2
        FOR UPDATE
        "#,
    )
    .bind(workspace_id)
    .bind(document_id)
    .fetch_optional(&mut **tx)
    .await
}

async fn ensure_collab_state_row(
    tx: &mut Transaction<'_, Postgres>,
    workspace_id: Uuid,
    document_id: Uuid,
    content_json: &Value,
) -> Result<Result<(), CollabDbError>, sqlx::Error> {
    let existing: Option<(i64,)> = sqlx::query_as(
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
    if existing.is_some() {
        return Ok(Ok(()));
    }

    lock_collab_init(tx, document_id).await?;
    let existing: Option<(i64,)> = sqlx::query_as(
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
    if existing.is_some() {
        return Ok(Ok(()));
    }

    if content_json != &empty_document_json() {
        return Ok(Err(CollabDbError::NotFound));
    }

    sqlx::query(
        r#"
        INSERT INTO fvoci.document_states (
            workspace_id, document_id, state, encoding,
            writer_generation, snapshot_cutoff_seq, tail_seq
        ) VALUES ($1, $2, $3, $4, 0, 0, 0)
        "#,
    )
    .bind(workspace_id)
    .bind(document_id)
    .bind(EMPTY_YJS_STATE_V1)
    .bind(COLLAB_STATE_ENCODING_V1)
    .execute(&mut **tx)
    .await?;
    Ok(Ok(()))
}

async fn fetch_state_fence_for_update(
    tx: &mut Transaction<'_, Postgres>,
    workspace_id: Uuid,
    document_id: Uuid,
) -> Result<Option<(i64, i64)>, sqlx::Error> {
    sqlx::query_as(
        r#"
        SELECT writer_generation, tail_seq
        FROM fvoci.document_states
        WHERE workspace_id = $1 AND document_id = $2
        FOR UPDATE
        "#,
    )
    .bind(workspace_id)
    .bind(document_id)
    .fetch_optional(&mut **tx)
    .await
}

async fn fetch_state_for_update(
    tx: &mut Transaction<'_, Postgres>,
    workspace_id: Uuid,
    document_id: Uuid,
) -> Result<Option<StateRow>, sqlx::Error> {
    sqlx::query_as(
        r#"
        SELECT state, encoding, writer_generation, snapshot_cutoff_seq, tail_seq, updated_at
        FROM fvoci.document_states
        WHERE workspace_id = $1 AND document_id = $2
        FOR UPDATE
        "#,
    )
    .bind(workspace_id)
    .bind(document_id)
    .fetch_optional(&mut **tx)
    .await
}

async fn load_tail_updates(
    tx: &mut Transaction<'_, Postgres>,
    workspace_id: Uuid,
    document_id: Uuid,
    snapshot_cutoff_seq: i64,
    snapshot_len: i64,
) -> Result<Result<Vec<CollabUpdateRow>, CollabDbError>, sqlx::Error> {
    let stats: (i64, i64) = sqlx::query_as(
        r#"
        SELECT count(*)::bigint, coalesce(sum(octet_length(payload)), 0)::bigint
        FROM fvoci.document_collab_updates
        WHERE workspace_id = $1 AND document_id = $2 AND seq > $3
        "#,
    )
    .bind(workspace_id)
    .bind(document_id)
    .bind(snapshot_cutoff_seq)
    .fetch_one(&mut **tx)
    .await?;
    if let Err(err) = load_budget_allows(snapshot_len, stats.0, stats.1) {
        return Ok(Err(err));
    }
    let rows = sqlx::query_as::<_, (i64, Uuid, Vec<u8>)>(
        r#"
        SELECT seq, op_id, payload
        FROM fvoci.document_collab_updates
        WHERE workspace_id = $1
          AND document_id = $2
          AND seq > $3
        ORDER BY seq ASC
        LIMIT $4
        "#,
    )
    .bind(workspace_id)
    .bind(document_id)
    .bind(snapshot_cutoff_seq)
    .bind(MAX_COLLAB_TAIL_UPDATES)
    .fetch_all(&mut **tx)
    .await?;
    Ok(Ok(rows
        .into_iter()
        .map(|(seq, op_id, payload)| CollabUpdateRow {
            seq,
            op_id,
            payload,
        })
        .collect()))
}

fn state_row_to_load(row: StateRow, tail: Vec<CollabUpdateRow>) -> CollabLoadState {
    let (snapshot, _encoding, writer_generation, snapshot_cutoff_seq, tail_seq, _updated_at) = row;
    CollabLoadState {
        snapshot,
        tail,
        writer_generation,
        snapshot_cutoff_seq,
        tail_seq,
    }
}

async fn authorize_wiki_collab_write(
    tx: &mut Transaction<'_, Postgres>,
    workspace_id: Uuid,
    actor_user_id: Uuid,
    session_id: Uuid,
    document_id: Uuid,
) -> Result<Result<(Value,), CollabDbError>, sqlx::Error> {
    lock_membership_users(tx, &[actor_user_id]).await?;
    if !recheck_session(tx, actor_user_id, session_id).await? {
        return Ok(Err(CollabDbError::Forbidden));
    }
    if !workspace_is_live(tx, workspace_id).await? {
        return Ok(Err(CollabDbError::NotFound));
    }
    let role = membership_role_for_update(tx, workspace_id, actor_user_id).await?;
    if !wiki_can_edit(role) {
        return Ok(Err(CollabDbError::Forbidden));
    }
    let doc = lock_wiki_document_for_update(tx, workspace_id, document_id).await?;
    let Some((project_id, status, deleted_at)) = doc else {
        return Ok(Err(CollabDbError::NotFound));
    };
    if deleted_at.is_some() || project_id.is_some() {
        return Ok(Err(CollabDbError::NotFound));
    }
    if status == "archived" {
        return Ok(Err(CollabDbError::Forbidden));
    }
    let content: (Value,) = sqlx::query_as(
        "SELECT content_json FROM fvoci.documents WHERE workspace_id = $1 AND id = $2",
    )
    .bind(workspace_id)
    .bind(document_id)
    .fetch_one(&mut **tx)
    .await?;
    Ok(Ok(content))
}

async fn authorize_wiki_collab_read(
    tx: &mut Transaction<'_, Postgres>,
    workspace_id: Uuid,
    actor_user_id: Uuid,
    session_id: Uuid,
    document_id: Uuid,
) -> Result<Result<(), CollabDbError>, sqlx::Error> {
    lock_membership_users(tx, &[actor_user_id]).await?;
    if !recheck_session(tx, actor_user_id, session_id).await? {
        return Ok(Err(CollabDbError::Forbidden));
    }
    if !workspace_is_live(tx, workspace_id).await? {
        return Ok(Err(CollabDbError::NotFound));
    }
    let role = membership_role_for_update(tx, workspace_id, actor_user_id).await?;
    if !wiki_can_edit(role) {
        return Ok(Err(CollabDbError::NotFound));
    }
    let doc = lock_wiki_document_for_update(tx, workspace_id, document_id).await?;
    let Some((project_id, _status, deleted_at)) = doc else {
        return Ok(Err(CollabDbError::NotFound));
    };
    if deleted_at.is_some() || project_id.is_some() {
        return Ok(Err(CollabDbError::NotFound));
    }
    Ok(Ok(()))
}

async fn append_system_document_updated_event(
    tx: &mut Transaction<'_, Postgres>,
    workspace_id: Uuid,
    document_id: Uuid,
) -> Result<(), sqlx::Error> {
    let payload = json!({
        "documentId": document_id.to_string(),
        "collab": true,
    });
    sqlx::query(
        r#"
        INSERT INTO fvoci.events (
            id, workspace_id, actor_user_id, verb, target_type, target_id, payload, channel
        ) VALUES ($1, $2, NULL, 'document.updated', 'document', $3, $4, 'system')
        "#,
    )
    .bind(Uuid::now_v7())
    .bind(workspace_id)
    .bind(document_id)
    .bind(payload)
    .execute(&mut **tx)
    .await?;
    Ok(())
}

async fn record_collab_event_and_audit(
    tx: &mut Transaction<'_, Postgres>,
    record: CollabAuditRecord<'_>,
) -> Result<(), sqlx::Error> {
    let CollabAuditRecord {
        workspace_id,
        actor_user_id,
        document_id,
        op_id,
        seq,
        writer_generation,
        client_ip,
    } = record;
    let payload = json!({
        "documentId": document_id.to_string(),
        "opId": op_id.to_string(),
        "seq": seq,
        "writerGeneration": writer_generation,
    });
    append_event(
        tx,
        EventAppend {
            id: Uuid::now_v7(),
            workspace_id: Some(workspace_id),
            actor_user_id: Some(actor_user_id),
            verb: "document.collab_update_appended".to_string(),
            target_type: Some("document".to_string()),
            target_id: Some(document_id),
            payload: payload.clone(),
        },
    )
    .await?;
    append_audit(
        tx,
        AuditAppend {
            id: Uuid::now_v7(),
            workspace_id: Some(workspace_id),
            actor_user_id: Some(actor_user_id),
            verb: "document.collab_update_appended".to_string(),
            target_type: Some("document".to_string()),
            target_id: Some(document_id),
            payload,
            ip: client_ip.map(str::to_string),
        },
    )
    .await?;
    Ok(())
}

pub async fn claim_writer_and_load(
    pool: &PgPool,
    workspace_id: Uuid,
    actor_user_id: Uuid,
    session_id: Uuid,
    document_id: Uuid,
) -> Result<Result<ClaimWriterResult, CollabDbError>, sqlx::Error> {
    let mut tx = pool.begin().await?;
    set_tenant(&mut tx, workspace_id).await?;
    let content = match authorize_wiki_collab_write(
        &mut tx,
        workspace_id,
        actor_user_id,
        session_id,
        document_id,
    )
    .await?
    {
        Ok(content) => content,
        Err(err) => {
            tx.rollback().await?;
            return Ok(Err(err));
        }
    };
    match ensure_collab_state_row(&mut tx, workspace_id, document_id, &content.0).await? {
        Ok(()) => {}
        Err(err) => {
            tx.rollback().await?;
            return Ok(Err(err));
        }
    }
    let bumped: Option<(i64,)> = sqlx::query_as(
        r#"
        UPDATE fvoci.document_states
        SET writer_generation = writer_generation + 1,
            updated_at = now()
        WHERE workspace_id = $1 AND document_id = $2
        RETURNING writer_generation
        "#,
    )
    .bind(workspace_id)
    .bind(document_id)
    .fetch_optional(&mut *tx)
    .await?;
    let Some((writer_generation,)) = bumped else {
        tx.rollback().await?;
        return Ok(Err(CollabDbError::NotFound));
    };
    let state = fetch_state_for_update(&mut tx, workspace_id, document_id).await?;
    let Some(state) = state else {
        tx.rollback().await?;
        return Ok(Err(CollabDbError::NotFound));
    };
    if state.1 != COLLAB_STATE_ENCODING_V1 {
        tx.rollback().await?;
        return Ok(Err(CollabDbError::NotFound));
    }
    let tail = match load_tail_updates(
        &mut tx,
        workspace_id,
        document_id,
        state.3,
        state.0.len() as i64,
    )
    .await?
    {
        Ok(tail) => tail,
        Err(err) => {
            tx.rollback().await?;
            return Ok(Err(err));
        }
    };
    let load = state_row_to_load(state, tail);
    tx.commit().await?;
    Ok(Ok(ClaimWriterResult {
        writer_generation,
        load,
    }))
}

pub async fn load_collab_document(
    pool: &PgPool,
    workspace_id: Uuid,
    actor_user_id: Uuid,
    session_id: Uuid,
    document_id: Uuid,
) -> Result<Result<CollabLoadState, CollabDbError>, sqlx::Error> {
    let mut tx = pool.begin().await?;
    set_tenant(&mut tx, workspace_id).await?;
    let content = match authorize_wiki_collab_write(
        &mut tx,
        workspace_id,
        actor_user_id,
        session_id,
        document_id,
    )
    .await?
    {
        Ok(content) => content,
        Err(err) => {
            tx.rollback().await?;
            return Ok(Err(err));
        }
    };
    match ensure_collab_state_row(&mut tx, workspace_id, document_id, &content.0).await? {
        Ok(()) => {}
        Err(err) => {
            tx.rollback().await?;
            return Ok(Err(err));
        }
    }
    let state = fetch_state_for_update(&mut tx, workspace_id, document_id).await?;
    let Some(state) = state else {
        tx.rollback().await?;
        return Ok(Err(CollabDbError::NotFound));
    };
    if state.1 != COLLAB_STATE_ENCODING_V1 {
        tx.rollback().await?;
        return Ok(Err(CollabDbError::NotFound));
    }
    let tail = match load_tail_updates(
        &mut tx,
        workspace_id,
        document_id,
        state.3,
        state.0.len() as i64,
    )
    .await?
    {
        Ok(tail) => tail,
        Err(err) => {
            tx.rollback().await?;
            return Ok(Err(err));
        }
    };
    let load = state_row_to_load(state, tail);
    tx.commit().await?;
    Ok(Ok(load))
}

pub async fn append_collab_update(
    pool: &PgPool,
    input: AppendCollabInput<'_>,
) -> Result<Result<AppendCollabResult, CollabDbError>, sqlx::Error> {
    append_collab_update_timed(pool, input)
        .await
        .map(|(result, _)| result)
}

/// Same as [`append_collab_update`] with per-phase DB timings for `collab.stage` tracing.
pub async fn append_collab_update_timed(
    pool: &PgPool,
    input: AppendCollabInput<'_>,
) -> Result<
    (
        Result<AppendCollabResult, CollabDbError>,
        CollabDbStageTimings,
    ),
    sqlx::Error,
> {
    let mut timings = CollabDbStageTimings::default();
    if input.payload.is_empty() || input.payload.len() > MAX_COLLAB_UPDATE_BYTES {
        return Ok((Err(CollabDbError::PayloadTooLarge), timings));
    }
    let pool_started = Instant::now();
    let tx = pool.begin().await?;
    timings.pool_wait_us = pool_started.elapsed().as_micros() as u64;
    append_collab_update_in_tx(tx, input, timings).await
}

/// Append on a room's dedicated session connection (no pool acquire).
pub async fn append_collab_update_on_conn_timed(
    conn: &mut PgConnection,
    input: AppendCollabInput<'_>,
) -> Result<
    (
        Result<AppendCollabResult, CollabDbError>,
        CollabDbStageTimings,
    ),
    sqlx::Error,
> {
    let timings = CollabDbStageTimings::default();
    if input.payload.is_empty() || input.payload.len() > MAX_COLLAB_UPDATE_BYTES {
        return Ok((Err(CollabDbError::PayloadTooLarge), timings));
    }
    let tx = conn.begin().await?;
    append_collab_update_in_tx(tx, input, timings).await
}

async fn append_collab_update_in_tx(
    mut tx: Transaction<'_, Postgres>,
    input: AppendCollabInput<'_>,
    mut timings: CollabDbStageTimings,
) -> Result<
    (
        Result<AppendCollabResult, CollabDbError>,
        CollabDbStageTimings,
    ),
    sqlx::Error,
> {
    let AppendCollabInput {
        workspace_id,
        actor_user_id,
        session_id,
        document_id,
        writer_generation,
        expected_tail_seq,
        op_id,
        payload,
        client_ip,
    } = input;
    set_tenant(&mut tx, workspace_id).await?;
    let advisory_started = Instant::now();
    lock_membership_users(&mut tx, &[actor_user_id]).await?;
    timings.advisory_lock_us = advisory_started.elapsed().as_micros() as u64;
    let row_started = Instant::now();
    if !recheck_session(&mut tx, actor_user_id, session_id).await? {
        tx.rollback().await?;
        return Ok((Err(CollabDbError::Forbidden), timings));
    }
    if !workspace_is_live(&mut tx, workspace_id).await? {
        tx.rollback().await?;
        return Ok((Err(CollabDbError::NotFound), timings));
    }
    let role = membership_role_for_update(&mut tx, workspace_id, actor_user_id).await?;
    if !wiki_can_edit(role) {
        tx.rollback().await?;
        return Ok((Err(CollabDbError::Forbidden), timings));
    }
    let doc = lock_wiki_document_for_update(&mut tx, workspace_id, document_id).await?;
    let Some((project_id, status, deleted_at)) = doc else {
        tx.rollback().await?;
        return Ok((Err(CollabDbError::NotFound), timings));
    };
    if deleted_at.is_some() || project_id.is_some() {
        tx.rollback().await?;
        return Ok((Err(CollabDbError::NotFound), timings));
    }
    if status == "archived" {
        tx.rollback().await?;
        return Ok((Err(CollabDbError::Forbidden), timings));
    }
    let content: (Value,) = sqlx::query_as(
        "SELECT content_json FROM fvoci.documents WHERE workspace_id = $1 AND id = $2",
    )
    .bind(workspace_id)
    .bind(document_id)
    .fetch_one(&mut *tx)
    .await?;
    match ensure_collab_state_row(&mut tx, workspace_id, document_id, &content.0).await? {
        Ok(()) => {}
        Err(err) => {
            tx.rollback().await?;
            return Ok((Err(err), timings));
        }
    }
    let state = fetch_state_for_update(&mut tx, workspace_id, document_id).await?;
    timings.row_lock_us = row_started.elapsed().as_micros() as u64;
    let stmt_started = Instant::now();
    let Some(state) = state else {
        tx.rollback().await?;
        return Ok((Err(CollabDbError::NotFound), timings));
    };
    if state.2 != writer_generation {
        tx.rollback().await?;
        return Ok((Err(CollabDbError::StaleWriter), timings));
    }

    let incoming_len = payload.len() as i64;
    let incoming_digest = payload_sha256(payload);
    let existing: Option<(i64, i64, Vec<u8>, Uuid)> = sqlx::query_as(
        r#"
        SELECT seq, payload_len, payload_sha256, actor_user_id
        FROM fvoci.document_collab_op_receipts
        WHERE workspace_id = $1 AND document_id = $2 AND op_id = $3
        "#,
    )
    .bind(workspace_id)
    .bind(document_id)
    .bind(op_id)
    .fetch_optional(&mut *tx)
    .await?;
    if let Some((seq, existing_len, existing_digest, existing_actor)) = existing {
        if existing_len == incoming_len
            && existing_digest.as_slice() == incoming_digest.as_slice()
            && existing_actor == actor_user_id
        {
            let commit_started = Instant::now();
            tx.commit().await?;
            timings.commit_us = commit_started.elapsed().as_micros() as u64;
            timings.stmt_us = stmt_started.elapsed().as_micros() as u64 - timings.commit_us;
            return Ok((Ok(AppendCollabResult::DuplicateAck { seq }), timings));
        }
        tx.rollback().await?;
        return Ok((Err(CollabDbError::OpIdConflict), timings));
    }
    if state.4 != expected_tail_seq {
        tx.rollback().await?;
        return Ok((Err(CollabDbError::StaleCutoff), timings));
    }

    match tail_budget_allows_append(
        &mut tx,
        workspace_id,
        document_id,
        state.3,
        state.0.len() as i64,
        payload.len() as i64,
    )
    .await?
    {
        Ok(()) => {}
        Err(err) => {
            tx.rollback().await?;
            return Ok((Err(err), timings));
        }
    }
    let next_seq: Option<(i64,)> = sqlx::query_as(
        r#"
        UPDATE fvoci.document_states
        SET tail_seq = tail_seq + 1, updated_at = now()
        WHERE workspace_id = $1
          AND document_id = $2
          AND writer_generation = $3
        RETURNING tail_seq
        "#,
    )
    .bind(workspace_id)
    .bind(document_id)
    .bind(writer_generation)
    .fetch_optional(&mut *tx)
    .await?;
    let Some((seq,)) = next_seq else {
        tx.rollback().await?;
        return Ok((Err(CollabDbError::StaleWriter), timings));
    };

    sqlx::query(
        r#"
        INSERT INTO fvoci.document_collab_updates (
            workspace_id, document_id, seq, op_id, payload
        ) VALUES ($1, $2, $3, $4, $5)
        "#,
    )
    .bind(workspace_id)
    .bind(document_id)
    .bind(seq)
    .bind(op_id)
    .bind(payload)
    .execute(&mut *tx)
    .await?;

    sqlx::query(
        r#"
        INSERT INTO fvoci.document_collab_op_receipts (
            workspace_id, document_id, op_id, seq, payload_len, payload_sha256, actor_user_id
        ) VALUES ($1, $2, $3, $4, $5, $6, $7)
        "#,
    )
    .bind(workspace_id)
    .bind(document_id)
    .bind(op_id)
    .bind(seq)
    .bind(incoming_len)
    .bind(&incoming_digest)
    .bind(actor_user_id)
    .execute(&mut *tx)
    .await?;

    record_collab_event_and_audit(
        &mut tx,
        CollabAuditRecord {
            workspace_id,
            actor_user_id,
            document_id,
            op_id,
            seq,
            writer_generation,
            client_ip,
        },
    )
    .await?;

    timings.stmt_us = stmt_started.elapsed().as_micros() as u64;
    let commit_started = Instant::now();
    tx.commit().await?;
    timings.commit_us = commit_started.elapsed().as_micros() as u64;
    Ok((Ok(AppendCollabResult::Committed { seq }), timings))
}

pub async fn lookup_collab_operation(
    pool: &PgPool,
    workspace_id: Uuid,
    actor_user_id: Uuid,
    session_id: Uuid,
    document_id: Uuid,
    op_id: Uuid,
) -> Result<Result<Option<CollabOperationLookup>, CollabDbError>, sqlx::Error> {
    let mut tx = pool.begin().await?;
    set_tenant(&mut tx, workspace_id).await?;
    match authorize_wiki_collab_read(
        &mut tx,
        workspace_id,
        actor_user_id,
        session_id,
        document_id,
    )
    .await?
    {
        Ok(()) => {}
        Err(err) => {
            tx.rollback().await?;
            return Ok(Err(err));
        }
    }
    let row: Option<(i64, i64, Vec<u8>, Uuid)> = sqlx::query_as(
        r#"
        SELECT seq, payload_len, payload_sha256, actor_user_id
        FROM fvoci.document_collab_op_receipts
        WHERE workspace_id = $1 AND document_id = $2 AND op_id = $3
        "#,
    )
    .bind(workspace_id)
    .bind(document_id)
    .bind(op_id)
    .fetch_optional(&mut *tx)
    .await?;
    tx.commit().await?;
    Ok(Ok(row.map(
        |(seq, payload_len, payload_sha256, actor_user_id)| CollabOperationLookup {
            seq,
            payload_len,
            payload_sha256,
            actor_user_id,
        },
    )))
}

pub async fn verify_collab_operation(
    pool: &PgPool,
    input: VerifyCollabInput<'_>,
) -> Result<Result<CollabOperationLookup, CollabDbError>, sqlx::Error> {
    let VerifyCollabInput {
        workspace_id,
        actor_user_id,
        session_id,
        document_id,
        op_id,
        expected_payload_len,
        expected_payload_sha256,
        expected_actor_user_id,
    } = input;
    let mut tx = pool.begin().await?;
    set_tenant(&mut tx, workspace_id).await?;
    match authorize_wiki_collab_read(
        &mut tx,
        workspace_id,
        actor_user_id,
        session_id,
        document_id,
    )
    .await?
    {
        Ok(()) => {}
        Err(err) => {
            tx.rollback().await?;
            return Ok(Err(err));
        }
    }
    let row: Option<(i64, i64, Vec<u8>, Uuid)> = sqlx::query_as(
        r#"
        SELECT seq, payload_len, payload_sha256, actor_user_id
        FROM fvoci.document_collab_op_receipts
        WHERE workspace_id = $1 AND document_id = $2 AND op_id = $3
        "#,
    )
    .bind(workspace_id)
    .bind(document_id)
    .bind(op_id)
    .fetch_optional(&mut *tx)
    .await?;
    tx.commit().await?;
    let Some((seq, payload_len, payload_sha256, stored_actor)) = row else {
        return Ok(Err(CollabDbError::NotFound));
    };
    if payload_len != expected_payload_len
        || payload_sha256.as_slice() != expected_payload_sha256
        || stored_actor != expected_actor_user_id
    {
        return Ok(Err(CollabDbError::OpIdConflict));
    }
    Ok(Ok(CollabOperationLookup {
        seq,
        payload_len,
        payload_sha256,
        actor_user_id: stored_actor,
    }))
}

pub async fn compact_collab_snapshot(
    pool: &PgPool,
    input: CompactCollabInput<'_>,
) -> Result<Result<CollabLoadState, CollabDbError>, sqlx::Error> {
    let CompactCollabInput {
        workspace_id,
        actor_user_id,
        session_id,
        document_id,
        writer_generation,
        cutoff_seq,
        expected_tail_seq,
        new_snapshot,
        client_ip,
    } = input;
    if new_snapshot.is_empty() || new_snapshot.len() > MAX_COLLAB_SNAPSHOT_BYTES {
        return Ok(Err(CollabDbError::PayloadTooLarge));
    }

    let mut tx = pool.begin().await?;
    set_tenant(&mut tx, workspace_id).await?;
    let content = match authorize_wiki_collab_write(
        &mut tx,
        workspace_id,
        actor_user_id,
        session_id,
        document_id,
    )
    .await?
    {
        Ok(content) => content,
        Err(err) => {
            tx.rollback().await?;
            return Ok(Err(err));
        }
    };
    match ensure_collab_state_row(&mut tx, workspace_id, document_id, &content.0).await? {
        Ok(()) => {}
        Err(err) => {
            tx.rollback().await?;
            return Ok(Err(err));
        }
    }
    let state = fetch_state_for_update(&mut tx, workspace_id, document_id).await?;
    let Some(state) = state else {
        tx.rollback().await?;
        return Ok(Err(CollabDbError::NotFound));
    };
    if state.2 != writer_generation {
        tx.rollback().await?;
        return Ok(Err(CollabDbError::StaleWriter));
    }
    if expected_tail_seq != state.4 {
        tx.rollback().await?;
        return Ok(Err(CollabDbError::StaleCutoff));
    }
    if cutoff_seq < state.3 || cutoff_seq > state.4 {
        tx.rollback().await?;
        return Ok(Err(CollabDbError::InvalidCutoff));
    }

    let newer: Option<(i64,)> = sqlx::query_as(
        r#"
        SELECT seq
        FROM fvoci.document_collab_updates
        WHERE workspace_id = $1 AND document_id = $2 AND seq > $3
        ORDER BY seq ASC
        LIMIT 1
        "#,
    )
    .bind(workspace_id)
    .bind(document_id)
    .bind(cutoff_seq)
    .fetch_optional(&mut *tx)
    .await?;
    if cutoff_seq < state.4 && newer.is_some() {
        tx.rollback().await?;
        return Ok(Err(CollabDbError::InvalidCutoff));
    }

    sqlx::query(
        r#"
        UPDATE fvoci.document_states
        SET state = $3,
            snapshot_cutoff_seq = $4,
            updated_at = now()
        WHERE workspace_id = $1 AND document_id = $2 AND writer_generation = $5
        "#,
    )
    .bind(workspace_id)
    .bind(document_id)
    .bind(new_snapshot)
    .bind(cutoff_seq)
    .bind(writer_generation)
    .execute(&mut *tx)
    .await?;

    sqlx::query(
        r#"
        DELETE FROM fvoci.document_collab_updates
        WHERE workspace_id = $1 AND document_id = $2 AND seq <= $3
        "#,
    )
    .bind(workspace_id)
    .bind(document_id)
    .bind(cutoff_seq)
    .execute(&mut *tx)
    .await?;

    let payload = json!({
        "documentId": document_id.to_string(),
        "cutoffSeq": cutoff_seq,
        "writerGeneration": writer_generation,
    });
    append_event(
        &mut tx,
        EventAppend {
            id: Uuid::now_v7(),
            workspace_id: Some(workspace_id),
            actor_user_id: Some(actor_user_id),
            verb: "document.collab_snapshot_compacted".to_string(),
            target_type: Some("document".to_string()),
            target_id: Some(document_id),
            payload: payload.clone(),
        },
    )
    .await?;
    append_audit(
        &mut tx,
        AuditAppend {
            id: Uuid::now_v7(),
            workspace_id: Some(workspace_id),
            actor_user_id: Some(actor_user_id),
            verb: "document.collab_snapshot_compacted".to_string(),
            target_type: Some("document".to_string()),
            target_id: Some(document_id),
            payload,
            ip: client_ip.map(str::to_string),
        },
    )
    .await?;

    let refreshed = fetch_state_for_update(&mut tx, workspace_id, document_id).await?;
    let Some(refreshed) = refreshed else {
        tx.rollback().await?;
        return Ok(Err(CollabDbError::NotFound));
    };
    let tail = match load_tail_updates(
        &mut tx,
        workspace_id,
        document_id,
        refreshed.3,
        refreshed.0.len() as i64,
    )
    .await?
    {
        Ok(tail) => tail,
        Err(err) => {
            tx.rollback().await?;
            return Ok(Err(err));
        }
    };
    let load = state_row_to_load(refreshed, tail);
    tx.commit().await?;
    Ok(Ok(load))
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CollabAdmission {
    pub read_only: bool,
    pub archived: bool,
}

/// Resolve whether a live session may join a wiki document collab room.
pub async fn resolve_collab_admission(
    pool: &PgPool,
    workspace_id: Uuid,
    actor_user_id: Uuid,
    session_id: Uuid,
    document_id: Uuid,
) -> Result<Result<CollabAdmission, CollabDbError>, sqlx::Error> {
    resolve_collab_admission_timed(pool, workspace_id, actor_user_id, session_id, document_id)
        .await
        .map(|(result, _)| result)
}

/// Same as [`resolve_collab_admission`] with per-phase DB timings for `collab.stage` tracing.
pub async fn resolve_collab_admission_timed(
    pool: &PgPool,
    workspace_id: Uuid,
    actor_user_id: Uuid,
    session_id: Uuid,
    document_id: Uuid,
) -> Result<(Result<CollabAdmission, CollabDbError>, CollabDbStageTimings), sqlx::Error> {
    let mut timings = CollabDbStageTimings::default();
    let pool_started = Instant::now();
    let tx = pool.begin().await?;
    timings.pool_wait_us = pool_started.elapsed().as_micros() as u64;
    resolve_collab_admission_tx(
        tx,
        workspace_id,
        actor_user_id,
        session_id,
        document_id,
        timings,
    )
    .await
}

/// Admission check on a room's dedicated session connection (no pool acquire).
pub async fn resolve_collab_admission_on_conn_timed(
    conn: &mut PgConnection,
    workspace_id: Uuid,
    actor_user_id: Uuid,
    session_id: Uuid,
    document_id: Uuid,
) -> Result<(Result<CollabAdmission, CollabDbError>, CollabDbStageTimings), sqlx::Error> {
    let tx = conn.begin().await?;
    resolve_collab_admission_tx(
        tx,
        workspace_id,
        actor_user_id,
        session_id,
        document_id,
        CollabDbStageTimings::default(),
    )
    .await
}

async fn resolve_collab_admission_tx(
    mut tx: Transaction<'_, Postgres>,
    workspace_id: Uuid,
    actor_user_id: Uuid,
    session_id: Uuid,
    document_id: Uuid,
    mut timings: CollabDbStageTimings,
) -> Result<(Result<CollabAdmission, CollabDbError>, CollabDbStageTimings), sqlx::Error> {
    set_tenant(&mut tx, workspace_id).await?;
    let advisory_started = Instant::now();
    lock_membership_users(&mut tx, &[actor_user_id]).await?;
    timings.advisory_lock_us = advisory_started.elapsed().as_micros() as u64;
    let row_started = Instant::now();
    if !recheck_session(&mut tx, actor_user_id, session_id).await? {
        tx.rollback().await?;
        return Ok((Err(CollabDbError::Forbidden), timings));
    }
    if !workspace_is_live(&mut tx, workspace_id).await? {
        tx.rollback().await?;
        return Ok((Err(CollabDbError::NotFound), timings));
    }
    let role = membership_role_for_update(&mut tx, workspace_id, actor_user_id).await?;
    if !wiki_can_edit(role) {
        tx.rollback().await?;
        return Ok((Err(CollabDbError::NotFound), timings));
    }
    let doc = lock_wiki_document_for_update(&mut tx, workspace_id, document_id).await?;
    timings.row_lock_us = row_started.elapsed().as_micros() as u64;
    let Some((project_id, status, deleted_at)) = doc else {
        tx.rollback().await?;
        return Ok((Err(CollabDbError::NotFound), timings));
    };
    if deleted_at.is_some() || project_id.is_some() {
        tx.rollback().await?;
        return Ok((Err(CollabDbError::NotFound), timings));
    }
    let archived = status == "archived";
    let commit_started = Instant::now();
    tx.commit().await?;
    timings.commit_us = commit_started.elapsed().as_micros() as u64;
    Ok((
        Ok(CollabAdmission {
            read_only: archived,
            archived,
        }),
        timings,
    ))
}

/// Read-only collab load for sync without claiming writer generation.
pub async fn load_collab_readonly(
    pool: &PgPool,
    workspace_id: Uuid,
    actor_user_id: Uuid,
    session_id: Uuid,
    document_id: Uuid,
) -> Result<Result<CollabLoadState, CollabDbError>, sqlx::Error> {
    let mut tx = pool.begin().await?;
    set_tenant(&mut tx, workspace_id).await?;
    match authorize_wiki_collab_read(
        &mut tx,
        workspace_id,
        actor_user_id,
        session_id,
        document_id,
    )
    .await?
    {
        Ok(()) => {}
        Err(err) => {
            tx.rollback().await?;
            return Ok(Err(err));
        }
    }
    let content: (Value,) = sqlx::query_as(
        "SELECT content_json FROM fvoci.documents WHERE workspace_id = $1 AND id = $2",
    )
    .bind(workspace_id)
    .bind(document_id)
    .fetch_one(&mut *tx)
    .await?;
    match ensure_collab_state_row(&mut tx, workspace_id, document_id, &content.0).await? {
        Ok(()) => {}
        Err(err) => {
            tx.rollback().await?;
            return Ok(Err(err));
        }
    }
    let state = fetch_state_for_update(&mut tx, workspace_id, document_id).await?;
    let Some(state) = state else {
        tx.rollback().await?;
        return Ok(Err(CollabDbError::NotFound));
    };
    if state.1 != COLLAB_STATE_ENCODING_V1 {
        tx.rollback().await?;
        return Ok(Err(CollabDbError::NotFound));
    }
    let tail = match load_tail_updates(
        &mut tx,
        workspace_id,
        document_id,
        state.3,
        state.0.len() as i64,
    )
    .await?
    {
        Ok(tail) => tail,
        Err(err) => {
            tx.rollback().await?;
            return Ok(Err(err));
        }
    };
    let load = state_row_to_load(state, tail);
    tx.commit().await?;
    Ok(Ok(load))
}

/// Best-effort persisted collab bytes for memory admission (snapshot + tail payloads).
pub async fn estimate_persisted_collab_bytes(
    pool: &PgPool,
    workspace_id: Uuid,
    document_id: Uuid,
) -> Result<u64, sqlx::Error> {
    let row: Option<(i64, i64)> = sqlx::query_as(
        r#"
        SELECT
            coalesce(octet_length(ds.state), 0)::bigint,
            coalesce((
                SELECT sum(octet_length(payload))::bigint
                FROM fvoci.document_collab_updates
                WHERE workspace_id = ds.workspace_id
                  AND document_id = ds.document_id
                  AND seq > ds.snapshot_cutoff_seq
            ), 0)::bigint
        FROM fvoci.document_states ds
        WHERE ds.workspace_id = $1 AND ds.document_id = $2
        "#,
    )
    .bind(workspace_id)
    .bind(document_id)
    .fetch_optional(pool)
    .await?;
    Ok(match row {
        Some((snapshot_len, tail_len)) => (snapshot_len + tail_len) as u64,
        None => 2,
    })
}

pub async fn project_derived_body(
    pool: &PgPool,
    input: ProjectDerivedBodyInput,
) -> Result<Result<ProjectDerivedBodyResult, CollabDbError>, sqlx::Error> {
    let ProjectDerivedBodyInput {
        workspace_id,
        actor_user_id,
        session_id,
        document_id,
        writer_generation,
        expected_tail_seq,
        prepared,
    } = input;
    let content_json = prepared.content_json();
    let text = prepared.text();
    let chosung = prepared.chosung();

    let mut tx = pool.begin().await?;
    set_tenant(&mut tx, workspace_id).await?;
    match authorize_wiki_collab_write(
        &mut tx,
        workspace_id,
        actor_user_id,
        session_id,
        document_id,
    )
    .await?
    {
        Ok(_) => {}
        Err(err) => {
            tx.rollback().await?;
            return Ok(Err(err));
        }
    };

    let state = fetch_state_fence_for_update(&mut tx, workspace_id, document_id).await?;
    let Some((current_generation, current_tail_seq)) = state else {
        tx.rollback().await?;
        return Ok(Err(CollabDbError::NotFound));
    };
    if current_tail_seq == 0 {
        tx.rollback().await?;
        return Ok(Ok(ProjectDerivedBodyResult::SkippedSeed));
    }
    if current_generation != writer_generation {
        tx.rollback().await?;
        return Ok(Err(CollabDbError::StaleWriter));
    }
    if current_tail_seq != expected_tail_seq {
        tx.rollback().await?;
        return Ok(Err(CollabDbError::StaleCutoff));
    }

    let updated: Option<(Uuid,)> = sqlx::query_as(
        r#"
        UPDATE fvoci.documents
        SET content_json = $3,
            text = $4,
            chosung = $5,
            updated_at = now()
        WHERE workspace_id = $1
          AND id = $2
          AND content_json IS DISTINCT FROM $3::jsonb
        RETURNING id
        "#,
    )
    .bind(workspace_id)
    .bind(document_id)
    .bind(content_json)
    .bind(text)
    .bind(chosung)
    .fetch_optional(&mut *tx)
    .await?;

    if updated.is_none() {
        tx.commit().await?;
        return Ok(Ok(ProjectDerivedBodyResult::Unchanged));
    }

    append_system_document_updated_event(&mut tx, workspace_id, document_id).await?;
    tx.commit().await?;
    Ok(Ok(ProjectDerivedBodyResult::Updated))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_yjs_seed_is_fixed_bytes() {
        assert_eq!(EMPTY_YJS_STATE_V1, &[0, 0]);
    }

    #[test]
    fn load_budget_allows_within_caps() {
        assert!(load_budget_allows(16 * 1024 * 1024, 32, 15 * 1024 * 1024).is_ok());
    }

    #[test]
    fn load_budget_rejects_tail_row_count_over_cap() {
        assert_eq!(
            load_budget_allows(0, MAX_COLLAB_TAIL_UPDATES + 1, 0),
            Err(CollabDbError::StateBudgetExceeded)
        );
    }

    #[test]
    fn load_budget_rejects_snapshot_plus_tail_bytes_over_cap() {
        assert_eq!(
            load_budget_allows(MAX_COLLAB_LOAD_BYTES, 1, 1),
            Err(CollabDbError::StateBudgetExceeded)
        );
    }
}
