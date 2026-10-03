//! One scoped transaction transfers ordinary identities; unsupported graph
//! models fail before effects until the owned native/file adapter is connected.
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

fn copy_body(
    json: &Value,
    files: &std::collections::HashMap<Uuid, Uuid>,
) -> Result<(Value, std::collections::HashMap<String, String>), PersonalTransferDbError> {
    fn visit(
        node: &mut Value,
        depth: usize,
        ids: &mut std::collections::HashMap<String, String>,
        files: &std::collections::HashMap<Uuid, Uuid>,
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
        let kind = node.get("type").and_then(Value::as_str).unwrap_or("");
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
        } else if !matches!(
            kind,
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
        ) {
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
        // its attachment (mapped above); its children are still checked.
        if let Some(attrs) = node
            .get_mut("attrs")
            .and_then(Value::as_object_mut)
            .filter(|_| !file)
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
                visit(child, depth + 1, ids, files)?;
            }
        }
        Ok(())
    }
    let mut json = json.clone();
    let mut ids = std::collections::HashMap::new();
    visit(&mut json, 0, &mut ids, files)?;
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
}
struct Destination {
    workspace_name: String,
    project_name: String,
    visibility: String,
    root_id: Uuid,
    root_path: String,
    collection_id: Option<Uuid>,
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

/// 048 timer rows of the moved task (owner-private, actor-self RLS). The
/// task delete of a MOVE would cascade them away, so a MOVE refuses while any
/// run or legacy open reservation exists; relocating them needs a reviewed
/// timer checkpoint. A COPY leaves the original's timers untouched.
async fn owner_private_timer_rows(
    tx: &mut Transaction<'_, Postgres>,
    source: Uuid,
    actor: Uuid,
    selection: &PersonalTransferSelection,
) -> Result<bool, sqlx::Error> {
    let Some(task) = selection.task_id else {
        return Ok(false);
    };
    let previous: Option<String> =
        sqlx::query_scalar("SELECT current_setting('app.self_user_id', true)")
            .fetch_one(&mut **tx)
            .await?;
    set_self_user(tx, actor).await?;
    let found: bool = sqlx::query_scalar(
        "SELECT EXISTS(SELECT 1 FROM fvoci.task_timer_runs WHERE workspace_id=$1 AND task_id=$2)
            OR EXISTS(SELECT 1 FROM fvoci.task_timer_legacy_open WHERE workspace_id=$1 AND task_id=$2)",
    )
    .bind(source)
    .bind(task)
    .fetch_one(&mut **tx)
    .await?;
    sqlx::query("SELECT set_config('app.self_user_id', $1, true)")
        .bind(previous.unwrap_or_default())
        .execute(&mut **tx)
        .await?;
    Ok(found)
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
      OR EXISTS(SELECT 1 FROM fvoci.time_entries WHERE workspace_id=$1 AND task_id=$3)
      OR EXISTS(SELECT 1 FROM fvoci.collection_items WHERE workspace_id=$1 AND document_id=$2)
      OR EXISTS(SELECT 1 FROM fvoci.collection_values WHERE workspace_id=$1 AND item_id=$4)
      OR EXISTS(SELECT 1 FROM fvoci.collection_choices WHERE workspace_id=$1 AND item_id=$4)
      OR EXISTS(SELECT 1 FROM fvoci.collection_people WHERE workspace_id=$1 AND item_id=$4)
      OR EXISTS(SELECT 1 FROM fvoci.task_origins WHERE workspace_id=$1 AND document_id=$2 AND task_id IS DISTINCT FROM $3)
    "#).bind(source).bind(selection.document_id).bind(selection.task_id).bind(collection_item.as_ref().map(|item|item.id)).bind(selection.action==PersonalTransferAction::Move).fetch_one(&mut **tx).await?;
    let zotero = selection.action == PersonalTransferAction::Move
        && (owner_private_zotero_rows(tx, source, actor, selection).await?
            || owner_private_timer_rows(tx, source, actor, selection).await?);
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
        if !extract_internal_refs(&body.json).is_empty() {
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
    if !extract_internal_refs(&document_body.json).is_empty() {
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
        let (_, block_ids) = match copy_body(&document.content_json, &pair_files) {
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
            if let Err(error) = copy_body(&task.content_json, &pair_files) {
                return Ok(Err(error));
            }
        }
    }
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
                let rejected = std::cell::Cell::new(None);
                let closed = |kind, id| {
                    let present = match kind {
                        collab_engine::archive_history::ReferenceKind::Document => {
                            id == selection.document_id
                        }
                        collab_engine::archive_history::ReferenceKind::Task => {
                            Some(id) == selection.task_id
                        }
                        // Only the exact locked files that move with the pair.
                        collab_engine::archive_history::ReferenceKind::Attachment => {
                            moved_files.contains(&id)
                        }
                        _ => false,
                    };
                    if !present {
                        rejected.set(Some(kind));
                    }
                    present
                };
                if let Some(blocker) = inventory.closure_blocker(closed) {
                    let file = blocker == RetainedHistoryBlocker::OutsideClosure
                        && rejected.get()
                            == Some(collab_engine::archive_history::ReferenceKind::Attachment);
                    let (typed, reason) = match blocker {
                        RetainedHistoryBlocker::Incomplete
                        | RetainedHistoryBlocker::Unavailable => {
                            (Blocker::NativeHistory, blocker.reason())
                        }
                        _ if file => (Blocker::File, "retained file outside the moved pair"),
                        _ => (Blocker::OutgoingReference, blocker.reason()),
                    };
                    return Ok(Err(PersonalTransferDbError::Incomplete(typed, reason)));
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
    let canonical = json!({"files":files,"native":native,"source":source,"actor":actor,"session":session,"selection":selection,"document":{"title":doc.title,"icon":doc.icon,"kind":doc.kind,"status":doc.status,"version":doc.version,"body":doc.content_json,"updatedAt":doc.updated_at},"task":task,"bodyCuts":graph.body_cuts,"assignees":graph.assignees,"activity":activities,"destination":{"name":destination.workspace_name,"projectName":destination.project_name,"visibility":destination.visibility,"root":destination.root_id,"path":destination.root_path}});
    hex::encode(Sha256::digest(canonical.to_string().as_bytes()))
}

/// Observed parts of the disclosure graph and what this command does to each.
/// Only admitted graphs reach here: files, native history on MOVE, references
/// and dependents have already refused with a typed blocker.
async fn dispositions(
    tx: &mut Transaction<'_, Postgres>,
    source: Uuid,
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
    let dispositions = dispositions(&mut tx, source, selection, &graph).await?;
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
    let (document_json, block_ids) = if moving {
        (
            graph.document.content_json.clone(),
            std::collections::HashMap::new(),
        )
    } else {
        copy_body(&graph.document.content_json, &copied_files)
            .map_err(|_| sqlx::Error::Protocol("personal transfer copy body changed".into()))?
    };
    let task_json = match &graph.task {
        Some(task) if !moving => Some(
            copy_body(&task.content_json, &copied_files)
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

#[cfg(test)]
mod tests {
    use super::*;

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
        let (copied, ids) =
            copy_body(&body, &std::collections::HashMap::new()).expect("supported body");
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
            copy_body(&structured, &none),
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
                copy_body(&body, &none),
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
                        copy_body(&body, &none),
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
            let (copied, _) = copy_body(&body, &none).expect("falsy or array content is accepted");
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
                        copy_body(&body, &none),
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
                copy_body(&root, &none).is_ok(),
                "root marks are not read by the seed: {root}"
            );
        }
        for marks in [Value::Null, json!(false), json!(0), json!(""), json!([])] {
            let body = json!({"type":"doc","content":[{"type":"paragraph","content":[{"type":"text","text":"a","marks":marks}]}]});
            let (copied, _) = copy_body(&body, &none).expect("falsy or array marks are accepted");
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
        let (copied, _) = copy_body(&body, &files).expect("pair file maps");
        assert_eq!(copied["content"][1]["attrs"]["id"], json!(copy.to_string()));
        assert_eq!(copied["content"][1]["attrs"]["name"], json!("x.pdf"));
        assert_eq!(copied["content"][1]["attrs"]["image"], json!(true));
        assert!(!copied.to_string().contains(&source.to_string()));
        for id in [json!(outside.to_string()), json!("a"), Value::Null] {
            let body = json!({"type":"doc","content":[{"type":"attachment","attrs":{"id":id}}]});
            assert!(matches!(
                copy_body(&body, &files),
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
            copy_body(&nested_file, &files),
            Err(PersonalTransferDbError::Incomplete(
                Blocker::File,
                "copy body file outside the pair"
            ))
        ));
        let nested_ref = json!({"type":"doc","content":[{"type":"attachment","attrs":{"id":source.to_string()},
            "content":[{"type":"mention","attrs":{"entity":"task","id":Uuid::now_v7().to_string(),"label":"x"}}]}]});
        assert!(matches!(
            copy_body(&nested_ref, &files),
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
        let (copied, ids) = copy_body(&nested_block, &files).expect("nested block remaps");
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
            copy_body(&image, &files),
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
