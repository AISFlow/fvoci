//! One scoped transaction transfers ordinary identities; unsupported graph
//! models fail before effects until the owned native/file adapter is connected.
use super::backend::{Backend, DbTx, FamilyTx, OperationTx};
use super::codec::Cell;
use crate::api::personal_transfer::{
    PersonalTransferAction, PersonalTransferBlocker as Blocker, PersonalTransferBody,
    PersonalTransferConflict, PersonalTransferDisposition, PersonalTransferItem,
    PersonalTransferOutcome, PersonalTransferOutput, PersonalTransferPreview,
    PersonalTransferSelection,
};
use crate::attachments::{initial_extract_status, is_image_mime, ObjectStorage};
use crate::collab::derived_body::{extract_internal_refs, prepare_derived_body, InternalRefKind};
use crate::collab::seed::SeedEngine;
use crate::collab::wire::CollabKind;
use crate::db::attachments::{
    admit_transfer_storage, stage_attachment_for_transfer, StagedAttachment,
};
use crate::db::collab::{
    fetch_state_for_update, load_tail_updates, CollabTables, MAX_COLLAB_LOAD_BYTES,
};
use crate::db::context::{
    lock_key_from_uuid, lock_membership_users, lock_tree, recheck_session, set_self_user,
    set_tenant,
};
use crate::db::documents::{
    between, depth_of, lock_document_rows, record_document_event_and_audit, to_path_label,
    MAX_TREE_DEPTH,
};
use crate::db::native_history::{native_history_inventory, NativeDbError, NativeHistoryInventory};
use crate::db::personal_input::owns_personal_workspace;
use crate::db::projects::{lock_project, project_permission};
use crate::db::quota::StorageQuota;
use crate::db::tasks::{
    list_task_assignee_ids, map_task_row, record_task_event_and_audit, TaskChangeRecord,
    TaskRowRecord,
};
use crate::db::workspace::{membership_role_for_update, workspace_is_live};
use crate::native_history::{log_reason, ArchiveError, RetainedHistoryBlocker};
use crate::personal_transfer::{request_hash, valid_command, valid_selection};
use crate::projects::ProjectPermission;
use chrono::{DateTime, Utc};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use sqlx::{PgPool, Postgres, Row, Transaction};
use uuid::Uuid;

/// Existing isolated engine inputs, supplied by the current HTTP runtime.
#[derive(Clone)]
pub struct TransferBodyEngine {
    pub engine_bin: std::path::PathBuf,
    pub limits: collab_engine::limits::Limits,
}

struct CurrentBody {
    json: Value,
    cut: Value,
}

/// One key for a block identity and an origin anchor naming it. UUIDs are
/// canonicalized; other strings and scalars are kept exactly. Null or absent
/// means the block has no identity yet.
fn block_key(id: &Value) -> Option<String> {
    match id {
        Value::Null | Value::Array(_) | Value::Object(_) => None,
        Value::String(raw) => Some(
            Uuid::parse_str(raw)
                .map(|id| id.to_string())
                .unwrap_or_else(|_| raw.clone()),
        ),
        other => Some(other.to_string()),
    }
}

/// Current-content COPY uses new block identities as well as a fresh native
/// seed. A file node (attachment/image, attrs.id = attachment UUID) is
/// rewritten to the copy's new attachment from `files` (source -> copy);
/// a file outside the pair or an unusable id refuses. Reference mapping
/// remains an explicit preflight boundary.
/// JS truthiness of a JSON value, as the native seed reads `content` and
/// `marks`.
fn seed_truthy(value: &Value) -> bool {
    match value {
        Value::Null => false,
        Value::Bool(b) => *b,
        Value::Number(n) => n.as_f64().is_some_and(|f| f != 0.0 && !f.is_nan()),
        Value::String(s) => !s.is_empty(),
        Value::Array(_) | Value::Object(_) => true,
    }
}

/// The selected pair's identities in a body copy: each selected document or
/// task reference becomes the copy's identity (itself while validating).
struct PairRefs {
    document: (Uuid, Uuid),
    task: Option<(Uuid, Uuid)>,
}

impl PairRefs {
    fn copied(&self, kind: InternalRefKind, id: &str) -> Option<Uuid> {
        let id = Uuid::parse_str(id).ok()?;
        match kind {
            InternalRefKind::Document => (id == self.document.0).then_some(self.document.1),
            InternalRefKind::Task => self.task.filter(|task| task.0 == id).map(|task| task.1),
        }
    }
}

fn copy_body(
    json: &Value,
    files: &std::collections::HashMap<Uuid, Uuid>,
    pair: &PairRefs,
) -> Result<(Value, std::collections::HashMap<String, String>), PersonalTransferDbError> {
    fn visit(
        node: &mut Value,
        depth: usize,
        ids: &mut std::collections::HashMap<String, String>,
        files: &std::collections::HashMap<Uuid, Uuid>,
        pair: &PairRefs,
    ) -> Result<(), PersonalTransferDbError> {
        if depth > 64 {
            return Err(PersonalTransferDbError::Incomplete(
                Blocker::BodyEncoding,
                "copy body depth",
            ));
        }
        // The seed reads `content` with JS truthiness: a falsy value is no
        // children, anything else must be an array (collab-engine seed.rs
        // `fragment_from_json`). Refuse here what the seed would refuse late.
        if node
            .get("content")
            .is_some_and(|content| seed_truthy(content) && !content.is_array())
        {
            return Err(PersonalTransferDbError::Incomplete(
                Blocker::BodyEncoding,
                "copy body content is not an array",
            ));
        }
        // Same rule for `marks`, which the seed reads on every node below
        // the root (`node_from_json`), never on the root doc.
        if depth > 0
            && node
                .get("marks")
                .is_some_and(|marks| seed_truthy(marks) && !marks.is_array())
        {
            return Err(PersonalTransferDbError::Incomplete(
                Blocker::BodyEncoding,
                "copy body marks are not an array",
            ));
        }
        // Owned: `node` is borrowed mutably below while the kind is still read.
        let kind = node
            .get("type")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_owned();
        // The copy is seeded from this body: an `image` node is outside the
        // native seed schema (images are attachment nodes with `image`), so it
        // refuses here rather than failing the commit's seed.
        if kind == "image" {
            return Err(PersonalTransferDbError::Incomplete(
                Blocker::BodyEncoding,
                "copy body node outside the native schema",
            ));
        }
        let file = kind == "attachment";
        // A mention (target in `id`) or embed (target in `ref`) of the
        // selected pair points at the copy's own document/task; any other
        // reference, or any other entity, still refuses below.
        let target = match kind.as_str() {
            "mention" => Some("id"),
            "embed" => Some("ref"),
            _ => None,
        };
        let mut pair_reference = false;
        if let Some(key) = target {
            // Only this node's own typed target (its attrs), never one of its
            // descendants: a malformed or unsupported parent cannot borrow a
            // selected child's reference. Descendants are visited on their own.
            let own = json!({"type": kind.as_str(), "attrs": node.get("attrs").cloned().unwrap_or(Value::Null)});
            let references = extract_internal_refs(&own);
            let copied = match references.as_slice() {
                [reference] => pair.copied(reference.kind, &reference.id),
                _ => None,
            };
            if let (Some(copied), Some(attrs)) =
                (copied, node.get_mut("attrs").and_then(Value::as_object_mut))
            {
                attrs.insert(key.into(), Value::String(copied.to_string()));
                pair_reference = true;
            }
        }
        if file {
            let copied = node
                .get("attrs")
                .and_then(|attrs| attrs.get("id"))
                .and_then(Value::as_str)
                .and_then(|id| Uuid::parse_str(id).ok())
                .and_then(|id| files.get(&id).copied());
            let (Some(copied), Some(attrs)) =
                (copied, node.get_mut("attrs").and_then(Value::as_object_mut))
            else {
                return Err(PersonalTransferDbError::Incomplete(
                    Blocker::File,
                    "copy body file outside the pair",
                ));
            };
            attrs.insert("id".into(), Value::String(copied.to_string()));
        } else if !pair_reference
            && !matches!(
                kind.as_str(),
                "doc"
                    | "text"
                    | "paragraph"
                    | "heading"
                    | "blockquote"
                    | "bulletList"
                    | "orderedList"
                    | "listItem"
                    | "taskList"
                    | "taskItem"
                    | "codeBlock"
                    | "hardBreak"
                    | "horizontalRule"
                    | "table"
                    | "tableRow"
                    | "tableCell"
                    | "tableHeader"
            )
        {
            let blocker = if !extract_internal_refs(node).is_empty() {
                Blocker::OutgoingReference
            } else {
                Blocker::BodyEncoding
            };
            return Err(PersonalTransferDbError::Incomplete(
                blocker,
                "copy reference/file/node mapping",
            ));
        }
        // Block IDs are editor UUIDs, but body PUT/block PATCH accept any
        // string. Every present identity gets a fresh one, so the copy never
        // shares a block ID with the private original. A file node's id is
        // its attachment (mapped above) and a mention's id its target (mapped
        // above); their children are still checked. An embed's id is a block.
        if let Some(attrs) = node
            .get_mut("attrs")
            .and_then(Value::as_object_mut)
            .filter(|_| !file && kind != "mention")
        {
            if attrs
                .get("id")
                .is_some_and(|id| id.is_array() || id.is_object())
            {
                return Err(PersonalTransferDbError::Incomplete(
                    Blocker::BlockIdentity,
                    "copy block identity",
                ));
            }
            if let Some(original) = attrs.get("id").and_then(block_key) {
                let next = ids
                    .entry(original)
                    .or_insert_with(|| Uuid::now_v7().to_string())
                    .clone();
                attrs.insert("id".into(), Value::String(next));
            }
        }
        if let Some(content) = node.get_mut("content").and_then(Value::as_array_mut) {
            for child in content {
                visit(child, depth + 1, ids, files, pair)?;
            }
        }
        Ok(())
    }
    let mut json = json.clone();
    let mut ids = std::collections::HashMap::new();
    visit(&mut json, 0, &mut ids, files, pair)?;
    Ok((json, ids))
}

async fn current_body(
    tx: &mut Transaction<'_, Postgres>,
    workspace: Uuid,
    kind: CollabKind,
    id: Uuid,
    fallback: &Value,
    engine: Option<&TransferBodyEngine>,
    budget: Option<&mut ScanBudget>,
) -> TransferResult<CurrentBody> {
    let tables = CollabTables::for_kind(kind);
    let Some((snapshot, encoding, generation, cutoff, tail_seq, _)) =
        fetch_state_for_update(tx, tables, workspace, id).await?
    else {
        let sql = match kind {
            CollabKind::Document => "SELECT EXISTS(SELECT 1 FROM fvoci.document_collab_updates WHERE workspace_id=$1 AND document_id=$2) OR EXISTS(SELECT 1 FROM fvoci.document_collab_op_receipts WHERE workspace_id=$1 AND document_id=$2)",
            CollabKind::Task => "SELECT EXISTS(SELECT 1 FROM fvoci.task_collab_updates WHERE workspace_id=$1 AND task_id=$2) OR EXISTS(SELECT 1 FROM fvoci.task_collab_op_receipts WHERE workspace_id=$1 AND task_id=$2)",
        };
        let missing_state: bool = sqlx::query_scalar(sql)
            .bind(workspace)
            .bind(id)
            .fetch_one(&mut **tx)
            .await?;
        if missing_state {
            return Ok(Err(PersonalTransferDbError::Incomplete(
                Blocker::NativeStateMissing,
                "native body state missing",
            )));
        }
        return Ok(Ok(CurrentBody {
            json: fallback.clone(),
            cut: json!({"native":false}),
        }));
    };
    if encoding != 1 {
        return Ok(Err(PersonalTransferDbError::Incomplete(
            Blocker::BodyEncoding,
            "native body encoding",
        )));
    }
    let tail =
        match load_tail_updates(tx, tables, workspace, id, cutoff, snapshot.len() as i64).await? {
            Ok(value) => value,
            Err(_) => {
                return Ok(Err(PersonalTransferDbError::Incomplete(
                    Blocker::InventoryBudget,
                    "native body budget",
                )))
            }
        };
    if tail.last().map_or(cutoff, |update| update.seq) != tail_seq
        || tail
            .iter()
            .enumerate()
            .any(|(index, update)| update.seq != cutoff + index as i64 + 1)
    {
        return Ok(Err(PersonalTransferDbError::Incomplete(
            Blocker::NativeStateMissing,
            "native body cut",
        )));
    }
    if let Some(budget) = budget {
        let bytes = tail.iter().fold(snapshot.len() as i64, |sum, update| {
            sum.saturating_add(update.payload.len() as i64)
        });
        if !budget.admit(bytes) {
            return Ok(Err(PersonalTransferDbError::Incomplete(
                Blocker::InventoryBudget,
                "incoming reference inventory budget",
            )));
        }
    }
    let cut = json!({"native":true,"generation":generation,"cutoff":cutoff,"tail":tail_seq,
        "snapshot":hex_digest(&snapshot),"updates":tail.iter().map(|update|json!({"seq":update.seq,"opId":update.op_id,"payload":hex_digest(&update.payload)})).collect::<Vec<_>>()});
    let engine = engine
        .ok_or_else(|| sqlx::Error::Protocol("personal transfer body engine unavailable".into()))?
        .clone();
    let payloads = tail.into_iter().map(|update| update.payload).collect();
    let json = tokio::task::spawn_blocking(move || {
        crate::collab::revision::project_persisted_offline(
            engine.engine_bin,
            engine.limits,
            snapshot,
            payloads,
        )
    })
    .await
    .map_err(|_| sqlx::Error::Protocol("personal transfer body projection unavailable".into()))?
    .map_err(|_| sqlx::Error::Protocol("personal transfer body projection unavailable".into()))?;
    Ok(Ok(CurrentBody { json, cut }))
}

/// Every initialized referrer needs its own isolated engine projection while
/// the source writer fence is held. Bound the whole incoming scan, not just
/// each row: at most this many projections and, in total, one maximal engine
/// reload of bytes. Exceeding it refuses MOVE before effects; it is a staged
/// limit until the reference mapping adapter exists, not a skipped check.
const INCOMING_SCAN_MAX_PROJECTIONS: usize = 128;
const INCOMING_SCAN_MAX_BYTES: i64 = MAX_COLLAB_LOAD_BYTES;

#[derive(Debug, Default)]
struct ScanBudget {
    projections: usize,
    bytes: i64,
}
impl ScanBudget {
    /// Admit one projection of `bytes` before its engine process is spawned.
    fn admit(&mut self, bytes: i64) -> bool {
        let total = self.bytes.saturating_add(bytes);
        if self.projections >= INCOMING_SCAN_MAX_PROJECTIONS || total > INCOMING_SCAN_MAX_BYTES {
            return false;
        }
        self.projections += 1;
        self.bytes = total;
        true
    }
}

fn hex_digest(bytes: &[u8]) -> String {
    Sha256::digest(bytes)
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

/// The actor fence is shared with genuine append and derived-body writers.
/// Read their current native cuts; a lagging JSON projection is not absence.
async fn incoming_reference_boundary(
    tx: &mut Transaction<'_, Postgres>,
    source: Uuid,
    selection: &PersonalTransferSelection,
    engine: Option<&TransferBodyEngine>,
) -> TransferResult<()> {
    let rows: Vec<(String, Uuid, Value)> = sqlx::query_as(
        "SELECT 'document' AS kind,id,content_json FROM fvoci.documents WHERE workspace_id=$1 AND deleted_at IS NULL AND id<>$2 UNION ALL SELECT 'task' AS kind,id,content_json FROM fvoci.tasks WHERE workspace_id=$1 AND deleted_at IS NULL AND id IS DISTINCT FROM $3 ORDER BY kind,id LIMIT 1001",
    ).bind(source).bind(selection.document_id).bind(selection.task_id).fetch_all(&mut **tx).await?;
    if rows.len() > 1000 {
        return Ok(Err(PersonalTransferDbError::Incomplete(
            Blocker::InventoryBudget,
            "incoming reference inventory budget",
        )));
    }
    let mut budget = ScanBudget::default();
    for (kind, id, fallback) in rows {
        let kind = if kind == "document" {
            CollabKind::Document
        } else {
            CollabKind::Task
        };
        let body =
            match current_body(tx, source, kind, id, &fallback, engine, Some(&mut budget)).await? {
                Ok(value) => value,
                Err(error) => return Ok(Err(error)),
            };
        let incoming = extract_internal_refs(&body.json).iter().any(|reference| {
            let Ok(id) = Uuid::parse_str(&reference.id) else {
                return false;
            };
            match reference.kind {
                InternalRefKind::Document => id == selection.document_id,
                InternalRefKind::Task => Some(id) == selection.task_id,
            }
        });
        if incoming {
            return Ok(Err(PersonalTransferDbError::Incomplete(
                Blocker::IncomingReference,
                "incoming private reference mapping",
            )));
        }
    }
    Ok(Ok(()))
}

#[derive(Debug)]
pub enum PersonalTransferDbError {
    NotFound,
    Forbidden,
    Conflict(PersonalTransferConflict),
    InvalidInput,
    /// An authorized preflight refusal, never a partial successful move: a
    /// typed blocker for clients plus a diagnostic title for logs.
    Incomplete(Blocker, &'static str),
}
type TransferResult<T> = Result<Result<T, PersonalTransferDbError>, sqlx::Error>;

struct DocumentRow {
    title: String,
    icon: Option<String>,
    status: String,
    kind: String,
    schema_version: i32,
    version: i32,
    content_json: Value,
    text: String,
    chosung: String,
    created_by: Uuid,
    created_at: DateTime<Utc>,
    updated_at: DateTime<Utc>,
}
struct TaskSource {
    record: TaskRowRecord,
    /// 049: the estimate's declared unit, carried with the estimate as one
    /// pair (NULL = unspecified; never inferred).
    estimate_unit: Option<String>,
    content_json: Value,
    text: String,
    chosung: String,
    recurrence: Option<Value>,
}
struct Origin {
    request_id: Uuid,
    request_hash: String,
    anchor: Option<String>,
    created_at: DateTime<Utc>,
    updated_at: DateTime<Utc>,
}
struct Activity {
    id: Uuid,
    actor: Option<Uuid>,
    channel: String,
    kind: String,
    changes: Value,
    created_at: DateTime<Utc>,
}
struct CollectionItem {
    id: Uuid,
    version: i32,
    created_at: DateTime<Utc>,
    updated_at: DateTime<Utc>,
}
/// The running server's storage and quota, for moving attachments.
#[derive(Clone, Copy)]
pub struct TransferFiles<'a> {
    pub storage: &'a ObjectStorage,
    pub quota: &'a StorageQuota,
}

/// A moved attachment as its source row is (locked in the transaction):
/// every value the destination row is published with, so a staged copy of
/// an older row can never be published.
#[derive(Clone, Debug, PartialEq, Eq)]
struct FileIdentity {
    id: Uuid,
    document_id: Option<Uuid>,
    task_id: Option<Uuid>,
    uploader_id: Uuid,
    name: String,
    mime: String,
    declared_mime: Option<String>,
    storage_key: String,
    size_bytes: Option<i64>,
    image: bool,
    scan_status: String,
    preview: Option<Value>,
    created_at: DateTime<Utc>,
    completed_at: Option<DateTime<Utc>>,
}

impl FileIdentity {
    fn of(att: &crate::db::attachments::AttachmentRow) -> Self {
        Self {
            id: att.id,
            document_id: att.document_id,
            task_id: att.task_id,
            uploader_id: att.uploader_id,
            name: att.name.clone(),
            mime: att.mime.clone(),
            declared_mime: att.declared_mime.clone(),
            storage_key: att.storage_key.clone(),
            size_bytes: att.size_bytes,
            image: att.image,
            scan_status: att.scan_status.clone(),
            preview: att.variants.get("preview").cloned(),
            created_at: att.created_at,
            completed_at: att.completed_at,
        }
    }
}

struct SourceGraph {
    document: DocumentRow,
    /// MOVE only: the pair's attachments, each a clean stored object.
    files: Vec<FileIdentity>,
    /// MOVE only: the locked retained native history of the document and the
    /// selected task (W7 helper), moved with identical rows.
    native: Vec<NativeHistoryInventory>,
    task: Option<TaskSource>,
    origin: Option<Origin>,
    assignees: Vec<Uuid>,
    activities: Vec<Activity>,
    collection_item: Option<CollectionItem>,
    body_cuts: Value,
    /// MOVE only: the task's time graph, relocated with the same IDs.
    timer: TimerGraph,
}
struct Destination {
    workspace_name: String,
    project_name: String,
    visibility: String,
    root_id: Uuid,
    root_path: String,
    collection_id: Option<Uuid>,
}

/// Whether a current body names anything the transfer cannot carry. Only the
/// selected document and task (exact kind and UUID) are carried: a same-ID
/// MOVE keeps them as they are (the closure its retained native history is
/// held to) and a COPY points them at its new identities (`copy_body`). Any
/// other reference still needs the current reference disclosure mapping.
fn outside_reference(body: &Value, selection: &PersonalTransferSelection) -> bool {
    extract_internal_refs(body).iter().any(|reference| {
        !Uuid::parse_str(&reference.id).is_ok_and(|id| match reference.kind {
            InternalRefKind::Document => id == selection.document_id,
            InternalRefKind::Task => Some(id) == selection.task_id,
        })
    })
}

async fn writer_prefix(
    tx: &mut Transaction<'_, Postgres>,
    source: Uuid,
    actor: Uuid,
    session: Uuid,
) -> TransferResult<()> {
    set_tenant(tx, source).await?;
    lock_membership_users(tx, &[actor]).await?;
    if !recheck_session(tx, actor, session).await? {
        return Ok(Err(PersonalTransferDbError::Forbidden));
    }
    if !owns_personal_workspace(tx, source, actor).await? {
        return Ok(Err(PersonalTransferDbError::NotFound));
    }
    Ok(Ok(()))
}

async fn lock_trees(
    tx: &mut Transaction<'_, Postgres>,
    source: Uuid,
    destination: Uuid,
) -> Result<(), sqlx::Error> {
    let mut scopes = [source, destination];
    scopes.sort_by_key(|id| (lock_key_from_uuid(*id), *id));
    for ws in scopes {
        set_tenant(tx, ws).await?;
        lock_tree(tx, ws).await?;
    }
    set_tenant(tx, source).await
}

/// Target-lock waits can outlive the initial personal-owner read. Read the
/// current source authority after that wait and hold it through publication
/// (or receipt replay), so a later revocation cannot commit ahead of us.
async fn lock_current_source_owner(
    tx: &mut Transaction<'_, Postgres>,
    source: Uuid,
    actor: Uuid,
    destination: Uuid,
) -> Result<bool, sqlx::Error> {
    set_tenant(tx, source).await?;
    let authorized: Option<Uuid> = sqlx::query_scalar(
        r#"SELECT w.id FROM fvoci.workspaces w
        JOIN fvoci.users u ON u.personal_workspace_id = w.id
        JOIN fvoci.memberships m ON m.workspace_id = w.id AND m.user_id = u.id
        WHERE w.id = $1 AND w.kind = 'personal' AND w.deleted_at IS NULL
        AND u.id = $2 AND u.deleted_at IS NULL AND m.role = 'owner'
        FOR SHARE OF w, m"#,
    )
    .bind(source)
    .bind(actor)
    .fetch_optional(&mut **tx)
    .await?;
    set_tenant(tx, destination).await?;
    Ok(authorized.is_some())
}

/// Owner-private Zotero rows (migration 051) on the selected document or
/// task. A MOVE deletes the source rows, which would cascade the links and
/// detach the references, so any such row refuses before effects. The tables
/// are tenant AND owner scoped; the personal source has the actor as its only
/// member, so the actor's own view is the whole set. The caller already holds
/// the document and task rows FOR UPDATE: a concurrent link, reference or
/// re-pointing needs their FOR KEY SHARE and waits for this transaction.
/// The self user is restored to its previous value for later statements.
async fn owner_private_zotero_rows(
    tx: &mut Transaction<'_, Postgres>,
    source: Uuid,
    actor: Uuid,
    selection: &PersonalTransferSelection,
) -> Result<bool, sqlx::Error> {
    let previous: Option<String> =
        sqlx::query_scalar("SELECT current_setting('app.self_user_id', true)")
            .fetch_one(&mut **tx)
            .await?;
    set_self_user(tx, actor).await?;
    let found: bool = sqlx::query_scalar(
        "SELECT EXISTS(SELECT 1 FROM fvoci.zotero_references WHERE workspace_id=$1 AND document_id=$2)
            OR EXISTS(SELECT 1 FROM fvoci.zotero_links WHERE workspace_id=$1 AND (document_id=$2 OR task_id=$3))",
    )
    .bind(source)
    .bind(selection.document_id)
    .bind(selection.task_id)
    .fetch_one(&mut **tx)
    .await?;
    sqlx::query("SELECT set_config('app.self_user_id', $1, true)")
        .bind(previous.unwrap_or_default())
        .execute(&mut **tx)
        .await?;
    Ok(found)
}

/// Upper bound on each timer collection of a moved task; more refuses as
/// inventory_budget rather than truncating.
const TIMER_ROW_BUDGET: i64 = 10_000;

/// A 034 time entry of the moved task, every stored value (tenant-shared).
#[derive(Debug, Clone)]
struct TimeEntryRow {
    id: Uuid,
    started_at: DateTime<Utc>,
    ended_at: Option<DateTime<Utc>>,
    duration_seconds: Option<i32>,
    note: Option<String>,
}

/// A 034 row as read: (id, author, started, ended, duration, note).
type TimeEntryWithAuthor = (
    Uuid,
    Uuid,
    DateTime<Utc>,
    Option<DateTime<Utc>>,
    Option<i32>,
    Option<String>,
);

/// A 048 stopwatch run of the actor on the moved task (actor-private).
#[derive(Debug, Clone)]
struct TimerRunRow {
    id: Uuid,
    status: String,
    version: i32,
    started_at: DateTime<Utc>,
    stopped_at: Option<DateTime<Utc>>,
    note: Option<String>,
}

/// A 048 run segment, with its optional 034 projection.
#[derive(Debug, Clone)]
struct TimerSegmentRow {
    id: Uuid,
    run_id: Uuid,
    started_at: DateTime<Utc>,
    ended_at: Option<DateTime<Utc>>,
    time_entry_id: Option<Uuid>,
}

/// The moved task's canonical time graph, relocated with the same IDs. The
/// command receipts and audit rows that reference it are immutable history
/// and are never read into, moved or rewritten by the transfer. This relies
/// on migration 053 (the command run locator is historical, not a foreign
/// key) and the W5 replay/correction lookup that follow the stable IDs; the
/// writer is integrated only together with them.
#[derive(Debug, Clone, Default)]
struct TimerGraph {
    entries: Vec<TimeEntryRow>,
    runs: Vec<TimerRunRow>,
    segments: Vec<TimerSegmentRow>,
    /// Open entries holding a genuine legacy reservation in the source.
    reserved: Vec<Uuid>,
}

impl TimerGraph {
    fn digest(&self) -> Value {
        json!({
            "entries": self.entries.iter().map(|e| json!({"id":e.id,"startedAt":e.started_at,"endedAt":e.ended_at,"duration":e.duration_seconds,"note":e.note})).collect::<Vec<_>>(),
            "runs": self.runs.iter().map(|r| json!({"id":r.id,"status":r.status,"version":r.version,"startedAt":r.started_at,"stoppedAt":r.stopped_at,"note":r.note})).collect::<Vec<_>>(),
            "segments": self.segments.iter().map(|g| json!({"id":g.id,"run":g.run_id,"startedAt":g.started_at,"endedAt":g.ended_at,"entry":g.time_entry_id})).collect::<Vec<_>>(),
            "reserved": self.reserved,
        })
    }
}

/// The transaction's current `app.self_user_id`, to restore after a block
/// that runs under the actor's 048 self policy.
async fn previous_self_user(tx: &mut Transaction<'_, Postgres>) -> Result<String, sqlx::Error> {
    let previous: Option<String> =
        sqlx::query_scalar("SELECT current_setting('app.self_user_id', true)")
            .fetch_one(&mut **tx)
            .await?;
    Ok(previous.unwrap_or_default())
}

async fn restore_self_user(
    tx: &mut Transaction<'_, Postgres>,
    previous: &str,
) -> Result<(), sqlx::Error> {
    sqlx::query("SELECT set_config('app.self_user_id', $1, true)")
        .bind(previous)
        .execute(&mut **tx)
        .await?;
    Ok(())
}

/// MOVE: the locked, bounded time graph of the selected task, and the
/// pre-effect conflicts that would otherwise fail inside the publication.
/// 034 entries are read under the source tenant; 048 rows under the actor's
/// self policy (the personal source's only member is the actor, which the
/// caller's owner fence already holds). A foreign-author entry refuses rather
/// than being exported or cascaded away.
async fn timer_snapshot(
    tx: &mut Transaction<'_, Postgres>,
    source: Uuid,
    actor: Uuid,
    selection: &PersonalTransferSelection,
) -> TransferResult<TimerGraph> {
    let Some(task) = selection.task_id else {
        return Ok(Ok(TimerGraph::default()));
    };
    let budget = || {
        Ok(Err(PersonalTransferDbError::Incomplete(
            Blocker::InventoryBudget,
            "timer inventory budget",
        )))
    };
    set_tenant(tx, source).await?;
    let rows: Vec<TimeEntryWithAuthor> = sqlx::query_as(
        "SELECT id,user_id,started_at,ended_at,duration_seconds,note FROM fvoci.time_entries WHERE workspace_id=$1 AND task_id=$2 ORDER BY started_at,id LIMIT $3 FOR UPDATE",
    )
    .bind(source)
    .bind(task)
    .bind(TIMER_ROW_BUDGET + 1)
    .fetch_all(&mut **tx)
    .await?;
    if rows.len() as i64 > TIMER_ROW_BUDGET {
        return budget();
    }
    if rows.iter().any(|row| row.1 != actor) {
        return Ok(Err(PersonalTransferDbError::Incomplete(
            Blocker::DependentGraph,
            "time entry by another author",
        )));
    }
    let entries: Vec<TimeEntryRow> = rows
        .into_iter()
        .map(
            |(id, _, started_at, ended_at, duration_seconds, note)| TimeEntryRow {
                id,
                started_at,
                ended_at,
                duration_seconds,
                note,
            },
        )
        .collect();
    let previous = previous_self_user(tx).await?;
    set_self_user(tx, actor).await?;
    let runs: Vec<TimerRunRow> = sqlx::query_as::<_, (Uuid, String, i32, DateTime<Utc>, Option<DateTime<Utc>>, Option<String>)>(
        "SELECT id,status,version,started_at,stopped_at,note FROM fvoci.task_timer_runs WHERE workspace_id=$1 AND task_id=$2 AND user_id=$3 ORDER BY started_at,id LIMIT $4 FOR UPDATE",
    )
    .bind(source)
    .bind(task)
    .bind(actor)
    .bind(TIMER_ROW_BUDGET + 1)
    .fetch_all(&mut **tx)
    .await?
    .into_iter()
    .map(|(id, status, version, started_at, stopped_at, note)| TimerRunRow {
        id,
        status,
        version,
        started_at,
        stopped_at,
        note,
    })
    .collect();
    let segments: Vec<TimerSegmentRow> = sqlx::query_as::<_, (Uuid, Uuid, DateTime<Utc>, Option<DateTime<Utc>>, Option<Uuid>)>(
        "SELECT id,run_id,started_at,ended_at,time_entry_id FROM fvoci.task_timer_segments WHERE workspace_id=$1 AND task_id=$2 AND user_id=$3 ORDER BY started_at,id LIMIT $4 FOR UPDATE",
    )
    .bind(source)
    .bind(task)
    .bind(actor)
    .bind(TIMER_ROW_BUDGET + 1)
    .fetch_all(&mut **tx)
    .await?
    .into_iter()
    .map(|(id, run_id, started_at, ended_at, time_entry_id)| TimerSegmentRow {
        id,
        run_id,
        started_at,
        ended_at,
        time_entry_id,
    })
    .collect();
    let reserved: Vec<Uuid> = sqlx::query_scalar(
        "SELECT time_entry_id FROM fvoci.task_timer_legacy_open WHERE workspace_id=$1 AND task_id=$2 AND user_id=$3 ORDER BY time_entry_id FOR UPDATE",
    )
    .bind(source)
    .bind(task)
    .bind(actor)
    .fetch_all(&mut **tx)
    .await?;
    if runs.len() as i64 > TIMER_ROW_BUDGET || segments.len() as i64 > TIMER_ROW_BUDGET {
        restore_self_user(tx, &previous).await?;
        return budget();
    }
    // An open entry is reinserted, and the unchanged 048 trigger refuses it
    // while the actor has any other unfinished run or reservation; the 034
    // index allows one open entry per actor in the destination. Refuse both
    // here, before any effect, rather than closing or releasing anything.
    if entries.iter().any(|entry| entry.ended_at.is_none()) {
        let elsewhere: bool = sqlx::query_scalar(
            "SELECT EXISTS(SELECT 1 FROM fvoci.task_timer_runs WHERE user_id=$1 AND status<>'stopped' AND NOT (workspace_id=$2 AND task_id=$3))
                OR EXISTS(SELECT 1 FROM fvoci.task_timer_legacy_open WHERE user_id=$1 AND NOT (workspace_id=$2 AND task_id=$3))",
        )
        .bind(actor)
        .bind(source)
        .bind(task)
        .fetch_one(&mut **tx)
        .await?;
        set_tenant(tx, selection.destination_workspace_id).await?;
        let destination_open: bool = sqlx::query_scalar(
            "SELECT EXISTS(SELECT 1 FROM fvoci.time_entries WHERE workspace_id=$1 AND user_id=$2 AND ended_at IS NULL)",
        )
        .bind(selection.destination_workspace_id)
        .bind(actor)
        .fetch_one(&mut **tx)
        .await?;
        set_tenant(tx, source).await?;
        if elsewhere || destination_open {
            restore_self_user(tx, &previous).await?;
            return Ok(Err(PersonalTransferDbError::Incomplete(
                Blocker::TimerBusy,
                "another unfinished timer or open time entry",
            )));
        }
    }
    restore_self_user(tx, &previous).await?;
    Ok(Ok(TimerGraph {
        entries,
        runs,
        segments,
        reserved,
    }))
}

/// MOVE publication of the time graph under the destination tenant, after the
/// destination task exists: closed entries; the (at most one, per 034) open
/// entry, dropping the reservation its INSERT trigger creates unless the source
/// held a genuine one; stopped runs, then any unfinished run; segments last.
/// Every value is the source's; only the tenant locator changes.
async fn relocate_timer(
    tx: &mut Transaction<'_, Postgres>,
    destination: Uuid,
    actor: Uuid,
    task: Uuid,
    timer: &TimerGraph,
) -> Result<(), sqlx::Error> {
    if timer.entries.is_empty() && timer.runs.is_empty() {
        return Ok(());
    }
    let previous = previous_self_user(tx).await?;
    set_self_user(tx, actor).await?;
    let insert_entry = "INSERT INTO fvoci.time_entries(id,workspace_id,task_id,user_id,started_at,ended_at,duration_seconds,note) VALUES($1,$2,$3,$4,$5,$6,$7,$8)";
    let mut ordered: Vec<&TimeEntryRow> = timer.entries.iter().collect();
    ordered.sort_by_key(|entry| entry.ended_at.is_none());
    for entry in ordered {
        sqlx::query(insert_entry)
            .bind(entry.id)
            .bind(destination)
            .bind(task)
            .bind(actor)
            .bind(entry.started_at)
            .bind(entry.ended_at)
            .bind(entry.duration_seconds)
            .bind(&entry.note)
            .execute(&mut **tx)
            .await?;
        if entry.ended_at.is_none() && !timer.reserved.contains(&entry.id) {
            sqlx::query(
                "DELETE FROM fvoci.task_timer_legacy_open WHERE time_entry_id=$1 AND user_id=$2",
            )
            .bind(entry.id)
            .bind(actor)
            .execute(&mut **tx)
            .await?;
        }
    }
    let mut runs: Vec<&TimerRunRow> = timer.runs.iter().collect();
    runs.sort_by_key(|run| run.status != "stopped");
    for run in runs {
        sqlx::query("INSERT INTO fvoci.task_timer_runs(id,user_id,workspace_id,task_id,status,version,started_at,stopped_at,note) VALUES($1,$2,$3,$4,$5,$6,$7,$8,$9)")
            .bind(run.id)
            .bind(actor)
            .bind(destination)
            .bind(task)
            .bind(&run.status)
            .bind(run.version)
            .bind(run.started_at)
            .bind(run.stopped_at)
            .bind(&run.note)
            .execute(&mut **tx)
            .await?;
    }
    for segment in &timer.segments {
        sqlx::query("INSERT INTO fvoci.task_timer_segments(id,run_id,user_id,workspace_id,task_id,started_at,ended_at,time_entry_id) VALUES($1,$2,$3,$4,$5,$6,$7,$8)")
            .bind(segment.id)
            .bind(segment.run_id)
            .bind(actor)
            .bind(destination)
            .bind(task)
            .bind(segment.started_at)
            .bind(segment.ended_at)
            .bind(segment.time_entry_id)
            .execute(&mut **tx)
            .await?;
    }
    restore_self_user(tx, &previous).await
}

/// Locks source metadata before checking the target project. A held target
/// project row is an observable fixture barrier; authorization is rechecked
/// only after acquiring it, never cached from a preliminary UI lookup.
async fn source_graph(
    tx: &mut Transaction<'_, Postgres>,
    source: Uuid,
    actor: Uuid,
    selection: &PersonalTransferSelection,
    engine: Option<&TransferBodyEngine>,
) -> TransferResult<SourceGraph> {
    set_tenant(tx, source).await?;
    lock_document_rows(tx, source, &[selection.document_id]).await?;
    let Some(row) = sqlx::query(
        "SELECT * FROM fvoci.documents WHERE workspace_id=$1 AND id=$2 AND deleted_at IS NULL",
    )
    .bind(source)
    .bind(selection.document_id)
    .fetch_optional(&mut **tx)
    .await?
    else {
        return Ok(Err(PersonalTransferDbError::NotFound));
    };
    if row.try_get::<Option<Uuid>, _>("project_id")?.is_some()
        || row.try_get::<Option<Uuid>, _>("parent_id")?.is_some()
    {
        return Ok(Err(PersonalTransferDbError::Incomplete(
            Blocker::Hierarchy,
            "source hierarchy adapter",
        )));
    }
    let mut document = DocumentRow {
        title: row.try_get("title")?,
        icon: row.try_get("icon")?,
        status: row.try_get("status")?,
        kind: row.try_get("kind")?,
        schema_version: row.try_get("schema_version")?,
        version: row.try_get("version")?,
        content_json: row.try_get("content_json")?,
        text: row.try_get("text")?,
        chosung: row.try_get("chosung")?,
        created_by: row.try_get("created_by")?,
        created_at: row.try_get("created_at")?,
        updated_at: row.try_get("updated_at")?,
    };
    if document.status == "archived" {
        return Ok(Err(PersonalTransferDbError::NotFound));
    }
    if document.version != selection.expected_document_version {
        return Ok(Err(PersonalTransferDbError::Conflict(
            PersonalTransferConflict::PreviewStale,
        )));
    }
    let mut task = None;
    let mut assignees = Vec::new();
    let mut activities = Vec::new();
    let mut collection_item = None;
    let mut origin = None;
    if let Some(id) = selection.task_id {
        let Some(project_id)=sqlx::query_scalar::<_,Uuid>("SELECT project_id FROM fvoci.tasks WHERE workspace_id=$1 AND id=$2 AND deleted_at IS NULL").bind(source).bind(id).fetch_optional(&mut **tx).await? else {return Ok(Err(PersonalTransferDbError::NotFound));};
        let Some(project) = lock_project(tx, source, project_id).await? else {
            return Ok(Err(PersonalTransferDbError::NotFound));
        };
        if project.status == "archived"
            || !project_permission(tx, source, actor, &project)
                .await?
                .at_least(ProjectPermission::Edit)
        {
            return Ok(Err(PersonalTransferDbError::NotFound));
        }
        let Some(row)=sqlx::query("SELECT *, type AS task_type, estimate::text AS estimate FROM fvoci.tasks WHERE workspace_id=$1 AND id=$2 AND deleted_at IS NULL FOR UPDATE").bind(source).bind(id).fetch_optional(&mut **tx).await? else{return Ok(Err(PersonalTransferDbError::NotFound));};
        let record = map_task_row(&row)?;
        if Some(record.version) != selection.expected_task_version {
            return Ok(Err(PersonalTransferDbError::Conflict(
                PersonalTransferConflict::PreviewStale,
            )));
        }
        if record.archived_at.is_some() {
            return Ok(Err(PersonalTransferDbError::NotFound));
        }
        if record.parent_id.is_some()
            || record.milestone_id.is_some()
            || record.task_type == "subtask"
        {
            return Ok(Err(PersonalTransferDbError::Incomplete(
                Blocker::Hierarchy,
                "task hierarchy mapping",
            )));
        }
        assignees = list_task_assignee_ids(tx, source, id).await?;
        if assignees.iter().any(|id| *id != actor) {
            return Ok(Err(PersonalTransferDbError::Incomplete(
                Blocker::Assignee,
                "assignee mapping",
            )));
        }
        task = Some(TaskSource {
            record,
            estimate_unit: row.try_get("estimate_unit")?,
            content_json: row.try_get("content_json")?,
            text: row.try_get("text")?,
            chosung: row.try_get("chosung")?,
            recurrence: row.try_get("recurrence")?,
        });
        for row in sqlx::query("SELECT id,actor_user_id,channel,kind,changes,created_at FROM fvoci.task_activity WHERE workspace_id=$1 AND task_id=$2 ORDER BY created_at,id LIMIT 1001").bind(source).bind(id).fetch_all(&mut **tx).await? {
            activities.push(Activity{id:row.try_get("id")?,actor:row.try_get("actor_user_id")?,channel:row.try_get("channel")?,kind:row.try_get("kind")?,changes:row.try_get("changes")?,created_at:row.try_get("created_at")?});
        }
        if activities.len() > 1000 {
            return Ok(Err(PersonalTransferDbError::Incomplete(
                Blocker::InventoryBudget,
                "activity budget",
            )));
        }
        let Some(row)=sqlx::query("SELECT i.id,i.version,i.created_at,i.updated_at FROM fvoci.collection_items i JOIN fvoci.collections c ON c.workspace_id=i.workspace_id AND c.id=i.collection_id WHERE i.workspace_id=$1 AND i.task_id=$2 AND c.project_id=$3 AND c.kind='task' AND c.deleted_at IS NULL FOR UPDATE OF i").bind(source).bind(id).bind(project_id).fetch_optional(&mut **tx).await? else {return Ok(Err(PersonalTransferDbError::Incomplete(Blocker::DependentGraph, "task collection mapping")));};
        collection_item = Some(CollectionItem {
            id: row.try_get("id")?,
            version: row.try_get("version")?,
            created_at: row.try_get("created_at")?,
            updated_at: row.try_get("updated_at")?,
        });
        let Some(row)=sqlx::query("SELECT request_id,request_hash,anchor,created_at,updated_at FROM fvoci.task_origins WHERE workspace_id=$1 AND task_id=$2 AND document_id=$3 FOR UPDATE").bind(source).bind(id).bind(selection.document_id).fetch_optional(&mut **tx).await? else {return Ok(Err(PersonalTransferDbError::NotFound));};
        origin = Some(Origin {
            request_id: row.try_get("request_id")?,
            request_hash: row.try_get("request_hash")?,
            anchor: row.try_get("anchor")?,
            created_at: row.try_get("created_at")?,
            updated_at: row.try_get("updated_at")?,
        });
    }
    // Explicitly closed first graph. Nothing dependent is silently discarded.
    // Explicitly closed first graph, classified so the refusal is typed.
    // Same conditions as before; nothing dependent is silently discarded.
    let (hierarchy, native, files, dependent): (bool, bool, bool, bool) = sqlx::query_as(r#"SELECT
      EXISTS(SELECT 1 FROM fvoci.documents WHERE workspace_id=$1 AND parent_id=$2)
      OR EXISTS(SELECT 1 FROM fvoci.tasks WHERE workspace_id=$1 AND parent_id=$3),
      $5 AND (EXISTS(SELECT 1 FROM fvoci.document_states WHERE workspace_id=$1 AND document_id=$2)
      OR EXISTS(SELECT 1 FROM fvoci.document_collab_updates WHERE workspace_id=$1 AND document_id=$2)
      OR EXISTS(SELECT 1 FROM fvoci.document_collab_op_receipts WHERE workspace_id=$1 AND document_id=$2)
      OR EXISTS(SELECT 1 FROM fvoci.task_states WHERE workspace_id=$1 AND task_id=$3)
      OR EXISTS(SELECT 1 FROM fvoci.task_collab_updates WHERE workspace_id=$1 AND task_id=$3)
      OR EXISTS(SELECT 1 FROM fvoci.task_collab_op_receipts WHERE workspace_id=$1 AND task_id=$3)
      OR EXISTS(SELECT 1 FROM fvoci.revisions WHERE workspace_id=$1 AND ((target_kind='document' AND target_id=$2) OR (target_kind='task' AND target_id=$3)))),
      EXISTS(SELECT 1 FROM fvoci.attachments WHERE workspace_id=$1 AND (document_id=$2 OR task_id=$3)),
      EXISTS(SELECT 1 FROM fvoci.comments WHERE workspace_id=$1 AND (document_id=$2 OR task_id=$3))
      OR EXISTS(SELECT 1 FROM fvoci.document_tag_assignments WHERE workspace_id=$1 AND document_id=$2)
      OR EXISTS(SELECT 1 FROM fvoci.document_members WHERE workspace_id=$1 AND document_id=$2)
      OR EXISTS(SELECT 1 FROM fvoci.stars WHERE workspace_id=$1 AND (document_id=$2 OR task_id=$3))
      OR EXISTS(SELECT 1 FROM fvoci.share_links WHERE workspace_id=$1 AND document_id=$2)
      OR EXISTS(SELECT 1 FROM fvoci.task_labels WHERE workspace_id=$1 AND task_id=$3)
      OR EXISTS(SELECT 1 FROM fvoci.github_issue_links WHERE workspace_id=$1 AND task_id=$3)
      OR EXISTS(SELECT 1 FROM fvoci.task_dependencies WHERE workspace_id=$1 AND (blocker_id=$3 OR blocked_id=$3))
      OR EXISTS(SELECT 1 FROM fvoci.collection_items WHERE workspace_id=$1 AND document_id=$2)
      OR EXISTS(SELECT 1 FROM fvoci.collection_values WHERE workspace_id=$1 AND item_id=$4)
      OR EXISTS(SELECT 1 FROM fvoci.collection_choices WHERE workspace_id=$1 AND item_id=$4)
      OR EXISTS(SELECT 1 FROM fvoci.collection_people WHERE workspace_id=$1 AND item_id=$4)
      OR EXISTS(SELECT 1 FROM fvoci.task_origins WHERE workspace_id=$1 AND document_id=$2 AND task_id IS DISTINCT FROM $3)
    "#).bind(source).bind(selection.document_id).bind(selection.task_id).bind(collection_item.as_ref().map(|item|item.id)).bind(selection.action==PersonalTransferAction::Move).fetch_one(&mut **tx).await?;
    let zotero = selection.action == PersonalTransferAction::Move
        && owner_private_zotero_rows(tx, source, actor, selection).await?;
    // The pair's files are inventoried and locked first: a MOVE's retained
    // history and a COPY's current body may show exactly these and no other.
    let mut moved_files = Vec::new();
    if files && !hierarchy {
        match movable_files(tx, source, selection).await? {
            Ok(found) => moved_files = found,
            Err(error) => return Ok(Err(error)),
        }
    }
    // MOVE carries the complete retained native history with the same IDs;
    // only a history that cannot be proven complete and closed refuses.
    let mut native_history = Vec::new();
    if selection.action == PersonalTransferAction::Move && !hierarchy {
        let moved_ids: Vec<Uuid> = moved_files.iter().map(|file| file.id).collect();
        match move_native_history(tx, source, actor, selection, engine, native, &moved_ids).await? {
            Ok(found) => native_history = found,
            Err(error) => return Ok(Err(error)),
        }
    }
    let blocker = if hierarchy {
        Some(Blocker::Hierarchy)
    } else if dependent || zotero {
        Some(Blocker::DependentGraph)
    } else {
        None
    };
    if let Some(blocker) = blocker {
        return Ok(Err(PersonalTransferDbError::Incomplete(
            blocker,
            "native/reference/file/dependent graph adapter",
        )));
    }
    let document_body = match current_body(
        tx,
        source,
        CollabKind::Document,
        selection.document_id,
        &document.content_json,
        engine,
        None,
    )
    .await?
    {
        Ok(value) => value,
        Err(error) => return Ok(Err(error)),
    };
    let mut task_cut = None;
    if let (Some(id), Some(task)) = (selection.task_id, task.as_mut()) {
        let body = match current_body(
            tx,
            source,
            CollabKind::Task,
            id,
            &task.content_json,
            engine,
            None,
        )
        .await?
        {
            Ok(value) => value,
            Err(error) => return Ok(Err(error)),
        };
        if outside_reference(&body.json, selection) {
            return Ok(Err(PersonalTransferDbError::Incomplete(
                Blocker::OutgoingReference,
                "current reference disclosure mapping",
            )));
        }
        let prepared = prepare_derived_body(body.json)
            .map_err(|_| sqlx::Error::Protocol("personal transfer task body invalid".into()))?;
        (task.content_json, task.text, task.chosung) = prepared.into_parts();
        task_cut = Some(body.cut);
    }
    if outside_reference(&document_body.json, selection) {
        return Ok(Err(PersonalTransferDbError::Incomplete(
            Blocker::OutgoingReference,
            "current reference disclosure mapping",
        )));
    }
    let prepared = prepare_derived_body(document_body.json)
        .map_err(|_| sqlx::Error::Protocol("personal transfer document body invalid".into()))?;
    (document.content_json, document.text, document.chosung) = prepared.into_parts();
    if selection.action == PersonalTransferAction::Move {
        // A moved body may only show files that move with it.
        let mut shown = Vec::new();
        body_files(&document.content_json, &mut shown);
        if let Some(task) = &task {
            body_files(&task.content_json, &mut shown);
        }
        if shown
            .iter()
            .any(|id| !id.is_some_and(|id| moved_files.iter().any(|file| file.id == id)))
        {
            return Ok(Err(PersonalTransferDbError::Incomplete(
                Blocker::File,
                "body file outside the moved pair",
            )));
        }
        if let Err(error) = incoming_reference_boundary(tx, source, selection, engine).await? {
            return Ok(Err(error));
        }
    } else {
        // Validation only: each file of the pair maps (to itself here; the
        // commit maps it to the copy's new attachment).
        let pair_files: std::collections::HashMap<Uuid, Uuid> =
            moved_files.iter().map(|file| (file.id, file.id)).collect();
        let pair = PairRefs {
            document: (selection.document_id, selection.document_id),
            task: selection.task_id.map(|id| (id, id)),
        };
        let (_, block_ids) = match copy_body(&document.content_json, &pair_files, &pair) {
            Ok(value) => value,
            Err(error) => return Ok(Err(error)),
        };
        if let Some(anchor) = origin.as_ref().and_then(|origin| origin.anchor.as_ref()) {
            let mapped = block_key(&Value::String(anchor.clone()))
                .is_some_and(|key| block_ids.contains_key(&key));
            if !mapped {
                return Ok(Err(PersonalTransferDbError::Incomplete(
                    Blocker::BlockIdentity,
                    "copy origin block mapping",
                )));
            }
        }
        if let Some(task) = &task {
            if let Err(error) = copy_body(&task.content_json, &pair_files, &pair) {
                return Ok(Err(error));
            }
        }
    }
    // MOVE relocates the task's time graph with the same IDs; COPY creates a
    // task without any and leaves the original's untouched.
    let timer = if selection.action == PersonalTransferAction::Move {
        match timer_snapshot(tx, source, actor, selection).await? {
            Ok(timer) => timer,
            Err(error) => return Ok(Err(error)),
        }
    } else {
        TimerGraph::default()
    };
    Ok(Ok(SourceGraph {
        document,
        files: moved_files,
        native: native_history,
        task,
        origin,
        assignees,
        activities,
        collection_item,
        body_cuts: json!({"document":document_body.cut,"task":task_cut}),
        timer,
    }))
}

/// The moved pair's attachments, row-locked for the transaction: each must be
/// a clean stored object with no live extract/preview lease, as staging
/// requires; anything else refuses before effects.
async fn movable_files(
    tx: &mut Transaction<'_, Postgres>,
    source: Uuid,
    selection: &PersonalTransferSelection,
) -> TransferResult<Vec<FileIdentity>> {
    set_tenant(tx, source).await?;
    let rows: Vec<(FileIdentity, String, bool)> = sqlx::query(
        r#"SELECT id, document_id, task_id, uploader_id, name, mime, declared_mime, storage_key,
             size_bytes, image, scan_status, variants->'preview' AS preview, created_at,
             completed_at, status,
             coalesce(extract_lease_expires_at > clock_timestamp(), false)
             OR coalesce(preview_lease_expires_at > clock_timestamp(), false) AS leased
           FROM fvoci.attachments
           WHERE workspace_id=$1 AND (document_id=$2 OR task_id=$3)
           ORDER BY id FOR UPDATE"#,
    )
    .bind(source)
    .bind(selection.document_id)
    .bind(selection.task_id)
    .fetch_all(&mut **tx)
    .await?
    .iter()
    .map(|row| {
        let file = FileIdentity {
            id: row.try_get("id")?,
            document_id: row.try_get("document_id")?,
            task_id: row.try_get("task_id")?,
            uploader_id: row.try_get("uploader_id")?,
            name: row.try_get("name")?,
            mime: row.try_get("mime")?,
            declared_mime: row.try_get("declared_mime")?,
            storage_key: row.try_get("storage_key")?,
            size_bytes: row.try_get("size_bytes")?,
            image: row.try_get("image")?,
            scan_status: row.try_get("scan_status")?,
            preview: row.try_get("preview")?,
            created_at: row.try_get("created_at")?,
            completed_at: row.try_get("completed_at")?,
        };
        Ok((file, row.try_get("status")?, row.try_get("leased")?))
    })
    .collect::<Result<_, sqlx::Error>>()?;
    let mut files = Vec::with_capacity(rows.len());
    for (file, status, leased) in rows {
        if status != "stored"
            || file.scan_status == "infected"
            || leased
            || file.size_bytes.is_none()
        {
            return Ok(Err(PersonalTransferDbError::Incomplete(
                Blocker::File,
                "attachment not ready",
            )));
        }
        files.push(file);
    }
    Ok(Ok(files))
}

/// Attachment ids a body shows (`attachment`/`image` nodes, `attrs.id`);
/// `None` for a node without a usable id.
fn body_files(node: &Value, out: &mut Vec<Option<Uuid>>) {
    if matches!(
        node.get("type").and_then(Value::as_str),
        Some("attachment" | "image")
    ) {
        out.push(
            node.get("attrs")
                .and_then(|attrs| attrs.get("id"))
                .and_then(Value::as_str)
                .and_then(|id| Uuid::parse_str(id).ok()),
        );
    }
    if let Some(children) = node.get("content").and_then(Value::as_array) {
        for child in children {
            body_files(child, out);
        }
    }
}

/// A staged attachment and the destination row id it is published under
/// (the source id for a MOVE, a new id for a COPY).
struct StagedFile {
    staged: StagedAttachment,
    destination: Uuid,
}

/// Stages the moved pair's attachments before the transaction, so no storage
/// I/O runs inside it; every fresh key is journaled first, so a refusal or a
/// crash leaves only keys the reclaim job deletes.
async fn stage_moved_files(
    pool: &PgPool,
    files: Option<TransferFiles<'_>>,
    source: Uuid,
    actor: Uuid,
    session: Uuid,
    selection: &PersonalTransferSelection,
    admitted: &[FileIdentity],
) -> TransferResult<Vec<StagedFile>> {
    let ids: Vec<Uuid> = admitted.iter().map(|file| file.id).collect();
    let Some(files) = files else {
        return Ok(Err(PersonalTransferDbError::Incomplete(
            Blocker::File,
            "file storage unavailable",
        )));
    };
    let mut staged = Vec::with_capacity(ids.len());
    for id in ids {
        let destination = if selection.action == PersonalTransferAction::Move {
            id
        } else {
            Uuid::now_v7()
        };
        match stage_attachment_for_transfer(
            pool,
            files.storage,
            source,
            selection.destination_workspace_id,
            actor,
            session,
            id,
            destination,
        )
        .await?
        {
            Ok(one) => staged.push(StagedFile {
                staged: one,
                destination,
            }),
            Err(crate::db::attachments::StageAttachmentError::Storage) => {
                return Err(sqlx::Error::Protocol(
                    "personal transfer file storage unavailable".into(),
                ))
            }
            Err(_) => {
                return Ok(Err(PersonalTransferDbError::Incomplete(
                    Blocker::File,
                    "attachment not ready",
                )))
            }
        }
    }
    Ok(Ok(staged))
}

/// In the transfer transaction: the staged objects must be exactly the
/// locked source attachments, the destination quota must admit them under
/// its storage lock, and every staged journal row must still exist (locked
/// here; absent means the reclaim job took the key).
async fn admit_staged_files(
    tx: &mut Transaction<'_, Postgres>,
    files: Option<TransferFiles<'_>>,
    selection: &PersonalTransferSelection,
    graph: &SourceGraph,
    staged: &[StagedFile],
) -> TransferResult<()> {
    let current: Vec<FileIdentity> = staged
        .iter()
        .map(|one| FileIdentity::of(&one.staged.source))
        .collect();
    if current != graph.files {
        return Ok(Err(PersonalTransferDbError::Incomplete(
            Blocker::File,
            "attachment changed while staging",
        )));
    }
    if staged.is_empty() {
        return Ok(Ok(()));
    }
    let Some(files) = files else {
        return Ok(Err(PersonalTransferDbError::Incomplete(
            Blocker::File,
            "file storage unavailable",
        )));
    };
    let dst = selection.destination_workspace_id;
    set_tenant(tx, dst).await?;
    let sizes: Vec<i64> = staged
        .iter()
        .map(|one| one.staged.original.size_bytes)
        .collect();
    if admit_transfer_storage(tx, files.quota, dst, &sizes)
        .await?
        .is_err()
    {
        return Ok(Err(PersonalTransferDbError::Incomplete(
            Blocker::File,
            "destination storage limit",
        )));
    }
    let journal = staged_journal(staged);
    let held: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM (SELECT id FROM fvoci.attachment_object_cleanups WHERE workspace_id=$1 AND id = ANY($2) FOR UPDATE) held",
    )
    .bind(dst)
    .bind(&journal)
    .fetch_one(&mut **tx)
    .await?;
    if held != journal.len() as i64 {
        return Ok(Err(PersonalTransferDbError::Incomplete(
            Blocker::File,
            "staged file reclaimed",
        )));
    }
    Ok(Ok(()))
}

fn staged_journal(staged: &[StagedFile]) -> Vec<Uuid> {
    staged
        .iter()
        .flat_map(|one| {
            std::iter::once(one.staged.original.journal_id).chain(
                one.staged
                    .preview
                    .as_ref()
                    .map(|(preview, _)| preview.journal_id),
            )
        })
        .collect()
}

/// The locked native history of the moved document and selected task through
/// W7's helper (no parser or reseed here), or the typed refusal before any
/// effect. Retained references must stay inside the moved pair. Without an
/// engine, existing native rows refuse rather than move as a bare body.
async fn move_native_history(
    tx: &mut Transaction<'_, Postgres>,
    source: Uuid,
    actor: Uuid,
    selection: &PersonalTransferSelection,
    engine: Option<&TransferBodyEngine>,
    native_rows: bool,
    moved_files: &[Uuid],
) -> TransferResult<Vec<NativeHistoryInventory>> {
    let Some(engine) = engine else {
        return Ok(if native_rows {
            Err(PersonalTransferDbError::Incomplete(
                Blocker::NativeHistory,
                "native history engine unavailable",
            ))
        } else {
            Ok(Vec::new())
        });
    };
    let cancel = tokio_util::sync::CancellationToken::new();
    let mut targets = vec![(CollabKind::Document, selection.document_id)];
    targets.extend(selection.task_id.map(|id| (CollabKind::Task, id)));
    let mut found = Vec::new();
    for (kind, id) in targets {
        set_tenant(tx, source).await?;
        match native_history_inventory(
            tx,
            source,
            actor,
            kind,
            id,
            &engine.engine_bin,
            engine.limits,
            &cancel,
        )
        .await
        {
            Ok(None) => {}
            Ok(Some(inventory)) => {
                // The kind of the reference the closure actually rejects, so an
                // outside retained file refuses as a file, not a reference.
                if let Some(error) = inventory_closure_refusal(&inventory, selection, moved_files) {
                    return Ok(Err(error));
                }
                found.push(inventory);
            }
            Err(error) => return native_refusal(error).map(Err),
        }
    }
    if let Err(error) = admit_native_copy(tx, source, &found).await? {
        return Ok(Err(error));
    }
    Ok(Ok(found))
}

/// Typed refusal for a helper error; worker/cancel and SQL errors are server
/// errors, never a blocker. Reasons are W7's bounded static literals.
fn native_refusal(error: NativeDbError) -> Result<PersonalTransferDbError, sqlx::Error> {
    let incomplete = |blocker, reason| Ok(PersonalTransferDbError::Incomplete(blocker, reason));
    match error {
        NativeDbError::Sql(error) => Err(error),
        NativeDbError::Forbidden => Ok(PersonalTransferDbError::NotFound),
        NativeDbError::Conflict | NativeDbError::Fenced => {
            incomplete(Blocker::NativeHistory, "native history")
        }
        NativeDbError::Archive(error) => {
            let reason = log_reason(&error);
            tracing::info!(reason, "personal transfer native history refused");
            match error {
                ArchiveError::Limit => incomplete(Blocker::InventoryBudget, reason),
                ArchiveError::Unsupported(detail)
                    if detail == "missing native state for captured body" =>
                {
                    incomplete(Blocker::NativeStateMissing, reason)
                }
                ArchiveError::Unsupported(detail) if detail == "native encoding" => {
                    incomplete(Blocker::BodyEncoding, reason)
                }
                ArchiveError::Worker | ArchiveError::Cancelled => Err(sqlx::Error::Protocol(
                    "personal transfer native history engine unavailable".into(),
                )),
                _ => incomplete(Blocker::NativeHistory, reason),
            }
        }
    }
}

async fn destination(
    tx: &mut Transaction<'_, Postgres>,
    actor: Uuid,
    selection: &PersonalTransferSelection,
) -> TransferResult<Destination> {
    let ws = selection.destination_workspace_id;
    let project_id = selection.destination_project_id;
    set_tenant(tx, ws).await?;
    if !workspace_is_live(tx, ws).await?
        || membership_role_for_update(tx, ws, actor).await?.is_none()
    {
        return Ok(Err(PersonalTransferDbError::NotFound));
    }
    let Some((name, kind)) = sqlx::query_as::<_, (String, String)>(
        "SELECT name,kind FROM fvoci.workspaces WHERE id=$1 AND deleted_at IS NULL",
    )
    .bind(ws)
    .fetch_optional(&mut **tx)
    .await?
    else {
        return Ok(Err(PersonalTransferDbError::NotFound));
    };
    if kind != "team" {
        return Ok(Err(PersonalTransferDbError::InvalidInput));
    }
    let Some(project) = lock_project(tx, ws, project_id).await? else {
        return Ok(Err(PersonalTransferDbError::NotFound));
    };
    if project.status == "archived"
        || !project_permission(tx, ws, actor, &project)
            .await?
            .at_least(ProjectPermission::Edit)
    {
        return Ok(Err(PersonalTransferDbError::NotFound));
    }
    let Some(root_id) = project.root_document_id else {
        return Ok(Err(PersonalTransferDbError::NotFound));
    };
    lock_document_rows(tx, ws, &[root_id]).await?;
    let Some((root_path,status))=sqlx::query_as::<_,(String,String)>("SELECT path,status FROM fvoci.documents WHERE workspace_id=$1 AND project_id=$2 AND id=$3 AND deleted_at IS NULL").bind(ws).bind(project_id).bind(root_id).fetch_optional(&mut **tx).await? else{return Ok(Err(PersonalTransferDbError::NotFound));};
    if status == "archived" || depth_of(&root_path) >= MAX_TREE_DEPTH {
        return Ok(Err(PersonalTransferDbError::NotFound));
    }
    let mut collection_id = None;
    if let Some(status_id) = selection.destination_status_id {
        let Some((wip_limit,))=sqlx::query_as::<_,(Option<i32>,)>("SELECT wip_limit FROM fvoci.statuses WHERE workspace_id=$1 AND project_id=$2 AND id=$3 FOR UPDATE").bind(ws).bind(project_id).bind(status_id).fetch_optional(&mut **tx).await? else{return Ok(Err(PersonalTransferDbError::NotFound));};
        if wip_limit.is_some() {
            return Ok(Err(PersonalTransferDbError::Incomplete(
                Blocker::WipReservation,
                "destination WIP reservation",
            )));
        }
        collection_id=sqlx::query_scalar("SELECT id FROM fvoci.collections WHERE workspace_id=$1 AND project_id=$2 AND kind='task' AND deleted_at IS NULL").bind(ws).bind(project_id).fetch_optional(&mut **tx).await?;
        if collection_id.is_none() {
            return Ok(Err(PersonalTransferDbError::NotFound));
        }
    }
    Ok(Ok(Destination {
        workspace_name: name,
        project_name: project.name,
        visibility: project.visibility,
        root_id,
        root_path,
        collection_id,
    }))
}

fn collab_kind_name(kind: CollabKind) -> &'static str {
    match kind {
        CollabKind::Document => "document",
        CollabKind::Task => "task",
    }
}

fn preview_digest(
    source: Uuid,
    actor: Uuid,
    session: Uuid,
    selection: &PersonalTransferSelection,
    graph: &SourceGraph,
    destination: &Destination,
) -> String {
    // Metadata PATCH need not advance the native body version. Bind the actual
    // metadata and update time as well, so an old preview cannot publish edits.
    let doc = &graph.document;
    let task=graph.task.as_ref().map(|task|{let r=&task.record;json!({"id":r.id,"project":r.project_id,"title":r.title,"body":task.content_json,"version":r.version,"updatedAt":r.updated_at,"status":r.status_id,"start":r.start_date,"due":r.due_date,"dueAt":r.due_at,"estimate":r.estimate,"estimateUnit":task.estimate_unit,"recurrence":task.recurrence})});
    let activities:Vec<Value>=graph.activities.iter().map(|a|json!({"id":a.id,"changes":a.changes,"actor":a.actor,"channel":a.channel,"kind":a.kind,"createdAt":a.created_at})).collect();
    // The helper's stable content digest, never its per-transaction binding.
    let native: Vec<Value> = graph
        .native
        .iter()
        .map(|n| json!({"kind":collab_kind_name(n.kind),"target":n.target_id,"cutoff":n.snapshot_cutoff_seq,"tail":n.tail_seq,"content":n.content_digest}))
        .collect();
    let files: Vec<Value> = graph
        .files
        .iter()
        .map(|f| json!({"id":f.id,"document":f.document_id,"task":f.task_id,"uploader":f.uploader_id,"name":f.name,"mime":f.mime,"declaredMime":f.declared_mime,"key":f.storage_key,"size":f.size_bytes,"image":f.image,"scan":f.scan_status,"preview":f.preview,"createdAt":f.created_at,"completedAt":f.completed_at}))
        .collect();
    let canonical = json!({"files":files,"native":native,"source":source,"actor":actor,"session":session,"selection":selection,"document":{"title":doc.title,"icon":doc.icon,"kind":doc.kind,"status":doc.status,"version":doc.version,"body":doc.content_json,"updatedAt":doc.updated_at},"task":task,"bodyCuts":graph.body_cuts,"assignees":graph.assignees,"activity":activities,"timer":graph.timer.digest(),"destination":{"name":destination.workspace_name,"projectName":destination.project_name,"visibility":destination.visibility,"root":destination.root_id,"path":destination.root_path}});
    hex::encode(Sha256::digest(canonical.to_string().as_bytes()))
}

/// Observed parts of the disclosure graph and what this command does to each.
/// Only admitted graphs reach here: files, native history on MOVE, references
/// and dependents have already refused with a typed blocker.
async fn dispositions(
    tx: &mut Transaction<'_, Postgres>,
    source: Uuid,
    actor: Uuid,
    selection: &PersonalTransferSelection,
    graph: &SourceGraph,
) -> Result<Vec<PersonalTransferDisposition>, sqlx::Error> {
    use PersonalTransferItem as Item;
    use PersonalTransferOutcome as Outcome;
    let moving = selection.action == PersonalTransferAction::Move;
    let carried = if moving {
        Outcome::Moved
    } else {
        Outcome::CopiedNewId
    };
    let mut out = vec![PersonalTransferDisposition {
        item: Item::Document,
        outcome: carried,
        count: 1,
    }];
    if graph.task.is_some() {
        out.push(PersonalTransferDisposition {
            item: Item::Task,
            outcome: carried,
            count: 1,
        });
    }
    if !graph.activities.is_empty() {
        out.push(PersonalTransferDisposition {
            item: Item::Activity,
            // A copy starts its own activity; the original's stays private.
            outcome: if moving {
                Outcome::Moved
            } else {
                Outcome::NotIncluded
            },
            count: graph.activities.len() as u32,
        });
    }
    if !graph.files.is_empty() {
        out.push(PersonalTransferDisposition {
            item: Item::Attachment,
            outcome: carried,
            count: u32::try_from(graph.files.len()).unwrap_or(u32::MAX),
        });
    }
    let moved_revisions: usize = graph.native.iter().map(|n| n.revisions.len()).sum();
    if moving && moved_revisions > 0 {
        out.push(PersonalTransferDisposition {
            item: Item::History,
            outcome: Outcome::Moved,
            count: u32::try_from(moved_revisions).unwrap_or(u32::MAX),
        });
    }
    // Time: a MOVE carries the task's entries (tenant-shared) and the actor's
    // own runs; a COPY's task has none and the original keeps them privately.
    let (entries, runs) = if moving {
        (
            graph.timer.entries.len() as i64,
            graph.timer.runs.len() as i64,
        )
    } else if let Some(task) = selection.task_id {
        set_tenant(tx, source).await?;
        let entries: i64 = sqlx::query_scalar(
            "SELECT count(*) FROM fvoci.time_entries WHERE workspace_id=$1 AND task_id=$2",
        )
        .bind(source)
        .bind(task)
        .fetch_one(&mut **tx)
        .await?;
        let previous = previous_self_user(tx).await?;
        set_self_user(tx, actor).await?;
        let runs: i64 = sqlx::query_scalar(
            "SELECT count(*) FROM fvoci.task_timer_runs WHERE workspace_id=$1 AND task_id=$2 AND user_id=$3",
        )
        .bind(source)
        .bind(task)
        .bind(actor)
        .fetch_one(&mut **tx)
        .await?;
        restore_self_user(tx, &previous).await?;
        (entries, runs)
    } else {
        (0, 0)
    };
    let kept = if moving {
        Outcome::Moved
    } else {
        Outcome::RetainedPrivate
    };
    for (item, count) in [(Item::TimeEntry, entries), (Item::Timer, runs)] {
        if count > 0 {
            out.push(PersonalTransferDisposition {
                item,
                outcome: kept,
                count: u32::try_from(count).unwrap_or(u32::MAX),
            });
        }
    }
    if !moving {
        set_tenant(tx, source).await?;
        let revisions: i64 = sqlx::query_scalar("SELECT count(*) FROM fvoci.revisions WHERE workspace_id=$1 AND ((target_kind='document' AND target_id=$2) OR (target_kind='task' AND target_id=$3))")
            .bind(source)
            .bind(selection.document_id)
            .bind(selection.task_id)
            .fetch_one(&mut **tx)
            .await?;
        if revisions > 0 {
            out.push(PersonalTransferDisposition {
                item: Item::History,
                outcome: Outcome::RetainedPrivate,
                count: u32::try_from(revisions).unwrap_or(u32::MAX),
            });
        }
    }
    Ok(out)
}

pub async fn preview_personal_transfer(
    pool: &PgPool,
    source: Uuid,
    actor: Uuid,
    session: Uuid,
    selection: &PersonalTransferSelection,
    engine: Option<&TransferBodyEngine>,
) -> TransferResult<PersonalTransferPreview> {
    if !valid_selection(source, selection) {
        return Ok(Err(PersonalTransferDbError::InvalidInput));
    }
    let mut tx = pool.begin().await?;
    if let Err(err) = writer_prefix(&mut tx, source, actor, session).await? {
        tx.rollback().await?;
        return Ok(Err(err));
    }
    lock_trees(&mut tx, source, selection.destination_workspace_id).await?;
    let graph = match source_graph(&mut tx, source, actor, selection, engine).await? {
        Ok(value) => value,
        Err(err) => {
            tx.rollback().await?;
            return Ok(Err(err));
        }
    };
    let target = match destination(&mut tx, actor, selection).await? {
        Ok(value) => value,
        Err(err) => {
            tx.rollback().await?;
            return Ok(Err(err));
        }
    };
    if !lock_current_source_owner(&mut tx, source, actor, selection.destination_workspace_id)
        .await?
    {
        tx.rollback().await?;
        return Ok(Err(PersonalTransferDbError::NotFound));
    }
    let dispositions = dispositions(&mut tx, source, actor, selection, &graph).await?;
    let preview = PersonalTransferPreview {
        digest: preview_digest(source, actor, session, selection, &graph, &target),
        document_title: graph.document.title,
        task_title: graph.task.as_ref().map(|task| task.record.title.clone()),
        workspace_name: target.workspace_name,
        project_name: target.project_name,
        project_visibility: target.visibility,
        source_retained: selection.action == PersonalTransferAction::Copy,
        attachment_count: u32::try_from(graph.files.len()).unwrap_or(u32::MAX),
        activity_count: graph.activities.len() as u32,
        dispositions,
    };
    tx.rollback().await?;
    Ok(Ok(preview))
}

pub async fn preview_personal_transfer_backend(
    backend: &Backend,
    source: Uuid,
    actor: Uuid,
    session: Uuid,
    selection: &PersonalTransferSelection,
    engine: Option<&TransferBodyEngine>,
) -> TransferResult<PersonalTransferPreview> {
    if let Backend::Postgres(pool) = backend {
        return preview_personal_transfer(pool, source, actor, session, selection, engine).await;
    }
    if !valid_selection(source, selection) {
        return Ok(Err(PersonalTransferDbError::InvalidInput));
    }
    let mut tx = backend.begin_write().await?;
    let result = async {
        let family = family_transfer_tx(&mut tx)?;
        if let Err(error) = family_transfer_writer(family, source, actor, session).await? {
            return Ok(Err(error));
        }
        family.scope_transfer_pair(source, selection.destination_workspace_id)?;
        let graph = match family_source_graph(family, source, actor, selection, engine).await? {
            Ok(graph) => graph,
            Err(error) => return Ok(Err(error)),
        };
        let target = match family_destination(family, actor, selection).await? {
            Ok(target) => target,
            Err(error) => return Ok(Err(error)),
        };
        let dispositions = family_dispositions(family, source, actor, selection, &graph).await?;
        Ok(Ok(PersonalTransferPreview {
            digest: preview_digest(source, actor, session, selection, &graph, &target),
            document_title: graph.document.title,
            task_title: graph.task.as_ref().map(|t| t.record.title.clone()),
            workspace_name: target.workspace_name,
            project_name: target.project_name,
            project_visibility: target.visibility,
            source_retained: selection.action == PersonalTransferAction::Copy,
            attachment_count: u32::try_from(graph.files.len()).unwrap_or(u32::MAX),
            activity_count: graph.activities.len() as u32,
            dispositions,
        }))
    }
    .await;
    tx.rollback().await?;
    result
}

async fn replay(
    tx: &mut Transaction<'_, Postgres>,
    source: Uuid,
    actor: Uuid,
    session: Uuid,
    body: &PersonalTransferBody,
) -> TransferResult<Option<PersonalTransferOutput>> {
    set_tenant(tx, source).await?;
    let Some(row)=sqlx::query("SELECT * FROM fvoci.personal_transfer_commands WHERE workspace_id=$1 AND actor_user_id=$2 AND request_id=$3").bind(source).bind(actor).bind(body.request_id).fetch_optional(&mut **tx).await? else{return Ok(Ok(None));};
    if row.try_get::<String, _>("request_hash")? != request_hash(source, actor, session, body) {
        return Ok(Err(PersonalTransferDbError::Conflict(
            PersonalTransferConflict::CommandChanged,
        )));
    }
    let target = match destination(tx, actor, &body.selection).await? {
        Ok(value) => value,
        Err(err) => return Ok(Err(err)),
    };
    if !lock_current_source_owner(tx, source, actor, body.selection.destination_workspace_id)
        .await?
    {
        return Ok(Err(PersonalTransferDbError::NotFound));
    }
    let document_id: Uuid = row.try_get("document_id")?;
    let task_id: Option<Uuid> = row.try_get("task_id")?;
    let exists:bool=sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM fvoci.documents WHERE workspace_id=$1 AND project_id=$2 AND id=$3 AND deleted_at IS NULL) AND ($4::uuid IS NULL OR EXISTS(SELECT 1 FROM fvoci.tasks WHERE workspace_id=$1 AND project_id=$2 AND id=$4 AND deleted_at IS NULL))").bind(body.selection.destination_workspace_id).bind(body.selection.destination_project_id).bind(document_id).bind(task_id).fetch_one(&mut **tx).await?;
    if !exists {
        return Ok(Err(PersonalTransferDbError::NotFound));
    }
    let _ = target;
    Ok(Ok(Some(PersonalTransferOutput {
        workspace_id: row.try_get("destination_workspace_id")?,
        project_id: row.try_get("destination_project_id")?,
        document_id,
        document_number: row.try_get("document_number")?,
        task_id,
        task_number: row.try_get("task_number")?,
        replayed: true,
    })))
}

/// Real consumer entry point. Both graphs and receipt commit together; any SQL
/// error unwinds the transaction, including FK-trigger receipt retirement.
#[allow(clippy::too_many_arguments)]
pub async fn transfer_personal_item(
    pool: &PgPool,
    source: Uuid,
    actor: Uuid,
    session: Uuid,
    body: &PersonalTransferBody,
    client_ip: Option<&str>,
    channel: &str,
    engine: Option<&TransferBodyEngine>,
    files: Option<TransferFiles<'_>>,
) -> TransferResult<PersonalTransferOutput> {
    if !valid_command(source, body) {
        return Ok(Err(PersonalTransferDbError::InvalidInput));
    }
    // A command is admitted in full (writer, replay, source graph,
    // destination, owner, digest) before anything is staged; that
    // transaction has no effect. Only the admitted files of a MOVE or COPY
    // are staged. Every check runs again below, in the transaction that
    // publishes.
    let mut staged = Vec::new();
    {
        let mut tx = pool.begin().await?;
        match admit_command(&mut tx, source, actor, session, body, engine).await? {
            Err(err) => {
                tx.rollback().await?;
                return Ok(Err(err));
            }
            Ok(Admission::Replayed(output)) => {
                tx.commit().await?;
                return Ok(Ok(output));
            }
            Ok(Admission::Ready(ready)) => {
                let (graph, _) = *ready;
                tx.rollback().await?;
                if !graph.files.is_empty() {
                    staged = match stage_moved_files(
                        pool,
                        files,
                        source,
                        actor,
                        session,
                        &body.selection,
                        &graph.files,
                    )
                    .await?
                    {
                        Ok(staged) => staged,
                        Err(err) => return Ok(Err(err)),
                    };
                }
            }
        }
    }
    let mut tx = pool.begin().await?;
    let (graph, target) = match admit_command(&mut tx, source, actor, session, body, engine).await?
    {
        Err(err) => {
            tx.rollback().await?;
            return Ok(Err(err));
        }
        Ok(Admission::Replayed(output)) => {
            tx.commit().await?;
            return Ok(Ok(output));
        }
        Ok(Admission::Ready(ready)) => *ready,
    };
    if let Err(err) = admit_staged_files(&mut tx, files, &body.selection, &graph, &staged).await? {
        tx.rollback().await?;
        return Ok(Err(err));
    }
    let output = commit_graph(
        &mut tx, source, actor, session, body, &graph, &target, client_ip, channel, engine, &staged,
    )
    .await?;
    tx.commit().await?;
    Ok(Ok(output))
}

#[allow(clippy::too_many_arguments)]
pub async fn transfer_personal_item_backend(
    backend: &Backend,
    source: Uuid,
    actor: Uuid,
    session: Uuid,
    body: &PersonalTransferBody,
    ip: Option<&str>,
    channel: &str,
    engine: Option<&TransferBodyEngine>,
    files: Option<TransferFiles<'_>>,
) -> TransferResult<PersonalTransferOutput> {
    if let Backend::Postgres(pool) = backend {
        return transfer_personal_item(
            pool, source, actor, session, body, ip, channel, engine, files,
        )
        .await;
    }
    if !valid_command(source, body) {
        return Ok(Err(PersonalTransferDbError::InvalidInput));
    }
    let mut tx = backend.begin_write().await?;
    let admission = family_admit_command(
        family_transfer_tx(&mut tx)?,
        source,
        actor,
        session,
        body,
        engine,
    )
    .await;
    tx.rollback().await?;
    let graph = match admission? {
        Err(error) => return Ok(Err(error)),
        Ok(Admission::Replayed(output)) => return Ok(Ok(output)),
        Ok(Admission::Ready(graph)) => graph.0,
    };
    let mut staged = Vec::new();
    if !graph.files.is_empty() {
        let Some(files) = files else {
            return Ok(Err(PersonalTransferDbError::Incomplete(
                Blocker::File,
                "file storage unavailable",
            )));
        };
        for file in &graph.files {
            let destination = if body.selection.action == PersonalTransferAction::Move {
                file.id
            } else {
                Uuid::now_v7()
            };
            match super::attachments::stage_attachment_for_transfer_backend(
                backend,
                files.storage,
                source,
                body.selection.destination_workspace_id,
                actor,
                session,
                file.id,
                destination,
            )
            .await?
            {
                Ok(staged_file) => staged.push(StagedFile {
                    staged: staged_file,
                    destination,
                }),
                Err(super::attachments::StageAttachmentError::Storage) => {
                    return Err(sqlx::Error::Protocol(
                        "personal transfer file storage unavailable".into(),
                    ))
                }
                Err(_) => {
                    return Ok(Err(PersonalTransferDbError::Incomplete(
                        Blocker::File,
                        "attachment not ready",
                    )))
                }
            }
        }
    }
    let mut tx = backend.begin_write().await?;
    let result = async {
        let family = family_transfer_tx(&mut tx)?;
        let (graph, target) =
            match family_admit_command(family, source, actor, session, body, engine).await? {
                Err(error) => return Ok(Err(error)),
                Ok(Admission::Replayed(output)) => return Ok(Ok(output)),
                Ok(Admission::Ready(ready)) => *ready,
            };
        let current: Vec<_> = staged
            .iter()
            .map(|s| FileIdentity::of(&s.staged.source))
            .collect();
        if current != graph.files {
            return Ok(Err(PersonalTransferDbError::Incomplete(
                Blocker::File,
                "attachment changed while staging",
            )));
        }
        if !staged.is_empty() {
            let files =
                files.ok_or_else(|| sqlx::Error::Protocol("transfer storage missing".into()))?;
            family.switch_transfer_tenant(body.selection.destination_workspace_id)?;
            let sizes: Vec<_> = staged
                .iter()
                .map(|s| s.staged.original.size_bytes)
                .collect();
            if super::attachments::admit_transfer_storage_operation(
                &mut OperationTx::SqliteFamily(&mut *family),
                files.quota,
                body.selection.destination_workspace_id,
                &sizes,
            )
            .await?
            .is_err()
            {
                return Ok(Err(PersonalTransferDbError::Incomplete(
                    Blocker::File,
                    "destination storage limit",
                )));
            }
            for id in staged_journal(&staged) {
                if family
                    .query(
                        "SELECT id FROM attachment_object_cleanups WHERE workspace_id=?1 AND id=?2",
                        &[
                            Cell::uuid(body.selection.destination_workspace_id),
                            Cell::uuid(id),
                        ],
                    )
                    .await?
                    .is_empty()
                {
                    return Ok(Err(PersonalTransferDbError::Incomplete(
                        Blocker::File,
                        "staged file reclaimed",
                    )));
                }
            }
        }
        family_commit_graph(
            family, source, actor, session, body, &graph, &target, ip, channel, engine, &staged,
        )
        .await
        .map(Ok)
    }
    .await;
    match result {
        Ok(Ok(output)) => {
            tx.commit_with_cleanup()
                .await
                .map_err(|error| sqlx::Error::AnyDriverError(Box::new(error)))?;
            Ok(Ok(output))
        }
        Ok(Err(error)) => {
            tx.rollback().await?;
            Ok(Err(error))
        }
        Err(error) => {
            if let Err(cleanup) = tx.rollback().await {
                return Err(super::backend::rollback_cleanup_unknown(
                    Some(Box::new(error)),
                    cleanup,
                ));
            }
            Err(error)
        }
    }
}

enum Admission {
    Replayed(PersonalTransferOutput),
    Ready(Box<(SourceGraph, Destination)>),
}

/// The confirmed command's checks, in order, inside `tx`: writer and
/// session, both trees locked, an identical receipt replays, then the
/// current source graph, destination, source owner and the reviewed digest.
async fn admit_command(
    tx: &mut Transaction<'_, Postgres>,
    source: Uuid,
    actor: Uuid,
    session: Uuid,
    body: &PersonalTransferBody,
    engine: Option<&TransferBodyEngine>,
) -> TransferResult<Admission> {
    if let Err(err) = writer_prefix(tx, source, actor, session).await? {
        return Ok(Err(err));
    }
    lock_trees(tx, source, body.selection.destination_workspace_id).await?;
    match replay(tx, source, actor, session, body).await? {
        Ok(Some(output)) => return Ok(Ok(Admission::Replayed(output))),
        Err(err) => return Ok(Err(err)),
        Ok(None) => {}
    }
    let graph = match source_graph(tx, source, actor, &body.selection, engine).await? {
        Ok(value) => value,
        Err(err) => return Ok(Err(err)),
    };
    let target = match destination(tx, actor, &body.selection).await? {
        Ok(value) => value,
        Err(err) => return Ok(Err(err)),
    };
    if !lock_current_source_owner(tx, source, actor, body.selection.destination_workspace_id)
        .await?
    {
        return Ok(Err(PersonalTransferDbError::NotFound));
    }
    if preview_digest(source, actor, session, &body.selection, &graph, &target)
        != body.preview_digest
    {
        return Ok(Err(PersonalTransferDbError::Conflict(
            PersonalTransferConflict::PreviewStale,
        )));
    }
    Ok(Ok(Admission::Ready(Box::new((graph, target)))))
}

#[derive(Clone, Copy, PartialEq, Eq)]
struct NativeTables {
    states: &'static str,
    updates: &'static str,
    receipts: &'static str,
}

fn native_tables(kind: CollabKind) -> (NativeTables, &'static str) {
    match kind {
        CollabKind::Document => (
            NativeTables {
                states: "document_states",
                updates: "document_collab_updates",
                receipts: "document_collab_op_receipts",
            },
            "document_id",
        ),
        CollabKind::Task => (
            NativeTables {
                states: "task_states",
                updates: "task_collab_updates",
                receipts: "task_collab_op_receipts",
            },
            "task_id",
        ),
    }
}

/// Transaction-local staging table for one native table (dropped at commit).
fn staged(table: &str) -> String {
    format!("pg_temp.w2_move_{table}")
}

/// The combined copy of both moved targets fits the helper's limits.
fn within_native_copy_budget(native_bytes: i64, graph_bytes: i64, rows: i64) -> bool {
    native_bytes <= crate::native_history::MAX_BYTES as i64
        && graph_bytes <= crate::native_history::MAX_GRAPH_BYTES as i64
        && rows <= crate::native_history::MAX_ENTRIES as i64
}

/// Pre-effect admission of the native copy: the restricted role can create
/// transaction-local staging tables, and the combined native bytes,
/// revision JSON/text and row count of both targets fit the existing limits.
/// Measured in SQL under the helper's locks; no payload is read.
async fn admit_native_copy(
    tx: &mut Transaction<'_, Postgres>,
    source: Uuid,
    native: &[NativeHistoryInventory],
) -> TransferResult<()> {
    if native.is_empty() {
        return Ok(Ok(()));
    }
    let temp: bool =
        sqlx::query_scalar("SELECT pg_catalog.has_database_privilege(current_database(), 'TEMP')")
            .fetch_one(&mut **tx)
            .await?;
    if !temp {
        return Ok(Err(PersonalTransferDbError::Incomplete(
            Blocker::NativeHistory,
            "native history staging unavailable",
        )));
    }
    set_tenant(tx, source).await?;
    let (mut native_bytes, mut graph_bytes, mut rows) = (0i64, 0i64, 0i64);
    for inventory in native {
        let (tables, id_column) = native_tables(inventory.kind);
        let (bytes, graph, count): (i64, i64, i64) = sqlx::query_as(&format!(
            r#"SELECT
                 coalesce((SELECT octet_length(state) FROM fvoci.{states} WHERE workspace_id=$1 AND {id_column}=$2),0)::bigint
                 + coalesce((SELECT sum(octet_length(payload)) FROM fvoci.{updates} WHERE workspace_id=$1 AND {id_column}=$2),0)::bigint
                 + coalesce((SELECT sum(octet_length(y_snapshot)) FROM fvoci.revisions WHERE workspace_id=$1 AND target_kind=$3 AND target_id=$2),0)::bigint,
               coalesce((SELECT sum(octet_length(content_json::text) + octet_length(text)) FROM fvoci.revisions WHERE workspace_id=$1 AND target_kind=$3 AND target_id=$2),0)::bigint,
               (SELECT count(*) FROM fvoci.{states} WHERE workspace_id=$1 AND {id_column}=$2)
                 + (SELECT count(*) FROM fvoci.{updates} WHERE workspace_id=$1 AND {id_column}=$2)
                 + (SELECT count(*) FROM fvoci.{receipts} WHERE workspace_id=$1 AND {id_column}=$2)
                 + (SELECT count(*) FROM fvoci.revisions WHERE workspace_id=$1 AND target_kind=$3 AND target_id=$2)"#,
            states = tables.states,
            updates = tables.updates,
            receipts = tables.receipts,
        ))
        .bind(source)
        .bind(inventory.target_id)
        .bind(collab_kind_name(inventory.kind))
        .fetch_one(&mut **tx)
        .await?;
        native_bytes = native_bytes.saturating_add(bytes);
        graph_bytes = graph_bytes.saturating_add(graph);
        rows = rows.saturating_add(count);
    }
    if !within_native_copy_budget(native_bytes, graph_bytes, rows) {
        return Ok(Err(PersonalTransferDbError::Incomplete(
            Blocker::InventoryBudget,
            "native history copy budget",
        )));
    }
    Ok(Ok(()))
}

/// Stages every retained native row of each inventoried target in
/// transaction-local tables (server side: no payload enters the
/// application), checks the staged identities against the inventory and
/// deletes the source revisions, which have no cascade from their target.
/// The other rows go with the source document/task delete. Returns the
/// staged tables to restore.
async fn take_native_rows(
    tx: &mut Transaction<'_, Postgres>,
    source: Uuid,
    native: &[NativeHistoryInventory],
) -> Result<Vec<&'static str>, sqlx::Error> {
    let changed = || sqlx::Error::Protocol("personal transfer native history changed".into());
    let mut tables: Vec<&'static str> = Vec::new();
    for inventory in native {
        let (group, _) = native_tables(inventory.kind);
        for table in [group.states, group.updates, group.receipts, "revisions"] {
            if !tables.contains(&table) {
                tables.push(table);
            }
        }
    }
    if tables.is_empty() {
        return Ok(tables);
    }
    set_tenant(tx, source).await?;
    for table in &tables {
        sqlx::query(&format!(
            "CREATE TEMPORARY TABLE {} (LIKE fvoci.{table}) ON COMMIT DROP",
            staged(table)
        ))
        .execute(&mut **tx)
        .await?;
    }
    for inventory in native {
        let (group, id_column) = native_tables(inventory.kind);
        let kind = collab_kind_name(inventory.kind);
        let id = inventory.target_id;
        for table in [group.states, group.updates, group.receipts] {
            sqlx::query(&format!(
                "INSERT INTO {} SELECT * FROM fvoci.{table} WHERE workspace_id=$1 AND {id_column}=$2",
                staged(table)
            ))
            .bind(source)
            .bind(id)
            .execute(&mut **tx)
            .await?;
        }
        let revisions = sqlx::query(&format!(
            "INSERT INTO {} SELECT * FROM fvoci.revisions WHERE workspace_id=$1 AND target_kind=$2 AND target_id=$3",
            staged("revisions")
        ))
        .bind(source)
        .bind(kind)
        .bind(id)
        .execute(&mut **tx)
        .await?
        .rows_affected();
        let cut: Option<(i64, i64)> = sqlx::query_as(&format!(
            "SELECT snapshot_cutoff_seq, tail_seq FROM {} WHERE {id_column}=$1",
            staged(group.states)
        ))
        .bind(id)
        .fetch_optional(&mut **tx)
        .await?;
        let tail: Vec<(i64, Uuid)> = sqlx::query_as(&format!(
            "SELECT seq, op_id FROM {} WHERE {id_column}=$1 ORDER BY seq",
            staged(group.updates)
        ))
        .bind(id)
        .fetch_all(&mut **tx)
        .await?;
        let mut receipts: Vec<(Uuid, i64, Uuid)> = sqlx::query_as(&format!(
            "SELECT op_id, seq, actor_user_id FROM {} WHERE {id_column}=$1",
            staged(group.receipts)
        ))
        .bind(id)
        .fetch_all(&mut **tx)
        .await?;
        let revision_ids: Vec<Uuid> = sqlx::query_scalar(&format!(
            "SELECT id FROM {} WHERE target_kind=$1 AND target_id=$2 ORDER BY created_at, id",
            staged("revisions")
        ))
        .bind(kind)
        .bind(id)
        .fetch_all(&mut **tx)
        .await?;
        let mut expected_receipts = inventory.receipts.clone();
        receipts.sort();
        expected_receipts.sort();
        if cut != Some((inventory.snapshot_cutoff_seq, inventory.tail_seq))
            || tail != inventory.tail
            || receipts != expected_receipts
            || revision_ids != inventory.revisions
        {
            return Err(changed());
        }
        let deleted = sqlx::query(
            "DELETE FROM fvoci.revisions WHERE workspace_id=$1 AND target_kind=$2 AND target_id=$3",
        )
        .bind(source)
        .bind(kind)
        .bind(id)
        .execute(&mut **tx)
        .await?
        .rows_affected();
        if deleted != revisions {
            return Err(changed());
        }
    }
    Ok(tables)
}

/// Restores the staged rows unchanged except for the destination workspace
/// (server side), then drops the staging tables.
async fn restore_native_rows(
    tx: &mut Transaction<'_, Postgres>,
    dst: Uuid,
    tables: &[&'static str],
) -> Result<(), sqlx::Error> {
    set_tenant(tx, dst).await?;
    for table in tables {
        let staged = staged(table);
        sqlx::query(&format!("UPDATE {staged} SET workspace_id=$1"))
            .bind(dst)
            .execute(&mut **tx)
            .await?;
        let staged_rows: i64 = sqlx::query_scalar(&format!("SELECT count(*) FROM {staged}"))
            .fetch_one(&mut **tx)
            .await?;
        let inserted = sqlx::query(&format!("INSERT INTO fvoci.{table} SELECT * FROM {staged}"))
            .execute(&mut **tx)
            .await?
            .rows_affected();
        if inserted as i64 != staged_rows {
            return Err(sqlx::Error::Protocol(
                "personal transfer native history restore incomplete".into(),
            ));
        }
        sqlx::query(&format!("DROP TABLE {staged}"))
            .execute(&mut **tx)
            .await?;
    }
    Ok(())
}

/// The staged attachments in the destination with the staged keys: a MOVE
/// keeps each attachment UUID, a COPY publishes its new UUID under the new
/// document/task (the source rows stay untouched). Content-derived state is
/// carried (name, MIME, size, image, scan result, the staged preview);
/// extraction restarts as for an imported file so the destination's text
/// index is rebuilt; leases start empty. The staged journal rows, locked by
/// [`admit_staged_files`], are deleted here.
async fn publish_staged_files(
    tx: &mut Transaction<'_, Postgres>,
    dst: Uuid,
    staged: &[StagedFile],
    document_id: Uuid,
    task_id: Option<Uuid>,
) -> Result<(), sqlx::Error> {
    if staged.is_empty() {
        return Ok(());
    }
    set_tenant(tx, dst).await?;
    for file in staged {
        let one = &file.staged;
        let att = &one.source;
        let (variants, preview_status) = match &one.preview {
            Some((_, preview)) => (json!({ "preview": preview }), "ok"),
            None if is_image_mime(&att.mime)
                && crate::attachments::preview::preview_mime_supported(&att.mime) =>
            {
                (json!({}), "pending")
            }
            None => (json!({}), "skipped"),
        };
        sqlx::query(
            r#"INSERT INTO fvoci.attachments (
                 id, workspace_id, document_id, task_id, uploader_id, status, name, mime,
                 declared_mime, size_bytes, reserved_size_bytes, storage_key, image, variants,
                 extract_status, scan_status, preview_status, created_at, completed_at
               ) VALUES ($1, $2, $3, $4, $5, 'stored', $6, $7, $8, $9, $9, $10, $11, $12,
                 $13, $14, $15, $16, $17)"#,
        )
        .bind(file.destination)
        .bind(dst)
        .bind(att.document_id.map(|_| document_id))
        .bind(att.task_id.and(task_id))
        .bind(att.uploader_id)
        .bind(&att.name)
        .bind(&att.mime)
        .bind(&att.declared_mime)
        .bind(one.original.size_bytes)
        .bind(&one.original.key)
        .bind(att.image)
        .bind(variants)
        .bind(initial_extract_status(&att.name, &att.mime))
        .bind(&att.scan_status)
        .bind(preview_status)
        .bind(att.created_at)
        .bind(att.completed_at)
        .execute(&mut **tx)
        .await?;
    }
    sqlx::query(
        "DELETE FROM fvoci.attachment_object_cleanups WHERE workspace_id=$1 AND id = ANY($2)",
    )
    .bind(dst)
    .bind(staged_journal(staged))
    .execute(&mut **tx)
    .await?;
    Ok(())
}

#[allow(clippy::too_many_arguments)]
async fn commit_graph(
    tx: &mut Transaction<'_, Postgres>,
    source: Uuid,
    actor: Uuid,
    session: Uuid,
    body: &PersonalTransferBody,
    graph: &SourceGraph,
    target: &Destination,
    ip: Option<&str>,
    channel: &str,
    engine: Option<&TransferBodyEngine>,
    staged: &[StagedFile],
) -> Result<PersonalTransferOutput, sqlx::Error> {
    let selection = &body.selection;
    let dst = selection.destination_workspace_id;
    let project = selection.destination_project_id;
    let moving = selection.action == PersonalTransferAction::Move;
    let document_id = if moving {
        selection.document_id
    } else {
        Uuid::now_v7()
    };
    let task_id = selection
        .task_id
        .map(|id| if moving { id } else { Uuid::now_v7() });
    // Source attachment -> the copy's new attachment (identity for a MOVE).
    let copied_files: std::collections::HashMap<Uuid, Uuid> = staged
        .iter()
        .map(|file| (file.staged.source.id, file.destination))
        .collect();
    // The selected pair -> the copy's new document/task (unused for a MOVE).
    let pair = PairRefs {
        document: (selection.document_id, document_id),
        task: selection.task_id.zip(task_id),
    };
    let (document_json, block_ids) = if moving {
        (
            graph.document.content_json.clone(),
            std::collections::HashMap::new(),
        )
    } else {
        copy_body(&graph.document.content_json, &copied_files, &pair)
            .map_err(|_| sqlx::Error::Protocol("personal transfer copy body changed".into()))?
    };
    let task_json = match &graph.task {
        Some(task) if !moving => Some(
            copy_body(&task.content_json, &copied_files, &pair)
                .map_err(|_| sqlx::Error::Protocol("personal transfer copy body changed".into()))?
                .0,
        ),
        Some(task) => Some(task.content_json.clone()),
        None => None,
    };
    // COPY owns independent current-content collaboration seeds. Source
    // native history/receipts remain untouched; preview never seeds anything.
    let (document_seed, task_seed) = if moving {
        (None, None)
    } else {
        let engine = engine.ok_or_else(|| {
            sqlx::Error::Protocol("personal transfer seed engine unavailable".into())
        })?;
        let seed = SeedEngine::new(engine.engine_bin.clone(), engine.limits);
        let document = seed
            .tiptap_to_yjs_update(&document_json)
            .await
            .map_err(|_| {
                sqlx::Error::Protocol("personal transfer document seed unavailable".into())
            })?;
        let task = match &task_json {
            Some(json) => Some(seed.tiptap_to_yjs_update(json).await.map_err(|_| {
                sqlx::Error::Protocol("personal transfer task seed unavailable".into())
            })?),
            None => None,
        };
        (Some(document), task)
    };
    // Retained native rows, read whole under the source tenant before the
    // delete cascades them; restored byte-identical under the destination.
    let native_rows = if moving {
        take_native_rows(tx, source, &graph.native).await?
    } else {
        Vec::new()
    };
    if moving {
        set_tenant(tx, source).await?;
        // The existing AFTER DELETE trigger journals the old keys in the
        // source workspace, where nothing references them any more.
        if !staged.is_empty() {
            let ids: Vec<Uuid> = staged.iter().map(|one| one.staged.source.id).collect();
            sqlx::query("DELETE FROM fvoci.attachments WHERE workspace_id=$1 AND id = ANY($2)")
                .bind(source)
                .bind(&ids)
                .execute(&mut **tx)
                .await?;
        }
        if let Some(id) = selection.task_id {
            sqlx::query("DELETE FROM fvoci.tasks WHERE workspace_id=$1 AND id=$2")
                .bind(source)
                .bind(id)
                .execute(&mut **tx)
                .await?;
        }
        sqlx::query("DELETE FROM fvoci.documents WHERE workspace_id=$1 AND id=$2")
            .bind(source)
            .bind(selection.document_id)
            .execute(&mut **tx)
            .await?;
        record_document_event_and_audit(
            tx,
            source,
            actor,
            "document.deleted",
            selection.document_id,
            json!({}),
            ip,
        )
        .await?;
        if let Some(task) = &graph.task {
            record_task_event_and_audit(
                tx,
                TaskChangeRecord {
                    workspace_id: source,
                    actor_user_id: actor,
                    verb: "task.deleted",
                    target_type: "task",
                    target_id: task.record.id,
                    payload: json!({"taskId":task.record.id,"projectId":task.record.project_id}),
                    client_ip: ip,
                },
            )
            .await?;
        }
    }
    set_tenant(tx, dst).await?;
    let allocation:i32=sqlx::query_scalar("UPDATE fvoci.projects SET next_number=next_number+$3,updated_at=now() WHERE workspace_id=$1 AND id=$2 RETURNING next_number-$3").bind(dst).bind(project).bind(if task_id.is_some(){2i32}else{1i32}).fetch_one(&mut **tx).await?;
    let doc_number = allocation;
    let task_number = task_id.map(|_| allocation + 1);
    let last_doc:Option<String>=sqlx::query_scalar("SELECT sort_key FROM fvoci.documents WHERE workspace_id=$1 AND parent_id=$2 AND deleted_at IS NULL ORDER BY sort_key COLLATE \"C\" DESC LIMIT 1").bind(dst).bind(target.root_id).fetch_optional(&mut **tx).await?;
    let doc_sort =
        between(last_doc.as_deref(), None).map_err(|err| sqlx::Error::Protocol(err.to_string()))?;
    let d = &graph.document;
    let path = format!("{}.{}", target.root_path, to_path_label(document_id));
    sqlx::query("INSERT INTO fvoci.documents(id,workspace_id,title,icon,path,parent_id,sort_key,project_id,number,status,schema_version,text,chosung,version,created_by,created_at,updated_at,content_json,kind) VALUES($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11,$12,$13,$14,$15,$16,$17,$18,$19)")
        .bind(document_id).bind(dst).bind(&d.title).bind(&d.icon).bind(path).bind(target.root_id).bind(doc_sort).bind(project).bind(doc_number).bind(&d.status).bind(d.schema_version).bind(&d.text).bind(&d.chosung).bind(if moving{d.version}else{1}).bind(if moving{d.created_by}else{actor}).bind(if moving{d.created_at}else{Utc::now()}).bind(if moving{d.updated_at}else{Utc::now()}).bind(&document_json).bind(&d.kind).execute(&mut **tx).await?;
    if let Some(seed) = document_seed {
        sqlx::query("INSERT INTO fvoci.document_states(workspace_id,document_id,state,encoding) VALUES($1,$2,$3,1)")
            .bind(dst).bind(document_id).bind(seed).execute(&mut **tx).await?;
    }
    if let (Some(task), Some(id), Some(number)) = (&graph.task, task_id, task_number) {
        let r = &task.record;
        let last_sort:Option<String>=sqlx::query_scalar("SELECT sort_key FROM fvoci.tasks WHERE workspace_id=$1 AND project_id=$2 AND status_id=$3 AND deleted_at IS NULL ORDER BY sort_key COLLATE \"C\" DESC LIMIT 1").bind(dst).bind(project).bind(selection.destination_status_id).fetch_optional(&mut **tx).await?;
        let sort = between(last_sort.as_deref(), None)
            .map_err(|err| sqlx::Error::Protocol(err.to_string()))?;
        sqlx::query("INSERT INTO fvoci.tasks(id,workspace_id,project_id,number,title,type,priority,status_id,start_date,due_date,due_at,estimate,recurrence,sort_key,schema_version,content_json,version,created_by,created_at,updated_at,text,chosung,estimate_unit) VALUES($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11,$12::text::numeric,$13,$14,$15,$16,$17,$18,$19,$20,$21,$22,$23)")
            .bind(id).bind(dst).bind(project).bind(number).bind(&r.title).bind(&r.task_type).bind(&r.priority).bind(selection.destination_status_id).bind(r.start_date).bind(r.due_date).bind(r.due_at).bind(&r.estimate).bind(&task.recurrence).bind(sort).bind(r.schema_version).bind(task_json.as_ref()).bind(if moving{r.version}else{1}).bind(if moving{r.created_by}else{actor}).bind(if moving{r.created_at}else{Utc::now()}).bind(if moving{r.updated_at}else{Utc::now()}).bind(&task.text).bind(&task.chosung).bind(&task.estimate_unit).execute(&mut **tx).await?;
        if let Some(seed) = &task_seed {
            sqlx::query("INSERT INTO fvoci.task_states(workspace_id,task_id,state,encoding) VALUES($1,$2,$3,1)")
                .bind(dst).bind(id).bind(seed).execute(&mut **tx).await?;
        }
        for assignee in &graph.assignees {
            sqlx::query(
                "INSERT INTO fvoci.task_assignees(workspace_id,task_id,user_id) VALUES($1,$2,$3)",
            )
            .bind(dst)
            .bind(id)
            .bind(assignee)
            .execute(&mut **tx)
            .await?;
        }
        if moving {
            // INSERT task's real trigger attached a fresh item. Replace only this
            // new empty generated item with the original stable item identity.
            if let Some(item) = &graph.collection_item {
                sqlx::query(
                    "DELETE FROM fvoci.collection_items WHERE workspace_id=$1 AND task_id=$2",
                )
                .bind(dst)
                .bind(id)
                .execute(&mut **tx)
                .await?;
                sqlx::query("INSERT INTO fvoci.collection_items(id,workspace_id,collection_id,task_id,version,created_at,updated_at) VALUES($1,$2,$3,$4,$5,$6,$7)").bind(item.id).bind(dst).bind(target.collection_id).bind(id).bind(item.version).bind(item.created_at).bind(item.updated_at).execute(&mut **tx).await?;
            }
            for activity in &graph.activities {
                sqlx::query("INSERT INTO fvoci.task_activity(id,workspace_id,task_id,actor_user_id,channel,kind,changes,created_at) VALUES($1,$2,$3,$4,$5,$6,$7,$8)").bind(activity.id).bind(dst).bind(id).bind(activity.actor).bind(&activity.channel).bind(&activity.kind).bind(&activity.changes).bind(activity.created_at).execute(&mut **tx).await?;
            }
            relocate_timer(tx, dst, actor, id, &graph.timer).await?;
        } else {
            crate::db::task_activity::record_task_activity(
                tx,
                dst,
                id,
                actor,
                channel,
                None,
                &crate::tasks::activity::ActivitySnapshot::new(),
            )
            .await?;
        }
        if let Some(origin) = &graph.origin {
            let anchor = if moving {
                origin.anchor.clone()
            } else {
                origin.anchor.as_ref().and_then(|id| {
                    block_key(&Value::String(id.clone()))
                        .and_then(|key| block_ids.get(&key).cloned())
                })
            };
            sqlx::query("INSERT INTO fvoci.task_origins(workspace_id,task_id,document_id,request_id,request_hash,anchor,created_at,updated_at) VALUES($1,$2,$3,$4,$5,$6,$7,$8)").bind(dst).bind(id).bind(document_id).bind(if moving{origin.request_id}else{body.request_id}).bind(&origin.request_hash).bind(anchor).bind(if moving{origin.created_at}else{Utc::now()}).bind(if moving{origin.updated_at}else{Utc::now()}).execute(&mut **tx).await?;
        }
        record_task_event_and_audit(
            tx,
            TaskChangeRecord {
                workspace_id: dst,
                actor_user_id: actor,
                verb: "task.created",
                target_type: "task",
                target_id: id,
                payload: json!({"taskId":id,"projectId":project}),
                client_ip: ip,
            },
        )
        .await?;
    }
    publish_staged_files(tx, dst, staged, document_id, task_id).await?;
    restore_native_rows(tx, dst, &native_rows).await?;
    record_document_event_and_audit(
        tx,
        dst,
        actor,
        "document.created",
        document_id,
        json!({"projectId":project}),
        ip,
    )
    .await?;
    set_tenant(tx, source).await?;
    sqlx::query("INSERT INTO fvoci.personal_transfer_commands(workspace_id,actor_user_id,request_id,session_id,request_hash,action,destination_workspace_id,destination_project_id,document_id,document_number,task_id,task_number) VALUES($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11,$12)")
        .bind(source).bind(actor).bind(body.request_id).bind(session).bind(request_hash(source,actor,session,body)).bind(selection.action.as_str()).bind(dst).bind(project).bind(document_id).bind(doc_number).bind(task_id).bind(task_number).execute(&mut **tx).await?;
    Ok(PersonalTransferOutput {
        workspace_id: dst,
        project_id: project,
        document_id,
        document_number: doc_number,
        task_id,
        task_number,
        replayed: false,
    })
}

// Family operations hold one reserved writer from authorization through the
// source graph, destination admission and publication. No PG SQL is rewritten.

fn family_transfer_tx(tx: &mut DbTx) -> Result<&mut FamilyTx, sqlx::Error> {
    match tx {
        DbTx::SqliteFamily(family) => Ok(family),
        _ => Err(sqlx::Error::Protocol(
            "expected family transfer writer".into(),
        )),
    }
}

async fn family_transfer_writer(
    tx: &mut FamilyTx,
    source: Uuid,
    actor: Uuid,
    session: Uuid,
) -> TransferResult<()> {
    tx.require_writer()?;
    tx.set_tenant(source)?;
    tx.require_tenant(source)?;
    if !OperationTx::SqliteFamily(&mut *tx)
        .recheck_session(actor, session)
        .await?
    {
        return Ok(Err(PersonalTransferDbError::Forbidden));
    }
    let rows = tx
        .query(
            r#"SELECT EXISTS(SELECT 1
        FROM workspaces w
        JOIN users u ON u.personal_workspace_id=w.id
        JOIN memberships m ON m.workspace_id=w.id
        AND m.user_id=u.id
        WHERE w.id=?1
        AND w.kind='personal'
        AND w.deleted_at IS NULL
        AND u.id=?2
        AND u.deleted_at IS NULL
        AND m.role='owner')"#,
            &[Cell::uuid(source), Cell::uuid(actor)],
        )
        .await?;
    if !rows[0].cell(0)?.boolean()? {
        return Ok(Err(PersonalTransferDbError::NotFound));
    }
    Ok(Ok(()))
}

async fn family_admit_command(
    tx: &mut FamilyTx,
    source: Uuid,
    actor: Uuid,
    session: Uuid,
    body: &PersonalTransferBody,
    engine: Option<&TransferBodyEngine>,
) -> TransferResult<Admission> {
    if let Err(error) = family_transfer_writer(tx, source, actor, session).await? {
        return Ok(Err(error));
    }
    tx.scope_transfer_pair(source, body.selection.destination_workspace_id)?;
    let rows=tx.query(r#"SELECT request_hash,destination_workspace_id,destination_project_id,document_id,document_number,task_id,task_number
        FROM personal_transfer_commands
        WHERE workspace_id=?1
        AND actor_user_id=?2
        AND request_id=?3"#,&[Cell::uuid(source),Cell::uuid(actor),Cell::uuid(body.request_id)]).await?;
    if let Some(r) = rows.first() {
        if r.cell(0)?.string()? != request_hash(source, actor, session, body) {
            return Ok(Err(PersonalTransferDbError::Conflict(
                PersonalTransferConflict::CommandChanged,
            )));
        }
        if let Err(error) = family_destination(tx, actor, &body.selection).await? {
            return Ok(Err(error));
        }
        let doc = r.cell(3)?.id()?;
        let task = r.cell(5)?.optional(Cell::id)?;
        let exists = tx
            .query(
                r#"SELECT EXISTS(SELECT 1
            FROM documents
            WHERE workspace_id=?1
            AND project_id=?2
            AND id=?3
            AND deleted_at IS NULL)
            AND (?4 IS NULL
            OR EXISTS(SELECT 1
            FROM tasks
            WHERE workspace_id=?1
            AND project_id=?2
            AND id=?4
            AND deleted_at IS NULL))"#,
                &[
                    Cell::uuid(body.selection.destination_workspace_id),
                    Cell::uuid(body.selection.destination_project_id),
                    Cell::uuid(doc),
                    Cell::optional_uuid(task),
                ],
            )
            .await?;
        if !exists[0].cell(0)?.boolean()? {
            return Ok(Err(PersonalTransferDbError::NotFound));
        }
        return Ok(Ok(Admission::Replayed(PersonalTransferOutput {
            workspace_id: r.cell(1)?.id()?,
            project_id: r.cell(2)?.id()?,
            document_id: doc,
            document_number: r.cell(4)?.int32()?,
            task_id: task,
            task_number: r.cell(6)?.optional(Cell::int32)?,
            replayed: true,
        })));
    }
    let graph = match family_source_graph(tx, source, actor, &body.selection, engine).await? {
        Ok(graph) => graph,
        Err(error) => return Ok(Err(error)),
    };
    let target = match family_destination(tx, actor, &body.selection).await? {
        Ok(target) => target,
        Err(error) => return Ok(Err(error)),
    };
    if preview_digest(source, actor, session, &body.selection, &graph, &target)
        != body.preview_digest
    {
        return Ok(Err(PersonalTransferDbError::Conflict(
            PersonalTransferConflict::PreviewStale,
        )));
    }
    Ok(Ok(Admission::Ready(Box::new((graph, target)))))
}

async fn family_destination(
    tx: &mut FamilyTx,
    actor: Uuid,
    selection: &PersonalTransferSelection,
) -> TransferResult<Destination> {
    let ws = selection.destination_workspace_id;
    let project = selection.destination_project_id;
    tx.switch_transfer_tenant(ws)?;
    tx.require_tenant(ws)?;
    let mut op = OperationTx::SqliteFamily(&mut *tx);
    if !op.workspace_is_live(ws).await? || op.membership_role(ws, actor, true).await?.is_none() {
        return Ok(Err(PersonalTransferDbError::NotFound));
    }
    let kind = tx
        .query("SELECT kind FROM workspaces WHERE id=?1", &[Cell::uuid(ws)])
        .await?;
    if kind[0].cell(0)?.string()? != "team" {
        return Ok(Err(PersonalTransferDbError::InvalidInput));
    }
    if !OperationTx::SqliteFamily(&mut *tx)
        .project_permission_by_id(ws, actor, project)
        .await?
        .is_some_and(|p| p.at_least(ProjectPermission::Edit))
    {
        return Ok(Err(PersonalTransferDbError::NotFound));
    }
    let rows = tx
        .query(
            r#"SELECT w.name,w.kind,p.name,p.visibility,p.root_document_id,d.path,d.status,p.status
        FROM workspaces w
        JOIN projects p ON p.workspace_id=w.id
        AND p.id=?2
        AND p.deleted_at IS NULL
        JOIN documents d ON d.workspace_id=w.id
        AND d.project_id=p.id
        AND d.id=p.root_document_id
        AND d.deleted_at IS NULL
        WHERE w.id=?1
        AND w.deleted_at IS NULL"#,
            &[Cell::uuid(ws), Cell::uuid(project)],
        )
        .await?;
    let Some(r) = rows.first() else {
        return Ok(Err(PersonalTransferDbError::NotFound));
    };
    if r.cell(1)?.string()? != "team" {
        return Ok(Err(PersonalTransferDbError::InvalidInput));
    }
    let root_path = r.cell(5)?.string()?;
    if r.cell(6)?.string()? == "archived"
        || r.cell(7)?.string()? == "archived"
        || depth_of(&root_path) >= MAX_TREE_DEPTH
    {
        return Ok(Err(PersonalTransferDbError::NotFound));
    }
    let mut collection_id = None;
    if let Some(status) = selection.destination_status_id {
        let rows = tx
            .query(
                "SELECT wip_limit FROM statuses WHERE workspace_id=?1 AND project_id=?2 AND id=?3",
                &[Cell::uuid(ws), Cell::uuid(project), Cell::uuid(status)],
            )
            .await?;
        let Some(status) = rows.first() else {
            return Ok(Err(PersonalTransferDbError::NotFound));
        };
        if status.cell(0)?.optional(Cell::int32)?.is_some() {
            return Ok(Err(PersonalTransferDbError::Incomplete(
                Blocker::WipReservation,
                "destination WIP reservation",
            )));
        }
        collection_id = tx
            .query(
                r#"SELECT id
            FROM collections
            WHERE workspace_id=?1
            AND project_id=?2
            AND kind='task'
            AND deleted_at IS NULL"#,
                &[Cell::uuid(ws), Cell::uuid(project)],
            )
            .await?
            .first()
            .map(|r| r.cell(0)?.id())
            .transpose()?;
        if collection_id.is_none() {
            return Ok(Err(PersonalTransferDbError::NotFound));
        }
    }
    Ok(Ok(Destination {
        workspace_name: r.cell(0)?.string()?,
        project_name: r.cell(2)?.string()?,
        visibility: r.cell(3)?.string()?,
        root_id: r.cell(4)?.id()?,
        root_path,
        collection_id,
    }))
}

#[allow(clippy::too_many_arguments)]
async fn family_current_body(
    tx: &mut FamilyTx,
    ws: Uuid,
    kind: CollabKind,
    id: Uuid,
    fallback: &Value,
    engine: Option<&TransferBodyEngine>,
    budget: Option<&mut ScanBudget>,
) -> TransferResult<CurrentBody> {
    tx.switch_transfer_tenant(ws)?;
    tx.require_tenant(ws)?;
    tx.require_writer()?;
    let args = [Cell::uuid(ws), Cell::uuid(id)];
    let rows = tx
        .query(
            match kind {
                CollabKind::Document => {
                    r#"SELECT state,encoding,writer_generation,snapshot_cutoff_seq,tail_seq
        FROM document_states
        WHERE workspace_id=?1
        AND document_id=?2"#
                }
                CollabKind::Task => {
                    r#"SELECT state,encoding,writer_generation,snapshot_cutoff_seq,tail_seq
        FROM task_states
        WHERE workspace_id=?1
        AND task_id=?2"#
                }
            },
            &args,
        )
        .await?;
    let Some(r) = rows.first() else {
        let orphan = tx
            .query(
                match kind {
                    CollabKind::Document => {
                        r#"SELECT EXISTS(SELECT 1
            FROM document_collab_updates
            WHERE workspace_id=?1
            AND document_id=?2)
            OR EXISTS(SELECT 1
            FROM document_collab_op_receipts
            WHERE workspace_id=?1
            AND document_id=?2)"#
                    }
                    CollabKind::Task => {
                        r#"SELECT EXISTS(SELECT 1
            FROM task_collab_updates
            WHERE workspace_id=?1
            AND task_id=?2)
            OR EXISTS(SELECT 1
            FROM task_collab_op_receipts
            WHERE workspace_id=?1
            AND task_id=?2)"#
                    }
                },
                &args,
            )
            .await?[0]
            .cell(0)?
            .boolean()?;
        return if orphan {
            Ok(Err(PersonalTransferDbError::Incomplete(
                Blocker::NativeStateMissing,
                "native body state missing",
            )))
        } else {
            Ok(Ok(CurrentBody {
                json: fallback.clone(),
                cut: json!({"native":false}),
            }))
        };
    };
    let snapshot = r.cell(0)?.bytes()?;
    if r.cell(1)?.integer()? != 1 {
        return Ok(Err(PersonalTransferDbError::Incomplete(
            Blocker::BodyEncoding,
            "native body encoding",
        )));
    }
    let generation = r.cell(2)?.integer()?;
    let cutoff = r.cell(3)?.integer()?;
    let tail_seq = r.cell(4)?.integer()?;
    let bytes = tx
        .query(
            match kind {
                CollabKind::Document => {
                    r#"SELECT COALESCE(sum(length(payload)),0)
        FROM document_collab_updates
        WHERE workspace_id=?1
        AND document_id=?2
        AND seq>?3"#
                }
                CollabKind::Task => {
                    r#"SELECT COALESCE(sum(length(payload)),0)
        FROM task_collab_updates
        WHERE workspace_id=?1
        AND task_id=?2
        AND seq>?3"#
                }
            },
            &[args[0].clone(), args[1].clone(), Cell::Integer(cutoff)],
        )
        .await?[0]
        .cell(0)?
        .integer()?
        .saturating_add(snapshot.len() as i64);
    if bytes > MAX_COLLAB_LOAD_BYTES || budget.is_some_and(|b| !b.admit(bytes)) {
        return Ok(Err(PersonalTransferDbError::Incomplete(
            Blocker::InventoryBudget,
            "native body budget",
        )));
    }
    let tail = tx
        .query(
            match kind {
                CollabKind::Document => {
                    r#"SELECT seq,op_id,payload
        FROM document_collab_updates
        WHERE workspace_id=?1
        AND document_id=?2
        AND seq>?3
        ORDER BY seq"#
                }
                CollabKind::Task => {
                    r#"SELECT seq,op_id,payload
        FROM task_collab_updates
        WHERE workspace_id=?1
        AND task_id=?2
        AND seq>?3
        ORDER BY seq"#
                }
            },
            &[args[0].clone(), args[1].clone(), Cell::Integer(cutoff)],
        )
        .await?;
    let updates = tail
        .iter()
        .map(|r| Ok((r.cell(0)?.integer()?, r.cell(1)?.id()?, r.cell(2)?.bytes()?)))
        .collect::<Result<Vec<_>, sqlx::Error>>()?;
    if updates.last().map_or(cutoff, |u| u.0) != tail_seq
        || updates
            .iter()
            .enumerate()
            .any(|(i, u)| u.0 != cutoff + i as i64 + 1)
    {
        return Ok(Err(PersonalTransferDbError::Incomplete(
            Blocker::NativeStateMissing,
            "native body cut",
        )));
    }
    let cut = json!({"native":true,"generation":generation,"cutoff":cutoff,"tail":tail_seq,"snapshot":hex_digest(&snapshot),"updates":updates.iter().map(|u|json!({"seq":u.0,"opId":u.1,"payload":hex_digest(&u.2)})).collect::<Vec<_>>()});
    let engine = engine
        .ok_or_else(|| sqlx::Error::Protocol("personal transfer body engine unavailable".into()))?
        .clone();
    let json = tokio::task::spawn_blocking(move || {
        crate::collab::revision::project_persisted_offline(
            engine.engine_bin,
            engine.limits,
            snapshot,
            updates.into_iter().map(|u| u.2).collect(),
        )
    })
    .await
    .map_err(|_| sqlx::Error::Protocol("personal transfer body projection unavailable".into()))?
    .map_err(|_| sqlx::Error::Protocol("personal transfer body projection unavailable".into()))?;
    Ok(Ok(CurrentBody { json, cut }))
}

async fn family_source_graph(
    tx: &mut FamilyTx,
    source: Uuid,
    actor: Uuid,
    selection: &PersonalTransferSelection,
    engine: Option<&TransferBodyEngine>,
) -> TransferResult<SourceGraph> {
    tx.switch_transfer_tenant(source)?;
    tx.require_tenant(source)?;
    tx.require_writer()?;
    let args = [
        Cell::uuid(source),
        Cell::uuid(selection.document_id),
        Cell::optional_uuid(selection.task_id),
    ];
    let docs=tx.query(r#"SELECT title,icon,status,kind,schema_version,version,content_json,text,chosung,created_by,created_at,updated_at,project_id,parent_id
        FROM documents
        WHERE workspace_id=?1
        AND id=?2
        AND deleted_at IS NULL"#,&args[..2]).await?;
    let Some(r) = docs.first() else {
        return Ok(Err(PersonalTransferDbError::NotFound));
    };
    if r.cell(12)?.optional(Cell::id)?.is_some() || r.cell(13)?.optional(Cell::id)?.is_some() {
        return Ok(Err(PersonalTransferDbError::Incomplete(
            Blocker::Hierarchy,
            "source hierarchy adapter",
        )));
    }
    let mut document = DocumentRow {
        title: r.cell(0)?.string()?,
        icon: r.cell(1)?.optional(Cell::string)?,
        status: r.cell(2)?.string()?,
        kind: r.cell(3)?.string()?,
        schema_version: r.cell(4)?.int32()?,
        version: r.cell(5)?.int32()?,
        content_json: r.cell(6)?.value()?,
        text: r.cell(7)?.string()?,
        chosung: r.cell(8)?.string()?,
        created_by: r.cell(9)?.id()?,
        created_at: r.cell(10)?.datetime()?,
        updated_at: r.cell(11)?.datetime()?,
    };
    if document.status == "archived" {
        return Ok(Err(PersonalTransferDbError::NotFound));
    }
    if document.version != selection.expected_document_version {
        return Ok(Err(PersonalTransferDbError::Conflict(
            PersonalTransferConflict::PreviewStale,
        )));
    }
    let (mut task, mut origin, mut collection_item) = (None, None, None);
    let mut assignees = Vec::new();
    let mut activities = Vec::new();
    if let Some(id) = selection.task_id {
        let Some((meta, deleted)) = super::tasks::family_task_row(tx, source, id).await? else {
            return Ok(Err(PersonalTransferDbError::NotFound));
        };
        if deleted || meta.archived_at.is_some() {
            return Ok(Err(PersonalTransferDbError::NotFound));
        }
        let project=tx.query("SELECT status FROM projects WHERE workspace_id=?1 AND id=?2 AND deleted_at IS NULL",&[Cell::uuid(source),Cell::uuid(meta.project_id)]).await?;
        if project
            .first()
            .map(|r| r.cell(0)?.string())
            .transpose()?
            .is_none_or(|s| s == "archived")
            || !OperationTx::SqliteFamily(&mut *tx)
                .project_permission_by_id(source, actor, meta.project_id)
                .await?
                .is_some_and(|p| p.at_least(ProjectPermission::Edit))
        {
            return Ok(Err(PersonalTransferDbError::NotFound));
        }
        if Some(meta.version) != selection.expected_task_version {
            return Ok(Err(PersonalTransferDbError::Conflict(
                PersonalTransferConflict::PreviewStale,
            )));
        }
        if meta.parent_id.is_some() || meta.milestone_id.is_some() || meta.task_type == "subtask" {
            return Ok(Err(PersonalTransferDbError::Incomplete(
                Blocker::Hierarchy,
                "task hierarchy mapping",
            )));
        }
        let taskargs = [Cell::uuid(source), Cell::uuid(id)];
        assignees = tx
            .query(
                r#"SELECT user_id
            FROM task_assignees
            WHERE workspace_id=?1
            AND task_id=?2
            ORDER BY user_id"#,
                &taskargs,
            )
            .await?
            .iter()
            .map(|r| r.cell(0)?.id())
            .collect::<Result<_, _>>()?;
        if assignees.iter().any(|id| *id != actor) {
            return Ok(Err(PersonalTransferDbError::Incomplete(
                Blocker::Assignee,
                "assignee mapping",
            )));
        }
        let extra=tx.query("SELECT estimate_unit,content_json,text,chosung FROM tasks WHERE workspace_id=?1 AND id=?2",&taskargs).await?;
        let e = &extra[0];
        let record = TaskRowRecord {
            id: meta.id,
            project_id: meta.project_id,
            number: meta.number,
            title: meta.title,
            task_type: meta.task_type,
            priority: meta.priority,
            status_id: meta.status_id,
            start_date: meta.start_date,
            due_date: meta.due_date,
            due_at: meta.due_at,
            estimate: meta.estimate,
            parent_id: meta.parent_id,
            milestone_id: meta.milestone_id,
            sort_key: meta.sort_key,
            schema_version: meta.schema_version,
            version: meta.version,
            archived_at: meta.archived_at,
            created_by: meta.created_by,
            created_at: meta.created_at,
            updated_at: meta.updated_at,
        };
        task = Some(TaskSource {
            record,
            estimate_unit: e.cell(0)?.optional(Cell::string)?,
            content_json: e.cell(1)?.value()?,
            text: e.cell(2)?.string()?,
            chosung: e.cell(3)?.string()?,
            recurrence: meta.recurrence,
        });
        activities = tx
            .query(
                r#"SELECT id,actor_user_id,channel,kind,changes,created_at
            FROM task_activity
            WHERE workspace_id=?1
            AND task_id=?2
            ORDER BY created_at,id LIMIT 1001"#,
                &taskargs,
            )
            .await?
            .iter()
            .map(|r| {
                Ok(Activity {
                    id: r.cell(0)?.id()?,
                    actor: r.cell(1)?.optional(Cell::id)?,
                    channel: r.cell(2)?.string()?,
                    kind: r.cell(3)?.string()?,
                    changes: r.cell(4)?.value()?,
                    created_at: r.cell(5)?.datetime()?,
                })
            })
            .collect::<Result<_, sqlx::Error>>()?;
        if activities.len() > 1000 {
            return Ok(Err(PersonalTransferDbError::Incomplete(
                Blocker::InventoryBudget,
                "activity budget",
            )));
        }
        let rows = tx
            .query(
                r#"SELECT i.id,i.version,i.created_at,i.updated_at
            FROM collection_items i
            JOIN collections c ON c.workspace_id=i.workspace_id
            AND c.id=i.collection_id
            WHERE i.workspace_id=?1
            AND i.task_id=?2
            AND c.project_id=?3
            AND c.kind='task'
            AND c.deleted_at IS NULL"#,
                &[
                    taskargs[0].clone(),
                    taskargs[1].clone(),
                    Cell::uuid(meta.project_id),
                ],
            )
            .await?;
        let Some(r) = rows.first() else {
            return Ok(Err(PersonalTransferDbError::Incomplete(
                Blocker::DependentGraph,
                "task collection mapping",
            )));
        };
        collection_item = Some(CollectionItem {
            id: r.cell(0)?.id()?,
            version: r.cell(1)?.int32()?,
            created_at: r.cell(2)?.datetime()?,
            updated_at: r.cell(3)?.datetime()?,
        });
        let rows = tx
            .query(
                r#"SELECT request_id,request_hash,anchor,created_at,updated_at
            FROM task_origins
            WHERE workspace_id=?1
            AND task_id=?2
            AND document_id=?3"#,
                &[taskargs[0].clone(), taskargs[1].clone(), args[1].clone()],
            )
            .await?;
        let Some(r) = rows.first() else {
            return Ok(Err(PersonalTransferDbError::NotFound));
        };
        origin = Some(Origin {
            request_id: r.cell(0)?.id()?,
            request_hash: r.cell(1)?.string()?,
            anchor: r.cell(2)?.optional(Cell::string)?,
            created_at: r.cell(3)?.datetime()?,
            updated_at: r.cell(4)?.datetime()?,
        });
    }
    let checks=tx.query("SELECT
      EXISTS(SELECT 1 FROM documents WHERE workspace_id=?1 AND parent_id=?2) OR EXISTS(SELECT 1 FROM tasks WHERE workspace_id=?1 AND parent_id=?3),
      EXISTS(SELECT 1 FROM attachments WHERE workspace_id=?1 AND (document_id=?2 OR task_id=?3)),
      EXISTS(SELECT 1 FROM comments WHERE workspace_id=?1 AND (document_id=?2 OR task_id=?3))
      OR EXISTS(SELECT 1 FROM document_tag_assignments WHERE workspace_id=?1 AND document_id=?2)
      OR EXISTS(SELECT 1 FROM document_members WHERE workspace_id=?1 AND document_id=?2)
      OR EXISTS(SELECT 1 FROM stars WHERE workspace_id=?1 AND (document_id=?2 OR task_id=?3))
      OR EXISTS(SELECT 1 FROM share_links WHERE workspace_id=?1 AND document_id=?2)
      OR EXISTS(SELECT 1 FROM task_labels WHERE workspace_id=?1 AND task_id=?3)
      OR EXISTS(SELECT 1 FROM github_issue_links WHERE workspace_id=?1 AND task_id=?3)
      OR EXISTS(SELECT 1 FROM task_dependencies WHERE workspace_id=?1 AND (blocker_id=?3 OR blocked_id=?3))
      OR EXISTS(SELECT 1 FROM collection_items WHERE workspace_id=?1 AND document_id=?2)
      OR EXISTS(SELECT 1 FROM collection_values WHERE workspace_id=?1 AND item_id=?4)
      OR EXISTS(SELECT 1 FROM collection_choices WHERE workspace_id=?1 AND item_id=?4)
      OR EXISTS(SELECT 1 FROM collection_people WHERE workspace_id=?1 AND item_id=?4)
      OR EXISTS(SELECT 1 FROM task_origins WHERE workspace_id=?1 AND document_id=?2 AND task_id IS NOT ?3)",
      &[args[0].clone(),args[1].clone(),args[2].clone(),Cell::optional_uuid(collection_item.as_ref().map(|i|i.id))]).await?;
    if checks[0].cell(0)?.boolean()? {
        return Ok(Err(PersonalTransferDbError::Incomplete(
            Blocker::Hierarchy,
            "native/reference/file/dependent graph adapter",
        )));
    }
    let moving = selection.action == PersonalTransferAction::Move;
    let zotero = moving
        && tx
            .query(
                r#"SELECT EXISTS(SELECT 1
        FROM zotero_references
        WHERE workspace_id=?1
        AND document_id=?2
        AND owner_user_id=?4)
        OR EXISTS(SELECT 1
        FROM zotero_links
        WHERE workspace_id=?1
        AND (document_id=?2
        OR task_id=?3)
        AND owner_user_id=?4)"#,
                &[
                    args[0].clone(),
                    args[1].clone(),
                    args[2].clone(),
                    Cell::uuid(actor),
                ],
            )
            .await?[0]
            .cell(0)?
            .boolean()?;
    if checks[0].cell(2)?.boolean()? || zotero {
        return Ok(Err(PersonalTransferDbError::Incomplete(
            Blocker::DependentGraph,
            "native/reference/file/dependent graph adapter",
        )));
    }
    let files = if checks[0].cell(1)?.boolean()? {
        match family_movable_files(tx, source, selection).await? {
            Ok(files) => files,
            Err(error) => return Ok(Err(error)),
        }
    } else {
        Vec::new()
    };
    let native = if moving {
        let ids: Vec<_> = files.iter().map(|f| f.id).collect();
        match family_move_native(tx, source, actor, selection, engine, &ids).await? {
            Ok(native) => native,
            Err(error) => return Ok(Err(error)),
        }
    } else {
        Vec::new()
    };
    let body = match family_current_body(
        tx,
        source,
        CollabKind::Document,
        selection.document_id,
        &document.content_json,
        engine,
        None,
    )
    .await?
    {
        Ok(body) => body,
        Err(error) => return Ok(Err(error)),
    };
    let mut task_cut = None;
    if let (Some(id), Some(task)) = (selection.task_id, task.as_mut()) {
        let body = match family_current_body(
            tx,
            source,
            CollabKind::Task,
            id,
            &task.content_json,
            engine,
            None,
        )
        .await?
        {
            Ok(body) => body,
            Err(error) => return Ok(Err(error)),
        };
        if outside_reference(&body.json, selection) {
            return Ok(Err(PersonalTransferDbError::Incomplete(
                Blocker::OutgoingReference,
                "current reference disclosure mapping",
            )));
        }
        let prepared = prepare_derived_body(body.json)
            .map_err(|_| sqlx::Error::Protocol("personal transfer task body invalid".into()))?;
        (task.content_json, task.text, task.chosung) = prepared.into_parts();
        task_cut = Some(body.cut);
    }
    if outside_reference(&body.json, selection) {
        return Ok(Err(PersonalTransferDbError::Incomplete(
            Blocker::OutgoingReference,
            "current reference disclosure mapping",
        )));
    }
    let prepared = prepare_derived_body(body.json)
        .map_err(|_| sqlx::Error::Protocol("personal transfer document body invalid".into()))?;
    (document.content_json, document.text, document.chosung) = prepared.into_parts();
    if moving {
        let mut shown = Vec::new();
        body_files(&document.content_json, &mut shown);
        if let Some(task) = &task {
            body_files(&task.content_json, &mut shown);
        }
        if shown
            .iter()
            .any(|id| !id.is_some_and(|id| files.iter().any(|f| f.id == id)))
        {
            return Ok(Err(PersonalTransferDbError::Incomplete(
                Blocker::File,
                "body file outside the moved pair",
            )));
        }
        if let Err(error) = family_incoming_reference(tx, source, selection, engine).await? {
            return Ok(Err(error));
        }
    } else {
        let pair_files = files.iter().map(|f| (f.id, f.id)).collect();
        let pair = PairRefs {
            document: (selection.document_id, selection.document_id),
            task: selection.task_id.map(|id| (id, id)),
        };
        let (_, blocks) = match copy_body(&document.content_json, &pair_files, &pair) {
            Ok(body) => body,
            Err(error) => return Ok(Err(error)),
        };
        if let Some(anchor) = origin.as_ref().and_then(|o| o.anchor.as_ref()) {
            if !block_key(&Value::String(anchor.clone()))
                .is_some_and(|key| blocks.contains_key(&key))
            {
                return Ok(Err(PersonalTransferDbError::Incomplete(
                    Blocker::BlockIdentity,
                    "copy origin block mapping",
                )));
            }
        }
        if let Some(task) = &task {
            if let Err(error) = copy_body(&task.content_json, &pair_files, &pair) {
                return Ok(Err(error));
            }
        }
    }
    let timer = if moving {
        match family_timer_snapshot(tx, source, actor, selection).await? {
            Ok(timer) => timer,
            Err(error) => return Ok(Err(error)),
        }
    } else {
        TimerGraph::default()
    };
    Ok(Ok(SourceGraph {
        document,
        task,
        origin,
        collection_item,
        files,
        native,
        assignees,
        activities,
        body_cuts: json!({"document":body.cut,"task":task_cut}),
        timer,
    }))
}

async fn family_movable_files(
    tx: &mut FamilyTx,
    source: Uuid,
    selection: &PersonalTransferSelection,
) -> TransferResult<Vec<FileIdentity>> {
    tx.require_tenant(source)?;
    let rows=tx.query(r#"SELECT id,document_id,task_id,uploader_id,name,mime,declared_mime,storage_key,size_bytes,image,scan_status,json_extract(variants,'$.preview'),created_at,completed_at,status,COALESCE(extract_lease_expires_at>(unixepoch()*1000000+CAST(substr(strftime('%f','now'),4,3) AS INTEGER)*1000),0)
        OR COALESCE(preview_lease_expires_at>(unixepoch()*1000000+CAST(substr(strftime('%f','now'),4,3) AS INTEGER)*1000),0)
        FROM attachments
        WHERE workspace_id=?1
        AND (document_id=?2
        OR task_id=?3)
        ORDER BY id"#,&[Cell::uuid(source),Cell::uuid(selection.document_id),Cell::optional_uuid(selection.task_id)]).await?;
    let mut files = Vec::new();
    for r in rows {
        let file = FileIdentity {
            id: r.cell(0)?.id()?,
            document_id: r.cell(1)?.optional(Cell::id)?,
            task_id: r.cell(2)?.optional(Cell::id)?,
            uploader_id: r.cell(3)?.id()?,
            name: r.cell(4)?.string()?,
            mime: r.cell(5)?.string()?,
            declared_mime: r.cell(6)?.optional(Cell::string)?,
            storage_key: r.cell(7)?.string()?,
            size_bytes: r.cell(8)?.optional(Cell::integer)?,
            image: r.cell(9)?.boolean()?,
            scan_status: r.cell(10)?.string()?,
            preview: r.cell(11)?.optional(Cell::value)?,
            created_at: r.cell(12)?.datetime()?,
            completed_at: r.cell(13)?.optional(Cell::datetime)?,
        };
        if r.cell(14)?.string()? != "stored"
            || r.cell(15)?.boolean()?
            || file.scan_status == "infected"
            || file.size_bytes.is_none()
        {
            return Ok(Err(PersonalTransferDbError::Incomplete(
                Blocker::File,
                "attachment not ready",
            )));
        }
        files.push(file);
    }
    Ok(Ok(files))
}

async fn family_incoming_reference(
    tx: &mut FamilyTx,
    source: Uuid,
    selection: &PersonalTransferSelection,
    engine: Option<&TransferBodyEngine>,
) -> TransferResult<()> {
    tx.require_tenant(source)?;
    let rows = tx
        .query(
            r#"SELECT 'document',id,content_json
        FROM documents
        WHERE workspace_id=?1
        AND deleted_at IS NULL
        AND id<>?2 UNION ALL SELECT 'task',id,content_json
        FROM tasks
        WHERE workspace_id=?1
        AND deleted_at IS NULL
        AND id IS NOT ?3
        ORDER BY 1,2 LIMIT 1001"#,
            &[
                Cell::uuid(source),
                Cell::uuid(selection.document_id),
                Cell::optional_uuid(selection.task_id),
            ],
        )
        .await?;
    if rows.len() > 1000 {
        return Ok(Err(PersonalTransferDbError::Incomplete(
            Blocker::InventoryBudget,
            "incoming reference inventory budget",
        )));
    }
    let mut budget = ScanBudget::default();
    for r in rows {
        let kind = if r.cell(0)?.string()? == "document" {
            CollabKind::Document
        } else {
            CollabKind::Task
        };
        let body = match family_current_body(
            tx,
            source,
            kind,
            r.cell(1)?.id()?,
            &r.cell(2)?.value()?,
            engine,
            Some(&mut budget),
        )
        .await?
        {
            Ok(body) => body,
            Err(error) => return Ok(Err(error)),
        };
        if extract_internal_refs(&body.json).iter().any(|reference| {
            let Ok(id) = Uuid::parse_str(&reference.id) else {
                return false;
            };
            match reference.kind {
                InternalRefKind::Document => id == selection.document_id,
                InternalRefKind::Task => Some(id) == selection.task_id,
            }
        }) {
            return Ok(Err(PersonalTransferDbError::Incomplete(
                Blocker::IncomingReference,
                "incoming private reference mapping",
            )));
        }
    }
    Ok(Ok(()))
}

async fn family_move_native(
    tx: &mut FamilyTx,
    source: Uuid,
    actor: Uuid,
    selection: &PersonalTransferSelection,
    engine: Option<&TransferBodyEngine>,
    moved_files: &[Uuid],
) -> TransferResult<Vec<NativeHistoryInventory>> {
    tx.require_tenant(source)?;
    let mut native_rows = false;
    for (kind, id) in std::iter::once((CollabKind::Document, selection.document_id))
        .chain(selection.task_id.map(|id| (CollabKind::Task, id)))
    {
        native_rows |= tx
            .query(
                match kind {
                    CollabKind::Document => {
                        r#"SELECT EXISTS(SELECT 1
            FROM document_states
            WHERE workspace_id=?1
            AND document_id=?2)
            OR EXISTS(SELECT 1
            FROM document_collab_updates
            WHERE workspace_id=?1
            AND document_id=?2)
            OR EXISTS(SELECT 1
            FROM document_collab_op_receipts
            WHERE workspace_id=?1
            AND document_id=?2)
            OR EXISTS(SELECT 1
            FROM revisions
            WHERE workspace_id=?1
            AND target_id=?2
            AND target_kind=?3)"#
                    }
                    CollabKind::Task => {
                        r#"SELECT EXISTS(SELECT 1
            FROM task_states
            WHERE workspace_id=?1
            AND task_id=?2)
            OR EXISTS(SELECT 1
            FROM task_collab_updates
            WHERE workspace_id=?1
            AND task_id=?2)
            OR EXISTS(SELECT 1
            FROM task_collab_op_receipts
            WHERE workspace_id=?1
            AND task_id=?2)
            OR EXISTS(SELECT 1
            FROM revisions
            WHERE workspace_id=?1
            AND target_id=?2
            AND target_kind=?3)"#
                    }
                },
                &[
                    Cell::uuid(source),
                    Cell::uuid(id),
                    Cell::text(collab_kind_name(kind)),
                ],
            )
            .await?[0]
            .cell(0)?
            .boolean()?;
    }
    let Some(engine) = engine else {
        return Ok(if native_rows {
            Err(PersonalTransferDbError::Incomplete(
                Blocker::NativeHistory,
                "native history engine unavailable",
            ))
        } else {
            Ok(Vec::new())
        });
    };
    let cancel = tokio_util::sync::CancellationToken::new();
    let mut targets = vec![(CollabKind::Document, selection.document_id)];
    targets.extend(selection.task_id.map(|id| (CollabKind::Task, id)));
    let mut found = Vec::new();
    for (kind, id) in targets {
        tx.switch_transfer_tenant(source)?;
        tx.require_tenant(source)?;
        match super::native_history::native_history_inventory_family(
            tx,
            source,
            actor,
            kind,
            id,
            &engine.engine_bin,
            engine.limits,
            &cancel,
        )
        .await
        {
            Ok(None) => {}
            Ok(Some(inventory)) => {
                // The kind of the reference the closure actually rejects, so an
                // outside retained file refuses as a file, not a reference.
                if let Some(error) = inventory_closure_refusal(&inventory, selection, moved_files) {
                    return Ok(Err(error));
                }
                found.push(inventory);
            }
            Err(error) => return native_refusal(error).map(Err),
        }
    }
    if let Err(error) = family_admit_native_copy(tx, source, &found).await? {
        return Ok(Err(error));
    }
    Ok(Ok(found))
}

async fn family_dispositions(
    tx: &mut FamilyTx,
    source: Uuid,
    actor: Uuid,
    selection: &PersonalTransferSelection,
    graph: &SourceGraph,
) -> Result<Vec<PersonalTransferDisposition>, sqlx::Error> {
    use PersonalTransferItem as Item;
    use PersonalTransferOutcome as Outcome;
    tx.switch_transfer_tenant(source)?;
    tx.require_tenant(source)?;
    let moving = selection.action == PersonalTransferAction::Move;
    let carried = if moving {
        Outcome::Moved
    } else {
        Outcome::CopiedNewId
    };
    let mut out = vec![PersonalTransferDisposition {
        item: Item::Document,
        outcome: carried,
        count: 1,
    }];
    if graph.task.is_some() {
        out.push(PersonalTransferDisposition {
            item: Item::Task,
            outcome: carried,
            count: 1,
        });
    }
    if !graph.activities.is_empty() {
        out.push(PersonalTransferDisposition {
            item: Item::Activity,
            // A copy starts its own activity; the original's stays private.
            outcome: if moving {
                Outcome::Moved
            } else {
                Outcome::NotIncluded
            },
            count: graph.activities.len() as u32,
        });
    }
    if !graph.files.is_empty() {
        out.push(PersonalTransferDisposition {
            item: Item::Attachment,
            outcome: carried,
            count: u32::try_from(graph.files.len()).unwrap_or(u32::MAX),
        });
    }
    let moved_revisions: usize = graph.native.iter().map(|n| n.revisions.len()).sum();
    if moving && moved_revisions > 0 {
        out.push(PersonalTransferDisposition {
            item: Item::History,
            outcome: Outcome::Moved,
            count: u32::try_from(moved_revisions).unwrap_or(u32::MAX),
        });
    }
    // Time: a MOVE carries the task's entries (tenant-shared) and the actor's
    // own runs; a COPY's task has none and the original keeps them privately.
    let (entries, runs) = if moving {
        (
            graph.timer.entries.len() as i64,
            graph.timer.runs.len() as i64,
        )
    } else if let Some(task) = selection.task_id {
        tx.switch_transfer_tenant(source)?;
        tx.require_tenant(source)?;
        let entries = tx
            .query(
                "SELECT count(*) FROM time_entries WHERE workspace_id=?1 AND task_id=?2",
                &[Cell::uuid(source), Cell::uuid(task)],
            )
            .await?[0]
            .cell(0)?
            .integer()?;
        let runs = tx
            .query(
                r#"SELECT count(*)
            FROM task_timer_runs
            WHERE workspace_id=?1
            AND task_id=?2
            AND user_id=?3"#,
                &[Cell::uuid(source), Cell::uuid(task), Cell::uuid(actor)],
            )
            .await?[0]
            .cell(0)?
            .integer()?;
        (entries, runs)
    } else {
        (0, 0)
    };
    let kept = if moving {
        Outcome::Moved
    } else {
        Outcome::RetainedPrivate
    };
    for (item, count) in [(Item::TimeEntry, entries), (Item::Timer, runs)] {
        if count > 0 {
            out.push(PersonalTransferDisposition {
                item,
                outcome: kept,
                count: u32::try_from(count).unwrap_or(u32::MAX),
            });
        }
    }
    if !moving {
        tx.switch_transfer_tenant(source)?;
        tx.require_tenant(source)?;
        let revisions = tx
            .query(
                r#"SELECT count(*)
            FROM revisions
            WHERE workspace_id=?1
            AND ((target_kind='document'
            AND target_id=?2)
            OR (target_kind='task'
            AND target_id=?3))"#,
                &[
                    Cell::uuid(source),
                    Cell::uuid(selection.document_id),
                    Cell::optional_uuid(selection.task_id),
                ],
            )
            .await?[0]
            .cell(0)?
            .integer()?;
        if revisions > 0 {
            out.push(PersonalTransferDisposition {
                item: Item::History,
                outcome: Outcome::RetainedPrivate,
                count: u32::try_from(revisions).unwrap_or(u32::MAX),
            });
        }
    }
    Ok(out)
}

async fn family_admit_native_copy(
    tx: &mut FamilyTx,
    source: Uuid,
    native: &[NativeHistoryInventory],
) -> TransferResult<()> {
    if native.is_empty() {
        return Ok(Ok(()));
    }
    tx.switch_transfer_tenant(source)?;
    tx.require_tenant(source)?;
    let (mut native_bytes, mut graph_bytes, mut rows) = (0i64, 0i64, 0i64);
    for inventory in native {
        let r=tx.query(match inventory.kind {CollabKind::Document=>r#"SELECT COALESCE((SELECT length(state)
            FROM document_states
            WHERE workspace_id=?1
            AND document_id=?2),0)+COALESCE((SELECT sum(length(payload))
            FROM document_collab_updates
            WHERE workspace_id=?1
            AND document_id=?2),0)+COALESCE((SELECT sum(length(y_snapshot))
            FROM revisions
            WHERE workspace_id=?1
            AND target_kind=?3
            AND target_id=?2),0),COALESCE((SELECT sum(length(CAST(content_json AS BLOB))+length(CAST(text AS BLOB)))
            FROM revisions
            WHERE workspace_id=?1
            AND target_kind=?3
            AND target_id=?2),0),(SELECT count(*)
            FROM document_states
            WHERE workspace_id=?1
            AND document_id=?2)+(SELECT count(*)
            FROM document_collab_updates
            WHERE workspace_id=?1
            AND document_id=?2)+(SELECT count(*)
            FROM document_collab_op_receipts
            WHERE workspace_id=?1
            AND document_id=?2)+(SELECT count(*)
            FROM revisions
            WHERE workspace_id=?1
            AND target_kind=?3
            AND target_id=?2)"#,CollabKind::Task=>r#"SELECT COALESCE((SELECT length(state)
            FROM task_states
            WHERE workspace_id=?1
            AND task_id=?2),0)+COALESCE((SELECT sum(length(payload))
            FROM task_collab_updates
            WHERE workspace_id=?1
            AND task_id=?2),0)+COALESCE((SELECT sum(length(y_snapshot))
            FROM revisions
            WHERE workspace_id=?1
            AND target_kind=?3
            AND target_id=?2),0),COALESCE((SELECT sum(length(CAST(content_json AS BLOB))+length(CAST(text AS BLOB)))
            FROM revisions
            WHERE workspace_id=?1
            AND target_kind=?3
            AND target_id=?2),0),(SELECT count(*)
            FROM task_states
            WHERE workspace_id=?1
            AND task_id=?2)+(SELECT count(*)
            FROM task_collab_updates
            WHERE workspace_id=?1
            AND task_id=?2)+(SELECT count(*)
            FROM task_collab_op_receipts
            WHERE workspace_id=?1
            AND task_id=?2)+(SELECT count(*)
            FROM revisions
            WHERE workspace_id=?1
            AND target_kind=?3
            AND target_id=?2)"#,},&[Cell::uuid(source),Cell::uuid(inventory.target_id),Cell::text(collab_kind_name(inventory.kind))]).await?;
        let (bytes, graph, count) = (
            r[0].cell(0)?.integer()?,
            r[0].cell(1)?.integer()?,
            r[0].cell(2)?.integer()?,
        );
        native_bytes = native_bytes.saturating_add(bytes);
        graph_bytes = graph_bytes.saturating_add(graph);
        rows = rows.saturating_add(count);
    }
    if !within_native_copy_budget(native_bytes, graph_bytes, rows) {
        return Ok(Err(PersonalTransferDbError::Incomplete(
            Blocker::InventoryBudget,
            "native history copy budget",
        )));
    }
    Ok(Ok(()))
}

async fn family_timer_snapshot(
    tx: &mut FamilyTx,
    source: Uuid,
    actor: Uuid,
    selection: &PersonalTransferSelection,
) -> TransferResult<TimerGraph> {
    tx.require_tenant(source)?;
    let Some(task) = selection.task_id else {
        return Ok(Ok(TimerGraph::default()));
    };
    let args = [
        Cell::uuid(source),
        Cell::uuid(task),
        Cell::uuid(actor),
        Cell::Integer(TIMER_ROW_BUDGET + 1),
    ];
    let rows = tx
        .query(
            r#"SELECT id,user_id,started_at,ended_at,duration_seconds,note
        FROM time_entries
        WHERE workspace_id=?1
        AND task_id=?2
        ORDER BY started_at,id LIMIT ?4"#,
            &args,
        )
        .await?;
    for r in &rows {
        if r.cell(1)?.id()? != actor {
            return Ok(Err(PersonalTransferDbError::Incomplete(
                Blocker::DependentGraph,
                "time entry by another author",
            )));
        }
    }
    let entries = rows
        .iter()
        .map(|r| {
            Ok(TimeEntryRow {
                id: r.cell(0)?.id()?,
                started_at: r.cell(2)?.datetime()?,
                ended_at: r.cell(3)?.optional(Cell::datetime)?,
                duration_seconds: r.cell(4)?.optional(Cell::int32)?,
                note: r.cell(5)?.optional(Cell::string)?,
            })
        })
        .collect::<Result<Vec<_>, sqlx::Error>>()?;
    let runs = tx
        .query(
            r#"SELECT id,status,version,started_at,stopped_at,note
        FROM task_timer_runs
        WHERE workspace_id=?1
        AND task_id=?2
        AND user_id=?3
        ORDER BY started_at,id LIMIT ?4"#,
            &args,
        )
        .await?
        .iter()
        .map(|r| {
            Ok(TimerRunRow {
                id: r.cell(0)?.id()?,
                status: r.cell(1)?.string()?,
                version: r.cell(2)?.int32()?,
                started_at: r.cell(3)?.datetime()?,
                stopped_at: r.cell(4)?.optional(Cell::datetime)?,
                note: r.cell(5)?.optional(Cell::string)?,
            })
        })
        .collect::<Result<Vec<_>, sqlx::Error>>()?;
    let segments = tx
        .query(
            r#"SELECT id,run_id,started_at,ended_at,time_entry_id
        FROM task_timer_segments
        WHERE workspace_id=?1
        AND task_id=?2
        AND user_id=?3
        ORDER BY started_at,id LIMIT ?4"#,
            &args,
        )
        .await?
        .iter()
        .map(|r| {
            Ok(TimerSegmentRow {
                id: r.cell(0)?.id()?,
                run_id: r.cell(1)?.id()?,
                started_at: r.cell(2)?.datetime()?,
                ended_at: r.cell(3)?.optional(Cell::datetime)?,
                time_entry_id: r.cell(4)?.optional(Cell::id)?,
            })
        })
        .collect::<Result<Vec<_>, sqlx::Error>>()?;
    let reserved = tx
        .query(
            r#"SELECT time_entry_id
        FROM task_timer_legacy_open
        WHERE workspace_id=?1
        AND task_id=?2
        AND user_id=?3
        ORDER BY time_entry_id"#,
            &args[..3],
        )
        .await?
        .iter()
        .map(|r| r.cell(0)?.id())
        .collect::<Result<_, _>>()?;
    if entries.len() as i64 > TIMER_ROW_BUDGET
        || runs.len() as i64 > TIMER_ROW_BUDGET
        || segments.len() as i64 > TIMER_ROW_BUDGET
    {
        return Ok(Err(PersonalTransferDbError::Incomplete(
            Blocker::InventoryBudget,
            "timer inventory budget",
        )));
    }
    if entries.iter().any(|e| e.ended_at.is_none()) {
        let rows = tx
            .query(
                r#"SELECT EXISTS(SELECT 1
            FROM task_timer_runs
            WHERE user_id=?3
            AND status<>'stopped'
            AND NOT(workspace_id=?1
            AND task_id=?2))
            OR EXISTS(SELECT 1
            FROM task_timer_legacy_open
            WHERE user_id=?3
            AND NOT(workspace_id=?1
            AND task_id=?2))
            OR EXISTS(SELECT 1
            FROM time_entries
            WHERE workspace_id=?4
            AND user_id=?3
            AND ended_at IS NULL)"#,
                &[
                    args[0].clone(),
                    args[1].clone(),
                    args[2].clone(),
                    Cell::uuid(selection.destination_workspace_id),
                ],
            )
            .await?;
        if rows[0].cell(0)?.boolean()? {
            return Ok(Err(PersonalTransferDbError::Incomplete(
                Blocker::TimerBusy,
                "another unfinished timer or open time entry",
            )));
        }
    }
    Ok(Ok(TimerGraph {
        entries,
        runs,
        segments,
        reserved,
    }))
}

async fn family_relocate_timer(
    tx: &mut FamilyTx,
    destination: Uuid,
    actor: Uuid,
    task: Uuid,
    timer: &TimerGraph,
) -> Result<(), sqlx::Error> {
    tx.require_writer()?;
    tx.require_tenant(destination)?;
    let mut entries: Vec<_> = timer.entries.iter().collect();
    entries.sort_by_key(|e| e.ended_at.is_none());
    for e in entries {
        tx.execute(r#"INSERT INTO time_entries(id,workspace_id,task_id,user_id,started_at,ended_at,duration_seconds,note) VALUES(?1,?2,?3,?4,?5,?6,?7,?8)"#,&[Cell::uuid(e.id),Cell::uuid(destination),Cell::uuid(task),Cell::uuid(actor),Cell::instant(e.started_at)?,e.ended_at.map(Cell::instant).transpose()?.unwrap_or(Cell::Null),e.duration_seconds.map(|v|Cell::Integer(i64::from(v))).unwrap_or(Cell::Null),Cell::optional_text(e.note.as_deref())]).await?;
        if e.ended_at.is_none() && !timer.reserved.contains(&e.id) {
            tx.execute(
                "DELETE FROM task_timer_legacy_open WHERE time_entry_id=?1 AND user_id=?2",
                &[Cell::uuid(e.id), Cell::uuid(actor)],
            )
            .await?;
        }
    }
    let mut runs: Vec<_> = timer.runs.iter().collect();
    runs.sort_by_key(|r| r.status != "stopped");
    for r in runs {
        tx.execute(r#"INSERT INTO task_timer_runs(id,user_id,workspace_id,task_id,status,version,started_at,stopped_at,note) VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9)"#,&[Cell::uuid(r.id),Cell::uuid(actor),Cell::uuid(destination),Cell::uuid(task),Cell::text(&r.status),Cell::Integer(i64::from(r.version)),Cell::instant(r.started_at)?,r.stopped_at.map(Cell::instant).transpose()?.unwrap_or(Cell::Null),Cell::optional_text(r.note.as_deref())]).await?;
    }
    for s in &timer.segments {
        tx.execute(r#"INSERT INTO task_timer_segments(id,run_id,user_id,workspace_id,task_id,started_at,ended_at,time_entry_id) VALUES(?1,?2,?3,?4,?5,?6,?7,?8)"#,&[Cell::uuid(s.id),Cell::uuid(s.run_id),Cell::uuid(actor),Cell::uuid(destination),Cell::uuid(task),Cell::instant(s.started_at)?,s.ended_at.map(Cell::instant).transpose()?.unwrap_or(Cell::Null),Cell::optional_uuid(s.time_entry_id)]).await?;
    }
    Ok(())
}

async fn family_take_native(
    tx: &mut FamilyTx,
    source: Uuid,
    native: &[NativeHistoryInventory],
) -> Result<Vec<&'static str>, sqlx::Error> {
    tx.require_writer()?;
    tx.require_tenant(source)?;
    let mut tables = Vec::new();
    for inventory in native {
        let (group, _) = native_tables(inventory.kind);
        for table in [group.states, group.updates, group.receipts, "revisions"] {
            if !tables.contains(&table) {
                tables.push(table);
            }
        }
    }
    for table in &tables {
        tx.execute(
            match *table {
                "document_states" => {
                    r#"CREATE TEMP TABLE w2_move_document_states AS SELECT *
                FROM document_states
                WHERE 0"#
                }
                "document_collab_updates" => {
                    r#"CREATE TEMP TABLE w2_move_document_collab_updates AS SELECT *
                FROM document_collab_updates
                WHERE 0"#
                }
                "document_collab_op_receipts" => {
                    r#"CREATE TEMP TABLE w2_move_document_collab_op_receipts AS SELECT *
                FROM document_collab_op_receipts
                WHERE 0"#
                }
                "task_states" => {
                    r#"CREATE TEMP TABLE w2_move_task_states AS SELECT *
                FROM task_states
                WHERE 0"#
                }
                "task_collab_updates" => {
                    r#"CREATE TEMP TABLE w2_move_task_collab_updates AS SELECT *
                FROM task_collab_updates
                WHERE 0"#
                }
                "task_collab_op_receipts" => {
                    r#"CREATE TEMP TABLE w2_move_task_collab_op_receipts AS SELECT *
                FROM task_collab_op_receipts
                WHERE 0"#
                }
                "revisions" => {
                    r#"CREATE TEMP TABLE w2_move_revisions AS SELECT *
                FROM revisions
                WHERE 0"#
                }
                _ => return Err(sqlx::Error::Protocol("unknown native staging table".into())),
            },
            &[],
        )
        .await?;
    }
    for i in native {
        let (group, _) = native_tables(i.kind);
        let args = [Cell::uuid(source), Cell::uuid(i.target_id)];
        for table in [group.states, group.updates, group.receipts] {
            tx.execute(
                match table {
                    "document_states" => {
                        r#"INSERT INTO temp.w2_move_document_states SELECT *
                FROM document_states
                WHERE workspace_id=?1
                AND document_id=?2"#
                    }
                    "document_collab_updates" => {
                        r#"INSERT INTO temp.w2_move_document_collab_updates SELECT *
                FROM document_collab_updates
                WHERE workspace_id=?1
                AND document_id=?2"#
                    }
                    "document_collab_op_receipts" => {
                        r#"INSERT INTO temp.w2_move_document_collab_op_receipts SELECT *
                FROM document_collab_op_receipts
                WHERE workspace_id=?1
                AND document_id=?2"#
                    }
                    "task_states" => {
                        r#"INSERT INTO temp.w2_move_task_states SELECT *
                FROM task_states
                WHERE workspace_id=?1
                AND task_id=?2"#
                    }
                    "task_collab_updates" => {
                        r#"INSERT INTO temp.w2_move_task_collab_updates SELECT *
                FROM task_collab_updates
                WHERE workspace_id=?1
                AND task_id=?2"#
                    }
                    "task_collab_op_receipts" => {
                        r#"INSERT INTO temp.w2_move_task_collab_op_receipts SELECT *
                FROM task_collab_op_receipts
                WHERE workspace_id=?1
                AND task_id=?2"#
                    }
                    _ => return Err(sqlx::Error::Protocol("unknown native staging table".into())),
                },
                &args,
            )
            .await?;
        }
        let revargs = [
            args[0].clone(),
            args[1].clone(),
            Cell::text(collab_kind_name(i.kind)),
        ];
        tx.execute("INSERT INTO temp.w2_move_revisions SELECT * FROM revisions WHERE workspace_id=?1 AND target_id=?2 AND target_kind=?3",&revargs).await?;
        let cuts = tx
            .query(
                match i.kind {
                    CollabKind::Document => {
                        r#"SELECT snapshot_cutoff_seq,tail_seq
                    FROM temp.w2_move_document_states
                    WHERE document_id=?1"#
                    }
                    CollabKind::Task => {
                        r#"SELECT snapshot_cutoff_seq,tail_seq
                    FROM temp.w2_move_task_states
                    WHERE task_id=?1"#
                    }
                },
                &args[1..],
            )
            .await?;
        let tail = tx
            .query(
                match i.kind {
                    CollabKind::Document => {
                        r#"SELECT seq,op_id
                    FROM temp.w2_move_document_collab_updates
                    WHERE document_id=?1
                    ORDER BY seq"#
                    }
                    CollabKind::Task => {
                        r#"SELECT seq,op_id
                    FROM temp.w2_move_task_collab_updates
                    WHERE task_id=?1
                    ORDER BY seq"#
                    }
                },
                &args[1..],
            )
            .await?
            .iter()
            .map(|r| Ok((r.cell(0)?.integer()?, r.cell(1)?.id()?)))
            .collect::<Result<Vec<_>, sqlx::Error>>()?;
        let mut receipts = tx
            .query(
                match i.kind {
                    CollabKind::Document => {
                        r#"SELECT op_id,seq,actor_user_id
                    FROM temp.w2_move_document_collab_op_receipts
                    WHERE document_id=?1"#
                    }
                    CollabKind::Task => {
                        r#"SELECT op_id,seq,actor_user_id
                    FROM temp.w2_move_task_collab_op_receipts
                    WHERE task_id=?1"#
                    }
                },
                &args[1..],
            )
            .await?
            .iter()
            .map(|r| Ok((r.cell(0)?.id()?, r.cell(1)?.integer()?, r.cell(2)?.id()?)))
            .collect::<Result<Vec<_>, sqlx::Error>>()?;
        let revisions = tx
            .query(
                r#"SELECT id
            FROM temp.w2_move_revisions
            WHERE target_id=?1
            AND target_kind=?2
            ORDER BY created_at,id"#,
                &[args[1].clone(), revargs[2].clone()],
            )
            .await?
            .iter()
            .map(|r| r.cell(0)?.id())
            .collect::<Result<Vec<_>, _>>()?;
        receipts.sort();
        let mut expected = i.receipts.clone();
        expected.sort();
        if cuts.len() != 1
            || cuts[0].cell(0)?.integer()? != i.snapshot_cutoff_seq
            || cuts[0].cell(1)?.integer()? != i.tail_seq
            || tail != i.tail
            || receipts != expected
            || revisions != i.revisions
        {
            return Err(sqlx::Error::Protocol(
                "personal transfer native history changed".into(),
            ));
        }
        let deleted = tx
            .execute(
                "DELETE FROM revisions WHERE workspace_id=?1 AND target_id=?2 AND target_kind=?3",
                &revargs,
            )
            .await?;
        if deleted != revisions.len() as u64 {
            return Err(sqlx::Error::Protocol(
                "personal transfer native history changed".into(),
            ));
        }
    }
    Ok(tables)
}

async fn family_restore_native(
    tx: &mut FamilyTx,
    destination: Uuid,
    tables: &[&str],
) -> Result<(), sqlx::Error> {
    tx.require_writer()?;
    tx.require_tenant(destination)?;
    for table in tables {
        tx.execute(
            match *table {
                "document_states" => "UPDATE temp.w2_move_document_states SET workspace_id=?1",
                "document_collab_updates" => {
                    "UPDATE temp.w2_move_document_collab_updates SET workspace_id=?1"
                }
                "document_collab_op_receipts" => {
                    "UPDATE temp.w2_move_document_collab_op_receipts SET workspace_id=?1"
                }
                "task_states" => "UPDATE temp.w2_move_task_states SET workspace_id=?1",
                "task_collab_updates" => {
                    "UPDATE temp.w2_move_task_collab_updates SET workspace_id=?1"
                }
                "task_collab_op_receipts" => {
                    "UPDATE temp.w2_move_task_collab_op_receipts SET workspace_id=?1"
                }
                "revisions" => "UPDATE temp.w2_move_revisions SET workspace_id=?1",
                _ => return Err(sqlx::Error::Protocol("unknown native staging table".into())),
            },
            &[Cell::uuid(destination)],
        )
        .await?;
        let count = tx
            .query(
                match *table {
                    "document_states" => "SELECT count(*) FROM temp.w2_move_document_states",
                    "document_collab_updates" => {
                        "SELECT count(*) FROM temp.w2_move_document_collab_updates"
                    }
                    "document_collab_op_receipts" => {
                        "SELECT count(*) FROM temp.w2_move_document_collab_op_receipts"
                    }
                    "task_states" => "SELECT count(*) FROM temp.w2_move_task_states",
                    "task_collab_updates" => {
                        "SELECT count(*) FROM temp.w2_move_task_collab_updates"
                    }
                    "task_collab_op_receipts" => {
                        "SELECT count(*) FROM temp.w2_move_task_collab_op_receipts"
                    }
                    "revisions" => "SELECT count(*) FROM temp.w2_move_revisions",
                    _ => return Err(sqlx::Error::Protocol("unknown native staging table".into())),
                },
                &[],
            )
            .await?[0]
            .cell(0)?
            .integer()?;
        if tx
            .execute(
                match *table {
                    "document_states" => {
                        r#"INSERT INTO document_states SELECT *
                    FROM temp.w2_move_document_states"#
                    }
                    "document_collab_updates" => {
                        r#"INSERT INTO document_collab_updates SELECT *
                    FROM temp.w2_move_document_collab_updates"#
                    }
                    "document_collab_op_receipts" => {
                        r#"INSERT INTO document_collab_op_receipts SELECT *
                    FROM temp.w2_move_document_collab_op_receipts"#
                    }
                    "task_states" => {
                        r#"INSERT INTO task_states SELECT *
                    FROM temp.w2_move_task_states"#
                    }
                    "task_collab_updates" => {
                        r#"INSERT INTO task_collab_updates SELECT *
                    FROM temp.w2_move_task_collab_updates"#
                    }
                    "task_collab_op_receipts" => {
                        r#"INSERT INTO task_collab_op_receipts SELECT *
                    FROM temp.w2_move_task_collab_op_receipts"#
                    }
                    "revisions" => {
                        r#"INSERT INTO revisions SELECT *
                    FROM temp.w2_move_revisions"#
                    }
                    _ => return Err(sqlx::Error::Protocol("unknown native staging table".into())),
                },
                &[],
            )
            .await?
            != count as u64
        {
            return Err(sqlx::Error::Protocol(
                "personal transfer native history restore incomplete".into(),
            ));
        }
        tx.execute(
            match *table {
                "document_states" => "DROP TABLE temp.w2_move_document_states",
                "document_collab_updates" => "DROP TABLE temp.w2_move_document_collab_updates",
                "document_collab_op_receipts" => {
                    "DROP TABLE temp.w2_move_document_collab_op_receipts"
                }
                "task_states" => "DROP TABLE temp.w2_move_task_states",
                "task_collab_updates" => "DROP TABLE temp.w2_move_task_collab_updates",
                "task_collab_op_receipts" => "DROP TABLE temp.w2_move_task_collab_op_receipts",
                "revisions" => "DROP TABLE temp.w2_move_revisions",
                _ => return Err(sqlx::Error::Protocol("unknown native staging table".into())),
            },
            &[],
        )
        .await?;
    }
    Ok(())
}

#[allow(clippy::too_many_arguments)]
async fn family_transfer_change(
    tx: &mut FamilyTx,
    workspace: Uuid,
    actor: Uuid,
    verb: &str,
    kind: &str,
    id: Uuid,
    payload: Value,
    ip: Option<&str>,
) -> Result<(), sqlx::Error> {
    tx.require_writer()?;
    tx.require_tenant(workspace)?;
    let mut op = OperationTx::SqliteFamily(tx);
    op.set_tenant(workspace).await?;
    op.append_event(super::identity::EventAppend {
        id: Uuid::now_v7(),
        workspace_id: Some(workspace),
        actor_user_id: Some(actor),
        verb: verb.into(),
        target_type: Some(kind.into()),
        target_id: Some(id),
        payload: payload.clone(),
    })
    .await?;
    op.append_audit(super::identity::AuditAppend {
        id: Uuid::now_v7(),
        workspace_id: Some(workspace),
        actor_user_id: Some(actor),
        verb: verb.into(),
        target_type: Some(kind.into()),
        target_id: Some(id),
        payload,
        ip: ip.map(str::to_string),
    })
    .await
}

#[allow(clippy::too_many_arguments)]
async fn family_commit_graph(
    tx: &mut FamilyTx,
    source: Uuid,
    actor: Uuid,
    session: Uuid,
    body: &PersonalTransferBody,
    graph: &SourceGraph,
    target: &Destination,
    ip: Option<&str>,
    channel: &str,
    engine: Option<&TransferBodyEngine>,
    staged: &[StagedFile],
) -> Result<PersonalTransferOutput, sqlx::Error> {
    tx.require_writer()?;
    tx.require_tenant(body.selection.destination_workspace_id)?;
    let selection = &body.selection;
    let dst = selection.destination_workspace_id;
    let project = selection.destination_project_id;
    let moving = selection.action == PersonalTransferAction::Move;
    let document_id = if moving {
        selection.document_id
    } else {
        Uuid::now_v7()
    };
    let task_id = selection
        .task_id
        .map(|id| if moving { id } else { Uuid::now_v7() });
    let copied_files = staged
        .iter()
        .map(|f| (f.staged.source.id, f.destination))
        .collect();
    let pair = PairRefs {
        document: (selection.document_id, document_id),
        task: selection.task_id.zip(task_id),
    };
    let (document_json, block_ids) = if moving {
        (
            graph.document.content_json.clone(),
            std::collections::HashMap::new(),
        )
    } else {
        copy_body(&graph.document.content_json, &copied_files, &pair)
            .map_err(|_| sqlx::Error::Protocol("personal transfer copy body changed".into()))?
    };
    let task_json = match &graph.task {
        Some(task) if !moving => Some(
            copy_body(&task.content_json, &copied_files, &pair)
                .map_err(|_| sqlx::Error::Protocol("personal transfer copy body changed".into()))?
                .0,
        ),
        Some(task) => Some(task.content_json.clone()),
        None => None,
    };
    let (document_seed, task_seed) = if moving {
        (None, None)
    } else {
        let engine = engine.ok_or_else(|| {
            sqlx::Error::Protocol("personal transfer seed engine unavailable".into())
        })?;
        let seed = SeedEngine::new(engine.engine_bin.clone(), engine.limits);
        let document = seed
            .tiptap_to_yjs_update(&document_json)
            .await
            .map_err(|_| {
                sqlx::Error::Protocol("personal transfer document seed unavailable".into())
            })?;
        let task = match &task_json {
            Some(json) => Some(seed.tiptap_to_yjs_update(json).await.map_err(|_| {
                sqlx::Error::Protocol("personal transfer task seed unavailable".into())
            })?),
            None => None,
        };
        (Some(document), task)
    };
    tx.switch_transfer_tenant(source)?;
    tx.require_tenant(source)?;
    let native = if moving {
        family_take_native(tx, source, &graph.native).await?
    } else {
        Vec::new()
    };
    if moving {
        for f in staged {
            tx.execute(
                "DELETE FROM attachments WHERE workspace_id=?1 AND id=?2",
                &[Cell::uuid(source), Cell::uuid(f.staged.source.id)],
            )
            .await?;
        }
        if let Some(id) = selection.task_id {
            tx.execute(
                "DELETE FROM tasks WHERE workspace_id=?1 AND id=?2",
                &[Cell::uuid(source), Cell::uuid(id)],
            )
            .await?;
        }
        tx.execute(
            "DELETE FROM documents WHERE workspace_id=?1 AND id=?2",
            &[Cell::uuid(source), Cell::uuid(selection.document_id)],
        )
        .await?;
        family_transfer_change(
            tx,
            source,
            actor,
            "document.deleted",
            "document",
            selection.document_id,
            json!({}),
            ip,
        )
        .await?;
        if let Some(task) = &graph.task {
            family_transfer_change(
                tx,
                source,
                actor,
                "task.deleted",
                "task",
                task.record.id,
                json!({"taskId":task.record.id,"projectId":task.record.project_id}),
                ip,
            )
            .await?;
        }
    }
    tx.switch_transfer_tenant(dst)?;
    tx.require_tenant(dst)?;
    let count = if task_id.is_some() { 2 } else { 1 };
    let allocated=tx.query(r#"UPDATE projects SET next_number=next_number+?3,updated_at=unixepoch()*1000000+CAST(substr(strftime('%f','now'),4,3) AS INTEGER)*1000
        WHERE workspace_id=?1
        AND id=?2
        RETURNING next_number-?3"#,&[Cell::uuid(dst),Cell::uuid(project),Cell::Integer(count)]).await?;
    let doc_number = allocated[0].cell(0)?.int32()?;
    let task_number = task_id.map(|_| doc_number + 1);
    let last = tx
        .query(
            r#"SELECT sort_key
        FROM documents
        WHERE workspace_id=?1
        AND parent_id=?2
        AND deleted_at IS NULL
        ORDER BY sort_key COLLATE BINARY DESC LIMIT 1"#,
            &[Cell::uuid(dst), Cell::uuid(target.root_id)],
        )
        .await?
        .first()
        .map(|r| r.cell(0)?.string())
        .transpose()?;
    let sort = between(last.as_deref(), None).map_err(|e| sqlx::Error::Protocol(e.to_string()))?;
    let now = tx
        .query(
            "SELECT unixepoch()*1000000+CAST(substr(strftime('%f','now'),4,3) AS INTEGER)*1000",
            &[],
        )
        .await?[0]
        .cell(0)?
        .datetime()?;
    let d = &graph.document;
    let path = format!("{}.{}", target.root_path, to_path_label(document_id));
    tx.execute(r#"INSERT INTO documents(id,workspace_id,title,icon,path,parent_id,sort_key,project_id,number,status,schema_version,text,chosung,version,created_by,created_at,updated_at,content_json,kind) VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13,?14,?15,?16,?17,?18,?19)"#,&[Cell::uuid(document_id),Cell::uuid(dst),Cell::text(&d.title),Cell::optional_text(d.icon.as_deref()),Cell::text(path),Cell::uuid(target.root_id),Cell::text(sort),Cell::uuid(project),Cell::Integer(i64::from(doc_number)),Cell::text(&d.status),Cell::Integer(i64::from(d.schema_version)),Cell::text(&d.text),Cell::text(&d.chosung),Cell::Integer(i64::from(if moving{d.version}else{1})),Cell::uuid(if moving{d.created_by}else{actor}),Cell::instant(if moving{d.created_at}else{now})?,Cell::instant(if moving{d.updated_at}else{now})?,Cell::json(&document_json)?,Cell::text(&d.kind)]).await?;
    if let Some(seed) = document_seed {
        tx.execute("INSERT INTO document_states(workspace_id,document_id,state,encoding) VALUES(?1,?2,?3,1)",&[Cell::uuid(dst),Cell::uuid(document_id),Cell::Blob(seed)]).await?;
    }
    if let (Some(task), Some(id), Some(number)) = (&graph.task, task_id, task_number) {
        let r = &task.record;
        let status = selection.destination_status_id;
        let last = tx
            .query(
                r#"SELECT sort_key
            FROM tasks
            WHERE workspace_id=?1
            AND project_id=?2
            AND status_id=?3
            AND deleted_at IS NULL
            ORDER BY sort_key COLLATE BINARY DESC LIMIT 1"#,
                &[
                    Cell::uuid(dst),
                    Cell::uuid(project),
                    Cell::optional_uuid(status),
                ],
            )
            .await?
            .first()
            .map(|r| r.cell(0)?.string())
            .transpose()?;
        let sort =
            between(last.as_deref(), None).map_err(|e| sqlx::Error::Protocol(e.to_string()))?;
        tx.execute(r#"INSERT INTO tasks(id,workspace_id,project_id,number,title,type,priority,status_id,start_date,due_date,due_at,estimate,recurrence,sort_key,schema_version,content_json,version,created_by,created_at,updated_at,text,chosung,estimate_unit) VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13,?14,?15,?16,?17,?18,?19,?20,?21,?22,?23)"#,&[Cell::uuid(id),Cell::uuid(dst),Cell::uuid(project),Cell::Integer(i64::from(number)),Cell::text(&r.title),Cell::text(&r.task_type),Cell::text(&r.priority),Cell::optional_uuid(status),Cell::optional_text(r.start_date.map(|v|v.to_string()).as_deref()),Cell::optional_text(r.due_date.map(|v|v.to_string()).as_deref()),r.due_at.map(Cell::instant).transpose()?.unwrap_or(Cell::Null),Cell::optional_text(r.estimate.as_deref()),task.recurrence.as_ref().map(Cell::json).transpose()?.unwrap_or(Cell::Null),Cell::text(sort),Cell::Integer(i64::from(r.schema_version)),task_json.as_ref().map(Cell::json).transpose()?.unwrap_or(Cell::Null),Cell::Integer(i64::from(if moving{r.version}else{1})),Cell::uuid(if moving{r.created_by}else{actor}),Cell::instant(if moving{r.created_at}else{now})?,Cell::instant(if moving{r.updated_at}else{now})?,Cell::text(&task.text),Cell::text(&task.chosung),Cell::optional_text(task.estimate_unit.as_deref())]).await?;
        if let Some(seed) = task_seed {
            tx.execute(
                "INSERT INTO task_states(workspace_id,task_id,state,encoding) VALUES(?1,?2,?3,1)",
                &[Cell::uuid(dst), Cell::uuid(id), Cell::Blob(seed)],
            )
            .await?;
        }
        for assignee in &graph.assignees {
            tx.execute(
                "INSERT INTO task_assignees(workspace_id,task_id,user_id) VALUES(?1,?2,?3)",
                &[Cell::uuid(dst), Cell::uuid(id), Cell::uuid(*assignee)],
            )
            .await?;
        }
        let collection = target
            .collection_id
            .ok_or_else(|| sqlx::Error::Protocol("transfer task collection missing".into()))?;
        if let Some(item) = graph.collection_item.as_ref().filter(|_| moving) {
            tx.execute(r#"INSERT INTO collection_items(id,workspace_id,collection_id,task_id,version,created_at,updated_at) VALUES(?1,?2,?3,?4,?5,?6,?7)"#,&[Cell::uuid(item.id),Cell::uuid(dst),Cell::uuid(collection),Cell::uuid(id),Cell::Integer(i64::from(item.version)),Cell::instant(item.created_at)?,Cell::instant(item.updated_at)?]).await?;
        } else {
            tx.execute(r#"INSERT INTO collection_items(id,workspace_id,collection_id,task_id) VALUES(?1,?2,?3,?4)"#,&[Cell::uuid(Uuid::now_v7()),Cell::uuid(dst),Cell::uuid(collection),Cell::uuid(id)]).await?;
        }
        if moving {
            for a in &graph.activities {
                tx.execute(r#"INSERT INTO task_activity(id,workspace_id,task_id,actor_user_id,channel,kind,changes,created_at) VALUES(?1,?2,?3,?4,?5,?6,?7,?8)"#,&[Cell::uuid(a.id),Cell::uuid(dst),Cell::uuid(id),Cell::optional_uuid(a.actor),Cell::text(&a.channel),Cell::text(&a.kind),Cell::json(&a.changes)?,Cell::instant(a.created_at)?]).await?;
            }
            family_relocate_timer(tx, dst, actor, id, &graph.timer).await?;
        } else {
            super::tasks::family_record_task_activity(
                tx,
                dst,
                id,
                actor,
                channel,
                None,
                &crate::tasks::activity::ActivitySnapshot::new(),
            )
            .await?;
        }
        if let Some(origin) = &graph.origin {
            let anchor = if moving {
                origin.anchor.clone()
            } else {
                origin.anchor.as_ref().and_then(|id| {
                    block_key(&Value::String(id.clone()))
                        .and_then(|key| block_ids.get(&key).cloned())
                })
            };
            tx.execute(r#"INSERT INTO task_origins(workspace_id,task_id,document_id,request_id,request_hash,anchor,created_at,updated_at) VALUES(?1,?2,?3,?4,?5,?6,?7,?8)"#,&[Cell::uuid(dst),Cell::uuid(id),Cell::uuid(document_id),Cell::uuid(if moving{origin.request_id}else{body.request_id}),Cell::text(&origin.request_hash),Cell::optional_text(anchor.as_deref()),Cell::instant(if moving{origin.created_at}else{now})?,Cell::instant(if moving{origin.updated_at}else{now})?]).await?;
        }
        family_transfer_change(
            tx,
            dst,
            actor,
            "task.created",
            "task",
            id,
            json!({"taskId":id,"projectId":project}),
            ip,
        )
        .await?;
    }
    for file in staged {
        let one = &file.staged;
        let att = &one.source;
        let (variants, preview_status) = match &one.preview {
            Some((_, preview)) => (json!({"preview":preview}), "ok"),
            None if is_image_mime(&att.mime)
                && crate::attachments::preview::preview_mime_supported(&att.mime) =>
            {
                (json!({}), "pending")
            }
            None => (json!({}), "skipped"),
        };
        tx.execute(r#"INSERT INTO attachments(id,workspace_id,document_id,task_id,uploader_id,status,name,mime,declared_mime,size_bytes,reserved_size_bytes,storage_key,image,variants,extract_status,scan_status,preview_status,created_at,completed_at) VALUES(?1,?2,?3,?4,?5,'stored',?6,?7,?8,?9,?9,?10,?11,?12,?13,?14,?15,?16,?17)"#,&[Cell::uuid(file.destination),Cell::uuid(dst),Cell::optional_uuid(att.document_id.map(|_|document_id)),Cell::optional_uuid(att.task_id.and(task_id)),Cell::uuid(att.uploader_id),Cell::text(&att.name),Cell::text(&att.mime),Cell::optional_text(att.declared_mime.as_deref()),Cell::Integer(one.original.size_bytes),Cell::text(&one.original.key),Cell::Integer(i64::from(att.image)),Cell::json(&variants)?,Cell::text(initial_extract_status(&att.name,&att.mime)),Cell::text(&att.scan_status),Cell::text(preview_status),Cell::instant(att.created_at)?,att.completed_at.map(Cell::instant).transpose()?.unwrap_or(Cell::Null)]).await?;
    }
    for id in staged_journal(staged) {
        tx.execute(
            "DELETE FROM attachment_object_cleanups WHERE workspace_id=?1 AND id=?2",
            &[Cell::uuid(dst), Cell::uuid(id)],
        )
        .await?;
    }
    family_restore_native(tx, dst, &native).await?;
    family_transfer_change(
        tx,
        dst,
        actor,
        "document.created",
        "document",
        document_id,
        json!({"projectId":project}),
        ip,
    )
    .await?;
    tx.switch_transfer_tenant(source)?;
    tx.require_tenant(source)?;
    tx.execute(r#"INSERT INTO personal_transfer_commands(workspace_id,actor_user_id,request_id,session_id,request_hash,action,destination_workspace_id,destination_project_id,document_id,document_number,task_id,task_number) VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12)"#,&[Cell::uuid(source),Cell::uuid(actor),Cell::uuid(body.request_id),Cell::uuid(session),Cell::text(request_hash(source,actor,session,body)),Cell::text(selection.action.as_str()),Cell::uuid(dst),Cell::uuid(project),Cell::uuid(document_id),Cell::Integer(i64::from(doc_number)),Cell::optional_uuid(task_id),task_number.map(|v|Cell::Integer(i64::from(v))).unwrap_or(Cell::Null)]).await?;
    Ok(PersonalTransferOutput {
        workspace_id: dst,
        project_id: project,
        document_id,
        document_number: doc_number,
        task_id,
        task_number,
        replayed: false,
    })
}

fn inventory_closure_refusal(
    inventory: &NativeHistoryInventory,
    selection: &PersonalTransferSelection,
    moved_files: &[Uuid],
) -> Option<PersonalTransferDbError> {
    let rejected = std::cell::Cell::new(None);
    let closed = |kind, id| {
        let present = match kind {
            collab_engine::archive_history::ReferenceKind::Document => id == selection.document_id,
            collab_engine::archive_history::ReferenceKind::Task => Some(id) == selection.task_id,
            // Only the exact locked files that move with the pair.
            collab_engine::archive_history::ReferenceKind::Attachment => moved_files.contains(&id),
            _ => false,
        };
        if !present {
            rejected.set(Some(kind));
        }
        present
    };
    if let Some(blocker) = inventory.closure_blocker(closed) {
        let file = blocker == RetainedHistoryBlocker::OutsideClosure
            && rejected.get() == Some(collab_engine::archive_history::ReferenceKind::Attachment);
        let (typed, reason) = match blocker {
            RetainedHistoryBlocker::Incomplete | RetainedHistoryBlocker::Unavailable => {
                (Blocker::NativeHistory, blocker.reason())
            }
            _ if file => (Blocker::File, "retained file outside the moved pair"),
            _ => (Blocker::OutgoingReference, blocker.reason()),
        };
        return Some(PersonalTransferDbError::Incomplete(typed, reason));
    }

    None
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A pair none of the test bodies names.
    fn unrelated() -> PairRefs {
        PairRefs {
            document: (Uuid::now_v7(), Uuid::now_v7()),
            task: Some((Uuid::now_v7(), Uuid::now_v7())),
        }
    }

    fn selection(action: &str, document: Uuid, task: Option<Uuid>) -> PersonalTransferSelection {
        serde_json::from_value(json!({"action":action,"documentId":document,"taskId":task,
            "expectedDocumentVersion":1,"expectedTaskVersion":task.map(|_| 1),
            "destinationWorkspaceId":Uuid::now_v7(),"destinationProjectId":Uuid::now_v7(),
            "destinationStatusId":null}))
        .unwrap()
    }

    #[test]
    fn only_the_selected_pair_references_are_carried() {
        let (document, task, other) = (Uuid::now_v7(), Uuid::now_v7(), Uuid::now_v7());
        let body =
            |nodes: Value| json!({"type":"doc","content":[{"type":"paragraph","content":nodes}]});
        let mention = |entity: &str, id: Uuid| json!({"type":"mention","attrs":{"entity":entity,"id":id.to_string(),"label":"x"}});
        let embed = |entity: &str, id: Uuid| json!({"type":"embed","attrs":{"entity":entity,"ref":id.to_string()}});
        for action in ["move", "copy"] {
            let pair = selection(action, document, Some(task));
            assert!(!outside_reference(
                &body(json!([mention("task", task), embed("document", document)])),
                &pair
            ));
            assert!(!outside_reference(
                &body(json!([{"type":"text","text":"참조 없음"}])),
                &pair
            ));
            assert!(outside_reference(
                &body(json!([mention("task", task), mention("document", other)])),
                &pair
            ));
            // The same UUID under the other kind is not the pair member.
            assert!(outside_reference(
                &body(json!([mention("document", task)])),
                &pair
            ));
            let lone = selection(action, document, None);
            assert!(outside_reference(
                &body(json!([mention("task", task)])),
                &lone
            ));
        }
    }

    #[test]
    fn copy_points_selected_pair_references_at_the_copy() {
        let (document, task, other) = (Uuid::now_v7(), Uuid::now_v7(), Uuid::now_v7());
        let (new_document, new_task) = (Uuid::now_v7(), Uuid::now_v7());
        let pair = PairRefs {
            document: (document, new_document),
            task: Some((task, new_task)),
        };
        let block = Uuid::now_v7().to_string();
        let embed_block = Uuid::now_v7().to_string();
        let body = json!({"type":"doc","content":[
            {"type":"paragraph","attrs":{"id":block},"content":[
                {"type":"mention","attrs":{"entity":"task","id":task.to_string(),"label":"원본 작업"}}]},
            {"type":"embed","attrs":{"id":embed_block,"entity":"document","ref":document.to_string()}}]});
        let none = std::collections::HashMap::new();
        let (copied, ids) = copy_body(&body, &none, &pair).expect("pair references map");
        let mention = &copied["content"][0]["content"][0]["attrs"];
        assert_eq!(
            mention["id"],
            new_task.to_string(),
            "the mention target is the copy's task"
        );
        assert_eq!(mention["label"], "원본 작업");
        assert!(
            !ids.contains_key(&task.to_string()),
            "a mention target is not a block identity"
        );
        let embed = &copied["content"][1]["attrs"];
        assert_eq!(embed["ref"], new_document.to_string());
        assert_ne!(embed["id"], embed_block, "an embed's own block id is fresh");
        assert_eq!(
            embed["id"].as_str(),
            ids.get(&embed_block).map(String::as_str)
        );
        assert_ne!(copied["content"][0]["attrs"]["id"], block);
        assert!(!copied.to_string().contains(&task.to_string()));
        assert!(!copied.to_string().contains(&document.to_string()));
        // Anything else still refuses: outside target, crossed kind, a user
        // mention (no other entity support), several targets in one node.
        let refuse = |node: Value, expected: Blocker| {
            let body = json!({"type":"doc","content":[{"type":"paragraph","content":[node]}]});
            assert!(matches!(
                copy_body(&body, &none, &pair),
                Err(PersonalTransferDbError::Incomplete(blocker, "copy reference/file/node mapping")) if blocker == expected
            ));
        };
        refuse(
            json!({"type":"mention","attrs":{"entity":"task","id":other.to_string(),"label":"x"}}),
            Blocker::OutgoingReference,
        );
        refuse(
            json!({"type":"mention","attrs":{"entity":"document","id":task.to_string(),"label":"x"}}),
            Blocker::OutgoingReference,
        );
        refuse(
            json!({"type":"mention","attrs":{"entity":"user","id":other.to_string(),"label":"x"}}),
            Blocker::BodyEncoding,
        );
        refuse(
            json!({"type":"embed","attrs":{"entity":"task","ref":task.to_string(),"id":task.to_string()},
            "content":[{"type":"mention","attrs":{"entity":"task","id":other.to_string(),"label":"x"}}]}),
            Blocker::OutgoingReference,
        );
        // A parent never borrows a selected child's reference (F1): a malformed
        // embed target, an unknown embed entity and a user mention, each with a
        // child naming the selected pair, refuse instead of being rewritten.
        let child_document = json!([{"type":"mention","attrs":{"entity":"document","id":document.to_string(),"label":"x"}}]);
        let child_task =
            json!([{"type":"mention","attrs":{"entity":"task","id":task.to_string(),"label":"x"}}]);
        refuse(
            json!({"type":"embed","attrs":{"entity":"task","ref":"not-a-uuid"},"content":child_document}),
            Blocker::OutgoingReference,
        );
        refuse(
            json!({"type":"embed","attrs":{"entity":"person","ref":task.to_string()},"content":child_task}),
            Blocker::OutgoingReference,
        );
        refuse(
            json!({"type":"mention","attrs":{"entity":"user","id":other.to_string(),"label":"x"},"content":child_task}),
            Blocker::OutgoingReference,
        );
    }

    #[test]
    fn body_files_lists_every_shown_file_and_marks_unusable_ids() {
        let id = Uuid::now_v7();
        let body = json!({"type":"doc","content":[
            {"type":"paragraph","content":[{"type":"text","text":"본문"}]},
            {"type":"attachment","attrs":{"id":id.to_string(),"name":"a.pdf","image":true}},
            {"type":"callout","content":[{"type":"image","attrs":{"src":"x"}}]},
            {"type":"attachment","attrs":{"id":"not-a-uuid"}}
        ]});
        let mut shown = Vec::new();
        body_files(&body, &mut shown);
        assert_eq!(shown, vec![Some(id), None, None]);
        let mut none = Vec::new();
        body_files(&json!({"type":"doc","content":[]}), &mut none);
        assert!(none.is_empty());
    }

    #[test]
    fn native_copy_budget_admits_each_limit_and_refuses_one_more() {
        use crate::native_history::{MAX_BYTES, MAX_ENTRIES, MAX_GRAPH_BYTES};
        let (bytes, graph, rows) = (MAX_BYTES as i64, MAX_GRAPH_BYTES as i64, MAX_ENTRIES as i64);
        assert!(within_native_copy_budget(0, 0, 0));
        assert!(within_native_copy_budget(bytes, graph, rows));
        assert!(!within_native_copy_budget(bytes + 1, graph, rows));
        assert!(!within_native_copy_budget(bytes, graph + 1, rows));
        assert!(!within_native_copy_budget(bytes, graph, rows + 1));
    }

    #[test]
    fn native_history_errors_map_to_typed_blockers_or_server_errors() {
        let blocker = |error| match native_refusal(error) {
            Ok(PersonalTransferDbError::Incomplete(blocker, _)) => Some(blocker),
            _ => None,
        };
        assert_eq!(
            blocker(NativeDbError::Archive(ArchiveError::Limit)),
            Some(Blocker::InventoryBudget)
        );
        assert_eq!(
            blocker(NativeDbError::Archive(ArchiveError::Unsupported(
                "missing native state for captured body".into()
            ))),
            Some(Blocker::NativeStateMissing)
        );
        assert_eq!(
            blocker(NativeDbError::Archive(ArchiveError::Unsupported(
                "native encoding".into()
            ))),
            Some(Blocker::BodyEncoding)
        );
        for invalid in [
            "native history continuity",
            "native/body disagreement",
            "revision/native disagreement",
        ] {
            assert_eq!(
                blocker(NativeDbError::Archive(ArchiveError::Invalid(
                    invalid.into()
                ))),
                Some(Blocker::NativeHistory)
            );
        }
        assert_eq!(
            blocker(NativeDbError::Archive(ArchiveError::Unsupported(
                "collections".into()
            ))),
            Some(Blocker::NativeHistory)
        );
        assert!(matches!(
            native_refusal(NativeDbError::Forbidden),
            Ok(PersonalTransferDbError::NotFound)
        ));
        // Engine unavailability and SQL failures are server errors, never a
        // refusal the user could read as a property of their document.
        for error in [
            NativeDbError::Archive(ArchiveError::Worker),
            NativeDbError::Archive(ArchiveError::Cancelled),
            NativeDbError::Sql(sqlx::Error::PoolTimedOut),
        ] {
            assert!(native_refusal(error).is_err());
        }
    }

    fn paragraph(id: Value) -> Value {
        json!({"type":"paragraph","attrs":{"id":id},"content":[{"type":"text","text":"한글 🙂"}]})
    }

    #[test]
    fn copy_gives_every_block_identity_a_fresh_id() {
        let upper = "0190A1B2-C3D4-7E5F-8A9B-0C1D2E3F4A5B";
        let body = json!({"type":"doc","content":[
            paragraph(json!(upper)),
            paragraph(json!("legacy-block-7")),
            paragraph(json!(42)),
            paragraph(Value::Null),
            paragraph(json!("legacy-block-7")),
        ]});
        let (copied, ids) = copy_body(&body, &std::collections::HashMap::new(), &unrelated())
            .expect("supported body");
        let content = copied["content"].as_array().unwrap();
        for (index, original) in [
            (0, upper.to_lowercase()),
            (1, "legacy-block-7".into()),
            (2, "42".into()),
        ] {
            let fresh = content[index]["attrs"]["id"].as_str().unwrap();
            assert_ne!(fresh.to_lowercase(), original);
            assert_eq!(ids.get(&original).map(String::as_str), Some(fresh));
        }
        assert!(
            content[3]["attrs"]["id"].is_null(),
            "no identity stays none"
        );
        assert_eq!(content[1]["attrs"]["id"], content[4]["attrs"]["id"]);
        assert_eq!(content[0]["content"], body["content"][0]["content"]);
        assert_eq!(block_key(&json!(upper)), Some(upper.to_lowercase()));
    }

    #[test]
    fn copy_refuses_structured_ids_and_unsupported_nodes() {
        let structured = json!({"type":"doc","content":[paragraph(json!({"x":1}))]});
        let none = std::collections::HashMap::new();
        assert!(matches!(
            copy_body(&structured, &none, &unrelated()),
            Err(PersonalTransferDbError::Incomplete(
                Blocker::BlockIdentity,
                "copy block identity"
            ))
        ));
        let mention = json!({"type":"doc","content":[{"type":"paragraph","content":[
            {"type":"mention","attrs":{"entity":"task","id":Uuid::now_v7().to_string(),"label":"비공개"}}]}]});
        let callout = json!({"type":"doc","content":[{"type":"callout","content":[]}]});
        for (body, expected) in [
            (mention, Blocker::OutgoingReference),
            (callout, Blocker::BodyEncoding),
        ] {
            assert!(matches!(
                copy_body(&body, &none, &unrelated()),
                Err(PersonalTransferDbError::Incomplete(blocker, "copy reference/file/node mapping"))
                    if blocker == expected
            ));
        }
    }

    #[test]
    fn copy_content_follows_the_seed_truthiness_contract() {
        let none = std::collections::HashMap::new();
        for content in [
            json!({"x": 1}),
            json!({}),
            json!("a"),
            json!(1),
            json!(true),
        ] {
            for body in [
                json!({"type":"doc","content":[{"type":"paragraph","content":content.clone()}]}),
                json!({"type":"doc","content":content.clone()}),
            ] {
                assert!(
                    matches!(
                        copy_body(&body, &none, &unrelated()),
                        Err(PersonalTransferDbError::Incomplete(
                            Blocker::BodyEncoding,
                            "copy body content is not an array"
                        ))
                    ),
                    "{body}"
                );
            }
        }
        for content in [Value::Null, json!(false), json!(0), json!(""), json!([])] {
            let body = json!({"type":"doc","content":[{"type":"paragraph","content":content}]});
            let (copied, _) =
                copy_body(&body, &none, &unrelated()).expect("falsy or array content is accepted");
            assert_eq!(copied["content"][0]["type"], json!("paragraph"));
        }
    }

    #[test]
    fn copy_marks_follow_the_seed_truthiness_contract() {
        let none = std::collections::HashMap::new();
        // The seed reads `marks` on every node below the root (seed.rs
        // `node_from_json`), never on the root doc itself.
        for marks in [
            json!({"x": 1}),
            json!({}),
            json!("a"),
            json!(1),
            json!(true),
        ] {
            for body in [
                json!({"type":"doc","content":[{"type":"paragraph","marks":marks.clone()}]}),
                json!({"type":"doc","content":[{"type":"paragraph","content":[{"type":"text","text":"a","marks":marks.clone()}]}]}),
            ] {
                assert!(
                    matches!(
                        copy_body(&body, &none, &unrelated()),
                        Err(PersonalTransferDbError::Incomplete(
                            Blocker::BodyEncoding,
                            "copy body marks are not an array"
                        ))
                    ),
                    "{body}"
                );
            }
            let root = json!({"type":"doc","marks":marks.clone(),"content":[]});
            assert!(
                copy_body(&root, &none, &unrelated()).is_ok(),
                "root marks are not read by the seed: {root}"
            );
        }
        for marks in [Value::Null, json!(false), json!(0), json!(""), json!([])] {
            let body = json!({"type":"doc","content":[{"type":"paragraph","content":[{"type":"text","text":"a","marks":marks}]}]});
            let (copied, _) =
                copy_body(&body, &none, &unrelated()).expect("falsy or array marks are accepted");
            assert_eq!(copied["content"][0]["content"][0]["text"], json!("a"));
        }
    }

    #[test]
    fn copy_maps_pair_files_to_new_attachments_and_refuses_others() {
        let (source, copy, outside) = (Uuid::now_v7(), Uuid::now_v7(), Uuid::now_v7());
        let files = std::collections::HashMap::from([(source, copy)]);
        let body = json!({"type":"doc","content":[
            paragraph(json!("p1")),
            {"type":"attachment","attrs":{"id":source.to_string(),"name":"x.pdf","image":true}}
        ]});
        let (copied, _) = copy_body(&body, &files, &unrelated()).expect("pair file maps");
        assert_eq!(copied["content"][1]["attrs"]["id"], json!(copy.to_string()));
        assert_eq!(copied["content"][1]["attrs"]["name"], json!("x.pdf"));
        assert_eq!(copied["content"][1]["attrs"]["image"], json!(true));
        assert!(!copied.to_string().contains(&source.to_string()));
        for id in [json!(outside.to_string()), json!("a"), Value::Null] {
            let body = json!({"type":"doc","content":[{"type":"attachment","attrs":{"id":id}}]});
            assert!(matches!(
                copy_body(&body, &files, &unrelated()),
                Err(PersonalTransferDbError::Incomplete(
                    Blocker::File,
                    "copy body file outside the pair"
                ))
            ));
        }
        // Nested content of a file node is checked too: an outside file or a
        // reference inside a mapped attachment still refuses.
        let nested_file = json!({"type":"doc","content":[{"type":"attachment","attrs":{"id":source.to_string()},
            "content":[{"type":"attachment","attrs":{"id":outside.to_string()}}]}]});
        assert!(matches!(
            copy_body(&nested_file, &files, &unrelated()),
            Err(PersonalTransferDbError::Incomplete(
                Blocker::File,
                "copy body file outside the pair"
            ))
        ));
        let nested_ref = json!({"type":"doc","content":[{"type":"attachment","attrs":{"id":source.to_string()},
            "content":[{"type":"mention","attrs":{"entity":"task","id":Uuid::now_v7().to_string(),"label":"x"}}]}]});
        assert!(matches!(
            copy_body(&nested_ref, &files, &unrelated()),
            Err(PersonalTransferDbError::Incomplete(
                Blocker::OutgoingReference,
                _
            ))
        ));
        // A block nested in a mapped file node still gets a fresh identity
        // (the original block id never reaches the copy).
        let nested_block = json!({"type":"doc","content":[
            paragraph(json!("p-top")),
            {"type":"attachment","attrs":{"id":source.to_string()},
             "content":[paragraph(json!("p-nested"))]}
        ]});
        let (copied, ids) =
            copy_body(&nested_block, &files, &unrelated()).expect("nested block remaps");
        let nested = &copied["content"][1];
        assert_eq!(nested["attrs"]["id"], json!(copy.to_string()));
        let fresh = nested["content"][0]["attrs"]["id"].as_str().unwrap();
        assert_ne!(fresh, "p-nested");
        assert!(Uuid::parse_str(fresh).is_ok());
        assert_eq!(ids.get("p-nested").map(String::as_str), Some(fresh));
        assert_ne!(copied["content"][0]["attrs"]["id"], json!("p-top"));
        assert!(
            !copied.to_string().contains("p-nested")
                && !copied.to_string().contains(&source.to_string())
        );
        // An `image` node is outside the native seed schema: typed refusal.
        let image =
            json!({"type":"doc","content":[{"type":"image","attrs":{"id":source.to_string()}}]});
        assert!(matches!(
            copy_body(&image, &files, &unrelated()),
            Err(PersonalTransferDbError::Incomplete(
                Blocker::BodyEncoding,
                "copy body node outside the native schema"
            ))
        ));
    }

    #[test]
    fn incoming_scan_budget_bounds_projection_count() {
        let mut budget = ScanBudget::default();
        for _ in 0..INCOMING_SCAN_MAX_PROJECTIONS {
            assert!(budget.admit(1));
        }
        assert!(!budget.admit(0), "one projection past the cap is refused");
        assert_eq!(budget.projections, INCOMING_SCAN_MAX_PROJECTIONS);
    }

    #[test]
    fn incoming_scan_budget_bounds_total_bytes_across_rows() {
        let mut budget = ScanBudget::default();
        assert!(budget.admit(INCOMING_SCAN_MAX_BYTES - 1));
        assert!(budget.admit(1));
        assert!(!budget.admit(1), "total, not per-row, size is bounded");
        let mut huge = ScanBudget::default();
        assert!(!huge.admit(i64::MAX));
        assert_eq!(
            (huge.projections, huge.bytes),
            (0, 0),
            "refusal admits nothing"
        );
    }
}
