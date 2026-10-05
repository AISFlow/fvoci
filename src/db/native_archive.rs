//! Coherent app-role capture and one atomic native graph publication.
use std::collections::BTreeMap;

use serde::de::DeserializeOwned;
use serde::Serialize;
use serde_json::{json, Value};
use sqlx::{PgPool, Postgres, Row, Transaction};
use uuid::Uuid;

use crate::db::context::{
    begin_read, lock_membership_users, lock_tree, recheck_session, session_is_live, set_self_user,
    set_tenant,
};
use crate::db::identity::{AuditAppend, EventAppend};
use crate::db::import_jobs::{hold_import_fence, ImportClaim};
use crate::db::workspace::{membership_role, WorkspaceRole};
use crate::native_archive::*;

pub use crate::db::native_history::{
    native_history_inventory, NativeDbError, NativeHistoryInventory,
};
use crate::projects::ProjectPermission;

pub struct Capture {
    pub archive: Archive,
    /// Operational keys are used to read authorized files, never serialized.
    pub file_keys: BTreeMap<Uuid, String>,
}

/// Source selection: one project plus explicitly selected Zotero connectors
/// of the actor in the same personal workspace (empty = project only).
pub struct Selection<'a> {
    pub project: Uuid,
    pub zotero_connectors: &'a [Uuid],
}

async fn source_gate(
    tx: &mut Transaction<'_, Postgres>,
    workspace: Uuid,
    actor: Uuid,
    session: Uuid,
    selection: &Selection<'_>,
) -> Result<(), NativeDbError> {
    let project = selection.project;
    let connectors = selection.zotero_connectors;
    if !session_is_live(tx, actor, session).await?
        || !membership_role(tx, workspace, actor)
            .await?
            .is_some_and(|r| r.at_least(WorkspaceRole::Admin))
        || !sqlx::query_scalar::<_, bool>(
            "SELECT EXISTS(SELECT 1 FROM fvoci.workspaces WHERE id=$1 AND deleted_at IS NULL)",
        )
        .bind(workspace)
        .fetch_one(&mut **tx)
        .await?
    {
        return Err(NativeDbError::Forbidden);
    }
    if !crate::db::projects::project_permission_by_id(tx, workspace, actor, project)
        .await?
        .is_some_and(|p| p.at_least(ProjectPermission::View))
    {
        return Err(NativeDbError::Forbidden);
    }
    // Migration 051 rows are tenant AND owner scoped: the actor reads only
    // their own mirror, set for every source transaction (also to see links
    // from unselected connectors into the selection).
    set_self_user(tx, actor).await?;
    if connectors.is_empty() {
        return Ok(());
    }
    if connectors.len() > MAX_ZOTERO_CONNECTORS
        || connectors
            .iter()
            .collect::<std::collections::BTreeSet<_>>()
            .len()
            != connectors.len()
    {
        return Err(ArchiveError::Invalid("zotero connector selection".into()).into());
    }
    let owned: i64 = sqlx::query_scalar("SELECT count(*) FROM fvoci.zotero_connectors WHERE workspace_id=$1 AND owner_user_id=$2 AND id=ANY($3)")
        .bind(workspace).bind(actor).bind(connectors).fetch_one(&mut **tx).await?;
    if !crate::db::personal_input::owns_personal_workspace(tx, workspace, actor).await?
        || owned != connectors.len() as i64
    {
        return Err(NativeDbError::Forbidden);
    }
    Ok(())
}

pub async fn recheck_source(
    pool: &PgPool,
    workspace: Uuid,
    actor: Uuid,
    session: Uuid,
    selection: &Selection<'_>,
) -> Result<(), NativeDbError> {
    let mut tx = begin_read(pool).await?;
    set_tenant(&mut tx, workspace).await?;
    source_gate(&mut tx, workspace, actor, session, selection).await?;
    tx.commit().await?;
    Ok(())
}

fn graph_wiki(graph: &Graph) -> Vec<Uuid> {
    graph
        .documents
        .iter()
        .filter(|d| d.project_id.is_none())
        .map(|d| d.id)
        .collect()
}

fn graph_connectors(graph: &Graph) -> Vec<Uuid> {
    graph.zotero_connectors.iter().map(|c| c.id).collect()
}

/// Reauthorize the captured resource set at delivery time, including files
/// removed or moved after the coherent capture. Project permission alone does
/// not authorize bytes from a resource which has since left that project.
pub async fn recheck_delivery(
    pool: &PgPool,
    workspace: Uuid,
    actor: Uuid,
    session: Uuid,
    graph: &Graph,
    file_keys: &BTreeMap<Uuid, String>,
) -> Result<(), NativeDbError> {
    let project = graph.project.id;
    let wiki = graph_wiki(graph);
    let connectors = graph_connectors(graph);
    let mut tx = begin_read(pool).await?;
    set_tenant(&mut tx, workspace).await?;
    source_gate(
        &mut tx,
        workspace,
        actor,
        session,
        &Selection {
            project,
            zotero_connectors: &connectors,
        },
    )
    .await?;
    for (table, ids, live_ids) in [
        (
            "documents",
            graph.documents.iter().map(|d| d.id).collect::<Vec<_>>(),
            graph
                .documents
                .iter()
                .filter(|d| d.deleted_at.is_none())
                .map(|d| d.id)
                .collect::<Vec<_>>(),
        ),
        (
            "tasks",
            graph.tasks.iter().map(|t| t.id).collect::<Vec<_>>(),
            graph
                .tasks
                .iter()
                .filter(|t| t.deleted_at.is_none())
                .map(|t| t.id)
                .collect::<Vec<_>>(),
        ),
    ] {
        let count: i64 = sqlx::query_scalar(&format!(
            "SELECT count(*) FROM fvoci.{table} WHERE (project_id=$1 OR (project_id IS NULL AND id=ANY($4))) AND id=ANY($2) AND (NOT id=ANY($3) OR deleted_at IS NULL)"
        )).bind(project).bind(&ids).bind(&live_ids).bind(&wiki).fetch_one(&mut *tx).await?;
        if count != ids.len() as i64 {
            return Err(NativeDbError::Forbidden);
        }
    }
    let files = graph
        .attachments
        .iter()
        .map(|file| {
            let key = file_keys.get(&file.id).ok_or(NativeDbError::Forbidden)?;
            Ok(
                json!({"id":file.id,"document_id":file.document_id,"task_id":file.task_id,
            "storage_key":key,"size_bytes":file.size_bytes}),
            )
        })
        .collect::<Result<Vec<_>, NativeDbError>>()?;
    let count: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM fvoci.attachments a
        JOIN jsonb_to_recordset($2) AS e(id uuid,document_id uuid,task_id uuid,storage_key text,size_bytes bigint)
          ON a.id=e.id AND a.document_id IS NOT DISTINCT FROM e.document_id
          AND a.task_id IS NOT DISTINCT FROM e.task_id AND a.storage_key=e.storage_key AND a.size_bytes=e.size_bytes
        LEFT JOIN fvoci.documents d ON d.id=a.document_id
        LEFT JOIN fvoci.tasks t ON t.id=a.task_id
        WHERE a.status='stored' AND a.scan_status IN ('skipped','clean')
          AND (((d.project_id=$1 OR (d.project_id IS NULL AND d.id=ANY($3))) AND d.deleted_at IS NULL) OR (t.project_id=$1 AND t.deleted_at IS NULL))"
    ).bind(project).bind(json!(files)).bind(&wiki).fetch_one(&mut *tx).await?;
    if count != graph.attachments.len() as i64 {
        return Err(NativeDbError::Forbidden);
    }
    tx.commit().await?;
    Ok(())
}

/// Bind parameters shared by every owned capture query: $1 project, $2 the
/// selected wiki document ids, $3 workspace, $4 actor, $5 selected connectors.
/// Postgres receives each parameter's type, so a query may use a subset.
struct Scope {
    project: Uuid,
    wiki: Vec<Uuid>,
    workspace: Uuid,
    actor: Uuid,
    connectors: Vec<Uuid>,
    /// Graph JSON bytes admitted so far by `rows`, across every graph array:
    /// the 16 MiB graph budget is cumulative and charged before each fetch.
    graph_bytes: std::sync::atomic::AtomicI64,
}

/// Selected documents: the project's, plus the Zotero wiki closure.
const DOCS: &str =
    "(SELECT id FROM fvoci.documents WHERE project_id=$1 OR (project_id IS NULL AND id=ANY($2)))";

/// Archived collections: the project's, plus the workspace wiki document
/// collections holding a wiki-closure document (capture refuses one that
/// also holds anything outside the closure).
const COLLECTIONS: &str = "(SELECT id FROM fvoci.collections WHERE project_id=$1 UNION ALL SELECT c.id FROM fvoci.collections c WHERE c.project_id IS NULL AND c.kind='document' AND EXISTS(SELECT 1 FROM fvoci.collection_items i WHERE i.collection_id=c.id AND i.document_id=ANY($2)))";

fn scoped<'q>(
    query: sqlx::query::Query<'q, Postgres, sqlx::postgres::PgArguments>,
    scope: &'q Scope,
) -> sqlx::query::Query<'q, Postgres, sqlx::postgres::PgArguments> {
    query
        .bind(scope.project)
        .bind(&scope.wiki)
        .bind(scope.workspace)
        .bind(scope.actor)
        .bind(&scope.connectors)
}

/// Charges bytes the capture materializes outside `rows` to the same
/// cumulative graph budget.
fn admit_graph_bytes(scope: &Scope, bytes: usize) -> Result<(), NativeDbError> {
    let admitted = i64::try_from(bytes).ok().and_then(|bytes| {
        scope
            .graph_bytes
            .load(std::sync::atomic::Ordering::Relaxed)
            .checked_add(bytes)
    });
    match admitted {
        Some(admitted) if admitted <= MAX_GRAPH_BYTES as i64 => {
            scope
                .graph_bytes
                .store(admitted, std::sync::atomic::Ordering::Relaxed);
            Ok(())
        }
        _ => Err(ArchiveError::Limit.into()),
    }
}

/// Charges the history's label/milestone references plus the view-filter
/// bound (two per view row), 39 bytes each, to the cumulative graph budget;
/// capture calls it before any filter is parsed or collected.
fn admit_history_references(
    scope: &Scope,
    activity: &[Activity],
    views: &[View],
    collection_views: &[CollectionView],
) -> Result<(), NativeDbError> {
    let bytes = activity_reference_count(activity)?
        .checked_add(view_filter_ref_bound(views, collection_views)?)
        .and_then(|references| references.checked_mul(HISTORY_REFERENCE_BYTES))
        .ok_or(ArchiveError::Limit)?;
    admit_graph_bytes(scope, bytes)
}

async fn rows<T: DeserializeOwned>(
    tx: &mut Transaction<'_, Postgres>,
    sql: &str,
    scope: &Scope,
) -> Result<Vec<T>, NativeDbError> {
    let sql = sql
        .replace("{COLLECTIONS}", COLLECTIONS)
        .replace("{DOCS}", DOCS);
    let sql = sql.as_str();
    // Refuse oversized JSON sets in the coherent source transaction before
    // fetching them into the parent. LIMIT in each owned query also bounds
    // counts; a 10001st row is deliberately a failure, not a silent truncation.
    let (count, bytes): (i64, i64) = sqlx::query_as(&format!(
        "SELECT count(*)::bigint,coalesce(sum(octet_length(v::text)),0)::bigint FROM ({sql}) AS q(v)"
    ))
    .bind(scope.project)
    .bind(&scope.wiki)
    .bind(scope.workspace)
    .bind(scope.actor)
    .bind(&scope.connectors)
    .fetch_one(&mut **tx)
    .await?;
    let admitted = scope
        .graph_bytes
        .load(std::sync::atomic::Ordering::Relaxed)
        .saturating_add(bytes);
    if count > MAX_ENTRIES as i64 || bytes < 0 || admitted > MAX_GRAPH_BYTES as i64 {
        return Err(ArchiveError::Limit.into());
    }
    scope
        .graph_bytes
        .store(admitted, std::sync::atomic::Ordering::Relaxed);
    let values: Vec<Value> = sqlx::query_scalar(sql)
        .bind(scope.project)
        .bind(&scope.wiki)
        .bind(scope.workspace)
        .bind(scope.actor)
        .bind(&scope.connectors)
        .fetch_all(&mut **tx)
        .await?;
    if values.len() > MAX_ENTRIES {
        return Err(ArchiveError::Limit.into());
    }
    values
        .into_iter()
        .map(|value| {
            serde_json::from_value(value)
                .map_err(|_| ArchiveError::Unsupported("record schema".into()).into())
        })
        .collect()
}

pub async fn capture(
    pool: &PgPool,
    workspace: Uuid,
    actor: Uuid,
    session: Uuid,
    selection: &Selection<'_>,
) -> Result<Capture, NativeDbError> {
    let project = selection.project;
    let mut tx = begin_read(pool).await?;
    set_tenant(&mut tx, workspace).await?;
    source_gate(&mut tx, workspace, actor, session, selection).await?;
    let mut scope = Scope {
        project,
        wiki: Vec::new(),
        workspace,
        actor,
        connectors: selection.zotero_connectors.to_vec(),
        graph_bytes: std::sync::atomic::AtomicI64::new(0),
    };
    scope.wiki = wiki_closure(&mut tx, &scope).await?;
    capture_payload_budget(&mut tx, &scope).await?;
    let project_row = rows::<Project>(&mut tx, "SELECT to_jsonb(p)-'workspace_id' FROM fvoci.projects p WHERE id=$1 AND deleted_at IS NULL LIMIT 1", &scope).await?
        .pop().ok_or(NativeDbError::Forbidden)?;
    let documents = rows(&mut tx, "SELECT to_jsonb(d)-'workspace_id' FROM fvoci.documents d WHERE d.id IN {DOCS} ORDER BY path COLLATE \"C\",id LIMIT 10001", &scope).await?;
    // numeric estimates travel as their exact decimal text (a JSON number
    // would round through f64); publish casts the text back to numeric.
    let tasks = rows(&mut tx, "SELECT (to_jsonb(t)-'workspace_id')||jsonb_build_object('estimate',t.estimate::text) FROM fvoci.tasks t WHERE project_id=$1 ORDER BY id LIMIT 10001", &scope).await?;
    let workflows = rows(&mut tx, "SELECT to_jsonb(w)-'workspace_id' FROM fvoci.workflows w WHERE project_id=$1 ORDER BY id LIMIT 10001", &scope).await?;
    let statuses = rows(&mut tx, "SELECT to_jsonb(s)-'workspace_id' FROM fvoci.statuses s WHERE project_id=$1 ORDER BY sort_key COLLATE \"C\",id LIMIT 10001", &scope).await?;
    let assignees = rows(&mut tx, "SELECT to_jsonb(a)-'workspace_id' FROM fvoci.task_assignees a JOIN fvoci.tasks t ON t.id=a.task_id WHERE t.project_id=$1 ORDER BY a.task_id,a.user_id LIMIT 10001", &scope).await?;
    let comments = rows(&mut tx, "SELECT to_jsonb(c)-'workspace_id' FROM fvoci.comments c WHERE c.document_id IN {DOCS} OR c.task_id IN(SELECT id FROM fvoci.tasks WHERE project_id=$1) ORDER BY c.created_at,c.id LIMIT 10001", &scope).await?;
    let labels = rows(&mut tx, "SELECT to_jsonb(l)-'workspace_id' FROM fvoci.labels l WHERE l.project_id=$1 ORDER BY l.id LIMIT 10001", &scope).await?;
    let task_labels = rows(&mut tx, "SELECT to_jsonb(x)-'workspace_id' FROM fvoci.task_labels x JOIN fvoci.tasks t ON t.id=x.task_id WHERE t.project_id=$1 ORDER BY x.task_id,x.label_id LIMIT 10001", &scope).await?;
    let milestones = rows(&mut tx, "SELECT to_jsonb(m)-'workspace_id' FROM fvoci.milestones m WHERE m.project_id=$1 ORDER BY m.id LIMIT 10001", &scope).await?;
    // Tags named by archived documents' assignments (unassigned workspace
    // tags are workspace data, not this project's).
    let document_tag_assignments = rows(&mut tx, "SELECT jsonb_build_object('document_id',a.document_id,'tag_id',a.tag_id) FROM fvoci.document_tag_assignments a WHERE a.document_id IN {DOCS} ORDER BY a.document_id,a.tag_id LIMIT 10001", &scope).await?;
    let document_tags = rows(&mut tx, "SELECT to_jsonb(t)-'workspace_id' FROM fvoci.document_tags t WHERE t.id IN(SELECT a.tag_id FROM fvoci.document_tag_assignments a WHERE a.document_id IN {DOCS}) ORDER BY t.id LIMIT 10001", &scope).await?;
    // Every person's saved views of the project are read so another person's
    // private view is refused by validation, never silently left behind.
    let views: Vec<View> = rows(&mut tx, "SELECT to_jsonb(v)-'workspace_id' FROM fvoci.views v WHERE v.project_id=$1 ORDER BY v.id LIMIT 10001", &scope).await?;
    // Either end in the project: the writer keeps both ends in one project,
    // and validation refuses an edge whose other end is not archived.
    let dependencies = rows(&mut tx, "SELECT to_jsonb(d)-'workspace_id' FROM fvoci.task_dependencies d WHERE d.blocker_id IN(SELECT id FROM fvoci.tasks WHERE project_id=$1) OR d.blocked_id IN(SELECT id FROM fvoci.tasks WHERE project_id=$1) ORDER BY d.blocker_id,d.blocked_id LIMIT 10001", &scope).await?;
    let origins = rows(&mut tx, "SELECT to_jsonb(o)-'workspace_id' FROM fvoci.task_origins o JOIN fvoci.tasks t ON t.id=o.task_id WHERE t.project_id=$1 ORDER BY o.task_id LIMIT 10001", &scope).await?;
    let activity: Vec<Activity> = rows(&mut tx, "SELECT to_jsonb(a)-'workspace_id' FROM fvoci.task_activity a JOIN fvoci.tasks t ON t.id=a.task_id WHERE t.project_id=$1 ORDER BY a.id LIMIT 10001", &scope).await?;
    let collections = rows(&mut tx, "SELECT to_jsonb(c)-'workspace_id' FROM fvoci.collections c WHERE c.id IN {COLLECTIONS} ORDER BY c.id LIMIT 10001", &scope).await?;
    let collection_items = rows(&mut tx, "SELECT to_jsonb(i)-'workspace_id' FROM fvoci.collection_items i WHERE i.collection_id IN {COLLECTIONS} ORDER BY i.id LIMIT 10001", &scope).await?;
    // Person-made collection state of the archived collections; numbers as
    // their exact numeric text. Person-scoped rows of another person are read
    // so validation refuses them, never leaves them behind.
    let collection_fields = rows(&mut tx, "SELECT to_jsonb(f)-'workspace_id' FROM fvoci.collection_fields f WHERE f.collection_id IN {COLLECTIONS} ORDER BY f.id LIMIT 10001", &scope).await?;
    let collection_options = rows(&mut tx, "SELECT to_jsonb(o)-'workspace_id' FROM fvoci.collection_options o WHERE o.collection_id IN {COLLECTIONS} ORDER BY o.id LIMIT 10001", &scope).await?;
    let collection_values = rows(&mut tx, "SELECT (to_jsonb(v)-'workspace_id'-'value_number')||jsonb_build_object('value_number',v.value_number::text) FROM fvoci.collection_values v WHERE v.collection_id IN {COLLECTIONS} ORDER BY v.item_id,v.field_id LIMIT 10001", &scope).await?;
    let collection_choices = rows(&mut tx, "SELECT to_jsonb(x)-'workspace_id' FROM fvoci.collection_choices x WHERE x.collection_id IN {COLLECTIONS} ORDER BY x.item_id,x.field_id,x.option_id LIMIT 10001", &scope).await?;
    let collection_people = rows(&mut tx, "SELECT to_jsonb(x)-'workspace_id' FROM fvoci.collection_people x WHERE x.collection_id IN {COLLECTIONS} ORDER BY x.item_id,x.field_id,x.user_id LIMIT 10001", &scope).await?;
    let collection_views: Vec<CollectionView> = rows(&mut tx, "SELECT to_jsonb(v)-'workspace_id' FROM fvoci.collection_views v WHERE v.collection_id IN {COLLECTIONS} ORDER BY v.id LIMIT 10001", &scope).await?;
    // Purged labels/milestones the captured history or the captured view
    // filters still name: the history references and the view-filter bound
    // (two per view row) are counted and charged to the graph budget before
    // the filters are collected or any set is built (distinct purged ids are
    // capped per kind as they are added).
    // Tasks never change project and the writers assign only the project's
    // own labels/milestones, so a named id with no live project row was
    // purged; one still live anywhere in the workspace, as either kind, is
    // outside the closure, never a purged reference.
    admit_history_references(&scope, &activity, &views, &collection_views)?;
    let view_refs = view_filter_refs(&views, &collection_views);
    let live_labels: std::collections::BTreeSet<Uuid> =
        labels.iter().map(|l: &Label| l.id).collect();
    let live_milestones: std::collections::BTreeSet<Uuid> =
        milestones.iter().map(|m: &Milestone| m.id).collect();
    let (purged_labels, purged_milestones) =
        purged_refs(&activity, &view_refs, &live_labels, &live_milestones)?;
    let purged_label_refs: Vec<Uuid> = purged_labels.into_iter().collect();
    let purged_milestone_refs: Vec<Uuid> = purged_milestones.into_iter().collect();
    if !(purged_label_refs.is_empty() && purged_milestone_refs.is_empty()) {
        let named: Vec<Uuid> = purged_label_refs
            .iter()
            .chain(&purged_milestone_refs)
            .copied()
            .collect();
        let outside: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM fvoci.labels WHERE id=ANY($1)) OR EXISTS(SELECT 1 FROM fvoci.milestones WHERE id=ANY($1))")
            .bind(&named)
            .fetch_one(&mut *tx)
            .await?;
        if outside {
            return Err(
                ArchiveError::Unsupported("reference outside selected closure".into()).into(),
            );
        }
    }
    let mut entries = BTreeMap::new();
    let mut states = Vec::new();
    for (kind, table, parent) in [
        ("document", "document", "documents"),
        ("task", "task", "tasks"),
    ] {
        let selected = if kind == "document" {
            DOCS
        } else {
            "(SELECT id FROM fvoci.tasks WHERE project_id=$1)"
        };
        let state_rows = scoped(sqlx::query(&format!("SELECT to_jsonb(s)-'workspace_id'-'{table}_id'-'state'-'writer_generation' AS meta,s.{table}_id AS id,s.state FROM fvoci.{table}_states s JOIN fvoci.{parent} p ON p.id=s.{table}_id WHERE p.id IN {selected} ORDER BY p.id LIMIT 10001")), &scope)
            .fetch_all(&mut *tx).await?;
        if state_rows.len() > MAX_OBJECTS {
            return Err(ArchiveError::Limit.into());
        }
        for row in state_rows {
            let id: Uuid = row.get("id");
            let state: Vec<u8> = row.get("state");
            let state_entry = format!("native/{kind}/{id}/state.v1");
            entries.insert(state_entry.clone(), encode(&state));
            let mut meta: Value = row.get("meta");
            meta["target_kind"] = json!(kind);
            meta["target_id"] = json!(id);
            meta["state_entry"] = json!(state_entry);
            let cutoff = meta["snapshot_cutoff_seq"]
                .as_i64()
                .ok_or_else(|| ArchiveError::Invalid("cutoff".into()))?;
            let tail = meta["tail_seq"]
                .as_i64()
                .ok_or_else(|| ArchiveError::Invalid("tail".into()))?;
            let updates = sqlx::query(&format!("SELECT seq,op_id,payload,created_at FROM fvoci.{table}_collab_updates WHERE {table}_id=$1 AND seq>$2 AND seq<=$3 ORDER BY seq LIMIT 10001"))
                .bind(id).bind(cutoff).bind(tail).fetch_all(&mut *tx).await?;
            if updates.len() > MAX_ENTRIES {
                return Err(ArchiveError::Limit.into());
            }
            let updates: Vec<_> = updates
                .into_iter()
                .map(|row| {
                    let seq: i64 = row.get("seq");
                    let payload: Vec<u8> = row.get("payload");
                    let name = format!("native/{kind}/{id}/{seq}.v1");
                    entries.insert(name.clone(), encode(&payload));
                    Update {
                        seq,
                        op_id: row.get("op_id"),
                        payload_entry: name,
                        created_at: row
                            .get::<chrono::DateTime<chrono::Utc>, _>("created_at")
                            .to_rfc3339(),
                    }
                })
                .collect();
            let receipt_values: Vec<Value> = sqlx::query_scalar(&format!("SELECT to_jsonb(r)-'workspace_id'-'{table}_id'-'payload_sha256' || jsonb_build_object('payload_sha256',encode(r.payload_sha256,'hex')) FROM fvoci.{table}_collab_op_receipts r WHERE {table}_id=$1 ORDER BY seq,op_id LIMIT 10001"))
                .bind(id).fetch_all(&mut *tx).await?;
            if receipt_values.len() > MAX_ENTRIES {
                return Err(ArchiveError::Limit.into());
            }
            meta["updates"] = json!(updates);
            meta["receipts"] = json!(receipt_values);
            states.push(
                serde_json::from_value(meta)
                    .map_err(|_| ArchiveError::Unsupported("native record schema".into()))?,
            );
        }
    }
    let revision_rows = scoped(sqlx::query(&format!("SELECT to_jsonb(r)-'workspace_id'-'y_snapshot' AS meta,r.y_snapshot FROM fvoci.revisions r WHERE (target_kind='document' AND target_id IN {DOCS}) OR (target_kind='task' AND target_id IN(SELECT id FROM fvoci.tasks WHERE project_id=$1)) ORDER BY r.created_at,r.id LIMIT 10001")), &scope)
        .fetch_all(&mut *tx).await?;
    if revision_rows.len() > MAX_ENTRIES {
        return Err(ArchiveError::Limit.into());
    }
    let mut revisions = Vec::new();
    for row in revision_rows {
        let mut meta: Value = row.get("meta");
        let name = format!(
            "revisions/{}.snapshot.v1",
            meta["id"]
                .as_str()
                .ok_or_else(|| ArchiveError::Invalid("revision id".into()))?
        );
        let bytes: Vec<u8> = row.get("y_snapshot");
        entries.insert(name.clone(), encode(&bytes));
        meta["snapshot_entry"] = json!(name);
        revisions.push(
            serde_json::from_value(meta)
                .map_err(|_| ArchiveError::Unsupported("revision schema".into()))?,
        );
    }
    let file_rows = scoped(sqlx::query(&format!("SELECT id,document_id,task_id,uploader_id,name,mime,declared_mime,size_bytes,image,created_at,completed_at,storage_key,status,scan_status FROM fvoci.attachments WHERE document_id IN {DOCS} OR task_id IN(SELECT id FROM fvoci.tasks WHERE project_id=$1) ORDER BY id LIMIT 10001")), &scope)
        .fetch_all(&mut *tx).await?;
    if file_rows.len() > MAX_OBJECTS {
        return Err(ArchiveError::Limit.into());
    }
    let mut file_keys = BTreeMap::new();
    let mut attachments = Vec::new();
    for r in file_rows {
        // The product has no scanner today: ordinary uploads are stored as
        // 'skipped'. Only a known infected or unfinished file is refused.
        if r.get::<String, _>("status") != "stored"
            || !matches!(
                r.get::<String, _>("scan_status").as_str(),
                "skipped" | "clean"
            )
        {
            return Err(ArchiveError::Unsupported(
                "attachment must be stored and not infected".into(),
            )
            .into());
        }
        let id: Uuid = r.get("id");
        file_keys.insert(id, r.get("storage_key"));
        attachments.push(Attachment {
            id,
            document_id: r.get("document_id"),
            task_id: r.get("task_id"),
            uploader_id: r.get("uploader_id"),
            name: r.get("name"),
            mime: r.get("mime"),
            declared_mime: r.get("declared_mime"),
            size_bytes: r.get("size_bytes"),
            image: r.get("image"),
            created_at: r
                .get::<chrono::DateTime<chrono::Utc>, _>("created_at")
                .to_rfc3339(),
            completed_at: r
                .get::<chrono::DateTime<chrono::Utc>, _>("completed_at")
                .to_rfc3339(),
            payload_entry: format!("attachments/{id}/payload"),
        });
    }
    reject_unsupported(&mut tx, &scope).await?;
    // Exact 051 canonical columns; operational sync state is not captured and
    // zotero_credentials is never selected.
    let zotero_connectors = rows(&mut tx, "SELECT jsonb_build_object('id',c.id,'library_type',c.library_type,'remote_library_id',c.remote_library_id,'library_url',c.library_url,'completed_version',c.completed_version,'created_at',c.created_at,'updated_at',c.updated_at) FROM fvoci.zotero_connectors c WHERE c.workspace_id=$3 AND c.owner_user_id=$4 AND c.id=ANY($5) ORDER BY c.id LIMIT 10001", &scope).await?;
    let zotero_references = rows(&mut tx, "SELECT jsonb_build_object('id',r.id,'connector_id',r.connector_id,'document_id',r.document_id,'item_key',r.item_key,'remote_version',r.remote_version,'local_version',r.local_version,'bibliography',r.bibliography,'return_url',r.return_url,'availability',r.availability) FROM fvoci.zotero_references r WHERE r.workspace_id=$3 AND r.owner_user_id=$4 AND r.connector_id=ANY($5) ORDER BY r.connector_id,r.item_key LIMIT 10001", &scope).await?;
    let zotero_collections = rows(&mut tx, "SELECT jsonb_build_object('connector_id',c.connector_id,'collection_key',c.collection_key,'remote_version',c.remote_version,'name',c.name,'parent_key',c.parent_key,'availability',c.availability) FROM fvoci.zotero_collections c WHERE c.workspace_id=$3 AND c.owner_user_id=$4 AND c.connector_id=ANY($5) ORDER BY c.connector_id,c.collection_key LIMIT 10001", &scope).await?;
    let zotero_memberships = rows(&mut tx, "SELECT jsonb_build_object('connector_id',m.connector_id,'reference_id',m.reference_id,'collection_key',m.collection_key) FROM fvoci.zotero_memberships m WHERE m.workspace_id=$3 AND m.owner_user_id=$4 AND m.connector_id=ANY($5) ORDER BY m.connector_id,m.reference_id,m.collection_key LIMIT 10001", &scope).await?;
    let zotero_links = rows(&mut tx, "SELECT jsonb_build_object('id',l.id,'connector_id',l.connector_id,'reference_id',l.reference_id,'document_id',l.document_id,'task_id',l.task_id,'anchor',l.anchor) FROM fvoci.zotero_links l WHERE l.workspace_id=$3 AND l.owner_user_id=$4 AND l.connector_id=ANY($5) ORDER BY l.id LIMIT 10001", &scope).await?;
    // Capture receipts touching the selection, all targets inside it
    // (reject_unsupported refused anything else above).
    let personal_input_commands = rows(&mut tx, "SELECT jsonb_build_object('request_id',c.request_id,'request_hash',c.request_hash,'intent',c.intent,'document_id',c.document_id,'task_id',c.task_id,'project_id',c.project_id,'created_at',c.created_at) FROM fvoci.personal_input_commands c WHERE c.project_id=$1 OR c.document_id IN {DOCS} OR c.task_id IN(SELECT id FROM fvoci.tasks WHERE project_id=$1) ORDER BY c.created_at,c.request_id LIMIT 10001", &scope).await?;
    // Task time of the selected tasks: 034 entries (tenant-shared; another
    // author's entry is refused by validation) and the 048 actor-self rows,
    // whose RLS shows only the source actor's own stopwatch state.
    let time_entries = rows(&mut tx, "SELECT to_jsonb(e)-'workspace_id' FROM fvoci.time_entries e WHERE e.task_id IN(SELECT id FROM fvoci.tasks WHERE project_id=$1) ORDER BY e.started_at,e.id LIMIT 10001", &scope).await?;
    let timer_runs = rows(&mut tx, "SELECT to_jsonb(r)-'workspace_id' FROM fvoci.task_timer_runs r WHERE r.task_id IN(SELECT id FROM fvoci.tasks WHERE project_id=$1) ORDER BY r.started_at,r.id LIMIT 10001", &scope).await?;
    let timer_segments = rows(&mut tx, "SELECT to_jsonb(s)-'workspace_id' FROM fvoci.task_timer_segments s WHERE s.task_id IN(SELECT id FROM fvoci.tasks WHERE project_id=$1) ORDER BY s.started_at,s.id LIMIT 10001", &scope).await?;
    let timer_legacy_open = rows(&mut tx, "SELECT to_jsonb(l)-'workspace_id' FROM fvoci.task_timer_legacy_open l WHERE l.task_id IN(SELECT id FROM fvoci.tasks WHERE project_id=$1) ORDER BY l.time_entry_id LIMIT 10001", &scope).await?;
    // Receipts of the selected runs or of audited operations on the selected
    // tasks or their entries (a legacy release audits only the entry: task
    // and workspace NULL, receipt run NULL); the 052 restore marker is
    // destination provenance, not captured.
    let timer_commands = rows(&mut tx, "SELECT jsonb_build_object('request_id',c.request_id,'user_id',c.user_id,'request_hash',c.request_hash,'run_id',c.run_id,'result',c.result,'created_at',c.created_at) FROM fvoci.task_timer_commands c WHERE c.user_id=$4 AND (c.run_id IN(SELECT id FROM fvoci.task_timer_runs WHERE task_id IN(SELECT id FROM fvoci.tasks WHERE project_id=$1)) OR EXISTS(SELECT 1 FROM fvoci.task_timer_audit a WHERE a.user_id=c.user_id AND a.request_id=c.request_id AND (a.task_id IN(SELECT id FROM fvoci.tasks WHERE project_id=$1) OR a.time_entry_id IN(SELECT id FROM fvoci.time_entries WHERE task_id IN(SELECT id FROM fvoci.tasks WHERE project_id=$1))))) ORDER BY c.created_at,c.request_id LIMIT 10001", &scope).await?;
    // Audit of the selected tasks or their entries, plus the actor's audit of
    // a receipt on a selected run whose locators are all NULL (self cleanup).
    let timer_audit = rows(&mut tx, "SELECT to_jsonb(a) FROM fvoci.task_timer_audit a WHERE a.task_id IN(SELECT id FROM fvoci.tasks WHERE project_id=$1) OR a.time_entry_id IN(SELECT id FROM fvoci.time_entries WHERE task_id IN(SELECT id FROM fvoci.tasks WHERE project_id=$1)) OR (a.user_id=$4 AND EXISTS(SELECT 1 FROM fvoci.task_timer_commands c WHERE c.user_id=a.user_id AND c.request_id=a.request_id AND c.run_id IN(SELECT id FROM fvoci.task_timer_runs WHERE task_id IN(SELECT id FROM fvoci.tasks WHERE project_id=$1)))) ORDER BY a.created_at,a.id LIMIT 10001", &scope).await?;
    let captured_at: chrono::DateTime<chrono::Utc> =
        sqlx::query_scalar("SELECT transaction_timestamp()")
            .fetch_one(&mut *tx)
            .await?;
    tx.commit().await?;
    Ok(Capture {
        archive: Archive {
            graph: Graph {
                source_workspace_id: workspace,
                source_actor_id: actor,
                captured_at: captured_at.to_rfc3339(),
                project: project_row,
                workflows,
                statuses,
                documents,
                tasks,
                assignees,
                labels,
                task_labels,
                milestones,
                dependencies,
                views,
                document_tags,
                document_tag_assignments,
                origins,
                activity,
                purged_label_refs,
                purged_milestone_refs,
                comments,
                states,
                revisions,
                attachments,
                collections,
                collection_items,
                collection_fields,
                collection_options,
                collection_values,
                collection_choices,
                collection_people,
                collection_views,
                zotero_connectors,
                zotero_references,
                zotero_collections,
                zotero_memberships,
                zotero_links,
                personal_input_commands,
                time_entries,
                timer_runs,
                timer_segments,
                timer_legacy_open,
                timer_commands,
                timer_audit,
                native_inventory: None,
            },
            entries,
        },
        file_keys,
    })
}

/// Required personal wiki documents outside the project: backing documents
/// of the selected connectors' references and origin documents of the
/// selected project's tasks (personal-input captures), plus their wiki
/// ancestors. A dependency the selection cannot carry is refused before any
/// content read, never silently omitted.
async fn wiki_closure(
    tx: &mut Transaction<'_, Postgres>,
    scope: &Scope,
) -> Result<Vec<Uuid>, NativeDbError> {
    let zotero = || NativeDbError::from(ArchiveError::Unsupported("zotero closure".into()));
    let outside = || NativeDbError::from(ArchiveError::Unsupported("wiki closure".into()));
    let backing: Vec<(Uuid, Option<Uuid>, String)> = sqlx::query_as(
        "SELECT r.document_id,d.project_id,d.path FROM fvoci.zotero_references r JOIN fvoci.documents d ON d.workspace_id=r.workspace_id AND d.id=r.document_id
         WHERE r.workspace_id=$1 AND r.owner_user_id=$2 AND r.connector_id=ANY($3) ORDER BY r.document_id LIMIT 10001",
    )
    .bind(scope.workspace)
    .bind(scope.actor)
    .bind(&scope.connectors)
    .fetch_all(&mut **tx)
    .await?;
    let origins: Vec<(Uuid, Option<Uuid>, String)> = sqlx::query_as(
        "SELECT d.id,d.project_id,d.path FROM fvoci.task_origins o JOIN fvoci.tasks t ON t.id=o.task_id JOIN fvoci.documents d ON d.id=o.document_id
         WHERE t.project_id=$1 AND d.project_id IS DISTINCT FROM $1 ORDER BY d.id LIMIT 10001",
    )
    .bind(scope.project)
    .fetch_all(&mut **tx)
    .await?;
    if backing.len() > MAX_OBJECTS || origins.len() > MAX_OBJECTS {
        return Err(ArchiveError::Limit.into());
    }
    let mut closure = std::collections::BTreeSet::new();
    for (rows, refused) in [(&backing, zotero()), (&origins, outside())] {
        for (_, project, path) in rows {
            if project.is_some() {
                return Err(refused);
            }
            for label in path.split('.') {
                closure.insert(Uuid::try_parse(label).map_err(|_| outside())?);
            }
        }
    }
    let closure: Vec<Uuid> = closure.into_iter().collect();
    if closure.len() > MAX_OBJECTS {
        return Err(ArchiveError::Limit.into());
    }
    if closure.is_empty() {
        return Ok(closure);
    }
    // Ancestors are wiki documents too; an omitted wiki child, or a reference
    // of an unselected connector backed inside the closure, is refused in
    // this limited model.
    let (wiki, children, foreign_refs): (i64, bool, bool) = sqlx::query_as(
        "SELECT (SELECT count(*) FROM fvoci.documents WHERE id=ANY($1) AND project_id IS NULL),
                EXISTS(SELECT 1 FROM fvoci.documents WHERE parent_id=ANY($1) AND NOT id=ANY($1)),
                EXISTS(SELECT 1 FROM fvoci.zotero_references WHERE workspace_id=$2 AND document_id=ANY($1) AND NOT connector_id=ANY($3))",
    )
    .bind(&closure)
    .bind(scope.workspace)
    .bind(&scope.connectors)
    .fetch_one(&mut **tx)
    .await?;
    if foreign_refs {
        return Err(zotero());
    }
    if wiki != closure.len() as i64 || children {
        return Err(outside());
    }
    Ok(closure)
}

async fn capture_payload_budget(
    tx: &mut Transaction<'_, Postgres>,
    scope: &Scope,
) -> Result<(), NativeDbError> {
    // Read no bytea until the combined native/history/file payload fits. The
    // database evaluates this against the same REPEATABLE READ snapshot.
    let bytes: i64 = sqlx::query_scalar(&format!(
        "SELECT coalesce(sum(n),0)::bigint FROM (
            SELECT octet_length(s.state)::bigint AS n FROM fvoci.document_states s WHERE s.document_id IN {DOCS}
            UNION ALL SELECT octet_length(s.state)::bigint FROM fvoci.task_states s JOIN fvoci.tasks t ON t.id=s.task_id WHERE t.project_id=$1
            UNION ALL SELECT octet_length(u.payload)::bigint FROM fvoci.document_collab_updates u JOIN fvoci.document_states s ON s.document_id=u.document_id WHERE u.document_id IN {DOCS} AND u.seq>s.snapshot_cutoff_seq AND u.seq<=s.tail_seq
            UNION ALL SELECT octet_length(u.payload)::bigint FROM fvoci.task_collab_updates u JOIN fvoci.task_states s ON s.task_id=u.task_id JOIN fvoci.tasks t ON t.id=u.task_id WHERE t.project_id=$1 AND u.seq>s.snapshot_cutoff_seq AND u.seq<=s.tail_seq
            UNION ALL SELECT octet_length(r.y_snapshot)::bigint FROM fvoci.revisions r WHERE (r.target_kind='document' AND r.target_id IN {DOCS}) OR (r.target_kind='task' AND r.target_id IN(SELECT id FROM fvoci.tasks WHERE project_id=$1))
            UNION ALL SELECT a.size_bytes FROM fvoci.attachments a WHERE a.document_id IN {DOCS} OR a.task_id IN(SELECT id FROM fvoci.tasks WHERE project_id=$1)
        ) AS payload"
    ))
    .bind(scope.project)
    .bind(&scope.wiki)
    .fetch_one(&mut **tx)
    .await?;
    if bytes < 0 || bytes > MAX_BYTES as i64 {
        return Err(ArchiveError::Limit.into());
    }
    Ok(())
}

async fn reject_unsupported(
    tx: &mut Transaction<'_, Postgres>,
    scope: &Scope,
) -> Result<(), NativeDbError> {
    // Existing dependencies are checked, even when the UI did not select them.
    // These are explicit initial-adapter diagnostics, not final W7 exclusions.
    let tests = [
        // Native history without its state row cannot be carried coherently.
        ("missing native state for captured body", "SELECT EXISTS(SELECT 1 FROM fvoci.document_collab_updates u WHERE u.document_id IN {DOCS} AND NOT EXISTS(SELECT 1 FROM fvoci.document_states s WHERE s.document_id=u.document_id))
            OR EXISTS(SELECT 1 FROM fvoci.document_collab_op_receipts r WHERE r.document_id IN {DOCS} AND NOT EXISTS(SELECT 1 FROM fvoci.document_states s WHERE s.document_id=r.document_id))
            OR EXISTS(SELECT 1 FROM fvoci.task_collab_updates u WHERE u.task_id IN(SELECT id FROM fvoci.tasks WHERE project_id=$1) AND NOT EXISTS(SELECT 1 FROM fvoci.task_states s WHERE s.task_id=u.task_id))
            OR EXISTS(SELECT 1 FROM fvoci.task_collab_op_receipts r WHERE r.task_id IN(SELECT id FROM fvoci.tasks WHERE project_id=$1) AND NOT EXISTS(SELECT 1 FROM fvoci.task_states s WHERE s.task_id=r.task_id))
            OR EXISTS(SELECT 1 FROM fvoci.revisions v WHERE ((v.target_kind='document' AND v.target_id IN {DOCS} AND NOT EXISTS(SELECT 1 FROM fvoci.document_states s WHERE s.document_id=v.target_id))
                OR (v.target_kind='task' AND v.target_id IN(SELECT id FROM fvoci.tasks WHERE project_id=$1) AND NOT EXISTS(SELECT 1 FROM fvoci.task_states s WHERE s.task_id=v.target_id))))"),
        // Migration028 triggers give every project one task collection and every
        // task one item of it; person-made fields/values/views of the
        // project's collections are typed records, and so are those of a
        // live workspace wiki document collection whose every item is a
        // wiki-closure document (the item writer keeps wiki collections to
        // wiki documents). Still refused here: a deleted or missing/extra task
        // collection, an item whose target is outside the project, or any
        // other collection outside the project holding an archived document
        // or task (a deleted wiki collection, or one also holding a document
        // outside the closure - never pruned).
        ("collections", "SELECT (SELECT count(*) FROM fvoci.collections WHERE project_id=$1 AND kind='task') <> 1
            OR EXISTS(SELECT 1 FROM fvoci.collections c WHERE c.project_id=$1 AND c.deleted_at IS NOT NULL)
            OR EXISTS(SELECT 1 FROM fvoci.collection_items i JOIN fvoci.collections c ON c.id=i.collection_id WHERE c.project_id=$1 AND NOT (
                (i.task_id IS NOT NULL AND EXISTS(SELECT 1 FROM fvoci.tasks t WHERE t.id=i.task_id AND t.project_id=$1))
                OR (i.document_id IS NOT NULL AND i.document_id IN {DOCS} AND EXISTS(SELECT 1 FROM fvoci.documents d WHERE d.id=i.document_id AND d.project_id=$1))))
            OR (SELECT count(*) FROM fvoci.collection_items i JOIN fvoci.collections c ON c.id=i.collection_id WHERE c.project_id=$1 AND i.task_id IS NOT NULL) <> (SELECT count(*) FROM fvoci.tasks WHERE project_id=$1)
            OR EXISTS(SELECT 1 FROM fvoci.collection_items i JOIN fvoci.collections c ON c.id=i.collection_id WHERE (c.project_id IS DISTINCT FROM $1) AND (i.document_id IN {DOCS} OR i.task_id IN(SELECT id FROM fvoci.tasks WHERE project_id=$1))
                AND NOT (c.project_id IS NULL AND c.kind='document' AND c.deleted_at IS NULL
                    AND NOT EXISTS(SELECT 1 FROM fvoci.collection_items j WHERE j.collection_id=c.id AND (j.document_id IS NULL OR NOT j.document_id=ANY($2)))))"),
        // Receipts touching the selection travel retired; one by another actor
        // or with any target outside the selection cannot.
        ("personal input commands", "SELECT EXISTS(SELECT 1 FROM fvoci.personal_input_commands WHERE (project_id=$1 OR document_id IN {DOCS} OR task_id IN(SELECT id FROM fvoci.tasks WHERE project_id=$1))
            AND (actor_user_id<>$4 OR (project_id IS NOT NULL AND project_id<>$1) OR (document_id IS NOT NULL AND document_id NOT IN {DOCS}) OR (task_id IS NOT NULL AND task_id NOT IN(SELECT id FROM fvoci.tasks WHERE project_id=$1))))"),
        ("stars", "SELECT EXISTS(SELECT 1 FROM fvoci.stars WHERE document_id IN {DOCS} OR task_id IN(SELECT id FROM fvoci.tasks WHERE project_id=$1))"),
        // A link of an unselected connector into the selected documents/tasks
        // cannot travel without its connector (RLS: the actor's own rows).
        ("zotero links", "SELECT EXISTS(SELECT 1 FROM fvoci.zotero_links WHERE workspace_id=$3 AND owner_user_id=$4 AND NOT connector_id=ANY($5) AND (document_id IN {DOCS} OR task_id IN(SELECT id FROM fvoci.tasks WHERE project_id=$1)))"),
        ("external issue links", "SELECT EXISTS(SELECT 1 FROM fvoci.github_issue_links WHERE task_id IN(SELECT id FROM fvoci.tasks WHERE project_id=$1))"),
    ];
    for (kind, sql) in tests {
        if sqlx::query_scalar::<_, bool>(&sql.replace("{DOCS}", DOCS))
            .bind(scope.project)
            .bind(&scope.wiki)
            .bind(scope.workspace)
            .bind(scope.actor)
            .bind(&scope.connectors)
            .fetch_one(&mut **tx)
            .await?
        {
            return Err(ArchiveError::Unsupported(kind.into()).into());
        }
    }
    let unsupported_grant: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM fvoci.project_members WHERE project_id=$1 AND (user_id IS DISTINCT FROM $3 OR group_id IS NOT NULL)) OR EXISTS(SELECT 1 FROM fvoci.document_members WHERE document_id IN {DOCS})".replace("{DOCS}", DOCS).as_str())
        .bind(scope.project).bind(&scope.wiki).bind(scope.actor).fetch_one(&mut **tx).await?;
    if unsupported_grant {
        return Err(ArchiveError::Unsupported("multiple authors or grants".into()).into());
    }
    Ok(())
}

async fn destination_gate(
    tx: &mut Transaction<'_, Postgres>,
    workspace: Uuid,
    actor: Uuid,
    session: Uuid,
    locked: bool,
    empty: bool,
) -> Result<(), NativeDbError> {
    let live = if locked {
        lock_membership_users(tx, &[actor]).await?;
        recheck_session(tx, actor, session).await?
    } else {
        session_is_live(tx, actor, session).await?
    };
    if !live || membership_role(tx, workspace, actor).await? != Some(WorkspaceRole::Owner) {
        return Err(NativeDbError::Forbidden);
    }
    let owned: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM fvoci.workspaces w JOIN fvoci.users u ON u.personal_workspace_id=w.id WHERE w.id=$1 AND w.kind='personal' AND w.deleted_at IS NULL AND u.id=$2)")
        .bind(workspace).bind(actor).fetch_one(&mut **tx).await?;
    if !owned {
        return Err(NativeDbError::Forbidden);
    }
    if empty {
        let nonempty: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM fvoci.projects) OR EXISTS(SELECT 1 FROM fvoci.documents) OR EXISTS(SELECT 1 FROM fvoci.tasks) OR EXISTS(SELECT 1 FROM fvoci.attachments)")
            .fetch_one(&mut **tx).await?;
        if nonempty {
            return Err(NativeDbError::Conflict);
        }
    }
    Ok(())
}

pub async fn preflight_destination(
    pool: &PgPool,
    workspace: Uuid,
    actor: Uuid,
    session: Uuid,
) -> Result<(), NativeDbError> {
    let mut tx = begin_read(pool).await?;
    set_tenant(&mut tx, workspace).await?;
    destination_gate(&mut tx, workspace, actor, session, false, true).await?;
    tx.commit().await?;
    Ok(())
}
pub async fn authorize_destination(
    pool: &PgPool,
    workspace: Uuid,
    actor: Uuid,
    session: Uuid,
) -> Result<(), NativeDbError> {
    let mut tx = begin_read(pool).await?;
    set_tenant(&mut tx, workspace).await?;
    destination_gate(&mut tx, workspace, actor, session, false, false).await?;
    tx.commit().await?;
    Ok(())
}

pub async fn status(
    pool: &PgPool,
    workspace: Uuid,
    actor: Uuid,
    session: Uuid,
    id: Uuid,
) -> Result<crate::api::native_archive::NativeJobOutput, NativeDbError> {
    let mut tx = begin_read(pool).await?;
    set_tenant(&mut tx, workspace).await?;
    destination_gate(&mut tx, workspace, actor, session, false, false).await?;
    let row=sqlx::query("SELECT id,status,native_archive_hash,native_result,native_diagnostic FROM fvoci.import_jobs WHERE workspace_id=$1 AND id=$2 AND created_by=$3 AND source='native-archive'")
        .bind(workspace).bind(id).bind(actor).fetch_optional(&mut *tx).await?.ok_or(NativeDbError::Forbidden)?;
    let result: Option<Value> = row.get("native_result");
    let project_id = result
        .as_ref()
        .and_then(|v| v["projectId"].as_str())
        .and_then(|s| Uuid::parse_str(s).ok());
    if let Some(project) = project_id {
        if !crate::db::projects::project_permission_by_id(&mut tx, workspace, actor, project)
            .await?
            .is_some_and(|p| p.at_least(ProjectPermission::View))
        {
            return Err(NativeDbError::Forbidden);
        }
    }
    let output = crate::api::native_archive::NativeJobOutput {
        id: row.get("id"),
        status: row.get("status"),
        archive_hash: row.get("native_archive_hash"),
        project_id,
        diagnostic: row.get("native_diagnostic"),
    };
    tx.commit().await?;
    Ok(output)
}

pub async fn fail_native(
    pool: &PgPool,
    claim: &ImportClaim,
    diagnostic: &str,
) -> Result<bool, sqlx::Error> {
    let mut tx = pool.begin().await?;
    set_tenant(&mut tx, claim.workspace_id).await?;
    let changed=sqlx::query("UPDATE fvoci.import_jobs SET native_diagnostic=$4 WHERE workspace_id=$1 AND id=$2 AND status='running' AND lease_token=$3 AND lease_until>clock_timestamp()")
        .bind(claim.workspace_id).bind(claim.job_id).bind(claim.lease_token).bind(diagnostic).execute(&mut *tx).await?;
    if changed.rows_affected() != 1 {
        tx.rollback().await?;
        return Ok(false);
    }
    let finished = crate::db::import_jobs::finish_import_job_in_tx(
        &mut tx,
        claim,
        crate::db::import_jobs::ImportStatus::Failed,
    )
    .await?;
    if finished {
        tx.commit().await?;
    } else {
        tx.rollback().await?;
    }
    Ok(finished)
}

pub async fn recheck_file(
    pool: &PgPool,
    workspace: Uuid,
    actor: Uuid,
    session: Uuid,
    graph: &Graph,
    file: &Attachment,
    key: &str,
) -> Result<(), NativeDbError> {
    let project = graph.project.id;
    let wiki = graph_wiki(graph);
    let connectors = graph_connectors(graph);
    let mut tx = begin_read(pool).await?;
    set_tenant(&mut tx, workspace).await?;
    source_gate(
        &mut tx,
        workspace,
        actor,
        session,
        &Selection {
            project,
            zotero_connectors: &connectors,
        },
    )
    .await?;
    let valid:bool=sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM fvoci.attachments a LEFT JOIN fvoci.documents d ON d.id=a.document_id LEFT JOIN fvoci.tasks t ON t.id=a.task_id WHERE a.id=$1 AND a.status='stored' AND a.scan_status IN ('skipped','clean') AND a.storage_key=$2 AND a.size_bytes=$3 AND (((d.project_id=$4 OR (d.project_id IS NULL AND d.id=ANY($5))) AND d.deleted_at IS NULL) OR (t.project_id=$4 AND t.deleted_at IS NULL)))")
        .bind(file.id).bind(key).bind(file.size_bytes).bind(project).bind(&wiki).fetch_one(&mut *tx).await?;
    if !valid {
        return Err(NativeDbError::Forbidden);
    }
    tx.commit().await?;
    Ok(())
}

/// Repeated commands recover the same durable row, including a lost response
/// after completion. Hash/actor/target mismatch never creates a replacement.
pub async fn queue_restore(
    pool: &PgPool,
    workspace: Uuid,
    actor: Uuid,
    session: Uuid,
    request: Uuid,
    hash: &str,
    payload: &[u8],
) -> Result<Uuid, NativeDbError> {
    if payload.is_empty() || payload.len() > MAX_BYTES || digest(payload) != hash {
        return Err(ArchiveError::Invalid("confirmation hash".into()).into());
    }
    let mut tx = pool.begin().await?;
    set_tenant(&mut tx, workspace).await?;
    destination_gate(&mut tx, workspace, actor, session, true, false).await?;
    lock_tree(&mut tx, workspace).await?;
    let existing = sqlx::query("SELECT id,native_archive_hash FROM fvoci.import_jobs WHERE workspace_id=$1 AND created_by=$2 AND native_request_id=$3 AND source='native-archive'")
        .bind(workspace).bind(actor).bind(request).fetch_optional(&mut *tx).await?;
    if let Some(row) = existing {
        if row.get::<String, _>("native_archive_hash") != hash {
            return Err(NativeDbError::Conflict);
        }
        let id = row.get("id");
        tx.commit().await?;
        return Ok(id);
    }
    destination_gate(&mut tx, workspace, actor, session, false, true).await?;
    let id = Uuid::now_v7();
    sqlx::query("INSERT INTO fvoci.import_jobs(id,workspace_id,created_by,session_id,source,status,payload,native_request_id,native_archive_hash) VALUES($1,$2,$3,$4,'native-archive','running',$5,$6,$7)")
        .bind(id).bind(workspace).bind(actor).bind(session).bind(payload).bind(request).bind(hash).execute(&mut *tx).await?;
    tx.commit().await?;
    Ok(id)
}

/// Journal before storage put. The orphan GC cannot reclaim a live staged key
/// until its job lease window has elapsed; publication removes this exact key.
pub async fn stage_key(
    pool: &PgPool,
    claim: &ImportClaim,
    attachment: Uuid,
    key: &str,
) -> Result<(), NativeDbError> {
    let mut tx = pool.begin().await?;
    set_tenant(&mut tx, claim.workspace_id).await?;
    destination_gate(
        &mut tx,
        claim.workspace_id,
        claim.created_by,
        claim.session_id,
        true,
        true,
    )
    .await?;
    let fence = crate::db::documents::ImportFence {
        job_id: claim.job_id,
        lease_token: claim.lease_token,
    };
    if !hold_import_fence(&mut tx, claim.workspace_id, fence).await? {
        return Err(NativeDbError::Fenced);
    }
    if !crate::db::import_jobs::append_import_ref(
        &mut tx,
        claim.workspace_id,
        claim.job_id,
        claim.lease_token,
        crate::db::import_jobs::ImportRefKind::StoredKey,
        key,
    )
    .await?
    {
        return Err(NativeDbError::Fenced);
    }
    sqlx::query("INSERT INTO fvoci.attachment_object_cleanups(workspace_id,attachment_id,storage_key,due_at) VALUES($1,$2,$3,clock_timestamp()+interval '20 minutes')")
        .bind(claim.workspace_id).bind(attachment).bind(key).execute(&mut *tx).await?;
    tx.commit().await?;
    Ok(())
}

fn mapped<T: Serialize>(record: &T, workspace: Uuid, actor: Uuid) -> Result<Value, NativeDbError> {
    let mut value = serde_json::to_value(record)
        .map_err(|_| ArchiveError::Invalid("record serialization".into()))?;
    value["workspace_id"] = json!(workspace);
    for field in ["created_by", "uploader_id", "actor_user_id", "user_id"] {
        if value.get(field).is_some_and(|v| !v.is_null()) {
            value[field] = json!(actor);
        }
    }
    Ok(value)
}

// Only concrete typed records call this internal helper. SQL table/column
// names come from this module, never the uploaded graph or request.
async fn insert_record(
    tx: &mut Transaction<'_, Postgres>,
    table: &str,
    value: Value,
) -> Result<(), NativeDbError> {
    let fields = value
        .as_object()
        .ok_or_else(|| ArchiveError::Invalid("record".into()))?;
    let columns = fields
        .keys()
        .map(|key| format!("\"{key}\""))
        .collect::<Vec<_>>()
        .join(",");
    sqlx::query(&format!("INSERT INTO fvoci.{table} ({columns}) SELECT {columns} FROM jsonb_populate_record(NULL::fvoci.{table},$1::jsonb)"))
        .bind(value).execute(&mut **tx).await?;
    Ok(())
}

/// The retained-provenance guard of `publish_task_time` (public for the
/// cost measurement test): $1 actor, $2 run ids, $3 task ids, $4 entry ids,
/// $5 entry id texts, $6 segment id texts, $7 run id texts.
pub const RETAINED_PROVENANCE_SQL: &str = "SELECT EXISTS(SELECT 1 FROM fvoci.task_timer_commands WHERE user_id=$1 AND run_id=ANY($2))
    OR EXISTS(SELECT 1 FROM fvoci.task_timer_audit a WHERE a.user_id=$1 AND (a.task_id=ANY($3) OR a.time_entry_id=ANY($4)
        OR EXISTS(SELECT 1 FROM (VALUES (a.before_value),(a.after_value)) v(x) WHERE
            (x->>'kind'='manual' AND x->>'recordId'=ANY($5))
            OR (x->>'kind'='segment' AND x->>'recordId'=ANY($6))
            OR x->>'runId'=ANY($7))))";

/// Task time lands under the destination actor with every identity kept.
/// The person-global one-unfinished-stopwatch rule is checked before any
/// time row; the 034 trigger re-creates open reservations, so an archive
/// without a reservation for an open entry (an explicit release) removes the
/// re-created one. Receipts land retired (052 marker = this restore job).
async fn guard_task_time_pg(
    tx: &mut Transaction<'_, Postgres>,
    g: &Graph,
    actor: Uuid,
) -> Result<(), NativeDbError> {
    set_self_user(tx, actor).await?;
    // ABA guard: a restored identity (task, entry, segment, run) that this
    // actor's retained receipts or audit already name - e.g. after a purge -
    // would let old history replay or re-attach to a recreated row, even when
    // the archive omits those receipts/audits. Refused before any time row.
    // Audit record references are typed by their recorded kind: a "manual"
    // recordId names an entry, a "segment" recordId a segment (ids are only
    // unique per table).
    let tasks: Vec<Uuid> = g.tasks.iter().map(|t| t.id).collect();
    let entries: Vec<Uuid> = g.time_entries.iter().map(|e| e.id).collect();
    let runs: Vec<Uuid> = g.timer_runs.iter().map(|r| r.id).collect();
    let texts = |ids: &[Uuid]| ids.iter().map(Uuid::to_string).collect::<Vec<_>>();
    let segments: Vec<Uuid> = g.timer_segments.iter().map(|s| s.id).collect();
    let retained: bool = sqlx::query_scalar(RETAINED_PROVENANCE_SQL)
        .bind(actor)
        .bind(&runs)
        .bind(&tasks)
        .bind(&entries)
        .bind(texts(&entries))
        .bind(texts(&segments))
        .bind(texts(&runs))
        .fetch_one(&mut **tx)
        .await?;
    if retained {
        return Err(NativeDbError::Conflict);
    }
    if g.time_entries.is_empty()
        && g.timer_runs.is_empty()
        && g.timer_commands.is_empty()
        && g.timer_audit.is_empty()
    {
        return Ok(());
    }
    let unfinished = g.timer_runs.iter().any(|r| r.status != "stopped")
        || g.time_entries.iter().any(|e| e.ended_at.is_none());
    if unfinished {
        // Same actor row lock order as the 048 trigger.
        sqlx::query("SELECT id FROM fvoci.users WHERE id=$1 FOR UPDATE")
            .bind(actor)
            .execute(&mut **tx)
            .await?;
        let busy: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM fvoci.task_timer_runs WHERE user_id=$1 AND status<>'stopped') OR EXISTS(SELECT 1 FROM fvoci.task_timer_legacy_open WHERE user_id=$1)")
            .bind(actor)
            .fetch_one(&mut **tx)
            .await?;
        if busy {
            return Err(NativeDbError::Conflict);
        }
    }
    Ok(())
}

/// No overwrite/skip/reseed path. Global PK conflicts hidden by RLS are caught
/// by ordinary INSERT constraints and returned generically by the caller.
pub async fn publish(
    pool: &PgPool,
    claim: &ImportClaim,
    archive: &Archive,
    keys: &BTreeMap<Uuid, String>,
    quota: &crate::db::quota::StorageQuota,
) -> Result<(), NativeDbError> {
    let mut tx = pool.begin().await?;
    let result = publish_pg_in_tx(
        &mut tx,
        claim,
        archive,
        keys,
        quota,
        None,
        &CancellationToken::new(),
    )
    .await;
    match result {
        Ok(()) => {
            tx.commit().await?;
            Ok(())
        }
        Err(error) => {
            let rollback = tx.rollback().await;
            #[cfg(all(test, feature = "db-tests"))]
            let rollback = selected_tests::after_acknowledged_pg_rollback(claim.job_id, rollback);
            match rollback {
                Ok(()) => Err(error),
                Err(cleanup) => Err(crate::db::backend::rollback_cleanup_unknown(
                    Some(Box::new(error)),
                    cleanup,
                )
                .into()),
            }
        }
    }
}

async fn publish_pg_in_tx(
    tx: &mut Transaction<'_, Postgres>,
    claim: &ImportClaim,
    archive: &Archive,
    keys: &BTreeMap<Uuid, String>,
    quota: &crate::db::quota::StorageQuota,
    expected_archive_hash: Option<&str>,
    cancel: &CancellationToken,
) -> Result<(), NativeDbError> {
    archive.validate()?;
    native_cancel(cancel)?;
    set_tenant(tx, claim.workspace_id).await?;
    crate::db::quota::acquire_admission_lock(tx).await?;
    destination_gate(
        tx,
        claim.workspace_id,
        claim.created_by,
        claim.session_id,
        true,
        true,
    )
    .await?;
    lock_tree(tx, claim.workspace_id).await?;
    crate::db::attachments::lock_workspace_storage(tx, claim.workspace_id).await?;
    // Tree/storage locks precede the job row fence, preserving the existing
    // document/task/attachment operation ordering.
    destination_gate(
        tx,
        claim.workspace_id,
        claim.created_by,
        claim.session_id,
        false,
        true,
    )
    .await?;
    let fence = crate::db::documents::ImportFence {
        job_id: claim.job_id,
        lease_token: claim.lease_token,
    };
    if !hold_import_fence(tx, claim.workspace_id, fence).await? {
        return Err(NativeDbError::Fenced);
    }
    let stored_hash = OperationTx::Postgres(tx).native_claim(claim).await?;
    if expected_archive_hash.is_some_and(|expected| expected != stored_hash) {
        return Err(NativeDbError::Conflict);
    }
    OperationTx::Postgres(tx)
        .native_keys(claim, archive, keys)
        .await?;
    native_cancel(cancel)?;
    let g = &archive.graph;
    let workspace = claim.workspace_id;
    let actor = claim.created_by;
    let mut total = 0i64;
    for file in &g.attachments {
        quota
            .check(total, file.size_bytes)
            .map_err(|_| ArchiveError::Limit)?;
        total = total
            .checked_add(file.size_bytes)
            .ok_or(ArchiveError::Limit)?;
    }
    set_self_user(tx, actor).await?;
    guard_task_time_pg(tx, g, actor).await?;
    let records = prepare_native_records(archive, keys, workspace, actor, claim.job_id)?;
    for (table, value) in records {
        native_cancel(cancel)?;
        insert_native_pg_record(tx, table, value, g, workspace, actor).await?;
    }
    OperationTx::Postgres(tx)
        .install_archived_native_history(claim, archive)
        .await?;
    sqlx::query("UPDATE fvoci.projects SET root_document_id=$3 WHERE workspace_id=$1 AND id=$2")
        .bind(workspace)
        .bind(g.project.id)
        .bind(g.project.root_document_id)
        .execute(&mut **tx)
        .await?;
    sqlx::query("UPDATE fvoci.workspaces SET next_document_number=GREATEST(next_document_number,$2) WHERE id=$1")
        .bind(workspace).bind(g.documents.iter().map(|d| d.number).max().unwrap_or(0).checked_add(1).ok_or(ArchiveError::Limit)?).execute(&mut **tx).await?;
    OperationTx::Postgres(tx)
        .native_keys(claim, archive, keys)
        .await?;
    for row in &g.attachments {
        let key = keys.get(&row.id).ok_or(NativeDbError::Fenced)?;
        if sqlx::query("DELETE FROM fvoci.attachment_object_cleanups WHERE workspace_id=$1 AND attachment_id=$2 AND storage_key=$3")
            .bind(workspace).bind(row.id).bind(key).execute(&mut **tx).await?.rows_affected()!=1 {return Err(NativeDbError::Fenced);}
    }
    let result = native_result(g, claim, &stored_hash);
    sqlx::query("UPDATE fvoci.import_jobs SET native_result=$3,created_refs='{\"documentIds\":[],\"taskIds\":[],\"storedKeys\":[]}'::jsonb WHERE workspace_id=$1 AND id=$2")
        .bind(workspace).bind(claim.job_id).bind(&result).execute(&mut **tx).await?;
    let mut op = OperationTx::Postgres(tx);
    append_native_events(&mut op, g, claim, result).await?;
    native_cancel(cancel)?;
    let mut op = OperationTx::Postgres(tx);
    op.native_destination(workspace, actor, claim.session_id, true, false)
        .await?;
    if !op.hold_import_claim(claim).await?
        || !op
            .finish_import_job(claim, crate::db::import_jobs::ImportStatus::Completed)
            .await?
    {
        return Err(NativeDbError::Fenced);
    }
    #[cfg(all(test, feature = "db-tests"))]
    selected_tests::before_finish(&mut op, claim, cancel).await?;
    native_cancel(cancel)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The view-filter precharge refuses at the remaining-budget boundary,
    /// before any filter is collected, and leaves the budget untouched.
    #[test]
    fn native_archive_view_filter_precharge_refuses_at_the_remaining_budget() {
        let scope = |used: usize| Scope {
            project: Uuid::nil(),
            wiki: Vec::new(),
            workspace: Uuid::nil(),
            actor: Uuid::nil(),
            connectors: Vec::new(),
            graph_bytes: std::sync::atomic::AtomicI64::new(used as i64),
        };
        let views: Vec<View> = serde_json::from_value(json!([{"id":Uuid::nil(),
            "project_id":Uuid::nil(),"user_id":Uuid::nil(),"name":"보기","type":"list",
            "config":{"filters":{"labelId":Uuid::nil().to_string(),
            "milestoneId":Uuid::from_u128(1).to_string()},"sort":[]},
            "created_at":"2026-10-03T00:00:00Z","updated_at":"2026-10-03T00:00:00Z"}]))
        .unwrap();
        let charge = 2 * HISTORY_REFERENCE_BYTES;
        let exact = scope(MAX_GRAPH_BYTES - charge);
        admit_history_references(&exact, &[], &views, &[]).unwrap();
        assert_eq!(
            exact.graph_bytes.load(std::sync::atomic::Ordering::Relaxed),
            MAX_GRAPH_BYTES as i64
        );
        let short = scope(MAX_GRAPH_BYTES - charge + 1);
        assert!(matches!(
            admit_history_references(&short, &[], &views, &[]),
            Err(NativeDbError::Archive(ArchiveError::Limit))
        ));
        assert_eq!(
            short.graph_bytes.load(std::sync::atomic::Ordering::Relaxed),
            (MAX_GRAPH_BYTES - charge + 1) as i64
        );
        // No views, no history: nothing is charged.
        let empty = scope(MAX_GRAPH_BYTES);
        admit_history_references(&empty, &[], &[], &[]).unwrap();
    }
}

// Selected native adapters retain the original PostgreSQL public entry points.
// A commit error keeps the original typed receipt; none opens an observer.
use crate::db::backend::{Backend, FamilyTx, OperationTx};
use crate::db::codec::Cell;
use tokio_util::sync::CancellationToken;

fn native_cancel(cancel: &CancellationToken) -> Result<(), NativeDbError> {
    if cancel.is_cancelled() {
        Err(ArchiveError::Cancelled.into())
    } else {
        Ok(())
    }
}
fn native_commit_error(error: crate::db::backend::CommitUnknown) -> NativeDbError {
    sqlx::Error::AnyDriverError(Box::new(error)).into()
}

impl OperationTx<'_, '_> {
    async fn native_destination(
        &mut self,
        workspace: Uuid,
        actor: Uuid,
        session: Uuid,
        locked: bool,
        empty: bool,
    ) -> Result<(), NativeDbError> {
        if let Self::Postgres(tx) = self {
            return destination_gate(tx, workspace, actor, session, locked, empty).await;
        }
        let live = if locked {
            self.lock_membership_users(&[actor]).await?;
            self.recheck_session(actor, session).await?
        } else {
            self.session_is_live(actor, session).await?
        };
        if !live
            || self.membership_role(workspace, actor, locked).await? != Some(WorkspaceRole::Owner)
        {
            return Err(NativeDbError::Forbidden);
        }
        let Self::SqliteFamily(tx) = self else {
            unreachable!()
        };
        tx.require_tenant(workspace)?;
        let owned=tx.query("SELECT EXISTS(SELECT 1 FROM workspaces w JOIN users u ON u.personal_workspace_id=w.id WHERE w.id=?1 AND w.kind='personal' AND w.deleted_at IS NULL AND u.id=?2)",&[Cell::uuid(workspace),Cell::uuid(actor)]).await?;
        if !owned[0].cell(0)?.boolean()? {
            return Err(NativeDbError::Forbidden);
        }
        if empty {
            let rows=tx.query("SELECT EXISTS(SELECT 1 FROM projects WHERE workspace_id=?1) OR EXISTS(SELECT 1 FROM documents WHERE workspace_id=?1) OR EXISTS(SELECT 1 FROM tasks WHERE workspace_id=?1) OR EXISTS(SELECT 1 FROM attachments WHERE workspace_id=?1)",&[Cell::uuid(workspace)]).await?;
            if rows[0].cell(0)?.boolean()? {
                return Err(NativeDbError::Conflict);
            }
        }
        Ok(())
    }
    async fn native_claim(&mut self, claim: &ImportClaim) -> Result<String, NativeDbError> {
        if claim.source != crate::db::import_jobs::ImportSource::NativeArchive
            || !self.hold_import_claim(claim).await?
        {
            return Err(NativeDbError::Fenced);
        }
        let (hash, payload) = match self {
            Self::Postgres(tx) => {
                let row=sqlx::query("SELECT native_archive_hash,payload FROM fvoci.import_jobs WHERE workspace_id=$1 AND id=$2").bind(claim.workspace_id).bind(claim.job_id).fetch_one(&mut ***tx).await?;
                (row.try_get::<String, _>(0)?, row.try_get::<Vec<u8>, _>(1)?)
            }
            Self::SqliteFamily(tx) => {
                let rows=tx.query("SELECT native_archive_hash,payload FROM import_jobs WHERE workspace_id=?1 AND id=?2",&[Cell::uuid(claim.workspace_id),Cell::uuid(claim.job_id)]).await?;
                let row = rows.first().ok_or(sqlx::Error::RowNotFound)?;
                (row.cell(0)?.string()?, row.cell(1)?.bytes()?)
            }
        };
        if payload.is_empty() || payload.len() > MAX_BYTES || digest(&payload) != hash {
            return Err(NativeDbError::Conflict);
        }
        Ok(hash)
    }
}

async fn destination_backend(
    backend: &Backend,
    workspace: Uuid,
    actor: Uuid,
    session: Uuid,
    empty: bool,
) -> Result<(), NativeDbError> {
    let mut tx = backend.begin_read().await?;
    let result = async {
        let mut op = tx.operation();
        op.set_tenant(workspace).await?;
        op.native_destination(workspace, actor, session, false, empty)
            .await
    }
    .await;
    finish_native_read(tx, result).await
}

pub async fn preflight_destination_backend(
    backend: &Backend,
    workspace: Uuid,
    actor: Uuid,
    session: Uuid,
) -> Result<(), NativeDbError> {
    destination_backend(backend, workspace, actor, session, true).await
}
pub async fn authorize_destination_backend(
    backend: &Backend,
    workspace: Uuid,
    actor: Uuid,
    session: Uuid,
) -> Result<(), NativeDbError> {
    destination_backend(backend, workspace, actor, session, false).await
}

pub async fn queue_restore_backend(
    backend: &Backend,
    workspace: Uuid,
    actor: Uuid,
    session: Uuid,
    request: Uuid,
    hash: &str,
    payload: &[u8],
) -> Result<Uuid, NativeDbError> {
    if payload.is_empty() || payload.len() > MAX_BYTES || digest(payload) != hash {
        return Err(ArchiveError::Invalid("confirmation hash".into()).into());
    }
    let mut tx = backend.begin_write().await?;
    let result=async{
        let mut op=tx.operation();op.set_tenant(workspace).await?;
        op.native_destination(workspace,actor,session,true,false).await?;op.lock_tree(workspace).await?;
        let existing=match &mut op{
            OperationTx::Postgres(pg)=>{
                let row=sqlx::query("SELECT id,native_archive_hash FROM fvoci.import_jobs WHERE workspace_id=$1 AND created_by=$2 AND native_request_id=$3 AND source='native-archive'").bind(workspace).bind(actor).bind(request).fetch_optional(&mut ***pg).await?;
                row.map(|r|Ok::<_,sqlx::Error>((r.try_get::<Uuid,_>(0)?,r.try_get::<String,_>(1)?))).transpose()?
            }
            OperationTx::SqliteFamily(family)=>{
                let rows=family.query("SELECT id,native_archive_hash FROM import_jobs WHERE workspace_id=?1 AND created_by=?2 AND native_request_id=?3 AND source='native-archive'",&[Cell::uuid(workspace),Cell::uuid(actor),Cell::uuid(request)]).await?;
                rows.first().map(|r|Ok::<_,sqlx::Error>((r.cell(0)?.id()?,r.cell(1)?.string()?))).transpose()?
            }
        };
        if let Some((id,stored))=existing{if stored!=hash{return Err(NativeDbError::Conflict)}return Ok(id)}
        op.native_destination(workspace,actor,session,true,true).await?;
        let id=Uuid::now_v7();
        match &mut op{
            OperationTx::Postgres(pg)=>{sqlx::query("INSERT INTO fvoci.import_jobs(id,workspace_id,created_by,session_id,source,status,payload,native_request_id,native_archive_hash) VALUES($1,$2,$3,$4,'native-archive','running',$5,$6,$7)").bind(id).bind(workspace).bind(actor).bind(session).bind(payload).bind(request).bind(hash).execute(&mut ***pg).await?;}
            OperationTx::SqliteFamily(family)=>{family.execute("INSERT INTO import_jobs(id,workspace_id,created_by,session_id,source,status,payload,native_request_id,native_archive_hash) VALUES(?1,?2,?3,?4,'native-archive','running',?5,?6,?7)",&[Cell::uuid(id),Cell::uuid(workspace),Cell::uuid(actor),Cell::uuid(session),Cell::Blob(payload.to_vec()),Cell::uuid(request),Cell::text(hash)]).await?;}
        }
        op.native_destination(workspace,actor,session,true,false).await?;
        Ok(id)
    }.await;
    finish_native_write(tx, result).await
}

pub async fn status_backend(
    backend: &Backend,
    workspace: Uuid,
    actor: Uuid,
    session: Uuid,
    id: Uuid,
) -> Result<crate::api::native_archive::NativeJobOutput, NativeDbError> {
    if let Backend::Postgres(pool) = backend {
        return status(pool, workspace, actor, session, id).await;
    }
    let mut tx = backend.begin_read().await?;
    let result=async {
        let mut op=tx.operation();op.set_tenant(workspace).await?;op.native_destination(workspace,actor,session,false,false).await?;
        let OperationTx::SqliteFamily(family)=&mut op else{unreachable!()};
        let rows=family.query("SELECT id,status,native_archive_hash,native_result,native_diagnostic FROM import_jobs WHERE workspace_id=?1 AND id=?2 AND created_by=?3 AND source='native-archive'",&[Cell::uuid(workspace),Cell::uuid(id),Cell::uuid(actor)]).await?;
        let row=rows.first().ok_or(NativeDbError::Forbidden)?;
        let result=row.cell(3)?.optional(Cell::value)?;
        let project_id=result.as_ref().map(|v|v.get("projectId").and_then(Value::as_str).ok_or_else(||ArchiveError::Invalid("native result project".into())).and_then(|s|Uuid::parse_str(s).map_err(|_|ArchiveError::Invalid("native result UUID".into())))).transpose()?;
        let output=crate::api::native_archive::NativeJobOutput{id:row.cell(0)?.id()?,status:row.cell(1)?.string()?,archive_hash:row.cell(2)?.string()?,project_id,diagnostic:row.cell(4)?.optional(Cell::string)?};
        if let Some(project)=project_id {if !op.project_permission_by_id(workspace,actor,project).await?.is_some_and(|p|p.at_least(ProjectPermission::View)){return Err(NativeDbError::Forbidden)}}
        Ok(output)
    }.await;
    finish_native_read(tx, result).await
}

pub async fn stage_key_backend(
    backend: &Backend,
    claim: &ImportClaim,
    attachment: Uuid,
    key: &str,
    cancel: &CancellationToken,
) -> Result<(), NativeDbError> {
    native_cancel(cancel)?;
    if Uuid::parse_str(key).is_err() || Uuid::parse_str(key).is_ok_and(|id| id.to_string() != key) {
        return Err(ArchiveError::Invalid("storage key".into()).into());
    }
    let mut tx = backend.begin_write().await?;
    let result=async {
        let mut op=tx.operation();op.set_tenant(claim.workspace_id).await?;
        op.native_destination(claim.workspace_id,claim.created_by,claim.session_id,true,true).await?;
        op.native_claim(claim).await?;op.native_key_available(claim.workspace_id,attachment,key,false).await?;native_cancel(cancel)?;
        let fence=crate::db::documents::ImportFence{job_id:claim.job_id,lease_token:claim.lease_token};
        if !op.append_import_ref(claim.workspace_id,fence,crate::db::import_jobs::ImportRefKind::StoredKey,key).await?{return Err(NativeDbError::Fenced)}
        match &mut op {
            OperationTx::Postgres(pg)=>{sqlx::query("INSERT INTO fvoci.attachment_object_cleanups(workspace_id,attachment_id,storage_key,due_at) VALUES($1,$2,$3,clock_timestamp()+interval '20 minutes')").bind(claim.workspace_id).bind(attachment).bind(key).execute(&mut ***pg).await?;}
            OperationTx::SqliteFamily(family)=>{family.execute("INSERT INTO attachment_object_cleanups(id,workspace_id,attachment_id,storage_key,due_at) VALUES(?1,?2,?3,?4,unixepoch()*1000000+CAST(substr(strftime('%f','now'),4,3) AS INTEGER)*1000+1200000000)",&[Cell::uuid(Uuid::now_v7()),Cell::uuid(claim.workspace_id),Cell::uuid(attachment),Cell::text(key)]).await?;}
        }
        native_cancel(cancel)?;op.native_claim(claim).await?;
        op.native_destination(claim.workspace_id,claim.created_by,claim.session_id,true,true).await
    }.await;
    finish_native_write(tx, result).await
}

pub async fn fail_native_backend(
    backend: &Backend,
    claim: &ImportClaim,
    diagnostic: &str,
) -> Result<bool, sqlx::Error> {
    let mut tx = backend.begin_write().await?;
    let result=async {
        let mut op=tx.operation();op.set_tenant(claim.workspace_id).await?;
        if claim.source!=crate::db::import_jobs::ImportSource::NativeArchive || !op.hold_import_claim(claim).await?{return Ok(false)}
        match &mut op{
            OperationTx::Postgres(pg)=>{sqlx::query("UPDATE fvoci.import_jobs SET native_diagnostic=$3 WHERE workspace_id=$1 AND id=$2").bind(claim.workspace_id).bind(claim.job_id).bind(diagnostic).execute(&mut ***pg).await?;}
            OperationTx::SqliteFamily(family)=>{family.execute("UPDATE import_jobs SET native_diagnostic=?3 WHERE workspace_id=?1 AND id=?2",&[Cell::uuid(claim.workspace_id),Cell::uuid(claim.job_id),Cell::text(diagnostic)]).await?;}
        }
        op.finish_import_job(claim,crate::db::import_jobs::ImportStatus::Failed).await
    }.await;
    match result {
        Ok(true) => {
            tx.commit()
                .await
                .map_err(|e| sqlx::Error::AnyDriverError(Box::new(e)))?;
            Ok(true)
        }
        Ok(false) => {
            tx.rollback()
                .await
                .map_err(|cleanup| crate::db::backend::rollback_cleanup_unknown(None, cleanup))?;
            Ok(false)
        }
        Err(original) => match tx.rollback().await {
            Ok(()) => Err(original),
            Err(cleanup) => Err(crate::db::backend::rollback_cleanup_unknown(
                Some(Box::new(original)),
                cleanup,
            )),
        },
    }
}

#[derive(Clone, Copy)]
enum NativeStorage {
    Uuid,
    Instant,
    Date,
    Integer,
    Boolean,
    Text,
    Json,
}
struct NativeColumn {
    name: &'static str,
    kind: NativeStorage,
    nullable: bool,
}

fn native_cell(column: &NativeColumn, value: &Value) -> Result<Cell, NativeDbError> {
    let invalid = || ArchiveError::Invalid(format!("native {} value", column.name));
    if value.is_null() && column.nullable {
        return Ok(Cell::Null);
    }
    Ok(match column.kind {
        NativeStorage::Uuid => {
            Cell::uuid(Uuid::parse_str(value.as_str().ok_or_else(invalid)?).map_err(|_| invalid())?)
        }
        NativeStorage::Instant => Cell::instant(
            chrono::DateTime::parse_from_rfc3339(value.as_str().ok_or_else(invalid)?)
                .map_err(|_| invalid())?
                .with_timezone(&chrono::Utc),
        )?,
        NativeStorage::Date => {
            let text = value.as_str().ok_or_else(invalid)?;
            let cell = Cell::text(text);
            cell.date()?;
            cell
        }
        NativeStorage::Integer => Cell::Integer(value.as_i64().ok_or_else(invalid)?),
        NativeStorage::Boolean => Cell::Integer(i64::from(value.as_bool().ok_or_else(invalid)?)),
        NativeStorage::Text => Cell::text(value.as_str().ok_or_else(invalid)?),
        NativeStorage::Json => Cell::json(value)?,
    })
}
async fn native_family_record(
    tx: &mut FamilyTx,
    table: &str,
    value: Value,
) -> Result<(), NativeDbError> {
    tx.require_writer()?;
    let (sql, columns) = native_layout(table)?;
    let fields = value
        .as_object()
        .ok_or_else(|| ArchiveError::Invalid("native record".into()))?;
    // Required columns cannot disappear; optional absent and explicit SQL NULL
    // remain distinct from a required JSON null, e.g. timer audit before_value.
    if fields
        .keys()
        .any(|key| !columns.iter().any(|c| c.name == key))
    {
        return Err(ArchiveError::Invalid("unexpected native column".into()).into());
    }
    let args = columns
        .iter()
        .map(|c| native_cell(c, fields.get(c.name).unwrap_or(&Value::Null)))
        .collect::<Result<Vec<_>, _>>()?;
    tx.execute(sql, &args).await?;
    Ok(())
}

/// Content preparation is shared with the archive's maintained typed mapping
/// helpers; it contains no SQL identifiers or input-defined model dispatch.
fn prepare_native_records(
    archive: &Archive,
    keys: &BTreeMap<Uuid, String>,
    workspace: Uuid,
    actor: Uuid,
    job: Uuid,
) -> Result<Vec<(&'static str, Value)>, NativeDbError> {
    archive.validate()?;
    let g = &archive.graph;
    if keys.len() != g.attachments.len() {
        return Err(NativeDbError::Fenced);
    }
    let mut records = Vec::new();
    let mut project = mapped(&g.project, workspace, actor)?;
    project["root_document_id"] = Value::Null;
    project["next_number"] = json!(g.project.next_number.max(
        g.tasks
            .iter()
            .map(|r| r.number)
            .chain(g.documents.iter().map(|r| r.number))
            .max()
            .unwrap_or(0)
            .checked_add(1)
            .ok_or(ArchiveError::Limit)?
    ));
    records.push(("projects", project));
    rows_collections(&mut records, &g.collections, workspace, actor)?;
    macro_rules! rows {
        ($table:literal,$rows:expr) => {
            for row in $rows {
                records.push(($table, mapped(row, workspace, actor)?));
            }
        };
    }
    rows!("workflows", &g.workflows);
    rows!("statuses", &g.statuses);
    let mut documents = g.documents.iter().collect::<Vec<_>>();
    let mut inserted = std::collections::BTreeSet::new();
    while !documents.is_empty() {
        let index = documents
            .iter()
            .position(|d| d.parent_id.is_none_or(|id| inserted.contains(&id)))
            .ok_or_else(|| ArchiveError::Invalid("document parent cycle".into()))?;
        let d = documents.remove(index);
        records.push(("documents", mapped(d, workspace, actor)?));
        inserted.insert(d.id);
    }
    rows!("document_tags", &g.document_tags);
    rows!("document_tag_assignments", &g.document_tag_assignments);
    rows!("milestones", &g.milestones);
    let mut tasks = g.tasks.iter().collect::<Vec<_>>();
    let mut inserted = std::collections::BTreeSet::new();
    while !tasks.is_empty() {
        let index = tasks
            .iter()
            .position(|t| t.parent_id.is_none_or(|id| inserted.contains(&id)))
            .ok_or_else(|| ArchiveError::Invalid("task parent cycle".into()))?;
        let t = tasks.remove(index);
        records.push(("tasks", mapped(t, workspace, actor)?));
        inserted.insert(t.id);
    }

    rows!("collection_items", &g.collection_items);
    rows!("collection_fields", &g.collection_fields);
    rows!("collection_options", &g.collection_options);
    rows!("collection_values", &g.collection_values);
    rows!("collection_choices", &g.collection_choices);
    rows!("collection_people", &g.collection_people);
    let people_of = |collection: Option<Uuid>| {
        let fields = g
            .collection_fields
            .iter()
            .filter(|f| {
                Some(f.collection_id) == collection
                    && crate::collections::FieldType::parse(&f.r#type)
                        .is_some_and(|t| t.is_people())
            })
            .map(|f| f.id)
            .collect::<std::collections::BTreeSet<_>>();
        move |field: Uuid| fields.contains(&field)
    };
    for row in &g.collection_views {
        let mut v = mapped(row, workspace, actor)?;
        v["owner_id"] = json!(actor);
        v["config"] = mapped_collection_view_config(
            &row.config,
            &people_of(Some(row.collection_id)),
            g.source_actor_id,
            actor,
        );
        records.push(("collection_views", v));
    }
    rows!("task_assignees", &g.assignees);
    rows!("task_dependencies", &g.dependencies);
    let task_people = people_of(
        g.collections
            .iter()
            .find(|c| c.kind == "task")
            .map(|c| c.id),
    );
    for row in &g.views {
        let mut v = mapped(row, workspace, actor)?;
        v["config"] =
            mapped_project_view_config(&row.config, &task_people, g.source_actor_id, actor);
        records.push(("views", v));
    }
    rows!("labels", &g.labels);
    rows!("task_labels", &g.task_labels);
    rows!("task_origins", &g.origins);
    for row in &g.activity {
        let mut v = mapped(row, workspace, actor)?;
        v["actor_user_id"] = Value::Null;
        v["changes"] = mapped_activity_changes(&row.changes, g.source_actor_id, actor);
        records.push(("task_activity", v));
    }
    for row in comments_parent_first(&g.comments)
        .map_err(|_| ArchiveError::Invalid("comment parent cycle".into()))?
    {
        let mut v = mapped(row, workspace, actor)?;
        v["reactions"] = mapped_comment_reactions(&row.reactions, g.source_actor_id, actor);
        records.push(("comments", v));
    }
    for row in &g.attachments {
        let mut v = mapped(row, workspace, actor)?;
        v.as_object_mut()
            .expect("typed attachment")
            .remove("payload_entry");
        v["storage_key"] = json!(keys.get(&row.id).ok_or(NativeDbError::Fenced)?);
        v["reserved_size_bytes"] = json!(row.size_bytes);
        v["status"] = json!("stored");
        v["scan_status"] = json!("skipped");
        v["extract_status"] = json!(crate::attachments::initial_extract_status(
            &row.name, &row.mime
        ));
        v["extract_text"] = json!("");
        v["variants"] = json!({});
        records.push(("attachments", v));
    }
    for row in &g.zotero_connectors {
        let mut v = mapped(row, workspace, actor)?;
        v["owner_user_id"] = json!(actor);
        v["state"] = json!("disconnected");
        v["generation"] = json!(1);
        v["reconciliation_required"] = json!(true);
        records.push(("zotero_connectors", v));
    }
    macro_rules! zotero {
        ($table:literal,$rows:expr) => {
            for row in $rows {
                let mut v = mapped(row, workspace, actor)?;
                v["owner_user_id"] = json!(actor);
                records.push(($table, v));
            }
        };
    }
    zotero!("zotero_collections", &g.zotero_collections);
    zotero!("zotero_references", &g.zotero_references);
    zotero!("zotero_memberships", &g.zotero_memberships);
    zotero!("zotero_links", &g.zotero_links);
    rows!("time_entries", &g.time_entries);
    rows!("task_timer_runs", &g.timer_runs);
    rows!("task_timer_segments", &g.timer_segments);
    // The actual current initializer tracks every open manual entry. Match the
    // archived reservation explicitly; never adopt an unrelated reservation.
    // Already inserted by that trigger, so reconciled separately by publisher.
    for row in &g.timer_commands {
        let mut v = mapped(row, workspace, actor)?;
        v.as_object_mut()
            .expect("typed command")
            .remove("workspace_id");
        v["restored_from_archive"] = json!(job);
        records.push(("task_timer_commands", v));
    }
    for row in &g.timer_audit {
        let mut v =
            serde_json::to_value(row).map_err(|_| ArchiveError::Invalid("timer audit".into()))?;
        v["user_id"] = json!(actor);
        v["workspace_id"] = json!(row.workspace_id.map(|w| if w == g.source_workspace_id {
            workspace
        } else {
            w
        }));
        records.push(("task_timer_audit", v));
    }
    for row in &g.personal_input_commands {
        let mut v = mapped(row, workspace, actor)?;
        v["actor_user_id"] = json!(actor);
        for field in ["document_id", "task_id", "project_id"] {
            v[field] = Value::Null;
        }
        records.push(("personal_input_commands", v));
    }
    Ok(records)
}

// Fixed current-model native columns. No caller supplies a SQL identifier.
fn native_layout(table: &str) -> Result<(&'static str, &'static [NativeColumn]), NativeDbError> {
    use NativeStorage::*;
    Ok(match table {
        "projects" => ("INSERT INTO projects(\"workspace_id\",\"id\",\"key\",\"name\",\"description\",\"icon\",\"visibility\",\"root_document_id\",\"status\",\"next_number\",\"created_by\",\"created_at\",\"updated_at\",\"deleted_at\") VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13,?14)", &[NativeColumn {name:"workspace_id",kind:Uuid,nullable:false }, NativeColumn {name:"id",kind:Uuid,nullable:false }, NativeColumn {name:"key",kind:Text,nullable:false }, NativeColumn {name:"name",kind:Text,nullable:false }, NativeColumn {name:"description",kind:Text,nullable:true }, NativeColumn {name:"icon",kind:Text,nullable:true }, NativeColumn {name:"visibility",kind:Text,nullable:false }, NativeColumn {name:"root_document_id",kind:Uuid,nullable:true }, NativeColumn {name:"status",kind:Text,nullable:false }, NativeColumn {name:"next_number",kind:Integer,nullable:false }, NativeColumn {name:"created_by",kind:Uuid,nullable:false }, NativeColumn {name:"created_at",kind:Instant,nullable:false }, NativeColumn {name:"updated_at",kind:Instant,nullable:false }, NativeColumn {name:"deleted_at",kind:Instant,nullable:true }]),
        "workflows" => ("INSERT INTO workflows(\"workspace_id\",\"id\",\"project_id\",\"created_at\",\"updated_at\") VALUES(?1,?2,?3,?4,?5)", &[NativeColumn {name:"workspace_id",kind:Uuid,nullable:false }, NativeColumn {name:"id",kind:Uuid,nullable:false }, NativeColumn {name:"project_id",kind:Uuid,nullable:false }, NativeColumn {name:"created_at",kind:Instant,nullable:false }, NativeColumn {name:"updated_at",kind:Instant,nullable:false }]),
        "statuses" => ("INSERT INTO statuses(\"workspace_id\",\"id\",\"project_id\",\"workflow_id\",\"name\",\"category\",\"sort_key\",\"wip_limit\",\"created_at\",\"updated_at\") VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,?10)", &[NativeColumn {name:"workspace_id",kind:Uuid,nullable:false }, NativeColumn {name:"id",kind:Uuid,nullable:false }, NativeColumn {name:"project_id",kind:Uuid,nullable:false }, NativeColumn {name:"workflow_id",kind:Uuid,nullable:false }, NativeColumn {name:"name",kind:Text,nullable:false }, NativeColumn {name:"category",kind:Text,nullable:false }, NativeColumn {name:"sort_key",kind:Text,nullable:false }, NativeColumn {name:"wip_limit",kind:Integer,nullable:true }, NativeColumn {name:"created_at",kind:Instant,nullable:false }, NativeColumn {name:"updated_at",kind:Instant,nullable:false }]),
        "documents" => ("INSERT INTO documents(\"workspace_id\",\"id\",\"title\",\"icon\",\"path\",\"parent_id\",\"sort_key\",\"project_id\",\"number\",\"status\",\"schema_version\",\"text\",\"chosung\",\"version\",\"created_by\",\"created_at\",\"updated_at\",\"deleted_at\",\"content_json\",\"kind\") VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13,?14,?15,?16,?17,?18,?19,?20)", &[NativeColumn {name:"workspace_id",kind:Uuid,nullable:false }, NativeColumn {name:"id",kind:Uuid,nullable:false }, NativeColumn {name:"title",kind:Text,nullable:false }, NativeColumn {name:"icon",kind:Text,nullable:true }, NativeColumn {name:"path",kind:Text,nullable:false }, NativeColumn {name:"parent_id",kind:Uuid,nullable:true }, NativeColumn {name:"sort_key",kind:Text,nullable:false }, NativeColumn {name:"project_id",kind:Uuid,nullable:true }, NativeColumn {name:"number",kind:Integer,nullable:false }, NativeColumn {name:"status",kind:Text,nullable:false }, NativeColumn {name:"schema_version",kind:Integer,nullable:false }, NativeColumn {name:"text",kind:Text,nullable:false }, NativeColumn {name:"chosung",kind:Text,nullable:false }, NativeColumn {name:"version",kind:Integer,nullable:false }, NativeColumn {name:"created_by",kind:Uuid,nullable:false }, NativeColumn {name:"created_at",kind:Instant,nullable:false }, NativeColumn {name:"updated_at",kind:Instant,nullable:false }, NativeColumn {name:"deleted_at",kind:Instant,nullable:true }, NativeColumn {name:"content_json",kind:Json,nullable:false }, NativeColumn {name:"kind",kind:Text,nullable:false }]),
        "tasks" => ("INSERT INTO tasks(\"workspace_id\",\"id\",\"project_id\",\"number\",\"title\",\"type\",\"priority\",\"status_id\",\"start_date\",\"due_date\",\"due_at\",\"estimate\",\"parent_id\",\"milestone_id\",\"recurrence\",\"sort_key\",\"schema_version\",\"content_json\",\"version\",\"archived_at\",\"deleted_at\",\"created_by\",\"created_at\",\"updated_at\",\"text\",\"chosung\",\"estimate_unit\") VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13,?14,?15,?16,?17,?18,?19,?20,?21,?22,?23,?24,?25,?26,?27)", &[NativeColumn {name:"workspace_id",kind:Uuid,nullable:false }, NativeColumn {name:"id",kind:Uuid,nullable:false }, NativeColumn {name:"project_id",kind:Uuid,nullable:false }, NativeColumn {name:"number",kind:Integer,nullable:false }, NativeColumn {name:"title",kind:Text,nullable:false }, NativeColumn {name:"type",kind:Text,nullable:false }, NativeColumn {name:"priority",kind:Text,nullable:false }, NativeColumn {name:"status_id",kind:Uuid,nullable:false }, NativeColumn {name:"start_date",kind:Date,nullable:true }, NativeColumn {name:"due_date",kind:Date,nullable:true }, NativeColumn {name:"due_at",kind:Instant,nullable:true }, NativeColumn {name:"estimate",kind:Text,nullable:true }, NativeColumn {name:"parent_id",kind:Uuid,nullable:true }, NativeColumn {name:"milestone_id",kind:Uuid,nullable:true }, NativeColumn {name:"recurrence",kind:Json,nullable:true }, NativeColumn {name:"sort_key",kind:Text,nullable:false }, NativeColumn {name:"schema_version",kind:Integer,nullable:false }, NativeColumn {name:"content_json",kind:Json,nullable:false }, NativeColumn {name:"version",kind:Integer,nullable:false }, NativeColumn {name:"archived_at",kind:Instant,nullable:true }, NativeColumn {name:"deleted_at",kind:Instant,nullable:true }, NativeColumn {name:"created_by",kind:Uuid,nullable:false }, NativeColumn {name:"created_at",kind:Instant,nullable:false }, NativeColumn {name:"updated_at",kind:Instant,nullable:false }, NativeColumn {name:"text",kind:Text,nullable:false }, NativeColumn {name:"chosung",kind:Text,nullable:false }, NativeColumn {name:"estimate_unit",kind:Text,nullable:true }]),
        "task_assignees" => ("INSERT INTO task_assignees(\"workspace_id\",\"task_id\",\"user_id\") VALUES(?1,?2,?3)", &[NativeColumn {name:"workspace_id",kind:Uuid,nullable:false }, NativeColumn {name:"task_id",kind:Uuid,nullable:false }, NativeColumn {name:"user_id",kind:Uuid,nullable:false }]),
        "labels" => ("INSERT INTO labels(\"workspace_id\",\"id\",\"project_id\",\"name\",\"color\",\"created_at\",\"updated_at\") VALUES(?1,?2,?3,?4,?5,?6,?7)", &[NativeColumn {name:"workspace_id",kind:Uuid,nullable:false }, NativeColumn {name:"id",kind:Uuid,nullable:false }, NativeColumn {name:"project_id",kind:Uuid,nullable:false }, NativeColumn {name:"name",kind:Text,nullable:false }, NativeColumn {name:"color",kind:Text,nullable:false }, NativeColumn {name:"created_at",kind:Instant,nullable:false }, NativeColumn {name:"updated_at",kind:Instant,nullable:false }]),
        "task_labels" => ("INSERT INTO task_labels(\"workspace_id\",\"task_id\",\"label_id\") VALUES(?1,?2,?3)", &[NativeColumn {name:"workspace_id",kind:Uuid,nullable:false }, NativeColumn {name:"task_id",kind:Uuid,nullable:false }, NativeColumn {name:"label_id",kind:Uuid,nullable:false }]),
        "milestones" => ("INSERT INTO milestones(\"workspace_id\",\"id\",\"project_id\",\"name\",\"due_date\",\"sort_key\",\"created_at\",\"updated_at\") VALUES(?1,?2,?3,?4,?5,?6,?7,?8)", &[NativeColumn {name:"workspace_id",kind:Uuid,nullable:false }, NativeColumn {name:"id",kind:Uuid,nullable:false }, NativeColumn {name:"project_id",kind:Uuid,nullable:false }, NativeColumn {name:"name",kind:Text,nullable:false }, NativeColumn {name:"due_date",kind:Date,nullable:true }, NativeColumn {name:"sort_key",kind:Text,nullable:false }, NativeColumn {name:"created_at",kind:Instant,nullable:false }, NativeColumn {name:"updated_at",kind:Instant,nullable:false }]),
        "task_dependencies" => ("INSERT INTO task_dependencies(\"workspace_id\",\"blocker_id\",\"blocked_id\",\"type\",\"lag_days\") VALUES(?1,?2,?3,?4,?5)", &[NativeColumn {name:"workspace_id",kind:Uuid,nullable:false }, NativeColumn {name:"blocker_id",kind:Uuid,nullable:false }, NativeColumn {name:"blocked_id",kind:Uuid,nullable:false }, NativeColumn {name:"type",kind:Text,nullable:false }, NativeColumn {name:"lag_days",kind:Integer,nullable:false }]),
        "document_tags" => ("INSERT INTO document_tags(\"workspace_id\",\"id\",\"name\",\"color\",\"created_at\",\"updated_at\") VALUES(?1,?2,?3,?4,?5,?6)", &[NativeColumn {name:"workspace_id",kind:Uuid,nullable:false }, NativeColumn {name:"id",kind:Uuid,nullable:false }, NativeColumn {name:"name",kind:Text,nullable:false }, NativeColumn {name:"color",kind:Text,nullable:false }, NativeColumn {name:"created_at",kind:Instant,nullable:false }, NativeColumn {name:"updated_at",kind:Instant,nullable:false }]),
        "document_tag_assignments" => ("INSERT INTO document_tag_assignments(\"workspace_id\",\"document_id\",\"tag_id\") VALUES(?1,?2,?3)", &[NativeColumn {name:"workspace_id",kind:Uuid,nullable:false }, NativeColumn {name:"document_id",kind:Uuid,nullable:false }, NativeColumn {name:"tag_id",kind:Uuid,nullable:false }]),
        "views" => ("INSERT INTO views(\"workspace_id\",\"id\",\"project_id\",\"user_id\",\"name\",\"type\",\"config\",\"created_at\",\"updated_at\") VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9)", &[NativeColumn {name:"workspace_id",kind:Uuid,nullable:false }, NativeColumn {name:"id",kind:Uuid,nullable:false }, NativeColumn {name:"project_id",kind:Uuid,nullable:false }, NativeColumn {name:"user_id",kind:Uuid,nullable:false }, NativeColumn {name:"name",kind:Text,nullable:false }, NativeColumn {name:"type",kind:Text,nullable:false }, NativeColumn {name:"config",kind:Json,nullable:false }, NativeColumn {name:"created_at",kind:Instant,nullable:false }, NativeColumn {name:"updated_at",kind:Instant,nullable:false }]),
        "comments" => ("INSERT INTO comments(\"workspace_id\",\"id\",\"document_id\",\"task_id\",\"parent_id\",\"created_by\",\"body\",\"chosung\",\"resolved_at\",\"reactions\",\"created_at\",\"updated_at\") VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12)", &[NativeColumn {name:"workspace_id",kind:Uuid,nullable:false }, NativeColumn {name:"id",kind:Uuid,nullable:false }, NativeColumn {name:"document_id",kind:Uuid,nullable:true }, NativeColumn {name:"task_id",kind:Uuid,nullable:true }, NativeColumn {name:"parent_id",kind:Uuid,nullable:true }, NativeColumn {name:"created_by",kind:Uuid,nullable:false }, NativeColumn {name:"body",kind:Text,nullable:false }, NativeColumn {name:"chosung",kind:Text,nullable:false }, NativeColumn {name:"resolved_at",kind:Instant,nullable:true }, NativeColumn {name:"reactions",kind:Json,nullable:false }, NativeColumn {name:"created_at",kind:Instant,nullable:false }, NativeColumn {name:"updated_at",kind:Instant,nullable:false }]),
        "task_origins" => ("INSERT INTO task_origins(\"workspace_id\",\"task_id\",\"document_id\",\"request_id\",\"request_hash\",\"anchor\",\"created_at\",\"updated_at\") VALUES(?1,?2,?3,?4,?5,?6,?7,?8)", &[NativeColumn {name:"workspace_id",kind:Uuid,nullable:false }, NativeColumn {name:"task_id",kind:Uuid,nullable:false }, NativeColumn {name:"document_id",kind:Uuid,nullable:false }, NativeColumn {name:"request_id",kind:Uuid,nullable:false }, NativeColumn {name:"request_hash",kind:Text,nullable:false }, NativeColumn {name:"anchor",kind:Text,nullable:true }, NativeColumn {name:"created_at",kind:Instant,nullable:false }, NativeColumn {name:"updated_at",kind:Instant,nullable:false }]),
        "task_activity" => ("INSERT INTO task_activity(\"workspace_id\",\"id\",\"task_id\",\"actor_user_id\",\"channel\",\"kind\",\"changes\",\"created_at\") VALUES(?1,?2,?3,?4,?5,?6,?7,?8)", &[NativeColumn {name:"workspace_id",kind:Uuid,nullable:false }, NativeColumn {name:"id",kind:Uuid,nullable:false }, NativeColumn {name:"task_id",kind:Uuid,nullable:false }, NativeColumn {name:"actor_user_id",kind:Uuid,nullable:true }, NativeColumn {name:"channel",kind:Text,nullable:false }, NativeColumn {name:"kind",kind:Text,nullable:false }, NativeColumn {name:"changes",kind:Json,nullable:false }, NativeColumn {name:"created_at",kind:Instant,nullable:false }]),
        "attachments" => ("INSERT INTO attachments(\"workspace_id\",\"id\",\"document_id\",\"task_id\",\"uploader_id\",\"name\",\"mime\",\"declared_mime\",\"size_bytes\",\"image\",\"created_at\",\"completed_at\",\"storage_key\",\"reserved_size_bytes\",\"status\",\"scan_status\",\"extract_status\",\"extract_text\",\"variants\") VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13,?14,?15,?16,?17,?18,?19)", &[NativeColumn {name:"workspace_id",kind:Uuid,nullable:false }, NativeColumn {name:"id",kind:Uuid,nullable:false }, NativeColumn {name:"document_id",kind:Uuid,nullable:true }, NativeColumn {name:"task_id",kind:Uuid,nullable:true }, NativeColumn {name:"uploader_id",kind:Uuid,nullable:false }, NativeColumn {name:"name",kind:Text,nullable:false }, NativeColumn {name:"mime",kind:Text,nullable:false }, NativeColumn {name:"declared_mime",kind:Text,nullable:true }, NativeColumn {name:"size_bytes",kind:Integer,nullable:false }, NativeColumn {name:"image",kind:Boolean,nullable:false }, NativeColumn {name:"created_at",kind:Instant,nullable:false }, NativeColumn {name:"completed_at",kind:Instant,nullable:false }, NativeColumn {name:"storage_key",kind:Text,nullable:false }, NativeColumn {name:"reserved_size_bytes",kind:Integer,nullable:false }, NativeColumn {name:"status",kind:Text,nullable:false }, NativeColumn {name:"scan_status",kind:Text,nullable:false }, NativeColumn {name:"extract_status",kind:Text,nullable:false }, NativeColumn {name:"extract_text",kind:Text,nullable:false }, NativeColumn {name:"variants",kind:Json,nullable:false }]),
        "collections" => ("INSERT INTO collections(\"workspace_id\",\"id\",\"project_id\",\"kind\",\"name\",\"version\",\"deleted_at\",\"created_at\",\"updated_at\") VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9)", &[NativeColumn {name:"workspace_id",kind:Uuid,nullable:false }, NativeColumn {name:"id",kind:Uuid,nullable:false }, NativeColumn {name:"project_id",kind:Uuid,nullable:true }, NativeColumn {name:"kind",kind:Text,nullable:false }, NativeColumn {name:"name",kind:Text,nullable:false }, NativeColumn {name:"version",kind:Integer,nullable:false }, NativeColumn {name:"deleted_at",kind:Instant,nullable:true }, NativeColumn {name:"created_at",kind:Instant,nullable:false }, NativeColumn {name:"updated_at",kind:Instant,nullable:false }]),
        "collection_items" => ("INSERT INTO collection_items(\"workspace_id\",\"id\",\"collection_id\",\"document_id\",\"task_id\",\"version\",\"created_at\",\"updated_at\") VALUES(?1,?2,?3,?4,?5,?6,?7,?8)", &[NativeColumn {name:"workspace_id",kind:Uuid,nullable:false }, NativeColumn {name:"id",kind:Uuid,nullable:false }, NativeColumn {name:"collection_id",kind:Uuid,nullable:false }, NativeColumn {name:"document_id",kind:Uuid,nullable:true }, NativeColumn {name:"task_id",kind:Uuid,nullable:true }, NativeColumn {name:"version",kind:Integer,nullable:false }, NativeColumn {name:"created_at",kind:Instant,nullable:false }, NativeColumn {name:"updated_at",kind:Instant,nullable:false }]),
        "collection_fields" => ("INSERT INTO collection_fields(\"workspace_id\",\"id\",\"collection_id\",\"key\",\"name\",\"description\",\"type\",\"sort_key\",\"version\",\"deleted_at\",\"created_at\",\"updated_at\") VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12)", &[NativeColumn {name:"workspace_id",kind:Uuid,nullable:false }, NativeColumn {name:"id",kind:Uuid,nullable:false }, NativeColumn {name:"collection_id",kind:Uuid,nullable:false }, NativeColumn {name:"key",kind:Text,nullable:false }, NativeColumn {name:"name",kind:Text,nullable:false }, NativeColumn {name:"description",kind:Text,nullable:true }, NativeColumn {name:"type",kind:Text,nullable:false }, NativeColumn {name:"sort_key",kind:Text,nullable:false }, NativeColumn {name:"version",kind:Integer,nullable:false }, NativeColumn {name:"deleted_at",kind:Instant,nullable:true }, NativeColumn {name:"created_at",kind:Instant,nullable:false }, NativeColumn {name:"updated_at",kind:Instant,nullable:false }]),
        "collection_options" => ("INSERT INTO collection_options(\"workspace_id\",\"id\",\"collection_id\",\"field_id\",\"key\",\"label\",\"sort_key\",\"deleted_at\") VALUES(?1,?2,?3,?4,?5,?6,?7,?8)", &[NativeColumn {name:"workspace_id",kind:Uuid,nullable:false }, NativeColumn {name:"id",kind:Uuid,nullable:false }, NativeColumn {name:"collection_id",kind:Uuid,nullable:false }, NativeColumn {name:"field_id",kind:Uuid,nullable:false }, NativeColumn {name:"key",kind:Text,nullable:false }, NativeColumn {name:"label",kind:Text,nullable:false }, NativeColumn {name:"sort_key",kind:Text,nullable:false }, NativeColumn {name:"deleted_at",kind:Instant,nullable:true }]),
        "collection_values" => ("INSERT INTO collection_values(\"workspace_id\",\"collection_id\",\"item_id\",\"field_id\",\"field_type\",\"value_text\",\"value_number\",\"value_date\",\"value_ts\",\"value_bool\") VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,?10)", &[NativeColumn {name:"workspace_id",kind:Uuid,nullable:false }, NativeColumn {name:"collection_id",kind:Uuid,nullable:false }, NativeColumn {name:"item_id",kind:Uuid,nullable:false }, NativeColumn {name:"field_id",kind:Uuid,nullable:false }, NativeColumn {name:"field_type",kind:Text,nullable:false }, NativeColumn {name:"value_text",kind:Text,nullable:true }, NativeColumn {name:"value_number",kind:Text,nullable:true }, NativeColumn {name:"value_date",kind:Date,nullable:true }, NativeColumn {name:"value_ts",kind:Instant,nullable:true }, NativeColumn {name:"value_bool",kind:Boolean,nullable:true }]),
        "collection_choices" => ("INSERT INTO collection_choices(\"workspace_id\",\"collection_id\",\"item_id\",\"field_id\",\"field_type\",\"option_id\") VALUES(?1,?2,?3,?4,?5,?6)", &[NativeColumn {name:"workspace_id",kind:Uuid,nullable:false }, NativeColumn {name:"collection_id",kind:Uuid,nullable:false }, NativeColumn {name:"item_id",kind:Uuid,nullable:false }, NativeColumn {name:"field_id",kind:Uuid,nullable:false }, NativeColumn {name:"field_type",kind:Text,nullable:false }, NativeColumn {name:"option_id",kind:Uuid,nullable:false }]),
        "collection_people" => ("INSERT INTO collection_people(\"workspace_id\",\"collection_id\",\"item_id\",\"field_id\",\"field_type\",\"user_id\") VALUES(?1,?2,?3,?4,?5,?6)", &[NativeColumn {name:"workspace_id",kind:Uuid,nullable:false }, NativeColumn {name:"collection_id",kind:Uuid,nullable:false }, NativeColumn {name:"item_id",kind:Uuid,nullable:false }, NativeColumn {name:"field_id",kind:Uuid,nullable:false }, NativeColumn {name:"field_type",kind:Text,nullable:false }, NativeColumn {name:"user_id",kind:Uuid,nullable:false }]),
        "collection_views" => ("INSERT INTO collection_views(\"workspace_id\",\"id\",\"collection_id\",\"owner_id\",\"visibility\",\"name\",\"type\",\"config\",\"version\",\"created_at\",\"updated_at\") VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11)", &[NativeColumn {name:"workspace_id",kind:Uuid,nullable:false }, NativeColumn {name:"id",kind:Uuid,nullable:false }, NativeColumn {name:"collection_id",kind:Uuid,nullable:false }, NativeColumn {name:"owner_id",kind:Uuid,nullable:false }, NativeColumn {name:"visibility",kind:Text,nullable:false }, NativeColumn {name:"name",kind:Text,nullable:false }, NativeColumn {name:"type",kind:Text,nullable:false }, NativeColumn {name:"config",kind:Json,nullable:false }, NativeColumn {name:"version",kind:Integer,nullable:false }, NativeColumn {name:"created_at",kind:Instant,nullable:false }, NativeColumn {name:"updated_at",kind:Instant,nullable:false }]),
        "zotero_connectors" => ("INSERT INTO zotero_connectors(\"workspace_id\",\"id\",\"library_type\",\"remote_library_id\",\"library_url\",\"completed_version\",\"created_at\",\"updated_at\",\"owner_user_id\",\"state\",\"generation\",\"reconciliation_required\") VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12)", &[NativeColumn {name:"workspace_id",kind:Uuid,nullable:false }, NativeColumn {name:"id",kind:Uuid,nullable:false }, NativeColumn {name:"library_type",kind:Text,nullable:false }, NativeColumn {name:"remote_library_id",kind:Integer,nullable:false }, NativeColumn {name:"library_url",kind:Text,nullable:false }, NativeColumn {name:"completed_version",kind:Integer,nullable:false }, NativeColumn {name:"created_at",kind:Instant,nullable:false }, NativeColumn {name:"updated_at",kind:Instant,nullable:false }, NativeColumn {name:"owner_user_id",kind:Uuid,nullable:false }, NativeColumn {name:"state",kind:Text,nullable:false }, NativeColumn {name:"generation",kind:Integer,nullable:false }, NativeColumn {name:"reconciliation_required",kind:Boolean,nullable:false }]),
        "zotero_references" => ("INSERT INTO zotero_references(\"workspace_id\",\"id\",\"connector_id\",\"document_id\",\"item_key\",\"remote_version\",\"local_version\",\"bibliography\",\"return_url\",\"availability\",\"owner_user_id\") VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11)", &[NativeColumn {name:"workspace_id",kind:Uuid,nullable:false }, NativeColumn {name:"id",kind:Uuid,nullable:false }, NativeColumn {name:"connector_id",kind:Uuid,nullable:false }, NativeColumn {name:"document_id",kind:Uuid,nullable:true }, NativeColumn {name:"item_key",kind:Text,nullable:false }, NativeColumn {name:"remote_version",kind:Integer,nullable:false }, NativeColumn {name:"local_version",kind:Integer,nullable:false }, NativeColumn {name:"bibliography",kind:Json,nullable:false }, NativeColumn {name:"return_url",kind:Text,nullable:false }, NativeColumn {name:"availability",kind:Text,nullable:false }, NativeColumn {name:"owner_user_id",kind:Uuid,nullable:false }]),
        "zotero_collections" => ("INSERT INTO zotero_collections(\"workspace_id\",\"connector_id\",\"collection_key\",\"remote_version\",\"name\",\"parent_key\",\"availability\",\"owner_user_id\") VALUES(?1,?2,?3,?4,?5,?6,?7,?8)", &[NativeColumn {name:"workspace_id",kind:Uuid,nullable:false }, NativeColumn {name:"connector_id",kind:Uuid,nullable:false }, NativeColumn {name:"collection_key",kind:Text,nullable:false }, NativeColumn {name:"remote_version",kind:Integer,nullable:false }, NativeColumn {name:"name",kind:Text,nullable:false }, NativeColumn {name:"parent_key",kind:Text,nullable:true }, NativeColumn {name:"availability",kind:Text,nullable:false }, NativeColumn {name:"owner_user_id",kind:Uuid,nullable:false }]),
        "zotero_memberships" => ("INSERT INTO zotero_memberships(\"workspace_id\",\"connector_id\",\"reference_id\",\"collection_key\",\"owner_user_id\") VALUES(?1,?2,?3,?4,?5)", &[NativeColumn {name:"workspace_id",kind:Uuid,nullable:false }, NativeColumn {name:"connector_id",kind:Uuid,nullable:false }, NativeColumn {name:"reference_id",kind:Uuid,nullable:false }, NativeColumn {name:"collection_key",kind:Text,nullable:false }, NativeColumn {name:"owner_user_id",kind:Uuid,nullable:false }]),
        "zotero_links" => ("INSERT INTO zotero_links(\"workspace_id\",\"id\",\"connector_id\",\"reference_id\",\"document_id\",\"task_id\",\"anchor\",\"owner_user_id\") VALUES(?1,?2,?3,?4,?5,?6,?7,?8)", &[NativeColumn {name:"workspace_id",kind:Uuid,nullable:false }, NativeColumn {name:"id",kind:Uuid,nullable:false }, NativeColumn {name:"connector_id",kind:Uuid,nullable:false }, NativeColumn {name:"reference_id",kind:Uuid,nullable:false }, NativeColumn {name:"document_id",kind:Uuid,nullable:true }, NativeColumn {name:"task_id",kind:Uuid,nullable:true }, NativeColumn {name:"anchor",kind:Text,nullable:false }, NativeColumn {name:"owner_user_id",kind:Uuid,nullable:false }]),
        "personal_input_commands" => ("INSERT INTO personal_input_commands(\"workspace_id\",\"request_id\",\"request_hash\",\"intent\",\"document_id\",\"task_id\",\"project_id\",\"created_at\",\"actor_user_id\") VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9)", &[NativeColumn {name:"workspace_id",kind:Uuid,nullable:false }, NativeColumn {name:"request_id",kind:Uuid,nullable:false }, NativeColumn {name:"request_hash",kind:Text,nullable:false }, NativeColumn {name:"intent",kind:Text,nullable:false }, NativeColumn {name:"document_id",kind:Uuid,nullable:true }, NativeColumn {name:"task_id",kind:Uuid,nullable:true }, NativeColumn {name:"project_id",kind:Uuid,nullable:true }, NativeColumn {name:"created_at",kind:Instant,nullable:false }, NativeColumn {name:"actor_user_id",kind:Uuid,nullable:false }]),
        "time_entries" => ("INSERT INTO time_entries(\"workspace_id\",\"id\",\"task_id\",\"user_id\",\"started_at\",\"ended_at\",\"duration_seconds\",\"note\") VALUES(?1,?2,?3,?4,?5,?6,?7,?8)", &[NativeColumn {name:"workspace_id",kind:Uuid,nullable:false }, NativeColumn {name:"id",kind:Uuid,nullable:false }, NativeColumn {name:"task_id",kind:Uuid,nullable:false }, NativeColumn {name:"user_id",kind:Uuid,nullable:false }, NativeColumn {name:"started_at",kind:Instant,nullable:false }, NativeColumn {name:"ended_at",kind:Instant,nullable:true }, NativeColumn {name:"duration_seconds",kind:Integer,nullable:true }, NativeColumn {name:"note",kind:Text,nullable:true }]),
        "task_timer_runs" => ("INSERT INTO task_timer_runs(\"workspace_id\",\"id\",\"user_id\",\"task_id\",\"status\",\"version\",\"started_at\",\"stopped_at\",\"note\") VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9)", &[NativeColumn {name:"workspace_id",kind:Uuid,nullable:false }, NativeColumn {name:"id",kind:Uuid,nullable:false }, NativeColumn {name:"user_id",kind:Uuid,nullable:false }, NativeColumn {name:"task_id",kind:Uuid,nullable:false }, NativeColumn {name:"status",kind:Text,nullable:false }, NativeColumn {name:"version",kind:Integer,nullable:false }, NativeColumn {name:"started_at",kind:Instant,nullable:false }, NativeColumn {name:"stopped_at",kind:Instant,nullable:true }, NativeColumn {name:"note",kind:Text,nullable:true }]),
        "task_timer_segments" => ("INSERT INTO task_timer_segments(\"workspace_id\",\"id\",\"run_id\",\"user_id\",\"task_id\",\"started_at\",\"ended_at\",\"time_entry_id\") VALUES(?1,?2,?3,?4,?5,?6,?7,?8)", &[NativeColumn {name:"workspace_id",kind:Uuid,nullable:false }, NativeColumn {name:"id",kind:Uuid,nullable:false }, NativeColumn {name:"run_id",kind:Uuid,nullable:false }, NativeColumn {name:"user_id",kind:Uuid,nullable:false }, NativeColumn {name:"task_id",kind:Uuid,nullable:false }, NativeColumn {name:"started_at",kind:Instant,nullable:false }, NativeColumn {name:"ended_at",kind:Instant,nullable:true }, NativeColumn {name:"time_entry_id",kind:Uuid,nullable:true }]),
        "task_timer_legacy_open" => ("INSERT INTO task_timer_legacy_open(\"workspace_id\",\"time_entry_id\",\"user_id\",\"task_id\") VALUES(?1,?2,?3,?4)", &[NativeColumn {name:"workspace_id",kind:Uuid,nullable:false }, NativeColumn {name:"time_entry_id",kind:Uuid,nullable:false }, NativeColumn {name:"user_id",kind:Uuid,nullable:false }, NativeColumn {name:"task_id",kind:Uuid,nullable:false }]),
        "task_timer_commands" => ("INSERT INTO task_timer_commands(\"request_id\",\"user_id\",\"request_hash\",\"run_id\",\"result\",\"created_at\",\"restored_from_archive\") VALUES(?1,?2,?3,?4,?5,?6,?7)", &[NativeColumn {name:"request_id",kind:Uuid,nullable:false }, NativeColumn {name:"user_id",kind:Uuid,nullable:false }, NativeColumn {name:"request_hash",kind:Text,nullable:false }, NativeColumn {name:"run_id",kind:Uuid,nullable:true }, NativeColumn {name:"result",kind:Json,nullable:false }, NativeColumn {name:"created_at",kind:Instant,nullable:false }, NativeColumn {name:"restored_from_archive",kind:Uuid,nullable:false }]),
        "task_timer_audit" => ("INSERT INTO task_timer_audit(\"id\",\"user_id\",\"request_id\",\"workspace_id\",\"task_id\",\"time_entry_id\",\"verb\",\"before_value\",\"after_value\",\"reason\",\"created_at\") VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11)", &[NativeColumn {name:"id",kind:Uuid,nullable:false }, NativeColumn {name:"user_id",kind:Uuid,nullable:false }, NativeColumn {name:"request_id",kind:Uuid,nullable:false }, NativeColumn {name:"workspace_id",kind:Uuid,nullable:true }, NativeColumn {name:"task_id",kind:Uuid,nullable:true }, NativeColumn {name:"time_entry_id",kind:Uuid,nullable:true }, NativeColumn {name:"verb",kind:Text,nullable:false }, NativeColumn {name:"before_value",kind:Json,nullable:false }, NativeColumn {name:"after_value",kind:Json,nullable:false }, NativeColumn {name:"reason",kind:Text,nullable:false }, NativeColumn {name:"created_at",kind:Instant,nullable:false }]),
        _ => return Err(ArchiveError::Invalid("native table".into()).into()),
    })
}

impl OperationTx<'_, '_> {
    async fn native_keys(
        &mut self,
        claim: &ImportClaim,
        archive: &Archive,
        keys: &BTreeMap<Uuid, String>,
    ) -> Result<(), NativeDbError> {
        if keys.len() != archive.graph.attachments.len() {
            return Err(NativeDbError::Fenced);
        }
        for file in &archive.graph.attachments {
            let key = keys.get(&file.id).ok_or(NativeDbError::Fenced)?;
            self.native_key_available(claim.workspace_id, file.id, key, true)
                .await?;
            if !self
                .import_claim_contains_ref(
                    claim,
                    crate::db::import_jobs::ImportRefKind::StoredKey,
                    key,
                )
                .await?
            {
                return Err(NativeDbError::Fenced);
            }
            let count=match self {
                Self::Postgres(pg)=>sqlx::query_scalar::<_,i64>("SELECT count(*) FROM fvoci.attachment_object_cleanups WHERE workspace_id=$1 AND attachment_id=$2 AND storage_key=$3").bind(claim.workspace_id).bind(file.id).bind(key).fetch_one(&mut ***pg).await?,
                Self::SqliteFamily(family)=>family.query("SELECT count(*) FROM attachment_object_cleanups WHERE workspace_id=?1 AND attachment_id=?2 AND storage_key=?3",&[Cell::uuid(claim.workspace_id),Cell::uuid(file.id),Cell::text(key)]).await?[0].cell(0)?.integer()?
            };
            if count != 1 {
                return Err(NativeDbError::Fenced);
            }
        }
        Ok(())
    }
}
async fn family_time_guard(tx: &mut FamilyTx, g: &Graph, actor: Uuid) -> Result<(), NativeDbError> {
    // Retained identities are person-scoped, including purged tasks. Historical
    // locators in both audit JSON sides keep the same kind-specific ABA guard.
    let ids = |values: Vec<Uuid>| {
        json!(values
            .iter()
            .map(|id| hex::encode(id.as_bytes()))
            .collect::<Vec<_>>())
    };
    let text_ids =
        |values: Vec<Uuid>| json!(values.iter().map(Uuid::to_string).collect::<Vec<_>>());
    let rows=tx.query("SELECT EXISTS(SELECT 1 FROM task_timer_commands WHERE user_id=?1 AND lower(hex(run_id)) IN (SELECT value FROM json_each(?2))) OR EXISTS(SELECT 1 FROM task_timer_audit a WHERE a.user_id=?1 AND (lower(hex(a.task_id)) IN (SELECT value FROM json_each(?3)) OR lower(hex(a.time_entry_id)) IN (SELECT value FROM json_each(?4)) OR (json_extract(a.before_value,'$.kind')='manual' AND json_extract(a.before_value,'$.recordId') IN (SELECT value FROM json_each(?5))) OR (json_extract(a.after_value,'$.kind')='manual' AND json_extract(a.after_value,'$.recordId') IN (SELECT value FROM json_each(?5))) OR (json_extract(a.before_value,'$.kind')='segment' AND json_extract(a.before_value,'$.recordId') IN (SELECT value FROM json_each(?6))) OR (json_extract(a.after_value,'$.kind')='segment' AND json_extract(a.after_value,'$.recordId') IN (SELECT value FROM json_each(?6))) OR json_extract(a.before_value,'$.runId') IN (SELECT value FROM json_each(?7)) OR json_extract(a.after_value,'$.runId') IN (SELECT value FROM json_each(?7))))",&[
        Cell::uuid(actor),Cell::json(&ids(g.timer_runs.iter().map(|r|r.id).collect()))?,Cell::json(&ids(g.tasks.iter().map(|r|r.id).collect()))?,Cell::json(&ids(g.time_entries.iter().map(|r|r.id).collect()))?,Cell::json(&text_ids(g.time_entries.iter().map(|r|r.id).collect()))?,Cell::json(&text_ids(g.timer_segments.iter().map(|r|r.id).collect()))?,Cell::json(&text_ids(g.timer_runs.iter().map(|r|r.id).collect()))?
    ]).await?;
    if rows[0].cell(0)?.boolean()? {
        return Err(NativeDbError::Conflict);
    }
    if g.timer_runs.iter().any(|r| r.status != "stopped")
        || g.time_entries.iter().any(|r| r.ended_at.is_none())
    {
        let rows=tx.query("SELECT EXISTS(SELECT 1 FROM task_timer_runs WHERE user_id=?1 AND status<>'stopped') OR EXISTS(SELECT 1 FROM task_timer_legacy_open WHERE user_id=?1)",&[Cell::uuid(actor)]).await?;
        if rows[0].cell(0)?.boolean()? {
            return Err(NativeDbError::Conflict);
        }
    }
    Ok(())
}

pub async fn publish_backend(
    backend: &Backend,
    claim: &ImportClaim,
    archive: &Archive,
    keys: &BTreeMap<Uuid, String>,
    quota: &crate::db::quota::StorageQuota,
    expected_archive_hash: &str,
    cancel: &CancellationToken,
) -> Result<(), NativeDbError> {
    native_cancel(cancel)?;
    archive.validate()?;
    let mut tx = backend.begin_write().await?;
    let result=async {
        if let crate::db::backend::DbTransaction::Postgres(pg)=&mut tx {
            return publish_pg_in_tx(pg,claim,archive,keys,quota,Some(expected_archive_hash),cancel).await;
        }
        let mut op=tx.operation();op.set_tenant(claim.workspace_id).await?;
        op.native_destination(claim.workspace_id,claim.created_by,claim.session_id,true,true).await?;
        op.lock_tree(claim.workspace_id).await?;
        let hash=op.native_claim(claim).await?;if hash!=expected_archive_hash{return Err(NativeDbError::Conflict)}op.native_keys(claim,archive,keys).await?;
        let g=&archive.graph;let workspace=claim.workspace_id;let actor=claim.created_by;
        let OperationTx::SqliteFamily(family)=&mut op else {unreachable!()};
        family_time_guard(family,g,actor).await?;
        let used=family.query("SELECT coalesce(sum(reserved_size_bytes),0) FROM attachments WHERE workspace_id=?1",&[Cell::uuid(workspace)]).await?[0].cell(0)?.integer()?;
        let mut total=used;
        for file in &g.attachments{quota.check(total,file.size_bytes).map_err(|_|ArchiveError::Limit)?;total=total.checked_add(file.size_bytes).ok_or(ArchiveError::Limit)?;}
        let records=prepare_native_records(archive,keys,workspace,actor,claim.job_id)?;
        native_cancel(cancel)?;
        for (table,value) in records {native_cancel(cancel)?;native_family_record(family,table,value).await?;}
        op.install_archived_native_history(claim,archive).await?;
        let OperationTx::SqliteFamily(family)=&mut op else {unreachable!()};
        family.execute("INSERT INTO project_members(id,workspace_id,project_id,user_id,role) VALUES(?1,?2,?3,?4,'lead')",&[Cell::uuid(Uuid::now_v7()),Cell::uuid(workspace),Cell::uuid(g.project.id),Cell::uuid(actor)]).await?;
        family.execute("UPDATE projects SET root_document_id=?3 WHERE workspace_id=?1 AND id=?2",&[Cell::uuid(workspace),Cell::uuid(g.project.id),Cell::optional_uuid(g.project.root_document_id)]).await?;
        family.execute("UPDATE workspaces SET next_document_number=max(next_document_number,?2) WHERE id=?1",&[Cell::uuid(workspace),Cell::Integer(i64::from(g.documents.iter().map(|d|d.number).max().unwrap_or(0).checked_add(1).ok_or(ArchiveError::Limit)?))]).await?;
        for entry in g.time_entries.iter().filter(|e|e.ended_at.is_none()){
            if !g.timer_legacy_open.iter().any(|r|r.time_entry_id==entry.id){family.execute("DELETE FROM task_timer_legacy_open WHERE time_entry_id=?1 AND user_id=?2",&[Cell::uuid(entry.id),Cell::uuid(actor)]).await?;}
        }
        for row in &g.timer_legacy_open {
            let found=family.query("SELECT workspace_id,user_id,task_id FROM task_timer_legacy_open WHERE time_entry_id=?1",&[Cell::uuid(row.time_entry_id)]).await?;
            let row_current=found.first().ok_or(NativeDbError::Conflict)?;
            if row_current.cell(0)?.id()?!=workspace || row_current.cell(1)?.id()?!=actor || row_current.cell(2)?.id()?!=row.task_id{return Err(NativeDbError::Conflict)}
        }
        let result=native_result(g,claim,&hash);
        // Recheck current owner, session and full job before key transfer. Each
        // exact journal belongs to this attempt's current durable key refs.
        native_cancel(cancel)?;op.native_destination(workspace,actor,claim.session_id,true,false).await?;op.native_claim(claim).await?;op.native_keys(claim,archive,keys).await?;
        let OperationTx::SqliteFamily(family)=&mut op else {unreachable!()};
        for file in &g.attachments{let key=keys.get(&file.id).ok_or(NativeDbError::Fenced)?;if family.execute("DELETE FROM attachment_object_cleanups WHERE workspace_id=?1 AND attachment_id=?2 AND storage_key=?3",&[Cell::uuid(workspace),Cell::uuid(file.id),Cell::text(key)]).await?!=1{return Err(NativeDbError::Fenced)}}
        family.execute("UPDATE import_jobs SET native_result=?3,created_refs='{\"documentIds\":[],\"taskIds\":[],\"storedKeys\":[]}' WHERE workspace_id=?1 AND id=?2",&[Cell::uuid(workspace),Cell::uuid(claim.job_id),Cell::json(&result)?]).await?;
        append_native_events(&mut op,g,claim,result).await?;
        native_cancel(cancel)?;op.native_destination(workspace,actor,claim.session_id,true,false).await?;
        if !op.hold_import_claim(claim).await? || !op.finish_import_job(claim,crate::db::import_jobs::ImportStatus::Completed).await?{return Err(NativeDbError::Fenced)}
        #[cfg(all(test, feature = "db-tests"))]
        selected_tests::before_finish(&mut op,claim,cancel).await?;
        native_cancel(cancel)?;Ok(())
    }.await;
    finish_native_write(tx, result).await
}
fn native_result(g: &Graph, claim: &ImportClaim, hash: &str) -> Value {
    json!({"projectId":g.project.id,"archiveHash":hash,"sourceWorkspaceId":g.source_workspace_id,"sourceActorId":g.source_actor_id,"destinationActorId":claim.created_by,"attribution":"inert source provenance","documentIds":g.documents.iter().map(|d|d.id).collect::<Vec<_>>(),"taskIds":g.tasks.iter().map(|t|t.id).collect::<Vec<_>>(),"activityIds":g.activity.iter().map(|a|a.id).collect::<Vec<_>>(),"zoteroConnectorIds":g.zotero_connectors.iter().map(|c|c.id).collect::<Vec<_>>()})
}

fn native_json(column: &NativeColumn, cell: Cell) -> Result<Value, NativeDbError> {
    if cell == Cell::Null && column.nullable {
        return Ok(Value::Null);
    }
    Ok(match column.kind {
        NativeStorage::Uuid => json!(cell.id()?),
        NativeStorage::Instant => json!(cell.datetime()?.to_rfc3339()),
        NativeStorage::Date => json!(cell.date()?.format("%Y-%m-%d").to_string()),
        NativeStorage::Integer => json!(cell.integer()?),
        NativeStorage::Boolean => json!(cell.boolean()?),
        NativeStorage::Text => json!(cell.string()?),
        NativeStorage::Json => cell.value()?,
    })
}
fn native_scope_args(scope: &Scope) -> Result<Vec<Cell>, NativeDbError> {
    let ids = |ids: &[Uuid]| {
        json!(ids
            .iter()
            .map(|id| hex::encode(id.as_bytes()))
            .collect::<Vec<_>>())
    };
    Ok(vec![
        Cell::uuid(scope.project),
        Cell::json(&ids(&scope.wiki))?,
        Cell::uuid(scope.workspace),
        Cell::uuid(scope.actor),
        Cell::json(&ids(&scope.connectors))?,
    ])
}
async fn native_family_rows<T: DeserializeOwned>(
    tx: &mut FamilyTx,
    table: &str,
    scope: &Scope,
) -> Result<Vec<T>, NativeDbError> {
    let spec = native_read(table)?;
    let args = native_scope_args(scope)?;
    // Count and raw bytes on this exact snapshot before fetching any values.
    // A single oversized TEXT/BLOB cannot bypass the parent's cumulative cap.
    let budget = tx.query(spec.budget, &args[..spec.args]).await?;
    let count = budget[0].cell(0)?.integer()?;
    let raw = budget[0].cell(1)?.integer()?;
    if count > MAX_ENTRIES as i64
        || raw < 0
        || raw
            .checked_add(scope.graph_bytes.load(std::sync::atomic::Ordering::Relaxed))
            .is_none_or(|sum| sum > MAX_GRAPH_BYTES as i64)
    {
        return Err(ArchiveError::Limit.into());
    }
    let rows = tx.query(spec.sql, &args[..spec.args]).await?;
    let mut out = Vec::new();
    for row in rows {
        let mut value = serde_json::Map::new();
        for (index, col) in spec.columns.iter().enumerate() {
            value.insert(col.name.into(), native_json(col, row.cell(index)?)?);
        }
        let bytes = serde_json::to_vec(&value)
            .map_err(|_| ArchiveError::Invalid("native capture row".into()))?;
        admit_graph_bytes(scope, bytes.len())?;
        out.push(
            serde_json::from_value(Value::Object(value))
                .map_err(|_| ArchiveError::Unsupported("record schema".into()))?,
        );
    }
    Ok(out)
}
impl OperationTx<'_, '_> {
    async fn native_source(
        &mut self,
        workspace: Uuid,
        actor: Uuid,
        session: Uuid,
        selection: &Selection<'_>,
    ) -> Result<(), NativeDbError> {
        if let Self::Postgres(tx) = self {
            return source_gate(tx, workspace, actor, session, selection).await;
        }
        if !self.session_is_live(actor, session).await?
            || !self.workspace_is_live(workspace).await?
            || !self
                .membership_role(workspace, actor, false)
                .await?
                .is_some_and(|r| r.at_least(WorkspaceRole::Admin))
            || !self
                .project_permission_by_id(workspace, actor, selection.project)
                .await?
                .is_some_and(|p| p.at_least(ProjectPermission::View))
        {
            return Err(NativeDbError::Forbidden);
        }
        if selection.zotero_connectors.is_empty() {
            return Ok(());
        }
        if selection.zotero_connectors.len() > MAX_ZOTERO_CONNECTORS
            || selection
                .zotero_connectors
                .iter()
                .collect::<std::collections::BTreeSet<_>>()
                .len()
                != selection.zotero_connectors.len()
        {
            return Err(ArchiveError::Invalid("zotero connector selection".into()).into());
        }
        let Self::SqliteFamily(tx) = self else {
            unreachable!()
        };
        let ids = json!(selection
            .zotero_connectors
            .iter()
            .map(|id| hex::encode(id.as_bytes()))
            .collect::<Vec<_>>());
        let rows=tx.query("SELECT (SELECT count(*) FROM zotero_connectors WHERE workspace_id=?1 AND owner_user_id=?2 AND lower(hex(id)) IN(SELECT value FROM json_each(?3))),EXISTS(SELECT 1 FROM workspaces w JOIN users u ON u.personal_workspace_id=w.id WHERE w.id=?1 AND w.kind='personal' AND w.deleted_at IS NULL AND u.id=?2)",&[Cell::uuid(workspace),Cell::uuid(actor),Cell::json(&ids)?]).await?;
        if rows[0].cell(0)?.integer()? != selection.zotero_connectors.len() as i64
            || !rows[0].cell(1)?.boolean()?
        {
            return Err(NativeDbError::Forbidden);
        }
        Ok(())
    }
}
pub async fn recheck_source_backend(
    backend: &Backend,
    workspace: Uuid,
    actor: Uuid,
    session: Uuid,
    selection: &Selection<'_>,
) -> Result<(), NativeDbError> {
    let mut tx = backend.begin_read().await?;
    let result = async {
        let mut op = tx.operation();
        op.set_tenant(workspace).await?;
        op.native_source(workspace, actor, session, selection).await
    }
    .await;
    finish_native_read(tx, result).await
}

async fn family_wiki_closure(tx: &mut FamilyTx, scope: &Scope) -> Result<Vec<Uuid>, NativeDbError> {
    let ids = Cell::json(&json!(scope
        .connectors
        .iter()
        .map(|id| hex::encode(id.as_bytes()))
        .collect::<Vec<_>>()))?;
    let backing=tx.query("SELECT d.id,d.project_id,d.path FROM zotero_references r JOIN documents d ON d.workspace_id=r.workspace_id AND d.id=r.document_id WHERE r.workspace_id=?1 AND r.owner_user_id=?2 AND lower(hex(r.connector_id)) IN(SELECT value FROM json_each(?3)) ORDER BY r.document_id LIMIT 257",&[Cell::uuid(scope.workspace),Cell::uuid(scope.actor),ids.clone()]).await?;
    let origins=tx.query("SELECT d.id,d.project_id,d.path FROM task_origins o JOIN tasks t ON t.workspace_id=o.workspace_id AND t.id=o.task_id JOIN documents d ON d.workspace_id=o.workspace_id AND d.id=o.document_id WHERE t.workspace_id=?1 AND t.project_id=?2 AND d.project_id IS NOT ?2 ORDER BY d.id LIMIT 257",&[Cell::uuid(scope.workspace),Cell::uuid(scope.project)]).await?;
    if backing.len() > MAX_OBJECTS || origins.len() > MAX_OBJECTS {
        return Err(ArchiveError::Limit.into());
    }
    let mut closure = std::collections::BTreeSet::new();
    for row in backing.into_iter().chain(origins) {
        if row.cell(1)?.optional(Cell::id)?.is_some() {
            return Err(ArchiveError::Unsupported("wiki closure".into()).into());
        }
        for label in row.cell(2)?.string()?.split('.') {
            closure.insert(
                Uuid::try_parse(label).map_err(|_| ArchiveError::Invalid("wiki path".into()))?,
            );
        }
    }
    if closure.len() > MAX_OBJECTS {
        return Err(ArchiveError::Limit.into());
    }
    let closure = closure.into_iter().collect::<Vec<_>>();
    let selected = Cell::json(&json!(closure
        .iter()
        .map(|id| hex::encode(id.as_bytes()))
        .collect::<Vec<_>>()))?;
    let rows=tx.query("SELECT (SELECT count(*) FROM documents WHERE workspace_id=?1 AND project_id IS NULL AND lower(hex(id)) IN(SELECT value FROM json_each(?2))),EXISTS(SELECT 1 FROM documents WHERE workspace_id=?1 AND lower(hex(parent_id)) IN(SELECT value FROM json_each(?2)) AND lower(hex(id)) NOT IN(SELECT value FROM json_each(?2))),EXISTS(SELECT 1 FROM zotero_references WHERE workspace_id=?1 AND lower(hex(document_id)) IN(SELECT value FROM json_each(?2)) AND lower(hex(connector_id)) NOT IN(SELECT value FROM json_each(?3)))",&[Cell::uuid(scope.workspace),selected,ids]).await?;
    if rows[0].cell(0)?.integer()? != closure.len() as i64
        || rows[0].cell(1)?.boolean()?
        || rows[0].cell(2)?.boolean()?
    {
        return Err(ArchiveError::Unsupported("wiki closure".into()).into());
    }
    Ok(closure)
}

struct NativeRead {
    sql: &'static str,
    budget: &'static str,
    columns: &'static [NativeColumn],
    args: usize,
}
fn native_read(table: &str) -> Result<NativeRead, NativeDbError> {
    use NativeStorage::*;
    Ok(match table{
"projects"=>NativeRead{args:3,sql:"SELECT r.\"id\",r.\"key\",r.\"name\",r.\"description\",r.\"icon\",r.\"visibility\",r.\"root_document_id\",r.\"status\",r.\"next_number\",r.\"created_by\",r.\"created_at\",r.\"updated_at\",r.\"deleted_at\" FROM projects r WHERE (r.workspace_id=?3 AND r.id=?1 AND r.deleted_at IS NULL) ORDER BY r.id LIMIT 10001",budget:"SELECT count(*),coalesce(sum(coalesce(length(CAST(\"id\" AS BLOB)),0)+coalesce(length(CAST(\"key\" AS BLOB)),0)+coalesce(length(CAST(\"name\" AS BLOB)),0)+coalesce(length(CAST(\"description\" AS BLOB)),0)+coalesce(length(CAST(\"icon\" AS BLOB)),0)+coalesce(length(CAST(\"visibility\" AS BLOB)),0)+coalesce(length(CAST(\"root_document_id\" AS BLOB)),0)+coalesce(length(CAST(\"status\" AS BLOB)),0)+coalesce(length(CAST(\"next_number\" AS BLOB)),0)+coalesce(length(CAST(\"created_by\" AS BLOB)),0)+coalesce(length(CAST(\"created_at\" AS BLOB)),0)+coalesce(length(CAST(\"updated_at\" AS BLOB)),0)+coalesce(length(CAST(\"deleted_at\" AS BLOB)),0)),0) FROM (SELECT r.\"id\",r.\"key\",r.\"name\",r.\"description\",r.\"icon\",r.\"visibility\",r.\"root_document_id\",r.\"status\",r.\"next_number\",r.\"created_by\",r.\"created_at\",r.\"updated_at\",r.\"deleted_at\" FROM projects r WHERE (r.workspace_id=?3 AND r.id=?1 AND r.deleted_at IS NULL) ORDER BY r.id LIMIT 10001)",columns:&[NativeColumn{name:"id",kind:Uuid,nullable:false}, NativeColumn{name:"key",kind:Text,nullable:false}, NativeColumn{name:"name",kind:Text,nullable:false}, NativeColumn{name:"description",kind:Text,nullable:true}, NativeColumn{name:"icon",kind:Text,nullable:true}, NativeColumn{name:"visibility",kind:Text,nullable:false}, NativeColumn{name:"root_document_id",kind:Uuid,nullable:true}, NativeColumn{name:"status",kind:Text,nullable:false}, NativeColumn{name:"next_number",kind:Integer,nullable:false}, NativeColumn{name:"created_by",kind:Uuid,nullable:false}, NativeColumn{name:"created_at",kind:Instant,nullable:false}, NativeColumn{name:"updated_at",kind:Instant,nullable:false}, NativeColumn{name:"deleted_at",kind:Instant,nullable:true}]},
"workflows"=>NativeRead{args:3,sql:"SELECT r.\"id\",r.\"project_id\",r.\"created_at\",r.\"updated_at\" FROM workflows r WHERE (r.workspace_id=?3 AND r.project_id=?1) ORDER BY r.id LIMIT 10001",budget:"SELECT count(*),coalesce(sum(coalesce(length(CAST(\"id\" AS BLOB)),0)+coalesce(length(CAST(\"project_id\" AS BLOB)),0)+coalesce(length(CAST(\"created_at\" AS BLOB)),0)+coalesce(length(CAST(\"updated_at\" AS BLOB)),0)),0) FROM (SELECT r.\"id\",r.\"project_id\",r.\"created_at\",r.\"updated_at\" FROM workflows r WHERE (r.workspace_id=?3 AND r.project_id=?1) ORDER BY r.id LIMIT 10001)",columns:&[NativeColumn{name:"id",kind:Uuid,nullable:false}, NativeColumn{name:"project_id",kind:Uuid,nullable:false}, NativeColumn{name:"created_at",kind:Instant,nullable:false}, NativeColumn{name:"updated_at",kind:Instant,nullable:false}]},
"statuses"=>NativeRead{args:3,sql:"SELECT r.\"id\",r.\"project_id\",r.\"workflow_id\",r.\"name\",r.\"category\",r.\"sort_key\",r.\"wip_limit\",r.\"created_at\",r.\"updated_at\" FROM statuses r WHERE (r.workspace_id=?3 AND r.project_id=?1) ORDER BY r.sort_key COLLATE BINARY,r.id LIMIT 10001",budget:"SELECT count(*),coalesce(sum(coalesce(length(CAST(\"id\" AS BLOB)),0)+coalesce(length(CAST(\"project_id\" AS BLOB)),0)+coalesce(length(CAST(\"workflow_id\" AS BLOB)),0)+coalesce(length(CAST(\"name\" AS BLOB)),0)+coalesce(length(CAST(\"category\" AS BLOB)),0)+coalesce(length(CAST(\"sort_key\" AS BLOB)),0)+coalesce(length(CAST(\"wip_limit\" AS BLOB)),0)+coalesce(length(CAST(\"created_at\" AS BLOB)),0)+coalesce(length(CAST(\"updated_at\" AS BLOB)),0)),0) FROM (SELECT r.\"id\",r.\"project_id\",r.\"workflow_id\",r.\"name\",r.\"category\",r.\"sort_key\",r.\"wip_limit\",r.\"created_at\",r.\"updated_at\" FROM statuses r WHERE (r.workspace_id=?3 AND r.project_id=?1) ORDER BY r.sort_key COLLATE BINARY,r.id LIMIT 10001)",columns:&[NativeColumn{name:"id",kind:Uuid,nullable:false}, NativeColumn{name:"project_id",kind:Uuid,nullable:false}, NativeColumn{name:"workflow_id",kind:Uuid,nullable:false}, NativeColumn{name:"name",kind:Text,nullable:false}, NativeColumn{name:"category",kind:Text,nullable:false}, NativeColumn{name:"sort_key",kind:Text,nullable:false}, NativeColumn{name:"wip_limit",kind:Integer,nullable:true}, NativeColumn{name:"created_at",kind:Instant,nullable:false}, NativeColumn{name:"updated_at",kind:Instant,nullable:false}]},
"documents"=>NativeRead{args:3,sql:"SELECT r.\"id\",r.\"title\",r.\"icon\",r.\"path\",r.\"parent_id\",r.\"sort_key\",r.\"project_id\",r.\"number\",r.\"status\",r.\"schema_version\",r.\"text\",r.\"chosung\",r.\"version\",r.\"created_by\",r.\"created_at\",r.\"updated_at\",r.\"deleted_at\",r.\"content_json\",r.\"kind\" FROM documents r WHERE (r.workspace_id=?3 AND r.id IN (SELECT id FROM documents WHERE workspace_id=?3 AND (project_id=?1 OR (project_id IS NULL AND lower(hex(id)) IN (SELECT value FROM json_each(?2)))))) ORDER BY r.path COLLATE BINARY,r.id LIMIT 10001",budget:"SELECT count(*),coalesce(sum(coalesce(length(CAST(\"id\" AS BLOB)),0)+coalesce(length(CAST(\"title\" AS BLOB)),0)+coalesce(length(CAST(\"icon\" AS BLOB)),0)+coalesce(length(CAST(\"path\" AS BLOB)),0)+coalesce(length(CAST(\"parent_id\" AS BLOB)),0)+coalesce(length(CAST(\"sort_key\" AS BLOB)),0)+coalesce(length(CAST(\"project_id\" AS BLOB)),0)+coalesce(length(CAST(\"number\" AS BLOB)),0)+coalesce(length(CAST(\"status\" AS BLOB)),0)+coalesce(length(CAST(\"schema_version\" AS BLOB)),0)+coalesce(length(CAST(\"text\" AS BLOB)),0)+coalesce(length(CAST(\"chosung\" AS BLOB)),0)+coalesce(length(CAST(\"version\" AS BLOB)),0)+coalesce(length(CAST(\"created_by\" AS BLOB)),0)+coalesce(length(CAST(\"created_at\" AS BLOB)),0)+coalesce(length(CAST(\"updated_at\" AS BLOB)),0)+coalesce(length(CAST(\"deleted_at\" AS BLOB)),0)+coalesce(length(CAST(\"content_json\" AS BLOB)),0)+coalesce(length(CAST(\"kind\" AS BLOB)),0)),0) FROM (SELECT r.\"id\",r.\"title\",r.\"icon\",r.\"path\",r.\"parent_id\",r.\"sort_key\",r.\"project_id\",r.\"number\",r.\"status\",r.\"schema_version\",r.\"text\",r.\"chosung\",r.\"version\",r.\"created_by\",r.\"created_at\",r.\"updated_at\",r.\"deleted_at\",r.\"content_json\",r.\"kind\" FROM documents r WHERE (r.workspace_id=?3 AND r.id IN (SELECT id FROM documents WHERE workspace_id=?3 AND (project_id=?1 OR (project_id IS NULL AND lower(hex(id)) IN (SELECT value FROM json_each(?2)))))) ORDER BY r.path COLLATE BINARY,r.id LIMIT 10001)",columns:&[NativeColumn{name:"id",kind:Uuid,nullable:false}, NativeColumn{name:"title",kind:Text,nullable:false}, NativeColumn{name:"icon",kind:Text,nullable:true}, NativeColumn{name:"path",kind:Text,nullable:false}, NativeColumn{name:"parent_id",kind:Uuid,nullable:true}, NativeColumn{name:"sort_key",kind:Text,nullable:false}, NativeColumn{name:"project_id",kind:Uuid,nullable:true}, NativeColumn{name:"number",kind:Integer,nullable:false}, NativeColumn{name:"status",kind:Text,nullable:false}, NativeColumn{name:"schema_version",kind:Integer,nullable:false}, NativeColumn{name:"text",kind:Text,nullable:false}, NativeColumn{name:"chosung",kind:Text,nullable:false}, NativeColumn{name:"version",kind:Integer,nullable:false}, NativeColumn{name:"created_by",kind:Uuid,nullable:false}, NativeColumn{name:"created_at",kind:Instant,nullable:false}, NativeColumn{name:"updated_at",kind:Instant,nullable:false}, NativeColumn{name:"deleted_at",kind:Instant,nullable:true}, NativeColumn{name:"content_json",kind:Json,nullable:false}, NativeColumn{name:"kind",kind:Text,nullable:false}]},
"tasks"=>NativeRead{args:3,sql:"SELECT r.\"id\",r.\"project_id\",r.\"number\",r.\"title\",r.\"type\",r.\"priority\",r.\"status_id\",r.\"start_date\",r.\"due_date\",r.\"due_at\",r.\"estimate\",r.\"parent_id\",r.\"milestone_id\",r.\"recurrence\",r.\"sort_key\",r.\"schema_version\",r.\"content_json\",r.\"version\",r.\"archived_at\",r.\"deleted_at\",r.\"created_by\",r.\"created_at\",r.\"updated_at\",r.\"text\",r.\"chosung\",r.\"estimate_unit\" FROM tasks r WHERE (r.workspace_id=?3 AND r.project_id=?1) ORDER BY r.id LIMIT 10001",budget:"SELECT count(*),coalesce(sum(coalesce(length(CAST(\"id\" AS BLOB)),0)+coalesce(length(CAST(\"project_id\" AS BLOB)),0)+coalesce(length(CAST(\"number\" AS BLOB)),0)+coalesce(length(CAST(\"title\" AS BLOB)),0)+coalesce(length(CAST(\"type\" AS BLOB)),0)+coalesce(length(CAST(\"priority\" AS BLOB)),0)+coalesce(length(CAST(\"status_id\" AS BLOB)),0)+coalesce(length(CAST(\"start_date\" AS BLOB)),0)+coalesce(length(CAST(\"due_date\" AS BLOB)),0)+coalesce(length(CAST(\"due_at\" AS BLOB)),0)+coalesce(length(CAST(\"estimate\" AS BLOB)),0)+coalesce(length(CAST(\"parent_id\" AS BLOB)),0)+coalesce(length(CAST(\"milestone_id\" AS BLOB)),0)+coalesce(length(CAST(\"recurrence\" AS BLOB)),0)+coalesce(length(CAST(\"sort_key\" AS BLOB)),0)+coalesce(length(CAST(\"schema_version\" AS BLOB)),0)+coalesce(length(CAST(\"content_json\" AS BLOB)),0)+coalesce(length(CAST(\"version\" AS BLOB)),0)+coalesce(length(CAST(\"archived_at\" AS BLOB)),0)+coalesce(length(CAST(\"deleted_at\" AS BLOB)),0)+coalesce(length(CAST(\"created_by\" AS BLOB)),0)+coalesce(length(CAST(\"created_at\" AS BLOB)),0)+coalesce(length(CAST(\"updated_at\" AS BLOB)),0)+coalesce(length(CAST(\"text\" AS BLOB)),0)+coalesce(length(CAST(\"chosung\" AS BLOB)),0)+coalesce(length(CAST(\"estimate_unit\" AS BLOB)),0)),0) FROM (SELECT r.\"id\",r.\"project_id\",r.\"number\",r.\"title\",r.\"type\",r.\"priority\",r.\"status_id\",r.\"start_date\",r.\"due_date\",r.\"due_at\",r.\"estimate\",r.\"parent_id\",r.\"milestone_id\",r.\"recurrence\",r.\"sort_key\",r.\"schema_version\",r.\"content_json\",r.\"version\",r.\"archived_at\",r.\"deleted_at\",r.\"created_by\",r.\"created_at\",r.\"updated_at\",r.\"text\",r.\"chosung\",r.\"estimate_unit\" FROM tasks r WHERE (r.workspace_id=?3 AND r.project_id=?1) ORDER BY r.id LIMIT 10001)",columns:&[NativeColumn{name:"id",kind:Uuid,nullable:false}, NativeColumn{name:"project_id",kind:Uuid,nullable:false}, NativeColumn{name:"number",kind:Integer,nullable:false}, NativeColumn{name:"title",kind:Text,nullable:false}, NativeColumn{name:"type",kind:Text,nullable:false}, NativeColumn{name:"priority",kind:Text,nullable:false}, NativeColumn{name:"status_id",kind:Uuid,nullable:false}, NativeColumn{name:"start_date",kind:Date,nullable:true}, NativeColumn{name:"due_date",kind:Date,nullable:true}, NativeColumn{name:"due_at",kind:Instant,nullable:true}, NativeColumn{name:"estimate",kind:Text,nullable:true}, NativeColumn{name:"parent_id",kind:Uuid,nullable:true}, NativeColumn{name:"milestone_id",kind:Uuid,nullable:true}, NativeColumn{name:"recurrence",kind:Json,nullable:true}, NativeColumn{name:"sort_key",kind:Text,nullable:false}, NativeColumn{name:"schema_version",kind:Integer,nullable:false}, NativeColumn{name:"content_json",kind:Json,nullable:false}, NativeColumn{name:"version",kind:Integer,nullable:false}, NativeColumn{name:"archived_at",kind:Instant,nullable:true}, NativeColumn{name:"deleted_at",kind:Instant,nullable:true}, NativeColumn{name:"created_by",kind:Uuid,nullable:false}, NativeColumn{name:"created_at",kind:Instant,nullable:false}, NativeColumn{name:"updated_at",kind:Instant,nullable:false}, NativeColumn{name:"text",kind:Text,nullable:false}, NativeColumn{name:"chosung",kind:Text,nullable:false}, NativeColumn{name:"estimate_unit",kind:Text,nullable:true}]},
"task_assignees"=>NativeRead{args:3,sql:"SELECT r.\"task_id\",r.\"user_id\" FROM task_assignees r WHERE (r.workspace_id=?3 AND r.task_id IN (SELECT id FROM tasks WHERE workspace_id=?3 AND project_id=?1)) ORDER BY r.task_id,r.user_id LIMIT 10001",budget:"SELECT count(*),coalesce(sum(coalesce(length(CAST(\"task_id\" AS BLOB)),0)+coalesce(length(CAST(\"user_id\" AS BLOB)),0)),0) FROM (SELECT r.\"task_id\",r.\"user_id\" FROM task_assignees r WHERE (r.workspace_id=?3 AND r.task_id IN (SELECT id FROM tasks WHERE workspace_id=?3 AND project_id=?1)) ORDER BY r.task_id,r.user_id LIMIT 10001)",columns:&[NativeColumn{name:"task_id",kind:Uuid,nullable:false}, NativeColumn{name:"user_id",kind:Uuid,nullable:false}]},
"labels"=>NativeRead{args:3,sql:"SELECT r.\"id\",r.\"project_id\",r.\"name\",r.\"color\",r.\"created_at\",r.\"updated_at\" FROM labels r WHERE (r.workspace_id=?3 AND r.project_id=?1) ORDER BY r.id LIMIT 10001",budget:"SELECT count(*),coalesce(sum(coalesce(length(CAST(\"id\" AS BLOB)),0)+coalesce(length(CAST(\"project_id\" AS BLOB)),0)+coalesce(length(CAST(\"name\" AS BLOB)),0)+coalesce(length(CAST(\"color\" AS BLOB)),0)+coalesce(length(CAST(\"created_at\" AS BLOB)),0)+coalesce(length(CAST(\"updated_at\" AS BLOB)),0)),0) FROM (SELECT r.\"id\",r.\"project_id\",r.\"name\",r.\"color\",r.\"created_at\",r.\"updated_at\" FROM labels r WHERE (r.workspace_id=?3 AND r.project_id=?1) ORDER BY r.id LIMIT 10001)",columns:&[NativeColumn{name:"id",kind:Uuid,nullable:false}, NativeColumn{name:"project_id",kind:Uuid,nullable:false}, NativeColumn{name:"name",kind:Text,nullable:false}, NativeColumn{name:"color",kind:Text,nullable:false}, NativeColumn{name:"created_at",kind:Instant,nullable:false}, NativeColumn{name:"updated_at",kind:Instant,nullable:false}]},
"task_labels"=>NativeRead{args:3,sql:"SELECT r.\"task_id\",r.\"label_id\" FROM task_labels r WHERE (r.workspace_id=?3 AND r.task_id IN (SELECT id FROM tasks WHERE workspace_id=?3 AND project_id=?1)) ORDER BY r.task_id,r.label_id LIMIT 10001",budget:"SELECT count(*),coalesce(sum(coalesce(length(CAST(\"task_id\" AS BLOB)),0)+coalesce(length(CAST(\"label_id\" AS BLOB)),0)),0) FROM (SELECT r.\"task_id\",r.\"label_id\" FROM task_labels r WHERE (r.workspace_id=?3 AND r.task_id IN (SELECT id FROM tasks WHERE workspace_id=?3 AND project_id=?1)) ORDER BY r.task_id,r.label_id LIMIT 10001)",columns:&[NativeColumn{name:"task_id",kind:Uuid,nullable:false}, NativeColumn{name:"label_id",kind:Uuid,nullable:false}]},
"milestones"=>NativeRead{args:3,sql:"SELECT r.\"id\",r.\"project_id\",r.\"name\",r.\"due_date\",r.\"sort_key\",r.\"created_at\",r.\"updated_at\" FROM milestones r WHERE (r.workspace_id=?3 AND r.project_id=?1) ORDER BY r.id LIMIT 10001",budget:"SELECT count(*),coalesce(sum(coalesce(length(CAST(\"id\" AS BLOB)),0)+coalesce(length(CAST(\"project_id\" AS BLOB)),0)+coalesce(length(CAST(\"name\" AS BLOB)),0)+coalesce(length(CAST(\"due_date\" AS BLOB)),0)+coalesce(length(CAST(\"sort_key\" AS BLOB)),0)+coalesce(length(CAST(\"created_at\" AS BLOB)),0)+coalesce(length(CAST(\"updated_at\" AS BLOB)),0)),0) FROM (SELECT r.\"id\",r.\"project_id\",r.\"name\",r.\"due_date\",r.\"sort_key\",r.\"created_at\",r.\"updated_at\" FROM milestones r WHERE (r.workspace_id=?3 AND r.project_id=?1) ORDER BY r.id LIMIT 10001)",columns:&[NativeColumn{name:"id",kind:Uuid,nullable:false}, NativeColumn{name:"project_id",kind:Uuid,nullable:false}, NativeColumn{name:"name",kind:Text,nullable:false}, NativeColumn{name:"due_date",kind:Date,nullable:true}, NativeColumn{name:"sort_key",kind:Text,nullable:false}, NativeColumn{name:"created_at",kind:Instant,nullable:false}, NativeColumn{name:"updated_at",kind:Instant,nullable:false}]},
"task_dependencies"=>NativeRead{args:3,sql:"SELECT r.\"blocker_id\",r.\"blocked_id\",r.\"type\",r.\"lag_days\" FROM task_dependencies r WHERE (r.workspace_id=?3 AND (r.blocker_id IN (SELECT id FROM tasks WHERE workspace_id=?3 AND project_id=?1) OR r.blocked_id IN (SELECT id FROM tasks WHERE workspace_id=?3 AND project_id=?1))) ORDER BY r.blocker_id,r.blocked_id LIMIT 10001",budget:"SELECT count(*),coalesce(sum(coalesce(length(CAST(\"blocker_id\" AS BLOB)),0)+coalesce(length(CAST(\"blocked_id\" AS BLOB)),0)+coalesce(length(CAST(\"type\" AS BLOB)),0)+coalesce(length(CAST(\"lag_days\" AS BLOB)),0)),0) FROM (SELECT r.\"blocker_id\",r.\"blocked_id\",r.\"type\",r.\"lag_days\" FROM task_dependencies r WHERE (r.workspace_id=?3 AND (r.blocker_id IN (SELECT id FROM tasks WHERE workspace_id=?3 AND project_id=?1) OR r.blocked_id IN (SELECT id FROM tasks WHERE workspace_id=?3 AND project_id=?1))) ORDER BY r.blocker_id,r.blocked_id LIMIT 10001)",columns:&[NativeColumn{name:"blocker_id",kind:Uuid,nullable:false}, NativeColumn{name:"blocked_id",kind:Uuid,nullable:false}, NativeColumn{name:"type",kind:Text,nullable:false}, NativeColumn{name:"lag_days",kind:Integer,nullable:false}]},
"document_tags"=>NativeRead{args:3,sql:"SELECT r.\"id\",r.\"name\",r.\"color\",r.\"created_at\",r.\"updated_at\" FROM document_tags r WHERE (r.workspace_id=?3 AND r.id IN(SELECT tag_id FROM document_tag_assignments WHERE workspace_id=?3 AND document_id IN (SELECT id FROM documents WHERE workspace_id=?3 AND (project_id=?1 OR (project_id IS NULL AND lower(hex(id)) IN (SELECT value FROM json_each(?2))))))) ORDER BY r.id LIMIT 10001",budget:"SELECT count(*),coalesce(sum(coalesce(length(CAST(\"id\" AS BLOB)),0)+coalesce(length(CAST(\"name\" AS BLOB)),0)+coalesce(length(CAST(\"color\" AS BLOB)),0)+coalesce(length(CAST(\"created_at\" AS BLOB)),0)+coalesce(length(CAST(\"updated_at\" AS BLOB)),0)),0) FROM (SELECT r.\"id\",r.\"name\",r.\"color\",r.\"created_at\",r.\"updated_at\" FROM document_tags r WHERE (r.workspace_id=?3 AND r.id IN(SELECT tag_id FROM document_tag_assignments WHERE workspace_id=?3 AND document_id IN (SELECT id FROM documents WHERE workspace_id=?3 AND (project_id=?1 OR (project_id IS NULL AND lower(hex(id)) IN (SELECT value FROM json_each(?2))))))) ORDER BY r.id LIMIT 10001)",columns:&[NativeColumn{name:"id",kind:Uuid,nullable:false}, NativeColumn{name:"name",kind:Text,nullable:false}, NativeColumn{name:"color",kind:Text,nullable:false}, NativeColumn{name:"created_at",kind:Instant,nullable:false}, NativeColumn{name:"updated_at",kind:Instant,nullable:false}]},
"document_tag_assignments"=>NativeRead{args:3,sql:"SELECT r.\"document_id\",r.\"tag_id\" FROM document_tag_assignments r WHERE (r.workspace_id=?3 AND r.document_id IN (SELECT id FROM documents WHERE workspace_id=?3 AND (project_id=?1 OR (project_id IS NULL AND lower(hex(id)) IN (SELECT value FROM json_each(?2)))))) ORDER BY r.document_id,r.tag_id LIMIT 10001",budget:"SELECT count(*),coalesce(sum(coalesce(length(CAST(\"document_id\" AS BLOB)),0)+coalesce(length(CAST(\"tag_id\" AS BLOB)),0)),0) FROM (SELECT r.\"document_id\",r.\"tag_id\" FROM document_tag_assignments r WHERE (r.workspace_id=?3 AND r.document_id IN (SELECT id FROM documents WHERE workspace_id=?3 AND (project_id=?1 OR (project_id IS NULL AND lower(hex(id)) IN (SELECT value FROM json_each(?2)))))) ORDER BY r.document_id,r.tag_id LIMIT 10001)",columns:&[NativeColumn{name:"document_id",kind:Uuid,nullable:false}, NativeColumn{name:"tag_id",kind:Uuid,nullable:false}]},
"views"=>NativeRead{args:3,sql:"SELECT r.\"id\",r.\"project_id\",r.\"user_id\",r.\"name\",r.\"type\",r.\"config\",r.\"created_at\",r.\"updated_at\" FROM views r WHERE (r.workspace_id=?3 AND r.project_id=?1) ORDER BY r.id LIMIT 10001",budget:"SELECT count(*),coalesce(sum(coalesce(length(CAST(\"id\" AS BLOB)),0)+coalesce(length(CAST(\"project_id\" AS BLOB)),0)+coalesce(length(CAST(\"user_id\" AS BLOB)),0)+coalesce(length(CAST(\"name\" AS BLOB)),0)+coalesce(length(CAST(\"type\" AS BLOB)),0)+coalesce(length(CAST(\"config\" AS BLOB)),0)+coalesce(length(CAST(\"created_at\" AS BLOB)),0)+coalesce(length(CAST(\"updated_at\" AS BLOB)),0)),0) FROM (SELECT r.\"id\",r.\"project_id\",r.\"user_id\",r.\"name\",r.\"type\",r.\"config\",r.\"created_at\",r.\"updated_at\" FROM views r WHERE (r.workspace_id=?3 AND r.project_id=?1) ORDER BY r.id LIMIT 10001)",columns:&[NativeColumn{name:"id",kind:Uuid,nullable:false}, NativeColumn{name:"project_id",kind:Uuid,nullable:false}, NativeColumn{name:"user_id",kind:Uuid,nullable:false}, NativeColumn{name:"name",kind:Text,nullable:false}, NativeColumn{name:"type",kind:Text,nullable:false}, NativeColumn{name:"config",kind:Json,nullable:false}, NativeColumn{name:"created_at",kind:Instant,nullable:false}, NativeColumn{name:"updated_at",kind:Instant,nullable:false}]},
"comments"=>NativeRead{args:3,sql:"SELECT r.\"id\",r.\"document_id\",r.\"task_id\",r.\"parent_id\",r.\"created_by\",r.\"body\",r.\"chosung\",r.\"resolved_at\",r.\"reactions\",r.\"created_at\",r.\"updated_at\" FROM comments r WHERE (r.workspace_id=?3 AND (r.document_id IN (SELECT id FROM documents WHERE workspace_id=?3 AND (project_id=?1 OR (project_id IS NULL AND lower(hex(id)) IN (SELECT value FROM json_each(?2))))) OR r.task_id IN (SELECT id FROM tasks WHERE workspace_id=?3 AND project_id=?1))) ORDER BY r.created_at,r.id LIMIT 10001",budget:"SELECT count(*),coalesce(sum(coalesce(length(CAST(\"id\" AS BLOB)),0)+coalesce(length(CAST(\"document_id\" AS BLOB)),0)+coalesce(length(CAST(\"task_id\" AS BLOB)),0)+coalesce(length(CAST(\"parent_id\" AS BLOB)),0)+coalesce(length(CAST(\"created_by\" AS BLOB)),0)+coalesce(length(CAST(\"body\" AS BLOB)),0)+coalesce(length(CAST(\"chosung\" AS BLOB)),0)+coalesce(length(CAST(\"resolved_at\" AS BLOB)),0)+coalesce(length(CAST(\"reactions\" AS BLOB)),0)+coalesce(length(CAST(\"created_at\" AS BLOB)),0)+coalesce(length(CAST(\"updated_at\" AS BLOB)),0)),0) FROM (SELECT r.\"id\",r.\"document_id\",r.\"task_id\",r.\"parent_id\",r.\"created_by\",r.\"body\",r.\"chosung\",r.\"resolved_at\",r.\"reactions\",r.\"created_at\",r.\"updated_at\" FROM comments r WHERE (r.workspace_id=?3 AND (r.document_id IN (SELECT id FROM documents WHERE workspace_id=?3 AND (project_id=?1 OR (project_id IS NULL AND lower(hex(id)) IN (SELECT value FROM json_each(?2))))) OR r.task_id IN (SELECT id FROM tasks WHERE workspace_id=?3 AND project_id=?1))) ORDER BY r.created_at,r.id LIMIT 10001)",columns:&[NativeColumn{name:"id",kind:Uuid,nullable:false}, NativeColumn{name:"document_id",kind:Uuid,nullable:true}, NativeColumn{name:"task_id",kind:Uuid,nullable:true}, NativeColumn{name:"parent_id",kind:Uuid,nullable:true}, NativeColumn{name:"created_by",kind:Uuid,nullable:false}, NativeColumn{name:"body",kind:Text,nullable:false}, NativeColumn{name:"chosung",kind:Text,nullable:false}, NativeColumn{name:"resolved_at",kind:Instant,nullable:true}, NativeColumn{name:"reactions",kind:Json,nullable:false}, NativeColumn{name:"created_at",kind:Instant,nullable:false}, NativeColumn{name:"updated_at",kind:Instant,nullable:false}]},
"task_origins"=>NativeRead{args:3,sql:"SELECT r.\"task_id\",r.\"document_id\",r.\"request_id\",r.\"request_hash\",r.\"anchor\",r.\"created_at\",r.\"updated_at\" FROM task_origins r WHERE (r.workspace_id=?3 AND r.task_id IN (SELECT id FROM tasks WHERE workspace_id=?3 AND project_id=?1)) ORDER BY r.task_id LIMIT 10001",budget:"SELECT count(*),coalesce(sum(coalesce(length(CAST(\"task_id\" AS BLOB)),0)+coalesce(length(CAST(\"document_id\" AS BLOB)),0)+coalesce(length(CAST(\"request_id\" AS BLOB)),0)+coalesce(length(CAST(\"request_hash\" AS BLOB)),0)+coalesce(length(CAST(\"anchor\" AS BLOB)),0)+coalesce(length(CAST(\"created_at\" AS BLOB)),0)+coalesce(length(CAST(\"updated_at\" AS BLOB)),0)),0) FROM (SELECT r.\"task_id\",r.\"document_id\",r.\"request_id\",r.\"request_hash\",r.\"anchor\",r.\"created_at\",r.\"updated_at\" FROM task_origins r WHERE (r.workspace_id=?3 AND r.task_id IN (SELECT id FROM tasks WHERE workspace_id=?3 AND project_id=?1)) ORDER BY r.task_id LIMIT 10001)",columns:&[NativeColumn{name:"task_id",kind:Uuid,nullable:false}, NativeColumn{name:"document_id",kind:Uuid,nullable:false}, NativeColumn{name:"request_id",kind:Uuid,nullable:false}, NativeColumn{name:"request_hash",kind:Text,nullable:false}, NativeColumn{name:"anchor",kind:Text,nullable:true}, NativeColumn{name:"created_at",kind:Instant,nullable:false}, NativeColumn{name:"updated_at",kind:Instant,nullable:false}]},
"task_activity"=>NativeRead{args:3,sql:"SELECT r.\"id\",r.\"task_id\",r.\"actor_user_id\",r.\"channel\",r.\"kind\",r.\"changes\",r.\"created_at\" FROM task_activity r WHERE (r.workspace_id=?3 AND r.task_id IN (SELECT id FROM tasks WHERE workspace_id=?3 AND project_id=?1)) ORDER BY r.id LIMIT 10001",budget:"SELECT count(*),coalesce(sum(coalesce(length(CAST(\"id\" AS BLOB)),0)+coalesce(length(CAST(\"task_id\" AS BLOB)),0)+coalesce(length(CAST(\"actor_user_id\" AS BLOB)),0)+coalesce(length(CAST(\"channel\" AS BLOB)),0)+coalesce(length(CAST(\"kind\" AS BLOB)),0)+coalesce(length(CAST(\"changes\" AS BLOB)),0)+coalesce(length(CAST(\"created_at\" AS BLOB)),0)),0) FROM (SELECT r.\"id\",r.\"task_id\",r.\"actor_user_id\",r.\"channel\",r.\"kind\",r.\"changes\",r.\"created_at\" FROM task_activity r WHERE (r.workspace_id=?3 AND r.task_id IN (SELECT id FROM tasks WHERE workspace_id=?3 AND project_id=?1)) ORDER BY r.id LIMIT 10001)",columns:&[NativeColumn{name:"id",kind:Uuid,nullable:false}, NativeColumn{name:"task_id",kind:Uuid,nullable:false}, NativeColumn{name:"actor_user_id",kind:Uuid,nullable:true}, NativeColumn{name:"channel",kind:Text,nullable:false}, NativeColumn{name:"kind",kind:Text,nullable:false}, NativeColumn{name:"changes",kind:Json,nullable:false}, NativeColumn{name:"created_at",kind:Instant,nullable:false}]},
"revisions"=>NativeRead{args:3,sql:"SELECT r.\"id\",r.\"target_kind\",r.\"target_id\",r.\"encoding\",r.\"content_json\",r.\"text\",r.\"reason\",r.\"created_by\",r.\"created_at\",r.\"restored_from_id\",r.\"restore_correlation_id\",r.\"restore_base_tail_seq\",r.\"restore_committed_tail_seq\" FROM revisions r WHERE (r.workspace_id=?3 AND ((r.target_kind='document' AND r.target_id IN (SELECT id FROM documents WHERE workspace_id=?3 AND (project_id=?1 OR (project_id IS NULL AND lower(hex(id)) IN (SELECT value FROM json_each(?2)))))) OR (r.target_kind='task' AND r.target_id IN (SELECT id FROM tasks WHERE workspace_id=?3 AND project_id=?1)))) ORDER BY r.id LIMIT 10001",budget:"SELECT count(*),coalesce(sum(coalesce(length(CAST(\"id\" AS BLOB)),0)+coalesce(length(CAST(\"target_kind\" AS BLOB)),0)+coalesce(length(CAST(\"target_id\" AS BLOB)),0)+coalesce(length(CAST(\"encoding\" AS BLOB)),0)+coalesce(length(CAST(\"content_json\" AS BLOB)),0)+coalesce(length(CAST(\"text\" AS BLOB)),0)+coalesce(length(CAST(\"reason\" AS BLOB)),0)+coalesce(length(CAST(\"created_by\" AS BLOB)),0)+coalesce(length(CAST(\"created_at\" AS BLOB)),0)+coalesce(length(CAST(\"restored_from_id\" AS BLOB)),0)+coalesce(length(CAST(\"restore_correlation_id\" AS BLOB)),0)+coalesce(length(CAST(\"restore_base_tail_seq\" AS BLOB)),0)+coalesce(length(CAST(\"restore_committed_tail_seq\" AS BLOB)),0)),0) FROM (SELECT r.\"id\",r.\"target_kind\",r.\"target_id\",r.\"encoding\",r.\"content_json\",r.\"text\",r.\"reason\",r.\"created_by\",r.\"created_at\",r.\"restored_from_id\",r.\"restore_correlation_id\",r.\"restore_base_tail_seq\",r.\"restore_committed_tail_seq\" FROM revisions r WHERE (r.workspace_id=?3 AND ((r.target_kind='document' AND r.target_id IN (SELECT id FROM documents WHERE workspace_id=?3 AND (project_id=?1 OR (project_id IS NULL AND lower(hex(id)) IN (SELECT value FROM json_each(?2)))))) OR (r.target_kind='task' AND r.target_id IN (SELECT id FROM tasks WHERE workspace_id=?3 AND project_id=?1)))) ORDER BY r.id LIMIT 10001)",columns:&[NativeColumn{name:"id",kind:Uuid,nullable:false}, NativeColumn{name:"target_kind",kind:Text,nullable:false}, NativeColumn{name:"target_id",kind:Uuid,nullable:false}, NativeColumn{name:"encoding",kind:Integer,nullable:false}, NativeColumn{name:"content_json",kind:Json,nullable:false}, NativeColumn{name:"text",kind:Text,nullable:false}, NativeColumn{name:"reason",kind:Text,nullable:false}, NativeColumn{name:"created_by",kind:Uuid,nullable:true}, NativeColumn{name:"created_at",kind:Instant,nullable:false}, NativeColumn{name:"restored_from_id",kind:Uuid,nullable:true}, NativeColumn{name:"restore_correlation_id",kind:Uuid,nullable:true}, NativeColumn{name:"restore_base_tail_seq",kind:Integer,nullable:true}, NativeColumn{name:"restore_committed_tail_seq",kind:Integer,nullable:true}]},
"attachments"=>NativeRead{args:3,sql:"SELECT r.\"id\",r.\"document_id\",r.\"task_id\",r.\"uploader_id\",r.\"name\",r.\"mime\",r.\"declared_mime\",r.\"size_bytes\",r.\"image\",r.\"created_at\",r.\"completed_at\",r.\"storage_key\",r.\"status\",r.\"scan_status\" FROM attachments r WHERE (r.workspace_id=?3 AND (r.document_id IN (SELECT id FROM documents WHERE workspace_id=?3 AND (project_id=?1 OR (project_id IS NULL AND lower(hex(id)) IN (SELECT value FROM json_each(?2))))) OR r.task_id IN (SELECT id FROM tasks WHERE workspace_id=?3 AND project_id=?1))) ORDER BY r.id LIMIT 10001",budget:"SELECT count(*),coalesce(sum(coalesce(length(CAST(\"id\" AS BLOB)),0)+coalesce(length(CAST(\"document_id\" AS BLOB)),0)+coalesce(length(CAST(\"task_id\" AS BLOB)),0)+coalesce(length(CAST(\"uploader_id\" AS BLOB)),0)+coalesce(length(CAST(\"name\" AS BLOB)),0)+coalesce(length(CAST(\"mime\" AS BLOB)),0)+coalesce(length(CAST(\"declared_mime\" AS BLOB)),0)+coalesce(length(CAST(\"size_bytes\" AS BLOB)),0)+coalesce(length(CAST(\"image\" AS BLOB)),0)+coalesce(length(CAST(\"created_at\" AS BLOB)),0)+coalesce(length(CAST(\"completed_at\" AS BLOB)),0)+coalesce(length(CAST(\"storage_key\" AS BLOB)),0)+coalesce(length(CAST(\"status\" AS BLOB)),0)+coalesce(length(CAST(\"scan_status\" AS BLOB)),0)),0) FROM (SELECT r.\"id\",r.\"document_id\",r.\"task_id\",r.\"uploader_id\",r.\"name\",r.\"mime\",r.\"declared_mime\",r.\"size_bytes\",r.\"image\",r.\"created_at\",r.\"completed_at\",r.\"storage_key\",r.\"status\",r.\"scan_status\" FROM attachments r WHERE (r.workspace_id=?3 AND (r.document_id IN (SELECT id FROM documents WHERE workspace_id=?3 AND (project_id=?1 OR (project_id IS NULL AND lower(hex(id)) IN (SELECT value FROM json_each(?2))))) OR r.task_id IN (SELECT id FROM tasks WHERE workspace_id=?3 AND project_id=?1))) ORDER BY r.id LIMIT 10001)",columns:&[NativeColumn{name:"id",kind:Uuid,nullable:false}, NativeColumn{name:"document_id",kind:Uuid,nullable:true}, NativeColumn{name:"task_id",kind:Uuid,nullable:true}, NativeColumn{name:"uploader_id",kind:Uuid,nullable:false}, NativeColumn{name:"name",kind:Text,nullable:false}, NativeColumn{name:"mime",kind:Text,nullable:false}, NativeColumn{name:"declared_mime",kind:Text,nullable:true}, NativeColumn{name:"size_bytes",kind:Integer,nullable:false}, NativeColumn{name:"image",kind:Boolean,nullable:false}, NativeColumn{name:"created_at",kind:Instant,nullable:false}, NativeColumn{name:"completed_at",kind:Instant,nullable:false}, NativeColumn{name:"storage_key",kind:Text,nullable:false}, NativeColumn{name:"status",kind:Text,nullable:false}, NativeColumn{name:"scan_status",kind:Text,nullable:false}]},
"collections"=>NativeRead{args:3,sql:"SELECT r.\"id\",r.\"project_id\",r.\"kind\",r.\"name\",r.\"version\",r.\"deleted_at\",r.\"created_at\",r.\"updated_at\" FROM collections r WHERE (r.workspace_id=?3 AND r.id IN (SELECT id FROM collections WHERE workspace_id=?3 AND (project_id=?1 OR (project_id IS NULL AND kind='document' AND EXISTS(SELECT 1 FROM collection_items i WHERE i.collection_id=collections.id AND lower(hex(i.document_id)) IN(SELECT value FROM json_each(?2))))))) ORDER BY r.id LIMIT 10001",budget:"SELECT count(*),coalesce(sum(coalesce(length(CAST(\"id\" AS BLOB)),0)+coalesce(length(CAST(\"project_id\" AS BLOB)),0)+coalesce(length(CAST(\"kind\" AS BLOB)),0)+coalesce(length(CAST(\"name\" AS BLOB)),0)+coalesce(length(CAST(\"version\" AS BLOB)),0)+coalesce(length(CAST(\"deleted_at\" AS BLOB)),0)+coalesce(length(CAST(\"created_at\" AS BLOB)),0)+coalesce(length(CAST(\"updated_at\" AS BLOB)),0)),0) FROM (SELECT r.\"id\",r.\"project_id\",r.\"kind\",r.\"name\",r.\"version\",r.\"deleted_at\",r.\"created_at\",r.\"updated_at\" FROM collections r WHERE (r.workspace_id=?3 AND r.id IN (SELECT id FROM collections WHERE workspace_id=?3 AND (project_id=?1 OR (project_id IS NULL AND kind='document' AND EXISTS(SELECT 1 FROM collection_items i WHERE i.collection_id=collections.id AND lower(hex(i.document_id)) IN(SELECT value FROM json_each(?2))))))) ORDER BY r.id LIMIT 10001)",columns:&[NativeColumn{name:"id",kind:Uuid,nullable:false}, NativeColumn{name:"project_id",kind:Uuid,nullable:true}, NativeColumn{name:"kind",kind:Text,nullable:false}, NativeColumn{name:"name",kind:Text,nullable:false}, NativeColumn{name:"version",kind:Integer,nullable:false}, NativeColumn{name:"deleted_at",kind:Instant,nullable:true}, NativeColumn{name:"created_at",kind:Instant,nullable:false}, NativeColumn{name:"updated_at",kind:Instant,nullable:false}]},
"collection_items"=>NativeRead{args:3,sql:"SELECT r.\"id\",r.\"collection_id\",r.\"document_id\",r.\"task_id\",r.\"version\",r.\"created_at\",r.\"updated_at\" FROM collection_items r WHERE (r.workspace_id=?3 AND r.collection_id IN (SELECT id FROM collections WHERE workspace_id=?3 AND (project_id=?1 OR (project_id IS NULL AND kind='document' AND EXISTS(SELECT 1 FROM collection_items i WHERE i.collection_id=collections.id AND lower(hex(i.document_id)) IN(SELECT value FROM json_each(?2))))))) ORDER BY r.id LIMIT 10001",budget:"SELECT count(*),coalesce(sum(coalesce(length(CAST(\"id\" AS BLOB)),0)+coalesce(length(CAST(\"collection_id\" AS BLOB)),0)+coalesce(length(CAST(\"document_id\" AS BLOB)),0)+coalesce(length(CAST(\"task_id\" AS BLOB)),0)+coalesce(length(CAST(\"version\" AS BLOB)),0)+coalesce(length(CAST(\"created_at\" AS BLOB)),0)+coalesce(length(CAST(\"updated_at\" AS BLOB)),0)),0) FROM (SELECT r.\"id\",r.\"collection_id\",r.\"document_id\",r.\"task_id\",r.\"version\",r.\"created_at\",r.\"updated_at\" FROM collection_items r WHERE (r.workspace_id=?3 AND r.collection_id IN (SELECT id FROM collections WHERE workspace_id=?3 AND (project_id=?1 OR (project_id IS NULL AND kind='document' AND EXISTS(SELECT 1 FROM collection_items i WHERE i.collection_id=collections.id AND lower(hex(i.document_id)) IN(SELECT value FROM json_each(?2))))))) ORDER BY r.id LIMIT 10001)",columns:&[NativeColumn{name:"id",kind:Uuid,nullable:false}, NativeColumn{name:"collection_id",kind:Uuid,nullable:false}, NativeColumn{name:"document_id",kind:Uuid,nullable:true}, NativeColumn{name:"task_id",kind:Uuid,nullable:true}, NativeColumn{name:"version",kind:Integer,nullable:false}, NativeColumn{name:"created_at",kind:Instant,nullable:false}, NativeColumn{name:"updated_at",kind:Instant,nullable:false}]},
"collection_fields"=>NativeRead{args:3,sql:"SELECT r.\"id\",r.\"collection_id\",r.\"key\",r.\"name\",r.\"description\",r.\"type\",r.\"sort_key\",r.\"version\",r.\"deleted_at\",r.\"created_at\",r.\"updated_at\" FROM collection_fields r WHERE (r.workspace_id=?3 AND r.collection_id IN (SELECT id FROM collections WHERE workspace_id=?3 AND (project_id=?1 OR (project_id IS NULL AND kind='document' AND EXISTS(SELECT 1 FROM collection_items i WHERE i.collection_id=collections.id AND lower(hex(i.document_id)) IN(SELECT value FROM json_each(?2))))))) ORDER BY r.id LIMIT 10001",budget:"SELECT count(*),coalesce(sum(coalesce(length(CAST(\"id\" AS BLOB)),0)+coalesce(length(CAST(\"collection_id\" AS BLOB)),0)+coalesce(length(CAST(\"key\" AS BLOB)),0)+coalesce(length(CAST(\"name\" AS BLOB)),0)+coalesce(length(CAST(\"description\" AS BLOB)),0)+coalesce(length(CAST(\"type\" AS BLOB)),0)+coalesce(length(CAST(\"sort_key\" AS BLOB)),0)+coalesce(length(CAST(\"version\" AS BLOB)),0)+coalesce(length(CAST(\"deleted_at\" AS BLOB)),0)+coalesce(length(CAST(\"created_at\" AS BLOB)),0)+coalesce(length(CAST(\"updated_at\" AS BLOB)),0)),0) FROM (SELECT r.\"id\",r.\"collection_id\",r.\"key\",r.\"name\",r.\"description\",r.\"type\",r.\"sort_key\",r.\"version\",r.\"deleted_at\",r.\"created_at\",r.\"updated_at\" FROM collection_fields r WHERE (r.workspace_id=?3 AND r.collection_id IN (SELECT id FROM collections WHERE workspace_id=?3 AND (project_id=?1 OR (project_id IS NULL AND kind='document' AND EXISTS(SELECT 1 FROM collection_items i WHERE i.collection_id=collections.id AND lower(hex(i.document_id)) IN(SELECT value FROM json_each(?2))))))) ORDER BY r.id LIMIT 10001)",columns:&[NativeColumn{name:"id",kind:Uuid,nullable:false}, NativeColumn{name:"collection_id",kind:Uuid,nullable:false}, NativeColumn{name:"key",kind:Text,nullable:false}, NativeColumn{name:"name",kind:Text,nullable:false}, NativeColumn{name:"description",kind:Text,nullable:true}, NativeColumn{name:"type",kind:Text,nullable:false}, NativeColumn{name:"sort_key",kind:Text,nullable:false}, NativeColumn{name:"version",kind:Integer,nullable:false}, NativeColumn{name:"deleted_at",kind:Instant,nullable:true}, NativeColumn{name:"created_at",kind:Instant,nullable:false}, NativeColumn{name:"updated_at",kind:Instant,nullable:false}]},
"collection_options"=>NativeRead{args:3,sql:"SELECT r.\"id\",r.\"collection_id\",r.\"field_id\",r.\"key\",r.\"label\",r.\"sort_key\",r.\"deleted_at\" FROM collection_options r WHERE (r.workspace_id=?3 AND r.collection_id IN (SELECT id FROM collections WHERE workspace_id=?3 AND (project_id=?1 OR (project_id IS NULL AND kind='document' AND EXISTS(SELECT 1 FROM collection_items i WHERE i.collection_id=collections.id AND lower(hex(i.document_id)) IN(SELECT value FROM json_each(?2))))))) ORDER BY r.id LIMIT 10001",budget:"SELECT count(*),coalesce(sum(coalesce(length(CAST(\"id\" AS BLOB)),0)+coalesce(length(CAST(\"collection_id\" AS BLOB)),0)+coalesce(length(CAST(\"field_id\" AS BLOB)),0)+coalesce(length(CAST(\"key\" AS BLOB)),0)+coalesce(length(CAST(\"label\" AS BLOB)),0)+coalesce(length(CAST(\"sort_key\" AS BLOB)),0)+coalesce(length(CAST(\"deleted_at\" AS BLOB)),0)),0) FROM (SELECT r.\"id\",r.\"collection_id\",r.\"field_id\",r.\"key\",r.\"label\",r.\"sort_key\",r.\"deleted_at\" FROM collection_options r WHERE (r.workspace_id=?3 AND r.collection_id IN (SELECT id FROM collections WHERE workspace_id=?3 AND (project_id=?1 OR (project_id IS NULL AND kind='document' AND EXISTS(SELECT 1 FROM collection_items i WHERE i.collection_id=collections.id AND lower(hex(i.document_id)) IN(SELECT value FROM json_each(?2))))))) ORDER BY r.id LIMIT 10001)",columns:&[NativeColumn{name:"id",kind:Uuid,nullable:false}, NativeColumn{name:"collection_id",kind:Uuid,nullable:false}, NativeColumn{name:"field_id",kind:Uuid,nullable:false}, NativeColumn{name:"key",kind:Text,nullable:false}, NativeColumn{name:"label",kind:Text,nullable:false}, NativeColumn{name:"sort_key",kind:Text,nullable:false}, NativeColumn{name:"deleted_at",kind:Instant,nullable:true}]},
"collection_values"=>NativeRead{args:3,sql:"SELECT r.\"collection_id\",r.\"item_id\",r.\"field_id\",r.\"field_type\",r.\"value_text\",r.\"value_number\",r.\"value_date\",r.\"value_ts\",r.\"value_bool\" FROM collection_values r WHERE (r.workspace_id=?3 AND r.collection_id IN (SELECT id FROM collections WHERE workspace_id=?3 AND (project_id=?1 OR (project_id IS NULL AND kind='document' AND EXISTS(SELECT 1 FROM collection_items i WHERE i.collection_id=collections.id AND lower(hex(i.document_id)) IN(SELECT value FROM json_each(?2))))))) ORDER BY r.item_id,r.field_id LIMIT 10001",budget:"SELECT count(*),coalesce(sum(coalesce(length(CAST(\"collection_id\" AS BLOB)),0)+coalesce(length(CAST(\"item_id\" AS BLOB)),0)+coalesce(length(CAST(\"field_id\" AS BLOB)),0)+coalesce(length(CAST(\"field_type\" AS BLOB)),0)+coalesce(length(CAST(\"value_text\" AS BLOB)),0)+coalesce(length(CAST(\"value_number\" AS BLOB)),0)+coalesce(length(CAST(\"value_date\" AS BLOB)),0)+coalesce(length(CAST(\"value_ts\" AS BLOB)),0)+coalesce(length(CAST(\"value_bool\" AS BLOB)),0)),0) FROM (SELECT r.\"collection_id\",r.\"item_id\",r.\"field_id\",r.\"field_type\",r.\"value_text\",r.\"value_number\",r.\"value_date\",r.\"value_ts\",r.\"value_bool\" FROM collection_values r WHERE (r.workspace_id=?3 AND r.collection_id IN (SELECT id FROM collections WHERE workspace_id=?3 AND (project_id=?1 OR (project_id IS NULL AND kind='document' AND EXISTS(SELECT 1 FROM collection_items i WHERE i.collection_id=collections.id AND lower(hex(i.document_id)) IN(SELECT value FROM json_each(?2))))))) ORDER BY r.item_id,r.field_id LIMIT 10001)",columns:&[NativeColumn{name:"collection_id",kind:Uuid,nullable:false}, NativeColumn{name:"item_id",kind:Uuid,nullable:false}, NativeColumn{name:"field_id",kind:Uuid,nullable:false}, NativeColumn{name:"field_type",kind:Text,nullable:false}, NativeColumn{name:"value_text",kind:Text,nullable:true}, NativeColumn{name:"value_number",kind:Text,nullable:true}, NativeColumn{name:"value_date",kind:Date,nullable:true}, NativeColumn{name:"value_ts",kind:Instant,nullable:true}, NativeColumn{name:"value_bool",kind:Boolean,nullable:true}]},
"collection_choices"=>NativeRead{args:3,sql:"SELECT r.\"collection_id\",r.\"item_id\",r.\"field_id\",r.\"field_type\",r.\"option_id\" FROM collection_choices r WHERE (r.workspace_id=?3 AND r.collection_id IN (SELECT id FROM collections WHERE workspace_id=?3 AND (project_id=?1 OR (project_id IS NULL AND kind='document' AND EXISTS(SELECT 1 FROM collection_items i WHERE i.collection_id=collections.id AND lower(hex(i.document_id)) IN(SELECT value FROM json_each(?2))))))) ORDER BY r.item_id,r.field_id,r.option_id LIMIT 10001",budget:"SELECT count(*),coalesce(sum(coalesce(length(CAST(\"collection_id\" AS BLOB)),0)+coalesce(length(CAST(\"item_id\" AS BLOB)),0)+coalesce(length(CAST(\"field_id\" AS BLOB)),0)+coalesce(length(CAST(\"field_type\" AS BLOB)),0)+coalesce(length(CAST(\"option_id\" AS BLOB)),0)),0) FROM (SELECT r.\"collection_id\",r.\"item_id\",r.\"field_id\",r.\"field_type\",r.\"option_id\" FROM collection_choices r WHERE (r.workspace_id=?3 AND r.collection_id IN (SELECT id FROM collections WHERE workspace_id=?3 AND (project_id=?1 OR (project_id IS NULL AND kind='document' AND EXISTS(SELECT 1 FROM collection_items i WHERE i.collection_id=collections.id AND lower(hex(i.document_id)) IN(SELECT value FROM json_each(?2))))))) ORDER BY r.item_id,r.field_id,r.option_id LIMIT 10001)",columns:&[NativeColumn{name:"collection_id",kind:Uuid,nullable:false}, NativeColumn{name:"item_id",kind:Uuid,nullable:false}, NativeColumn{name:"field_id",kind:Uuid,nullable:false}, NativeColumn{name:"field_type",kind:Text,nullable:false}, NativeColumn{name:"option_id",kind:Uuid,nullable:false}]},
"collection_people"=>NativeRead{args:3,sql:"SELECT r.\"collection_id\",r.\"item_id\",r.\"field_id\",r.\"field_type\",r.\"user_id\" FROM collection_people r WHERE (r.workspace_id=?3 AND r.collection_id IN (SELECT id FROM collections WHERE workspace_id=?3 AND (project_id=?1 OR (project_id IS NULL AND kind='document' AND EXISTS(SELECT 1 FROM collection_items i WHERE i.collection_id=collections.id AND lower(hex(i.document_id)) IN(SELECT value FROM json_each(?2))))))) ORDER BY r.item_id,r.field_id,r.user_id LIMIT 10001",budget:"SELECT count(*),coalesce(sum(coalesce(length(CAST(\"collection_id\" AS BLOB)),0)+coalesce(length(CAST(\"item_id\" AS BLOB)),0)+coalesce(length(CAST(\"field_id\" AS BLOB)),0)+coalesce(length(CAST(\"field_type\" AS BLOB)),0)+coalesce(length(CAST(\"user_id\" AS BLOB)),0)),0) FROM (SELECT r.\"collection_id\",r.\"item_id\",r.\"field_id\",r.\"field_type\",r.\"user_id\" FROM collection_people r WHERE (r.workspace_id=?3 AND r.collection_id IN (SELECT id FROM collections WHERE workspace_id=?3 AND (project_id=?1 OR (project_id IS NULL AND kind='document' AND EXISTS(SELECT 1 FROM collection_items i WHERE i.collection_id=collections.id AND lower(hex(i.document_id)) IN(SELECT value FROM json_each(?2))))))) ORDER BY r.item_id,r.field_id,r.user_id LIMIT 10001)",columns:&[NativeColumn{name:"collection_id",kind:Uuid,nullable:false}, NativeColumn{name:"item_id",kind:Uuid,nullable:false}, NativeColumn{name:"field_id",kind:Uuid,nullable:false}, NativeColumn{name:"field_type",kind:Text,nullable:false}, NativeColumn{name:"user_id",kind:Uuid,nullable:false}]},
"collection_views"=>NativeRead{args:3,sql:"SELECT r.\"id\",r.\"collection_id\",r.\"owner_id\",r.\"visibility\",r.\"name\",r.\"type\",r.\"config\",r.\"version\",r.\"created_at\",r.\"updated_at\" FROM collection_views r WHERE (r.workspace_id=?3 AND r.collection_id IN (SELECT id FROM collections WHERE workspace_id=?3 AND (project_id=?1 OR (project_id IS NULL AND kind='document' AND EXISTS(SELECT 1 FROM collection_items i WHERE i.collection_id=collections.id AND lower(hex(i.document_id)) IN(SELECT value FROM json_each(?2))))))) ORDER BY r.id LIMIT 10001",budget:"SELECT count(*),coalesce(sum(coalesce(length(CAST(\"id\" AS BLOB)),0)+coalesce(length(CAST(\"collection_id\" AS BLOB)),0)+coalesce(length(CAST(\"owner_id\" AS BLOB)),0)+coalesce(length(CAST(\"visibility\" AS BLOB)),0)+coalesce(length(CAST(\"name\" AS BLOB)),0)+coalesce(length(CAST(\"type\" AS BLOB)),0)+coalesce(length(CAST(\"config\" AS BLOB)),0)+coalesce(length(CAST(\"version\" AS BLOB)),0)+coalesce(length(CAST(\"created_at\" AS BLOB)),0)+coalesce(length(CAST(\"updated_at\" AS BLOB)),0)),0) FROM (SELECT r.\"id\",r.\"collection_id\",r.\"owner_id\",r.\"visibility\",r.\"name\",r.\"type\",r.\"config\",r.\"version\",r.\"created_at\",r.\"updated_at\" FROM collection_views r WHERE (r.workspace_id=?3 AND r.collection_id IN (SELECT id FROM collections WHERE workspace_id=?3 AND (project_id=?1 OR (project_id IS NULL AND kind='document' AND EXISTS(SELECT 1 FROM collection_items i WHERE i.collection_id=collections.id AND lower(hex(i.document_id)) IN(SELECT value FROM json_each(?2))))))) ORDER BY r.id LIMIT 10001)",columns:&[NativeColumn{name:"id",kind:Uuid,nullable:false}, NativeColumn{name:"collection_id",kind:Uuid,nullable:false}, NativeColumn{name:"owner_id",kind:Uuid,nullable:false}, NativeColumn{name:"visibility",kind:Text,nullable:false}, NativeColumn{name:"name",kind:Text,nullable:false}, NativeColumn{name:"type",kind:Text,nullable:false}, NativeColumn{name:"config",kind:Json,nullable:false}, NativeColumn{name:"version",kind:Integer,nullable:false}, NativeColumn{name:"created_at",kind:Instant,nullable:false}, NativeColumn{name:"updated_at",kind:Instant,nullable:false}]},
"zotero_connectors"=>NativeRead{args:5,sql:"SELECT r.\"id\",r.\"library_type\",r.\"remote_library_id\",r.\"library_url\",r.\"completed_version\",r.\"created_at\",r.\"updated_at\" FROM zotero_connectors r WHERE (r.workspace_id=?3 AND r.owner_user_id=?4 AND lower(hex(r.id)) IN(SELECT value FROM json_each(?5))) ORDER BY r.id LIMIT 10001",budget:"SELECT count(*),coalesce(sum(coalesce(length(CAST(\"id\" AS BLOB)),0)+coalesce(length(CAST(\"library_type\" AS BLOB)),0)+coalesce(length(CAST(\"remote_library_id\" AS BLOB)),0)+coalesce(length(CAST(\"library_url\" AS BLOB)),0)+coalesce(length(CAST(\"completed_version\" AS BLOB)),0)+coalesce(length(CAST(\"created_at\" AS BLOB)),0)+coalesce(length(CAST(\"updated_at\" AS BLOB)),0)),0) FROM (SELECT r.\"id\",r.\"library_type\",r.\"remote_library_id\",r.\"library_url\",r.\"completed_version\",r.\"created_at\",r.\"updated_at\" FROM zotero_connectors r WHERE (r.workspace_id=?3 AND r.owner_user_id=?4 AND lower(hex(r.id)) IN(SELECT value FROM json_each(?5))) ORDER BY r.id LIMIT 10001)",columns:&[NativeColumn{name:"id",kind:Uuid,nullable:false}, NativeColumn{name:"library_type",kind:Text,nullable:false}, NativeColumn{name:"remote_library_id",kind:Integer,nullable:false}, NativeColumn{name:"library_url",kind:Text,nullable:false}, NativeColumn{name:"completed_version",kind:Integer,nullable:false}, NativeColumn{name:"created_at",kind:Instant,nullable:false}, NativeColumn{name:"updated_at",kind:Instant,nullable:false}]},
"zotero_references"=>NativeRead{args:5,sql:"SELECT r.\"id\",r.\"connector_id\",r.\"document_id\",r.\"item_key\",r.\"remote_version\",r.\"local_version\",r.\"bibliography\",r.\"return_url\",r.\"availability\" FROM zotero_references r WHERE (r.workspace_id=?3 AND r.owner_user_id=?4 AND lower(hex(r.connector_id)) IN(SELECT value FROM json_each(?5))) ORDER BY r.id LIMIT 10001",budget:"SELECT count(*),coalesce(sum(coalesce(length(CAST(\"id\" AS BLOB)),0)+coalesce(length(CAST(\"connector_id\" AS BLOB)),0)+coalesce(length(CAST(\"document_id\" AS BLOB)),0)+coalesce(length(CAST(\"item_key\" AS BLOB)),0)+coalesce(length(CAST(\"remote_version\" AS BLOB)),0)+coalesce(length(CAST(\"local_version\" AS BLOB)),0)+coalesce(length(CAST(\"bibliography\" AS BLOB)),0)+coalesce(length(CAST(\"return_url\" AS BLOB)),0)+coalesce(length(CAST(\"availability\" AS BLOB)),0)),0) FROM (SELECT r.\"id\",r.\"connector_id\",r.\"document_id\",r.\"item_key\",r.\"remote_version\",r.\"local_version\",r.\"bibliography\",r.\"return_url\",r.\"availability\" FROM zotero_references r WHERE (r.workspace_id=?3 AND r.owner_user_id=?4 AND lower(hex(r.connector_id)) IN(SELECT value FROM json_each(?5))) ORDER BY r.id LIMIT 10001)",columns:&[NativeColumn{name:"id",kind:Uuid,nullable:false}, NativeColumn{name:"connector_id",kind:Uuid,nullable:false}, NativeColumn{name:"document_id",kind:Uuid,nullable:true}, NativeColumn{name:"item_key",kind:Text,nullable:false}, NativeColumn{name:"remote_version",kind:Integer,nullable:false}, NativeColumn{name:"local_version",kind:Integer,nullable:false}, NativeColumn{name:"bibliography",kind:Json,nullable:false}, NativeColumn{name:"return_url",kind:Text,nullable:false}, NativeColumn{name:"availability",kind:Text,nullable:false}]},
"zotero_collections"=>NativeRead{args:5,sql:"SELECT r.\"connector_id\",r.\"collection_key\",r.\"remote_version\",r.\"name\",r.\"parent_key\",r.\"availability\" FROM zotero_collections r WHERE (r.workspace_id=?3 AND r.owner_user_id=?4 AND lower(hex(r.connector_id)) IN(SELECT value FROM json_each(?5))) ORDER BY r.connector_id,r.collection_key LIMIT 10001",budget:"SELECT count(*),coalesce(sum(coalesce(length(CAST(\"connector_id\" AS BLOB)),0)+coalesce(length(CAST(\"collection_key\" AS BLOB)),0)+coalesce(length(CAST(\"remote_version\" AS BLOB)),0)+coalesce(length(CAST(\"name\" AS BLOB)),0)+coalesce(length(CAST(\"parent_key\" AS BLOB)),0)+coalesce(length(CAST(\"availability\" AS BLOB)),0)),0) FROM (SELECT r.\"connector_id\",r.\"collection_key\",r.\"remote_version\",r.\"name\",r.\"parent_key\",r.\"availability\" FROM zotero_collections r WHERE (r.workspace_id=?3 AND r.owner_user_id=?4 AND lower(hex(r.connector_id)) IN(SELECT value FROM json_each(?5))) ORDER BY r.connector_id,r.collection_key LIMIT 10001)",columns:&[NativeColumn{name:"connector_id",kind:Uuid,nullable:false}, NativeColumn{name:"collection_key",kind:Text,nullable:false}, NativeColumn{name:"remote_version",kind:Integer,nullable:false}, NativeColumn{name:"name",kind:Text,nullable:false}, NativeColumn{name:"parent_key",kind:Text,nullable:true}, NativeColumn{name:"availability",kind:Text,nullable:false}]},
"zotero_memberships"=>NativeRead{args:5,sql:"SELECT r.\"connector_id\",r.\"reference_id\",r.\"collection_key\" FROM zotero_memberships r WHERE (r.workspace_id=?3 AND r.owner_user_id=?4 AND lower(hex(r.connector_id)) IN(SELECT value FROM json_each(?5))) ORDER BY r.connector_id,r.reference_id,r.collection_key LIMIT 10001",budget:"SELECT count(*),coalesce(sum(coalesce(length(CAST(\"connector_id\" AS BLOB)),0)+coalesce(length(CAST(\"reference_id\" AS BLOB)),0)+coalesce(length(CAST(\"collection_key\" AS BLOB)),0)),0) FROM (SELECT r.\"connector_id\",r.\"reference_id\",r.\"collection_key\" FROM zotero_memberships r WHERE (r.workspace_id=?3 AND r.owner_user_id=?4 AND lower(hex(r.connector_id)) IN(SELECT value FROM json_each(?5))) ORDER BY r.connector_id,r.reference_id,r.collection_key LIMIT 10001)",columns:&[NativeColumn{name:"connector_id",kind:Uuid,nullable:false}, NativeColumn{name:"reference_id",kind:Uuid,nullable:false}, NativeColumn{name:"collection_key",kind:Text,nullable:false}]},
"zotero_links"=>NativeRead{args:5,sql:"SELECT r.\"id\",r.\"connector_id\",r.\"reference_id\",r.\"document_id\",r.\"task_id\",r.\"anchor\" FROM zotero_links r WHERE (r.workspace_id=?3 AND r.owner_user_id=?4 AND lower(hex(r.connector_id)) IN(SELECT value FROM json_each(?5))) ORDER BY r.id LIMIT 10001",budget:"SELECT count(*),coalesce(sum(coalesce(length(CAST(\"id\" AS BLOB)),0)+coalesce(length(CAST(\"connector_id\" AS BLOB)),0)+coalesce(length(CAST(\"reference_id\" AS BLOB)),0)+coalesce(length(CAST(\"document_id\" AS BLOB)),0)+coalesce(length(CAST(\"task_id\" AS BLOB)),0)+coalesce(length(CAST(\"anchor\" AS BLOB)),0)),0) FROM (SELECT r.\"id\",r.\"connector_id\",r.\"reference_id\",r.\"document_id\",r.\"task_id\",r.\"anchor\" FROM zotero_links r WHERE (r.workspace_id=?3 AND r.owner_user_id=?4 AND lower(hex(r.connector_id)) IN(SELECT value FROM json_each(?5))) ORDER BY r.id LIMIT 10001)",columns:&[NativeColumn{name:"id",kind:Uuid,nullable:false}, NativeColumn{name:"connector_id",kind:Uuid,nullable:false}, NativeColumn{name:"reference_id",kind:Uuid,nullable:false}, NativeColumn{name:"document_id",kind:Uuid,nullable:true}, NativeColumn{name:"task_id",kind:Uuid,nullable:true}, NativeColumn{name:"anchor",kind:Text,nullable:false}]},
"personal_input_commands"=>NativeRead{args:3,sql:"SELECT r.\"request_id\",r.\"request_hash\",r.\"intent\",r.\"document_id\",r.\"task_id\",r.\"project_id\",r.\"created_at\" FROM personal_input_commands r WHERE (r.workspace_id=?3 AND (r.project_id=?1 OR r.document_id IN (SELECT id FROM documents WHERE workspace_id=?3 AND (project_id=?1 OR (project_id IS NULL AND lower(hex(id)) IN (SELECT value FROM json_each(?2))))) OR r.task_id IN (SELECT id FROM tasks WHERE workspace_id=?3 AND project_id=?1))) ORDER BY r.created_at,r.request_id LIMIT 10001",budget:"SELECT count(*),coalesce(sum(coalesce(length(CAST(\"request_id\" AS BLOB)),0)+coalesce(length(CAST(\"request_hash\" AS BLOB)),0)+coalesce(length(CAST(\"intent\" AS BLOB)),0)+coalesce(length(CAST(\"document_id\" AS BLOB)),0)+coalesce(length(CAST(\"task_id\" AS BLOB)),0)+coalesce(length(CAST(\"project_id\" AS BLOB)),0)+coalesce(length(CAST(\"created_at\" AS BLOB)),0)),0) FROM (SELECT r.\"request_id\",r.\"request_hash\",r.\"intent\",r.\"document_id\",r.\"task_id\",r.\"project_id\",r.\"created_at\" FROM personal_input_commands r WHERE (r.workspace_id=?3 AND (r.project_id=?1 OR r.document_id IN (SELECT id FROM documents WHERE workspace_id=?3 AND (project_id=?1 OR (project_id IS NULL AND lower(hex(id)) IN (SELECT value FROM json_each(?2))))) OR r.task_id IN (SELECT id FROM tasks WHERE workspace_id=?3 AND project_id=?1))) ORDER BY r.created_at,r.request_id LIMIT 10001)",columns:&[NativeColumn{name:"request_id",kind:Uuid,nullable:false}, NativeColumn{name:"request_hash",kind:Text,nullable:false}, NativeColumn{name:"intent",kind:Text,nullable:false}, NativeColumn{name:"document_id",kind:Uuid,nullable:true}, NativeColumn{name:"task_id",kind:Uuid,nullable:true}, NativeColumn{name:"project_id",kind:Uuid,nullable:true}, NativeColumn{name:"created_at",kind:Instant,nullable:false}]},
"time_entries"=>NativeRead{args:3,sql:"SELECT r.\"id\",r.\"task_id\",r.\"user_id\",r.\"started_at\",r.\"ended_at\",r.\"duration_seconds\",r.\"note\" FROM time_entries r WHERE (r.workspace_id=?3 AND r.task_id IN (SELECT id FROM tasks WHERE workspace_id=?3 AND project_id=?1)) ORDER BY r.id LIMIT 10001",budget:"SELECT count(*),coalesce(sum(coalesce(length(CAST(\"id\" AS BLOB)),0)+coalesce(length(CAST(\"task_id\" AS BLOB)),0)+coalesce(length(CAST(\"user_id\" AS BLOB)),0)+coalesce(length(CAST(\"started_at\" AS BLOB)),0)+coalesce(length(CAST(\"ended_at\" AS BLOB)),0)+coalesce(length(CAST(\"duration_seconds\" AS BLOB)),0)+coalesce(length(CAST(\"note\" AS BLOB)),0)),0) FROM (SELECT r.\"id\",r.\"task_id\",r.\"user_id\",r.\"started_at\",r.\"ended_at\",r.\"duration_seconds\",r.\"note\" FROM time_entries r WHERE (r.workspace_id=?3 AND r.task_id IN (SELECT id FROM tasks WHERE workspace_id=?3 AND project_id=?1)) ORDER BY r.id LIMIT 10001)",columns:&[NativeColumn{name:"id",kind:Uuid,nullable:false}, NativeColumn{name:"task_id",kind:Uuid,nullable:false}, NativeColumn{name:"user_id",kind:Uuid,nullable:false}, NativeColumn{name:"started_at",kind:Instant,nullable:false}, NativeColumn{name:"ended_at",kind:Instant,nullable:true}, NativeColumn{name:"duration_seconds",kind:Integer,nullable:true}, NativeColumn{name:"note",kind:Text,nullable:true}]},
"task_timer_runs"=>NativeRead{args:4,sql:"SELECT r.\"id\",r.\"user_id\",r.\"task_id\",r.\"status\",r.\"version\",r.\"started_at\",r.\"stopped_at\",r.\"note\" FROM task_timer_runs r WHERE (r.workspace_id=?3 AND r.task_id IN (SELECT id FROM tasks WHERE workspace_id=?3 AND project_id=?1) AND r.user_id=?4) ORDER BY r.id LIMIT 10001",budget:"SELECT count(*),coalesce(sum(coalesce(length(CAST(\"id\" AS BLOB)),0)+coalesce(length(CAST(\"user_id\" AS BLOB)),0)+coalesce(length(CAST(\"task_id\" AS BLOB)),0)+coalesce(length(CAST(\"status\" AS BLOB)),0)+coalesce(length(CAST(\"version\" AS BLOB)),0)+coalesce(length(CAST(\"started_at\" AS BLOB)),0)+coalesce(length(CAST(\"stopped_at\" AS BLOB)),0)+coalesce(length(CAST(\"note\" AS BLOB)),0)),0) FROM (SELECT r.\"id\",r.\"user_id\",r.\"task_id\",r.\"status\",r.\"version\",r.\"started_at\",r.\"stopped_at\",r.\"note\" FROM task_timer_runs r WHERE (r.workspace_id=?3 AND r.task_id IN (SELECT id FROM tasks WHERE workspace_id=?3 AND project_id=?1) AND r.user_id=?4) ORDER BY r.id LIMIT 10001)",columns:&[NativeColumn{name:"id",kind:Uuid,nullable:false}, NativeColumn{name:"user_id",kind:Uuid,nullable:false}, NativeColumn{name:"task_id",kind:Uuid,nullable:false}, NativeColumn{name:"status",kind:Text,nullable:false}, NativeColumn{name:"version",kind:Integer,nullable:false}, NativeColumn{name:"started_at",kind:Instant,nullable:false}, NativeColumn{name:"stopped_at",kind:Instant,nullable:true}, NativeColumn{name:"note",kind:Text,nullable:true}]},
"task_timer_segments"=>NativeRead{args:4,sql:"SELECT r.\"id\",r.\"run_id\",r.\"user_id\",r.\"task_id\",r.\"started_at\",r.\"ended_at\",r.\"time_entry_id\" FROM task_timer_segments r WHERE (r.workspace_id=?3 AND r.task_id IN (SELECT id FROM tasks WHERE workspace_id=?3 AND project_id=?1) AND r.user_id=?4) ORDER BY r.id LIMIT 10001",budget:"SELECT count(*),coalesce(sum(coalesce(length(CAST(\"id\" AS BLOB)),0)+coalesce(length(CAST(\"run_id\" AS BLOB)),0)+coalesce(length(CAST(\"user_id\" AS BLOB)),0)+coalesce(length(CAST(\"task_id\" AS BLOB)),0)+coalesce(length(CAST(\"started_at\" AS BLOB)),0)+coalesce(length(CAST(\"ended_at\" AS BLOB)),0)+coalesce(length(CAST(\"time_entry_id\" AS BLOB)),0)),0) FROM (SELECT r.\"id\",r.\"run_id\",r.\"user_id\",r.\"task_id\",r.\"started_at\",r.\"ended_at\",r.\"time_entry_id\" FROM task_timer_segments r WHERE (r.workspace_id=?3 AND r.task_id IN (SELECT id FROM tasks WHERE workspace_id=?3 AND project_id=?1) AND r.user_id=?4) ORDER BY r.id LIMIT 10001)",columns:&[NativeColumn{name:"id",kind:Uuid,nullable:false}, NativeColumn{name:"run_id",kind:Uuid,nullable:false}, NativeColumn{name:"user_id",kind:Uuid,nullable:false}, NativeColumn{name:"task_id",kind:Uuid,nullable:false}, NativeColumn{name:"started_at",kind:Instant,nullable:false}, NativeColumn{name:"ended_at",kind:Instant,nullable:true}, NativeColumn{name:"time_entry_id",kind:Uuid,nullable:true}]},
"task_timer_legacy_open"=>NativeRead{args:4,sql:"SELECT r.\"time_entry_id\",r.\"user_id\",r.\"task_id\" FROM task_timer_legacy_open r WHERE (r.workspace_id=?3 AND r.task_id IN (SELECT id FROM tasks WHERE workspace_id=?3 AND project_id=?1) AND r.user_id=?4) ORDER BY r.time_entry_id LIMIT 10001",budget:"SELECT count(*),coalesce(sum(coalesce(length(CAST(\"time_entry_id\" AS BLOB)),0)+coalesce(length(CAST(\"user_id\" AS BLOB)),0)+coalesce(length(CAST(\"task_id\" AS BLOB)),0)),0) FROM (SELECT r.\"time_entry_id\",r.\"user_id\",r.\"task_id\" FROM task_timer_legacy_open r WHERE (r.workspace_id=?3 AND r.task_id IN (SELECT id FROM tasks WHERE workspace_id=?3 AND project_id=?1) AND r.user_id=?4) ORDER BY r.time_entry_id LIMIT 10001)",columns:&[NativeColumn{name:"time_entry_id",kind:Uuid,nullable:false}, NativeColumn{name:"user_id",kind:Uuid,nullable:false}, NativeColumn{name:"task_id",kind:Uuid,nullable:false}]},
"task_timer_commands"=>NativeRead{args:4,sql:"SELECT r.\"request_id\",r.\"user_id\",r.\"request_hash\",r.\"run_id\",r.\"result\",r.\"created_at\" FROM task_timer_commands r WHERE (r.user_id=?4 AND (r.run_id IN(SELECT id FROM task_timer_runs WHERE workspace_id=?3 AND task_id IN (SELECT id FROM tasks WHERE workspace_id=?3 AND project_id=?1)) OR EXISTS(SELECT 1 FROM task_timer_audit a WHERE a.user_id=r.user_id AND a.request_id=r.request_id AND (a.task_id IN (SELECT id FROM tasks WHERE workspace_id=?3 AND project_id=?1) OR a.time_entry_id IN(SELECT id FROM time_entries WHERE workspace_id=?3 AND task_id IN (SELECT id FROM tasks WHERE workspace_id=?3 AND project_id=?1)))))) ORDER BY r.created_at,r.request_id LIMIT 10001",budget:"SELECT count(*),coalesce(sum(coalesce(length(CAST(\"request_id\" AS BLOB)),0)+coalesce(length(CAST(\"user_id\" AS BLOB)),0)+coalesce(length(CAST(\"request_hash\" AS BLOB)),0)+coalesce(length(CAST(\"run_id\" AS BLOB)),0)+coalesce(length(CAST(\"result\" AS BLOB)),0)+coalesce(length(CAST(\"created_at\" AS BLOB)),0)),0) FROM (SELECT r.\"request_id\",r.\"user_id\",r.\"request_hash\",r.\"run_id\",r.\"result\",r.\"created_at\" FROM task_timer_commands r WHERE (r.user_id=?4 AND (r.run_id IN(SELECT id FROM task_timer_runs WHERE workspace_id=?3 AND task_id IN (SELECT id FROM tasks WHERE workspace_id=?3 AND project_id=?1)) OR EXISTS(SELECT 1 FROM task_timer_audit a WHERE a.user_id=r.user_id AND a.request_id=r.request_id AND (a.task_id IN (SELECT id FROM tasks WHERE workspace_id=?3 AND project_id=?1) OR a.time_entry_id IN(SELECT id FROM time_entries WHERE workspace_id=?3 AND task_id IN (SELECT id FROM tasks WHERE workspace_id=?3 AND project_id=?1)))))) ORDER BY r.created_at,r.request_id LIMIT 10001)",columns:&[NativeColumn{name:"request_id",kind:Uuid,nullable:false}, NativeColumn{name:"user_id",kind:Uuid,nullable:false}, NativeColumn{name:"request_hash",kind:Text,nullable:false}, NativeColumn{name:"run_id",kind:Uuid,nullable:true}, NativeColumn{name:"result",kind:Json,nullable:false}, NativeColumn{name:"created_at",kind:Instant,nullable:false}]},
"task_timer_audit"=>NativeRead{args:4,sql:"SELECT r.\"id\",r.\"user_id\",r.\"request_id\",r.\"workspace_id\",r.\"task_id\",r.\"time_entry_id\",r.\"verb\",r.\"before_value\",r.\"after_value\",r.\"reason\",r.\"created_at\" FROM task_timer_audit r WHERE (r.task_id IN (SELECT id FROM tasks WHERE workspace_id=?3 AND project_id=?1) OR r.time_entry_id IN(SELECT id FROM time_entries WHERE workspace_id=?3 AND task_id IN (SELECT id FROM tasks WHERE workspace_id=?3 AND project_id=?1)) OR (r.user_id=?4 AND EXISTS(SELECT 1 FROM task_timer_commands c WHERE c.user_id=r.user_id AND c.request_id=r.request_id AND c.run_id IN(SELECT id FROM task_timer_runs WHERE workspace_id=?3 AND task_id IN (SELECT id FROM tasks WHERE workspace_id=?3 AND project_id=?1))))) ORDER BY r.id LIMIT 10001",budget:"SELECT count(*),coalesce(sum(coalesce(length(CAST(\"id\" AS BLOB)),0)+coalesce(length(CAST(\"user_id\" AS BLOB)),0)+coalesce(length(CAST(\"request_id\" AS BLOB)),0)+coalesce(length(CAST(\"workspace_id\" AS BLOB)),0)+coalesce(length(CAST(\"task_id\" AS BLOB)),0)+coalesce(length(CAST(\"time_entry_id\" AS BLOB)),0)+coalesce(length(CAST(\"verb\" AS BLOB)),0)+coalesce(length(CAST(\"before_value\" AS BLOB)),0)+coalesce(length(CAST(\"after_value\" AS BLOB)),0)+coalesce(length(CAST(\"reason\" AS BLOB)),0)+coalesce(length(CAST(\"created_at\" AS BLOB)),0)),0) FROM (SELECT r.\"id\",r.\"user_id\",r.\"request_id\",r.\"workspace_id\",r.\"task_id\",r.\"time_entry_id\",r.\"verb\",r.\"before_value\",r.\"after_value\",r.\"reason\",r.\"created_at\" FROM task_timer_audit r WHERE (r.task_id IN (SELECT id FROM tasks WHERE workspace_id=?3 AND project_id=?1) OR r.time_entry_id IN(SELECT id FROM time_entries WHERE workspace_id=?3 AND task_id IN (SELECT id FROM tasks WHERE workspace_id=?3 AND project_id=?1)) OR (r.user_id=?4 AND EXISTS(SELECT 1 FROM task_timer_commands c WHERE c.user_id=r.user_id AND c.request_id=r.request_id AND c.run_id IN(SELECT id FROM task_timer_runs WHERE workspace_id=?3 AND task_id IN (SELECT id FROM tasks WHERE workspace_id=?3 AND project_id=?1))))) ORDER BY r.id LIMIT 10001)",columns:&[NativeColumn{name:"id",kind:Uuid,nullable:false}, NativeColumn{name:"user_id",kind:Uuid,nullable:false}, NativeColumn{name:"request_id",kind:Uuid,nullable:false}, NativeColumn{name:"workspace_id",kind:Uuid,nullable:true}, NativeColumn{name:"task_id",kind:Uuid,nullable:true}, NativeColumn{name:"time_entry_id",kind:Uuid,nullable:true}, NativeColumn{name:"verb",kind:Text,nullable:false}, NativeColumn{name:"before_value",kind:Json,nullable:false}, NativeColumn{name:"after_value",kind:Json,nullable:false}, NativeColumn{name:"reason",kind:Text,nullable:false}, NativeColumn{name:"created_at",kind:Instant,nullable:false}]},
_=>return Err(ArchiveError::Invalid("native capture table".into()).into()),
})
}

async fn family_capture_guards(tx: &mut FamilyTx, scope: &Scope) -> Result<(), NativeDbError> {
    let args = native_scope_args(scope)?;
    if tx.query("SELECT (SELECT count(*) FROM collections WHERE workspace_id=?3 AND project_id=?1 AND kind='task')<>1 OR EXISTS(SELECT 1 FROM collections WHERE workspace_id=?3 AND project_id=?1 AND deleted_at IS NOT NULL) OR EXISTS(SELECT 1 FROM collection_items i JOIN collections c ON c.workspace_id=i.workspace_id AND c.id=i.collection_id WHERE c.workspace_id=?3 AND c.project_id=?1 AND NOT((i.task_id IS NOT NULL AND i.task_id IN (SELECT id FROM tasks WHERE workspace_id=?3 AND project_id=?1)) OR (i.document_id IS NOT NULL AND i.document_id IN(SELECT id FROM documents WHERE workspace_id=?3 AND project_id=?1)))) OR (SELECT count(*) FROM collection_items i JOIN collections c ON c.workspace_id=i.workspace_id AND c.id=i.collection_id WHERE c.workspace_id=?3 AND c.project_id=?1 AND i.task_id IS NOT NULL)<>(SELECT count(*) FROM tasks WHERE workspace_id=?3 AND project_id=?1) OR EXISTS(SELECT 1 FROM collection_items i JOIN collections c ON c.workspace_id=i.workspace_id AND c.id=i.collection_id WHERE c.workspace_id=?3 AND c.project_id IS NOT ?1 AND (i.document_id IN (SELECT id FROM documents WHERE workspace_id=?3 AND (project_id=?1 OR (project_id IS NULL AND lower(hex(id)) IN(SELECT value FROM json_each(?2))))) OR i.task_id IN (SELECT id FROM tasks WHERE workspace_id=?3 AND project_id=?1)) AND NOT(c.project_id IS NULL AND c.kind='document' AND c.deleted_at IS NULL AND NOT EXISTS(SELECT 1 FROM collection_items j WHERE j.workspace_id=c.workspace_id AND j.collection_id=c.id AND (j.document_id IS NULL OR lower(hex(j.document_id)) NOT IN(SELECT value FROM json_each(?2))))))",&args[..3]).await?[0].cell(0)?.boolean()?{return Err(ArchiveError::Unsupported("collections".into()).into())}
    if tx.query("SELECT EXISTS(SELECT 1 FROM personal_input_commands WHERE workspace_id=?3 AND (project_id=?1 OR document_id IN (SELECT id FROM documents WHERE workspace_id=?3 AND (project_id=?1 OR (project_id IS NULL AND lower(hex(id)) IN(SELECT value FROM json_each(?2))))) OR task_id IN (SELECT id FROM tasks WHERE workspace_id=?3 AND project_id=?1)) AND(actor_user_id<>?4 OR (project_id IS NOT NULL AND project_id<>?1) OR(document_id IS NOT NULL AND document_id NOT IN (SELECT id FROM documents WHERE workspace_id=?3 AND (project_id=?1 OR (project_id IS NULL AND lower(hex(id)) IN(SELECT value FROM json_each(?2)))))) OR(task_id IS NOT NULL AND task_id NOT IN (SELECT id FROM tasks WHERE workspace_id=?3 AND project_id=?1))))",&args[..4]).await?[0].cell(0)?.boolean()?{return Err(ArchiveError::Unsupported("personal input commands".into()).into())}
    if tx.query("SELECT EXISTS(SELECT 1 FROM stars WHERE workspace_id=?3 AND (document_id IN (SELECT id FROM documents WHERE workspace_id=?3 AND (project_id=?1 OR (project_id IS NULL AND lower(hex(id)) IN(SELECT value FROM json_each(?2))))) OR task_id IN (SELECT id FROM tasks WHERE workspace_id=?3 AND project_id=?1)))",&args[..3]).await?[0].cell(0)?.boolean()?{return Err(ArchiveError::Unsupported("stars".into()).into())}
    if tx.query("SELECT EXISTS(SELECT 1 FROM zotero_links WHERE workspace_id=?3 AND owner_user_id=?4 AND lower(hex(connector_id)) NOT IN(SELECT value FROM json_each(?5)) AND (document_id IN (SELECT id FROM documents WHERE workspace_id=?3 AND (project_id=?1 OR (project_id IS NULL AND lower(hex(id)) IN(SELECT value FROM json_each(?2))))) OR task_id IN (SELECT id FROM tasks WHERE workspace_id=?3 AND project_id=?1)))",&args[..5]).await?[0].cell(0)?.boolean()?{return Err(ArchiveError::Unsupported("zotero links".into()).into())}
    if tx.query("SELECT EXISTS(SELECT 1 FROM github_issue_links WHERE workspace_id=?3 AND task_id IN (SELECT id FROM tasks WHERE workspace_id=?3 AND project_id=?1))",&args[..3]).await?[0].cell(0)?.boolean()?{return Err(ArchiveError::Unsupported("external issue links".into()).into())}
    if tx.query("SELECT EXISTS(SELECT 1 FROM project_members WHERE workspace_id=?3 AND project_id=?1 AND(user_id IS NOT ?4 OR group_id IS NOT NULL)) OR EXISTS(SELECT 1 FROM document_members WHERE workspace_id=?3 AND document_id IN (SELECT id FROM documents WHERE workspace_id=?3 AND (project_id=?1 OR (project_id IS NULL AND lower(hex(id)) IN(SELECT value FROM json_each(?2))))))",&args[..4]).await?[0].cell(0)?.boolean()?{return Err(ArchiveError::Unsupported("multiple authors or grants".into()).into())}
    if tx.query("SELECT EXISTS(SELECT 1 FROM document_collab_updates u WHERE u.workspace_id=?3 AND u.document_id IN (SELECT id FROM documents WHERE workspace_id=?3 AND (project_id=?1 OR (project_id IS NULL AND lower(hex(id)) IN(SELECT value FROM json_each(?2))))) AND NOT EXISTS(SELECT 1 FROM document_states s WHERE s.workspace_id=u.workspace_id AND s.document_id=u.document_id)) OR EXISTS(SELECT 1 FROM document_collab_op_receipts u WHERE u.workspace_id=?3 AND u.document_id IN (SELECT id FROM documents WHERE workspace_id=?3 AND (project_id=?1 OR (project_id IS NULL AND lower(hex(id)) IN(SELECT value FROM json_each(?2))))) AND NOT EXISTS(SELECT 1 FROM document_states s WHERE s.workspace_id=u.workspace_id AND s.document_id=u.document_id)) OR EXISTS(SELECT 1 FROM revisions r WHERE r.workspace_id=?3 AND r.target_kind='document' AND r.target_id IN (SELECT id FROM documents WHERE workspace_id=?3 AND (project_id=?1 OR (project_id IS NULL AND lower(hex(id)) IN(SELECT value FROM json_each(?2))))) AND NOT EXISTS(SELECT 1 FROM document_states s WHERE s.workspace_id=r.workspace_id AND s.document_id=r.target_id)) OR EXISTS(SELECT 1 FROM task_collab_updates u WHERE u.workspace_id=?3 AND u.task_id IN (SELECT id FROM tasks WHERE workspace_id=?3 AND project_id=?1) AND NOT EXISTS(SELECT 1 FROM task_states s WHERE s.workspace_id=u.workspace_id AND s.task_id=u.task_id)) OR EXISTS(SELECT 1 FROM task_collab_op_receipts u WHERE u.workspace_id=?3 AND u.task_id IN (SELECT id FROM tasks WHERE workspace_id=?3 AND project_id=?1) AND NOT EXISTS(SELECT 1 FROM task_states s WHERE s.workspace_id=u.workspace_id AND s.task_id=u.task_id)) OR EXISTS(SELECT 1 FROM revisions r WHERE r.workspace_id=?3 AND r.target_kind='task' AND r.target_id IN (SELECT id FROM tasks WHERE workspace_id=?3 AND project_id=?1) AND NOT EXISTS(SELECT 1 FROM task_states s WHERE s.workspace_id=r.workspace_id AND s.task_id=r.target_id))",&args[..3]).await?[0].cell(0)?.boolean()?{return Err(ArchiveError::Unsupported("missing native state for captured body".into()).into())}
    let bytes=tx.query("SELECT coalesce(sum(n),0) FROM (SELECT length(s.state) AS n FROM document_states s WHERE s.workspace_id=?3 AND s.document_id IN (SELECT id FROM documents WHERE workspace_id=?3 AND (project_id=?1 OR (project_id IS NULL AND lower(hex(id)) IN(SELECT value FROM json_each(?2))))) UNION ALL SELECT length(u.payload) FROM document_collab_updates u JOIN document_states s ON s.workspace_id=u.workspace_id AND s.document_id=u.document_id WHERE u.workspace_id=?3 AND u.document_id IN (SELECT id FROM documents WHERE workspace_id=?3 AND (project_id=?1 OR (project_id IS NULL AND lower(hex(id)) IN(SELECT value FROM json_each(?2))))) AND u.seq>s.snapshot_cutoff_seq AND u.seq<=s.tail_seq UNION ALL SELECT length(s.state) AS n FROM task_states s WHERE s.workspace_id=?3 AND s.task_id IN (SELECT id FROM tasks WHERE workspace_id=?3 AND project_id=?1) UNION ALL SELECT length(u.payload) FROM task_collab_updates u JOIN task_states s ON s.workspace_id=u.workspace_id AND s.task_id=u.task_id WHERE u.workspace_id=?3 AND u.task_id IN (SELECT id FROM tasks WHERE workspace_id=?3 AND project_id=?1) AND u.seq>s.snapshot_cutoff_seq AND u.seq<=s.tail_seq UNION ALL SELECT length(y_snapshot) FROM revisions WHERE workspace_id=?3 AND ((target_kind='document' AND target_id IN (SELECT id FROM documents WHERE workspace_id=?3 AND (project_id=?1 OR (project_id IS NULL AND lower(hex(id)) IN(SELECT value FROM json_each(?2)))))) OR(target_kind='task' AND target_id IN (SELECT id FROM tasks WHERE workspace_id=?3 AND project_id=?1))) UNION ALL SELECT size_bytes FROM attachments WHERE workspace_id=?3 AND (document_id IN (SELECT id FROM documents WHERE workspace_id=?3 AND (project_id=?1 OR (project_id IS NULL AND lower(hex(id)) IN(SELECT value FROM json_each(?2))))) OR task_id IN (SELECT id FROM tasks WHERE workspace_id=?3 AND project_id=?1)))",&args[..3]).await?[0].cell(0)?.integer()?;
    if bytes < 0 || bytes > MAX_BYTES as i64 {
        return Err(ArchiveError::Limit.into());
    }
    Ok(())
}
async fn family_native_states(
    tx: &mut FamilyTx,
    scope: &Scope,
    entries: &mut BTreeMap<String, String>,
) -> Result<Vec<NativeState>, NativeDbError> {
    let ids = |values: Vec<Uuid>| {
        Cell::json(&json!(values
            .iter()
            .map(|id| hex::encode(id.as_bytes()))
            .collect::<Vec<_>>()))
    };
    let documents=tx.query("SELECT id FROM documents WHERE workspace_id=?1 AND (project_id=?2 OR(project_id IS NULL AND lower(hex(id)) IN(SELECT value FROM json_each(?3)))) ORDER BY id LIMIT 257",&[Cell::uuid(scope.workspace),Cell::uuid(scope.project),native_scope_args(scope)?[1].clone()]).await?;
    let tasks = tx
        .query(
            "SELECT id FROM tasks WHERE workspace_id=?1 AND project_id=?2 ORDER BY id LIMIT 257",
            &[Cell::uuid(scope.workspace), Cell::uuid(scope.project)],
        )
        .await?;
    if documents.len() > MAX_OBJECTS || tasks.len() > MAX_OBJECTS {
        return Err(ArchiveError::Limit.into());
    }
    let mut states = Vec::new();
    for (kind,parent_rows,state_sql,tail_sql,receipt_sql) in [
        ("document",documents,"SELECT document_id,state,encoding,snapshot_cutoff_seq,tail_seq,compacted_at,created_at,updated_at FROM document_states WHERE workspace_id=?1 AND lower(hex(document_id)) IN(SELECT value FROM json_each(?2)) ORDER BY document_id LIMIT 257","SELECT seq,op_id,payload,created_at FROM document_collab_updates WHERE workspace_id=?1 AND document_id=?2 AND seq>?3 AND seq<=?4 ORDER BY seq LIMIT 10001","SELECT op_id,seq,payload_len,payload_sha256,actor_user_id,created_at FROM document_collab_op_receipts WHERE workspace_id=?1 AND document_id=?2 ORDER BY seq,op_id LIMIT 10001"),
        ("task",tasks,"SELECT task_id,state,encoding,snapshot_cutoff_seq,tail_seq,compacted_at,created_at,updated_at FROM task_states WHERE workspace_id=?1 AND lower(hex(task_id)) IN(SELECT value FROM json_each(?2)) ORDER BY task_id LIMIT 257","SELECT seq,op_id,payload,created_at FROM task_collab_updates WHERE workspace_id=?1 AND task_id=?2 AND seq>?3 AND seq<=?4 ORDER BY seq LIMIT 10001","SELECT op_id,seq,payload_len,payload_sha256,actor_user_id,created_at FROM task_collab_op_receipts WHERE workspace_id=?1 AND task_id=?2 ORDER BY seq,op_id LIMIT 10001")
    ]{
        let selected=ids(parent_rows.iter().map(|r|r.cell(0)?.id()).collect::<Result<Vec<_>,sqlx::Error>>()?)?;
        let rows=tx.query(state_sql,&[Cell::uuid(scope.workspace),selected]).await?;
        if rows.len()>MAX_OBJECTS{return Err(ArchiveError::Limit.into())}
        for row in rows{
            let target=row.cell(0)?.id()?;let cutoff=row.cell(3)?.integer()?;let tail=row.cell(4)?.integer()?;
            let name=format!("native/{kind}/{target}/state.v1");entries.insert(name.clone(),encode(&row.cell(1)?.bytes()?));
            let tails=tx.query(tail_sql,&[Cell::uuid(scope.workspace),Cell::uuid(target),Cell::Integer(cutoff),Cell::Integer(tail)]).await?;
            let receipts=tx.query(receipt_sql,&[Cell::uuid(scope.workspace),Cell::uuid(target)]).await?;
            if tails.len()>MAX_ENTRIES || receipts.len()>MAX_ENTRIES{return Err(ArchiveError::Limit.into())}
            let mut updates=Vec::new();
            for update in tails{let seq=update.cell(0)?.integer()?;let entry=format!("native/{kind}/{target}/{seq}.v1");entries.insert(entry.clone(),encode(&update.cell(2)?.bytes()?));updates.push(Update{seq,op_id:update.cell(1)?.id()?,payload_entry:entry,created_at:update.cell(3)?.datetime()?.to_rfc3339()});}
            let receipts=receipts.into_iter().map(|r|Ok(Receipt{op_id:r.cell(0)?.id()?,seq:r.cell(1)?.integer()?,payload_len:r.cell(2)?.integer()?,payload_sha256:hex::encode(r.cell(3)?.bytes()?),actor_user_id:r.cell(4)?.id()?,created_at:r.cell(5)?.datetime()?.to_rfc3339()})).collect::<Result<Vec<_>,sqlx::Error>>()?;
            let state=NativeState{target_kind:kind.into(),target_id:target,state_entry:name,encoding:i16::try_from(row.cell(2)?.integer()?).map_err(|_|ArchiveError::Invalid("native encoding".into()))?,snapshot_cutoff_seq:cutoff,tail_seq:tail,compacted_at:row.cell(5)?.optional(Cell::datetime)?.map(|v|v.to_rfc3339()),created_at:row.cell(6)?.datetime()?.to_rfc3339(),updated_at:row.cell(7)?.datetime()?.to_rfc3339(),updates,receipts};
            admit_graph_bytes(scope,serde_json::to_vec(&state).map_err(|_|ArchiveError::Invalid("native metadata".into()))?.len())?;states.push(state);
        }
    }
    Ok(states)
}
pub async fn capture_backend(
    backend: &Backend,
    workspace: Uuid,
    actor: Uuid,
    session: Uuid,
    selection: &Selection<'_>,
) -> Result<Capture, NativeDbError> {
    if let Backend::Postgres(pool) = backend {
        return capture(pool, workspace, actor, session, selection).await;
    }
    let mut tx = backend.begin_read().await?;
    let result=async{
        let mut op=tx.operation();op.set_tenant(workspace).await?;op.native_source(workspace,actor,session,selection).await?;
        let OperationTx::SqliteFamily(family)=&mut op else{unreachable!()};
        let mut scope=Scope{project:selection.project,wiki:Vec::new(),workspace,actor,connectors:selection.zotero_connectors.to_vec(),graph_bytes:std::sync::atomic::AtomicI64::new(0)};
        scope.wiki=family_wiki_closure(family,&scope).await?;family_capture_guards(family,&scope).await?;
        macro_rules! read{($table:literal)=>{native_family_rows(family,$table,&scope).await?}}
        let project=read!("projects").pop().ok_or(NativeDbError::Forbidden)?;
        let documents=read!("documents");let tasks=read!("tasks");let workflows=read!("workflows");let statuses=read!("statuses");let assignees=read!("task_assignees");let comments=read!("comments");let labels=read!("labels");let task_labels=read!("task_labels");let milestones=read!("milestones");let document_tag_assignments=read!("document_tag_assignments");let document_tags=read!("document_tags");let views=read!("views");let dependencies=read!("task_dependencies");let origins=read!("task_origins");let activity=read!("task_activity");let collections=read!("collections");let collection_items=read!("collection_items");let collection_fields=read!("collection_fields");let collection_options=read!("collection_options");let collection_values=read!("collection_values");let collection_choices=read!("collection_choices");let collection_people=read!("collection_people");let collection_views=read!("collection_views");
        admit_history_references(&scope,&activity,&views,&collection_views)?;
        let view_refs=view_filter_refs(&views,&collection_views);
        let live_labels=labels.iter().map(|r:&Label|r.id).collect::<std::collections::BTreeSet<_>>();let live_milestones=milestones.iter().map(|r:&Milestone|r.id).collect::<std::collections::BTreeSet<_>>();
        let(purged_labels,purged_milestones)=purged_refs(&activity,&view_refs,&live_labels,&live_milestones)?;
        for id in purged_labels.iter().chain(&purged_milestones){let rows=family.query("SELECT EXISTS(SELECT 1 FROM labels WHERE workspace_id=?1 AND id=?2) OR EXISTS(SELECT 1 FROM milestones WHERE workspace_id=?1 AND id=?2)",&[Cell::uuid(workspace),Cell::uuid(*id)]).await?;if rows[0].cell(0)?.boolean()?{return Err(ArchiveError::Unsupported("reference outside selected closure".into()).into())}}
        let mut entries=BTreeMap::new();let states=family_native_states(family,&scope,&mut entries).await?;
        let revision_values:Vec<Value>=native_family_rows(family,"revisions",&scope).await?;let mut revisions=Vec::new();
        for mut value in revision_values{let id=Uuid::parse_str(value["id"].as_str().ok_or_else(||ArchiveError::Invalid("revision id".into()))?).map_err(|_|ArchiveError::Invalid("revision id".into()))?;let bytes=family.query("SELECT y_snapshot FROM revisions WHERE workspace_id=?1 AND id=?2",&[Cell::uuid(workspace),Cell::uuid(id)]).await?;let name=format!("revisions/{id}.snapshot.v1");entries.insert(name.clone(),encode(&bytes[0].cell(0)?.bytes()?));value["snapshot_entry"]=json!(name);revisions.push(serde_json::from_value(value).map_err(|_|ArchiveError::Invalid("revision schema".into()))?);}
        let file_values:Vec<Value>=native_family_rows(family,"attachments",&scope).await?;let mut attachments=Vec::new();let mut file_keys=BTreeMap::new();
        for mut value in file_values{if value["status"]!="stored" || !matches!(value["scan_status"].as_str(),Some("skipped"|"clean")){return Err(ArchiveError::Unsupported("attachment must be stored and not infected".into()).into())}let id=Uuid::parse_str(value["id"].as_str().ok_or_else(||ArchiveError::Invalid("file id".into()))?).map_err(|_|ArchiveError::Invalid("file id".into()))?;file_keys.insert(id,value["storage_key"].as_str().ok_or_else(||ArchiveError::Invalid("file key".into()))?.to_owned());for key in ["status","scan_status","storage_key"]{value.as_object_mut().expect("captured file").remove(key);}value["payload_entry"]=json!(format!("attachments/{id}/payload"));attachments.push(serde_json::from_value(value).map_err(|_|ArchiveError::Invalid("file schema".into()))?);}
        let zotero_connectors=read!("zotero_connectors");let zotero_references=read!("zotero_references");let zotero_collections=read!("zotero_collections");let zotero_memberships=read!("zotero_memberships");let zotero_links=read!("zotero_links");let personal_input_commands=read!("personal_input_commands");let time_entries=read!("time_entries");let timer_runs=read!("task_timer_runs");let timer_segments=read!("task_timer_segments");let timer_legacy_open=read!("task_timer_legacy_open");let timer_commands=read!("task_timer_commands");let timer_audit=read!("task_timer_audit");
        let now=family.query("SELECT unixepoch()*1000000+CAST(substr(strftime('%f','now'),4,3) AS INTEGER)*1000",&[]).await?[0].cell(0)?.datetime()?.to_rfc3339();
        Ok(Capture{archive:Archive{graph:Graph{source_workspace_id:workspace,source_actor_id:actor,captured_at:now,project,workflows,statuses,documents,tasks,assignees,labels,task_labels,milestones,dependencies,views,document_tags,document_tag_assignments,origins,activity,purged_label_refs:purged_labels.into_iter().collect(),purged_milestone_refs:purged_milestones.into_iter().collect(),comments,states,revisions,attachments,collections,collection_items,collection_fields,collection_options,collection_values,collection_choices,collection_people,collection_views,zotero_connectors,zotero_references,zotero_collections,zotero_memberships,zotero_links,personal_input_commands,time_entries,timer_runs,timer_segments,timer_legacy_open,timer_commands,timer_audit,native_inventory:None},entries},file_keys})
    }.await;
    finish_native_read(tx, result).await
}

async fn family_file_current(
    tx: &mut FamilyTx,
    workspace: Uuid,
    project: Uuid,
    wiki: &[Uuid],
    file: &Attachment,
    key: &str,
) -> Result<(), NativeDbError> {
    tx.require_tenant(workspace)?;
    let selected = Cell::json(&json!(wiki
        .iter()
        .map(|id| hex::encode(id.as_bytes()))
        .collect::<Vec<_>>()))?;
    let rows=tx.query("SELECT EXISTS(SELECT 1 FROM attachments a LEFT JOIN documents d ON d.workspace_id=a.workspace_id AND d.id=a.document_id LEFT JOIN tasks t ON t.workspace_id=a.workspace_id AND t.id=a.task_id WHERE a.workspace_id=?1 AND a.id=?2 AND a.document_id IS ?3 AND a.task_id IS ?4 AND a.status='stored' AND a.scan_status IN('skipped','clean') AND a.storage_key=?5 AND a.size_bytes=?6 AND (((d.project_id=?7 OR(d.project_id IS NULL AND lower(hex(d.id)) IN(SELECT value FROM json_each(?8)))) AND d.deleted_at IS NULL) OR(t.project_id=?7 AND t.deleted_at IS NULL)))",&[Cell::uuid(workspace),Cell::uuid(file.id),Cell::optional_uuid(file.document_id),Cell::optional_uuid(file.task_id),Cell::text(key),Cell::Integer(file.size_bytes),Cell::uuid(project),selected]).await?;
    if !rows[0].cell(0)?.boolean()? {
        return Err(NativeDbError::Forbidden);
    }
    Ok(())
}
pub async fn recheck_file_backend(
    backend: &Backend,
    workspace: Uuid,
    actor: Uuid,
    session: Uuid,
    graph: &Graph,
    file: &Attachment,
    key: &str,
) -> Result<(), NativeDbError> {
    if let Backend::Postgres(pool) = backend {
        return recheck_file(pool, workspace, actor, session, graph, file, key).await;
    }
    let mut tx = backend.begin_read().await?;
    let result = async {
        let mut op = tx.operation();
        op.set_tenant(workspace).await?;
        let connectors = graph_connectors(graph);
        op.native_source(
            workspace,
            actor,
            session,
            &Selection {
                project: graph.project.id,
                zotero_connectors: &connectors,
            },
        )
        .await?;
        let OperationTx::SqliteFamily(family) = &mut op else {
            unreachable!()
        };
        family_file_current(
            family,
            workspace,
            graph.project.id,
            &graph_wiki(graph),
            file,
            key,
        )
        .await
    }
    .await;
    finish_native_read(tx, result).await
}
pub async fn recheck_delivery_backend(
    backend: &Backend,
    workspace: Uuid,
    actor: Uuid,
    session: Uuid,
    graph: &Graph,
    file_keys: &BTreeMap<Uuid, String>,
) -> Result<(), NativeDbError> {
    if let Backend::Postgres(pool) = backend {
        return recheck_delivery(pool, workspace, actor, session, graph, file_keys).await;
    }
    let mut tx = backend.begin_read().await?;
    let result = async {
        let mut op = tx.operation();
        op.set_tenant(workspace).await?;
        let connectors = graph_connectors(graph);
        op.native_source(
            workspace,
            actor,
            session,
            &Selection {
                project: graph.project.id,
                zotero_connectors: &connectors,
            },
        )
        .await?;
        let OperationTx::SqliteFamily(family) = &mut op else {
            unreachable!()
        };
        for document in &graph.documents {
            let rows = family
                .query(
                    "SELECT project_id,deleted_at FROM documents WHERE workspace_id=?1 AND id=?2",
                    &[Cell::uuid(workspace), Cell::uuid(document.id)],
                )
                .await?;
            let row = rows.first().ok_or(NativeDbError::Forbidden)?;
            if row.cell(0)?.optional(Cell::id)? != document.project_id
                || (document.deleted_at.is_none() && row.cell(1)? != Cell::Null)
            {
                return Err(NativeDbError::Forbidden);
            }
        }
        for task in &graph.tasks {
            let rows = family
                .query(
                    "SELECT project_id,deleted_at FROM tasks WHERE workspace_id=?1 AND id=?2",
                    &[Cell::uuid(workspace), Cell::uuid(task.id)],
                )
                .await?;
            let row = rows.first().ok_or(NativeDbError::Forbidden)?;
            if row.cell(0)?.id()? != graph.project.id
                || (task.deleted_at.is_none() && row.cell(1)? != Cell::Null)
            {
                return Err(NativeDbError::Forbidden);
            }
        }
        for file in &graph.attachments {
            family_file_current(
                family,
                workspace,
                graph.project.id,
                &graph_wiki(graph),
                file,
                file_keys.get(&file.id).ok_or(NativeDbError::Forbidden)?,
            )
            .await?;
        }
        Ok(())
    }
    .await;
    finish_native_read(tx, result).await
}
async fn finish_native_read<T>(
    tx: crate::db::backend::DbTx,
    result: Result<T, NativeDbError>,
) -> Result<T, NativeDbError> {
    match tx.rollback().await {
        Ok(()) => result,
        Err(cleanup) => {
            let original = result
                .err()
                .map(|e| Box::new(e) as Box<dyn std::error::Error + Send + Sync>);
            Err(crate::db::backend::rollback_cleanup_unknown(original, cleanup).into())
        }
    }
}
async fn finish_native_write<T>(
    tx: crate::db::backend::DbTx,
    result: Result<T, NativeDbError>,
) -> Result<T, NativeDbError> {
    match result {
        Ok(output) => {
            tx.commit().await.map_err(native_commit_error)?;
            Ok(output)
        }
        Err(error) => finish_native_read(tx, Err(error)).await,
    }
}

fn rows_collections(
    records: &mut Vec<(&'static str, Value)>,
    rows: &[Collection],
    workspace: Uuid,
    actor: Uuid,
) -> Result<(), NativeDbError> {
    for row in rows {
        records.push(("collections", mapped(row, workspace, actor)?));
    }
    Ok(())
}
async fn insert_native_pg_record(
    tx: &mut Transaction<'_, Postgres>,
    table: &str,
    mut value: Value,
    g: &Graph,
    workspace: Uuid,
    actor: Uuid,
) -> Result<(), NativeDbError> {
    let id = value
        .get("id")
        .and_then(Value::as_str)
        .and_then(|s| Uuid::parse_str(s).ok());
    if table == "collections" && value["kind"] == "task" {
        let c = g
            .collections
            .iter()
            .find(|r| Some(r.id) == id)
            .ok_or_else(|| ArchiveError::Invalid("task collection".into()))?;
        let changed=sqlx::query("UPDATE fvoci.collections SET id=$3, name=$4, version=$5, created_at=$6::timestamptz, updated_at=$7::timestamptz WHERE workspace_id=$1 AND project_id=$2 AND kind='task'")
            .bind(workspace).bind(g.project.id).bind(c.id).bind(&c.name).bind(c.version).bind(&c.created_at).bind(&c.updated_at).execute(&mut **tx).await?;
        if changed.rows_affected() != 1 {
            return Err(ArchiveError::Invalid("baseline collection reconciliation".into()).into());
        }
        return Ok(());
    }
    if table == "collection_items" && !value["task_id"].is_null() {
        let item = g
            .collection_items
            .iter()
            .find(|r| Some(r.id) == id)
            .ok_or_else(|| ArchiveError::Invalid("task collection item".into()))?;
        let changed=sqlx::query("UPDATE fvoci.collection_items SET id=$4, version=$5, created_at=$6::timestamptz, updated_at=$7::timestamptz WHERE workspace_id=$1 AND collection_id=$2 AND task_id=$3")
            .bind(workspace).bind(item.collection_id).bind(item.task_id).bind(item.id).bind(item.version).bind(&item.created_at).bind(&item.updated_at).execute(&mut **tx).await?;
        if changed.rows_affected() != 1 {
            return Err(
                ArchiveError::Invalid("baseline collection item reconciliation".into()).into(),
            );
        }
        return Ok(());
    }
    if table == "task_timer_audit" {
        let a: TimerAudit = serde_json::from_value(value)
            .map_err(|_| ArchiveError::Invalid("timer audit".into()))?;
        sqlx::query("INSERT INTO fvoci.task_timer_audit(id,user_id,request_id,workspace_id,task_id,time_entry_id,verb,before_value,after_value,reason,created_at) VALUES($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11::timestamptz)")
            .bind(a.id).bind(actor).bind(a.request_id).bind(a.workspace_id).bind(a.task_id).bind(a.time_entry_id).bind(&a.verb).bind(&a.before_value).bind(&a.after_value).bind(&a.reason).bind(&a.created_at).execute(&mut **tx).await?;
        return Ok(());
    }
    // Concrete portable rows use the maintained PostgreSQL typed-record insert.
    let inserted_time = if table == "time_entries" { id } else { None };
    insert_record(tx, table, value).await?;
    if let Some(id) = inserted_time {
        if g.time_entries
            .iter()
            .any(|e| e.id == id && e.ended_at.is_none())
            && !g.timer_legacy_open.iter().any(|l| l.time_entry_id == id)
        {
            sqlx::query(
                "DELETE FROM fvoci.task_timer_legacy_open WHERE time_entry_id=$1 AND user_id=$2",
            )
            .bind(id)
            .bind(actor)
            .execute(&mut **tx)
            .await?;
        }
    }
    if table == "projects" {
        sqlx::query("INSERT INTO fvoci.project_members(id,workspace_id,project_id,user_id,role) VALUES($1,$2,$3,$4,'lead')").bind(Uuid::now_v7()).bind(workspace).bind(g.project.id).bind(actor).execute(&mut **tx).await?;
    }
    Ok(())
}
async fn append_native_events(
    op: &mut OperationTx<'_, '_>,
    g: &Graph,
    claim: &ImportClaim,
    result: Value,
) -> Result<(), NativeDbError> {
    let workspace = claim.workspace_id;
    let actor = claim.created_by;
    for (kind, id) in g
        .documents
        .iter()
        .map(|d| ("document", d.id))
        .chain(g.tasks.iter().map(|t| ("task", t.id)))
    {
        op.append_event(EventAppend {
            id: Uuid::now_v7(),
            workspace_id: Some(workspace),
            actor_user_id: Some(actor),
            verb: format!("{kind}.created"),
            target_type: Some(kind.into()),
            target_id: Some(id),
            payload: json!({"nativeRestoreJobId":claim.job_id}),
        })
        .await?;
    }
    op.append_event(EventAppend {
        id: Uuid::now_v7(),
        workspace_id: Some(workspace),
        actor_user_id: Some(actor),
        verb: "native_archive.restored".into(),
        target_type: Some("project".into()),
        target_id: Some(g.project.id),
        payload: result.clone(),
    })
    .await?;
    op.append_audit(AuditAppend {
        id: Uuid::now_v7(),
        workspace_id: Some(workspace),
        actor_user_id: Some(actor),
        verb: "native_archive.restored".into(),
        target_type: Some("project".into()),
        target_id: Some(g.project.id),
        payload: result,
        ip: None,
    })
    .await?;
    Ok(())
}

impl OperationTx<'_, '_> {
    /// Closed metadata-only collision guard. Restore the borrowed context
    /// before business mutation; this does not grant tenant or user authority.
    async fn native_key_available(
        &mut self,
        workspace: Uuid,
        attachment: Uuid,
        key: &str,
        published: bool,
    ) -> Result<(), NativeDbError> {
        if Uuid::parse_str(key).is_err()
            || Uuid::parse_str(key).is_ok_and(|id| id.to_string() != key)
        {
            return Err(ArchiveError::Invalid("storage key".into()).into());
        }
        let previous = self.set_system().await?;
        let result: Result<bool, sqlx::Error> = async {
            match self {
                Self::Postgres(pg) => sqlx::query_scalar(
                    "SELECT NOT EXISTS(SELECT 1 FROM fvoci.attachments WHERE (storage_key=$3 AND NOT($4 AND workspace_id=$1 AND id=$2)) OR variants->'preview'->>'key'=$3) AND NOT EXISTS(SELECT 1 FROM fvoci.attachment_object_cleanups WHERE storage_key=$3 AND NOT(workspace_id=$1 AND attachment_id=$2))",
                ).bind(workspace).bind(attachment).bind(key).bind(published)
                    .fetch_one(&mut ***pg).await,
                Self::SqliteFamily(family) => {
                    family.require_writer()?;
                    let rows = family.query(
                        "SELECT NOT EXISTS(SELECT 1 FROM attachments WHERE (storage_key=?3 AND NOT(?4 AND workspace_id=?1 AND id=?2)) OR json_extract(variants,'$.preview.key')=?3) AND NOT EXISTS(SELECT 1 FROM attachment_object_cleanups WHERE storage_key=?3 AND NOT(workspace_id=?1 AND attachment_id=?2))",
                        &[Cell::uuid(workspace), Cell::uuid(attachment), Cell::text(key), Cell::Integer(i64::from(published))],
                    ).await?;
                    rows[0].cell(0)?.boolean()
                }
            }
        }.await;
        self.restore_system(previous).await?;
        if !result? {
            return Err(NativeDbError::Fenced);
        }
        Ok(())
    }
}

#[cfg(all(test, feature = "db-tests"))]
mod selected_tests {
    use super::*;
    use std::path::PathBuf;
    use std::sync::{Mutex, OnceLock};

    // One-shot, exact-job faults. No production build or parallel actor sees
    // another job's fault, and no fake migration or COMMIT substitute is used.
    enum Fault {
        Statement,
        Deferred(Uuid),
        Cancel,
    }
    static FAULTS: OnceLock<Mutex<BTreeMap<Uuid, Fault>>> = OnceLock::new();
    // Exact-job propagation oracle AFTER the original PostgreSQL rollback
    // returned ACK. It is synthetic returned evidence, never provider loss.
    static PG_ROLLBACK_PROPAGATION: OnceLock<Mutex<std::collections::BTreeSet<Uuid>>> =
        OnceLock::new();
    pub(super) fn after_acknowledged_pg_rollback(
        job: Uuid,
        rollback: Result<(), sqlx::Error>,
    ) -> Result<(), sqlx::Error> {
        rollback?;
        if PG_ROLLBACK_PROPAGATION
            .get_or_init(Default::default)
            .lock()
            .unwrap()
            .remove(&job)
        {
            return Err(sqlx::Error::Protocol(
                "synthetic propagation AFTER real PostgreSQL rollback ACK".into(),
            ));
        }
        Ok(())
    }

    #[tokio::test]
    async fn native_archive_pg_rollback_ack_primary_and_synthetic_propagation() {
        let url = std::env::var("FVOCI_NATIVE_PG_APP_URL")
            .expect("root-allocated current-schema restricted PG app fixture required");
        let pool = crate::db::pool::connect_app_with_max(&url, 1)
            .await
            .unwrap();
        crate::db::migrate::assert_app_role(&pool).await.unwrap();
        crate::db::migrate::assert_schema_current(&pool)
            .await
            .unwrap();
        let claim = ImportClaim {
            workspace_id: Uuid::now_v7(),
            job_id: Uuid::now_v7(),
            lease_token: Uuid::now_v7(),
            attempt: 1,
            created_by: Uuid::now_v7(),
            session_id: Uuid::now_v7(),
            source: crate::db::import_jobs::ImportSource::NativeArchive,
            file_name: None,
            project_id: None,
            prior_refs: Default::default(),
        };
        // Valid structural graph, absent current actor/session: real retained
        // publisher denies under the restricted app role before graph effects.
        let archive = crate::native_archive::tests::policy_fixture();
        let quota = crate::db::quota::StorageQuota::Unlimited;
        let primary = super::publish(&pool, &claim, &archive, &BTreeMap::new(), &quota).await;
        assert!(matches!(primary, Err(NativeDbError::Forbidden)));
        assert!(PG_ROLLBACK_PROPAGATION
            .get_or_init(Default::default)
            .lock()
            .unwrap()
            .insert(claim.job_id));
        let failure = super::publish(&pool, &claim, &archive, &BTreeMap::new(), &quota)
            .await
            .unwrap_err();
        let NativeDbError::Sql(error) = failure else {
            panic!("synthetic rollback propagation lost its typed SQL receipt");
        };
        assert!(crate::db::backend::is_rollback_cleanup_unknown(&error));
        let sqlx::Error::AnyDriverError(error) = error else {
            unreachable!()
        };
        let receipt = error
            .downcast_ref::<crate::db::backend::RollbackCleanupUnknown>()
            .unwrap();
        assert!(matches!(
            receipt
                .original
                .as_ref()
                .and_then(|error| error.downcast_ref::<NativeDbError>()),
            Some(NativeDbError::Forbidden)
        ));
        assert!(
            matches!(&receipt.cleanup, sqlx::Error::Protocol(message) if message == "synthetic propagation AFTER real PostgreSQL rollback ACK")
        );
        assert!(!PG_ROLLBACK_PROPAGATION
            .get_or_init(Default::default)
            .lock()
            .unwrap()
            .contains(&claim.job_id));
        // The real rollback was ACKed in both controls; a fresh max1 writer is
        // usable without treating the synthetic error as provider settlement.
        let tx = pool.begin().await.unwrap();
        tx.rollback().await.unwrap();
        pool.close().await;
        println!("PG actual refused publication/rollback ACK; synthetic returned-cleanup propagation only, no provider-loss claim");
    }
    fn fault(job: Uuid, value: Fault) {
        assert!(FAULTS
            .get_or_init(Default::default)
            .lock()
            .unwrap()
            .insert(job, value)
            .is_none());
    }
    pub(super) async fn before_finish(
        op: &mut OperationTx<'_, '_>,
        claim: &ImportClaim,
        cancel: &CancellationToken,
    ) -> Result<(), NativeDbError> {
        let selected = FAULTS
            .get_or_init(Default::default)
            .lock()
            .unwrap()
            .remove(&claim.job_id);
        match selected {
            None => Ok(()),
            Some(Fault::Cancel) => {
                cancel.cancel();
                Ok(())
            }
            Some(Fault::Statement) => {
                match op {
                    OperationTx::Postgres(pg) => {
                        sqlx::query("UPDATE fvoci.import_jobs SET native_result='invalid-json'::jsonb WHERE workspace_id=$1 AND id=$2").bind(claim.workspace_id).bind(claim.job_id).execute(&mut ***pg).await?;
                    }
                    OperationTx::SqliteFamily(family) => {
                        family.execute("UPDATE import_jobs SET native_result='invalid-json' WHERE workspace_id=?1 AND id=?2", &[Cell::uuid(claim.workspace_id),Cell::uuid(claim.job_id)]).await?;
                    }
                }
                panic!("actual current-schema JSON constraint did not refuse the statement")
            }
            Some(Fault::Deferred(connector)) => {
                match op {
                    OperationTx::Postgres(pg) => {
                        sqlx::query("INSERT INTO fvoci.zotero_collections(workspace_id,owner_user_id,connector_id,collection_key,remote_version,name,parent_key) VALUES($1,$2,$3,'ABCDEFGH',1,'deferred native control','BCDEFGHJ')").bind(claim.workspace_id).bind(claim.created_by).bind(connector).execute(&mut ***pg).await?;
                    }
                    OperationTx::SqliteFamily(family) => {
                        family.execute("INSERT INTO zotero_collections(workspace_id,owner_user_id,connector_id,collection_key,remote_version,name,parent_key) VALUES(?1,?2,?3,'ABCDEFGH',1,'deferred native control','BCDEFGHJ')", &[Cell::uuid(claim.workspace_id),Cell::uuid(claim.created_by),Cell::uuid(connector)]).await?;
                    }
                }
                Ok(())
            }
        }
    }

    struct Fixture {
        root: PathBuf,
        pool: sqlx::SqlitePool,
        backend: Backend,
        workspace: Uuid,
        actor: Uuid,
        session: Uuid,
    }
    impl Fixture {
        async fn new() -> Self {
            let root =
                std::env::temp_dir().join(format!("fvoci-native-adapter-{}", Uuid::now_v7()));
            std::fs::create_dir_all(&root).unwrap();
            let path = root.join("app.sqlite");
            crate::db::migrate::run_sqlite_migrations(&path)
                .await
                .unwrap();
            // One app connection makes an accidental nested writer hang/fail
            // the real runner's existing limit, rather than silently succeeding.
            let pool = crate::db::pool::connect_sqlite_app(&path, 1).await.unwrap();
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
            let workspace = Uuid::now_v7();
            let actor = Uuid::now_v7();
            let session = Uuid::now_v7();
            sqlx::query("INSERT INTO workspaces(id,slug,name,kind) VALUES(?1,'native-control','populated current workspace','personal')").bind(workspace.as_bytes().as_slice()).execute(&pool).await.unwrap();
            sqlx::query("INSERT INTO users(id,email,given_name,personal_workspace_id) VALUES(?1,'native-control@example.invalid','Native control',?2)").bind(actor.as_bytes().as_slice()).bind(workspace.as_bytes().as_slice()).execute(&pool).await.unwrap();
            sqlx::query("INSERT INTO memberships(workspace_id,user_id,role) VALUES(?1,?2,'owner')")
                .bind(workspace.as_bytes().as_slice())
                .bind(actor.as_bytes().as_slice())
                .execute(&pool)
                .await
                .unwrap();
            sqlx::query("INSERT INTO sessions(id,user_id,token_hash,expires_at) VALUES(?1,?2,'native-control-token',unixepoch()*1000000+3600000000)").bind(session.as_bytes().as_slice()).bind(actor.as_bytes().as_slice()).execute(&pool).await.unwrap();
            Self {
                root,
                pool,
                backend,
                workspace,
                actor,
                session,
            }
        }
        async fn claim(&self) -> (ImportClaim, Uuid, String) {
            // This fixture exercises DB policy and row identity. It is NOT a
            // parser/native-engine or whole archive round-trip acceptance case.
            let bytes = b"native adapter DB policy input";
            self.claim_payload(bytes).await
        }
        async fn claim_payload(&self, bytes: &[u8]) -> (ImportClaim, Uuid, String) {
            let hash = digest(bytes);
            let request = Uuid::now_v7();
            let id = queue_restore_backend(
                &self.backend,
                self.workspace,
                self.actor,
                self.session,
                request,
                &hash,
                bytes,
            )
            .await
            .unwrap();
            let claim = crate::db::import_jobs::claim_next_import_job_backend(&self.backend)
                .await
                .unwrap()
                .unwrap();
            assert_eq!(claim.job_id, id);
            assert_eq!(claim.workspace_id, self.workspace);
            (claim, request, hash)
        }
        async fn graph_counts(&self) -> (i64, i64, i64, i64, i64, i64, i64, i64) {
            sqlx::query_as("SELECT (SELECT count(*) FROM projects WHERE workspace_id=?1),(SELECT count(*) FROM documents WHERE workspace_id=?1),(SELECT count(*) FROM tasks WHERE workspace_id=?1),(SELECT count(*) FROM document_states WHERE workspace_id=?1)+(SELECT count(*) FROM task_states WHERE workspace_id=?1),(SELECT count(*) FROM revisions WHERE workspace_id=?1),(SELECT count(*) FROM attachments WHERE workspace_id=?1),(SELECT count(*) FROM events WHERE workspace_id=?1),(SELECT count(*) FROM audit_log WHERE workspace_id=?1)")
                .bind(self.workspace.as_bytes().as_slice()).fetch_one(&self.pool).await.unwrap()
        }
        async fn unpublished(&self, claim: &ImportClaim) {
            assert_eq!(self.graph_counts().await, (0, 0, 0, 0, 0, 0, 0, 0));
            let row:(String,Option<String>,Option<Vec<u8>>,Option<Vec<u8>>)=sqlx::query_as("SELECT status,native_result,payload,lease_token FROM import_jobs WHERE workspace_id=?1 AND id=?2").bind(self.workspace.as_bytes().as_slice()).bind(claim.job_id.as_bytes().as_slice()).fetch_one(&self.pool).await.unwrap();
            assert_eq!(row.0, "running");
            assert_eq!(row.1, None);
            assert!(row.2.is_some());
            assert_eq!(
                row.3.as_deref(),
                Some(claim.lease_token.as_bytes().as_slice())
            );
            assert_eq!(
                sqlx::query_scalar::<_, String>("SELECT name FROM workspaces WHERE id=?1")
                    .bind(self.workspace.as_bytes().as_slice())
                    .fetch_one(&self.pool)
                    .await
                    .unwrap(),
                "populated current workspace"
            );
        }
        async fn close(self) {
            self.backend.close().await.unwrap();
            std::fs::remove_dir_all(&self.root).unwrap();
            assert!(!self.root.exists());
        }
    }
    fn archive_with_file() -> Archive {
        let mut archive = crate::native_archive::tests::policy_fixture();
        let bytes = b"native staged file";
        let id = Uuid::now_v7();
        archive.graph.attachments.push(serde_json::from_value(json!({"id":id,"document_id":archive.graph.documents[0].id,"task_id":null,"name":"native.txt","mime":"text/plain","declared_mime":null,"size_bytes":bytes.len(),"uploader_id":archive.graph.source_actor_id,"created_at":"2026-10-02T00:00:00Z","completed_at":"2026-10-02T00:00:00Z","image":false,"payload_entry":format!("attachments/{id}/payload")})).unwrap());
        archive
            .entries
            .insert(format!("attachments/{id}/payload"), encode(bytes));
        archive.validate().unwrap();
        archive
    }
    async fn publish(
        f: &Fixture,
        claim: &ImportClaim,
        archive: &Archive,
        keys: &BTreeMap<Uuid, String>,
        hash: &str,
        cancel: &CancellationToken,
    ) -> Result<(), NativeDbError> {
        publish_backend(
            &f.backend,
            claim,
            archive,
            keys,
            &crate::db::quota::StorageQuota::Unlimited,
            hash,
            cancel,
        )
        .await
    }

    // These bytes come from the maintained child, with retained deleted
    // structs. The policy fixture above deliberately supplies no such proof.
    async fn real_history_archive(config: &crate::collab::CollabConfig) -> Archive {
        use collab_engine::outcome::EngineStatus;
        use collab_engine::process::{ChildSlotKind, EngineSession, SpawnRequest};
        use collab_engine::protocol::Request;

        fn bytes(child: &mut EngineSession, request: Request) -> Vec<u8> {
            match child.call(&request).outcome {
                EngineStatus::Ok {
                    update_b64: Some(value),
                    ..
                } => collab_engine::b64::decode(&value).unwrap(),
                other => panic!("native corpus byte command failed: {other:?}"),
            }
        }
        fn body(child: &mut EngineSession) -> Value {
            match child.call(&Request::Project { encoding: 1 }).outcome {
                EngineStatus::Ok {
                    content_json: Some(value),
                    ..
                } => value,
                other => panic!("native corpus projection failed: {other:?}"),
            }
        }
        let engine_bin = config.engine_bin.clone();
        let limits = config.limits;
        let archive = tokio::task::spawn_blocking(move || {
            let mut archive = archive_with_file();
            for index in 0..archive.graph.states.len() {
                let kind = archive.graph.states[index].target_kind.clone();
                let target = archive.graph.states[index].target_id;
                let block = format!("retained-{kind}-block");
                let content = |text: &str| json!({"type":"doc","content":[{"type":"paragraph","attrs":{"id":block},"content":[{"type":"text","text":text,"marks":[{"type":"bold"}]}]}]});
                let first_body = content("보존 원본 🧪");
                let final_body = content("보존 후속 🧪");
                let mut child = EngineSession::spawn(SpawnRequest {
                    engine_bin: engine_bin.clone(), limits,
                    slot_kind: ChildSlotKind::Primary, slot_wait: None,
                    test_hang_ms: None, test_exit_after_read: None,
                    test_close_stdout_hang_ms: None, test_exit_after_write: None,
                }).unwrap();
                let initial = bytes(&mut child, Request::SeedFromTiptap {
                    content_json: serde_json::to_string(&first_body).unwrap(), encoding: 1,
                });
                assert!(initial.len() > 2, "empty policy seed is not a history corpus");
                assert!(child.call(&Request::Load {
                    snapshot_b64: Some(initial.clone()), tail_b64: vec![], encoding: 1,
                }).outcome.is_applied_ok());
                assert_eq!(body(&mut child), first_body);
                let compacted = bytes(&mut child, Request::Snapshot);
                let manual_snapshot = bytes(&mut child, Request::RevisionSnapshot);
                let replacement = bytes(&mut child, Request::SeedFromTiptap {
                    content_json: serde_json::to_string(&final_body).unwrap(), encoding: 1,
                });
                let forward = bytes(&mut child, Request::ReplaceFromUpdate {
                    update_b64: replacement, encoding: 1,
                });
                assert!(child.call(&Request::Apply {
                    update_b64: forward.clone(), encoding: 1,
                }).outcome.is_applied_ok());
                assert_eq!(body(&mut child), final_body);
                let session_snapshot = bytes(&mut child, Request::RevisionSnapshot);
                let scheduled_snapshot = bytes(&mut child, Request::RevisionSnapshot);
                assert_ne!(manual_snapshot, session_snapshot);
                assert_eq!(session_snapshot, scheduled_snapshot);
                let restore_update = bytes(&mut child, Request::RestoreFromSnapshot {
                    snap_b64: manual_snapshot.clone(), encoding: 1,
                });
                assert!(child.call(&Request::Apply {
                    update_b64: restore_update.clone(), encoding: 1,
                }).outcome.is_applied_ok());
                assert_eq!(body(&mut child), first_body);
                let restored_snapshot = bytes(&mut child, Request::RevisionSnapshot);
                drop(child); // Awaited child kill/reap is owned by EngineSession.

                let at = "2026-10-02T00:00:00Z";
                let first_op = Uuid::now_v7();
                let second_op = Uuid::now_v7();
                let restore_op = Uuid::now_v7();
                let state = &mut archive.graph.states[index];
                state.snapshot_cutoff_seq = 1;
                state.tail_seq = 3;
                state.compacted_at = Some(at.into());
                archive.entries.insert(state.state_entry.clone(), encode(&compacted));
                state.updates = [(second_op, 2, &forward), (restore_op, 3, &restore_update)]
                    .into_iter().map(|(op_id, seq, payload)| {
                        let payload_entry = format!("native/{kind}/{target}/{seq}.v1");
                        archive.entries.insert(payload_entry.clone(), encode(payload));
                        Update { seq, op_id, payload_entry, created_at: at.into() }
                    }).collect();
                state.receipts = [(first_op, 1, &initial), (second_op, 2, &forward), (restore_op, 3, &restore_update)]
                    .into_iter().map(|(op_id, seq, payload)| Receipt {
                        op_id, seq, payload_len: payload.len() as i64,
                        payload_sha256: digest(payload), actor_user_id: archive.graph.source_actor_id,
                        created_at: at.into(),
                    }).collect();
                let manual_id = Uuid::now_v7();
                for (id, reason, snapshot, content_json) in [
                    (manual_id, "manual", manual_snapshot, first_body.clone()),
                    (Uuid::now_v7(), "session", session_snapshot, final_body.clone()),
                    (Uuid::now_v7(), "scheduled", scheduled_snapshot, final_body),
                    (Uuid::now_v7(), "restore", restored_snapshot, first_body.clone()),
                ] {
                    let snapshot_entry = format!("revisions/{id}.snapshot.v1");
                    archive.entries.insert(snapshot_entry.clone(), encode(&snapshot));
                    archive.graph.revisions.push(Revision {
                        id, target_kind: kind.clone(), target_id: target, snapshot_entry,
                        encoding: 1, text: crate::collab::revision::prepare_revision_text(&content_json).unwrap(),
                        content_json, reason: reason.into(),
                        created_by: (reason != "scheduled").then_some(archive.graph.source_actor_id),
                        created_at: at.into(), restored_from_id: (reason == "restore").then_some(manual_id),
                        restore_correlation_id: (reason == "restore").then(Uuid::now_v7),
                        restore_base_tail_seq: (reason == "restore").then_some(2),
                        restore_committed_tail_seq: (reason == "restore").then_some(3),
                    });
                }
                let (content_json, text, chosung) = crate::collab::derived_body::prepare_derived_body(first_body).unwrap().into_parts();
                if kind == "document" {
                    let row = archive.graph.documents.iter_mut().find(|row| row.id == target).unwrap();
                    row.content_json = content_json; row.text = text; row.chosung = chosung;
                } else {
                    let row = archive.graph.tasks.iter_mut().find(|row| row.id == target).unwrap();
                    row.content_json = content_json; row.text = text; row.chosung = chosung;
                }
            }
            archive.validate().unwrap();
            archive
        }).await.unwrap();
        crate::native_archive::validate_native(archive, config.clone(), &CancellationToken::new())
            .await
            .expect("real native history must validate; no missing-engine skip")
    }

    #[tokio::test]
    async fn native_archive_selected_real_history_bytes_receipts_revisions_and_permissions() {
        assert!(
            std::env::var_os("FVOCI_COLLAB_ENGINE").is_some(),
            "allocated FVOCI_COLLAB_ENGINE is required"
        );
        let config = crate::collab::CollabConfig::from_env()
            .expect("allocated current native engine required");
        let archive = real_history_archive(&config).await;
        let mut wrong_projection = archive.clone();
        wrong_projection.graph.documents[0].content_json =
            archive.graph.revisions[1].content_json.clone();
        let prepared = crate::collab::derived_body::prepare_derived_body(
            wrong_projection.graph.documents[0].content_json.clone(),
        )
        .unwrap();
        wrong_projection.graph.documents[0].text = prepared.text().into();
        wrong_projection.graph.documents[0].chosung = prepared.chosung().into();
        wrong_projection.validate().unwrap(); // Structural validity alone is insufficient.
        assert!(crate::native_archive::validate_native(
            wrong_projection,
            config.clone(),
            &CancellationToken::new()
        )
        .await
        .is_err());

        let f = Fixture::new().await;
        // This same-writer adapter oracle uses graph JSON as its durable input;
        // actual ZIP/container + HTTP/import consumer qualification is separate.
        let payload = serde_json::to_vec(&archive).unwrap();
        let (claim, request, hash) = f.claim_payload(&payload).await;
        let file = &archive.graph.attachments[0];
        let key = Uuid::now_v7().to_string();
        let keys = BTreeMap::from([(file.id, key.clone())]);
        stage_key_backend(&f.backend, &claim, file.id, &key, &CancellationToken::new())
            .await
            .unwrap();
        let file_bytes = archive.bytes(&file.payload_entry).unwrap();
        let file_path = f.root.join(&key);
        std::fs::write(&file_path, &file_bytes).unwrap();
        // A late actual statement failure rolls back graph/history/finish while
        // retaining the pre-I/O journal and exact file for the same command.
        fault(claim.job_id, Fault::Statement);
        assert!(publish(
            &f,
            &claim,
            &archive,
            &keys,
            &hash,
            &CancellationToken::new()
        )
        .await
        .is_err());
        f.unpublished(&claim).await;
        assert_eq!(std::fs::read(&file_path).unwrap(), file_bytes);
        assert_eq!(
            sqlx::query_scalar::<_, i64>(
                "SELECT count(*) FROM attachment_object_cleanups WHERE storage_key=?1"
            )
            .bind(&key)
            .fetch_one(&f.pool)
            .await
            .unwrap(),
            1
        );
        publish(
            &f,
            &claim,
            &archive,
            &keys,
            &hash,
            &CancellationToken::new(),
        )
        .await
        .unwrap();
        assert_eq!(f.graph_counts().await, (1, 1, 1, 2, 8, 1, 3, 1));
        assert_eq!(
            queue_restore_backend(
                &f.backend,
                f.workspace,
                f.actor,
                f.session,
                request,
                &hash,
                &payload
            )
            .await
            .unwrap(),
            claim.job_id
        );
        assert!(matches!(
            publish(
                &f,
                &claim,
                &archive,
                &keys,
                &hash,
                &CancellationToken::new()
            )
            .await,
            Err(NativeDbError::Conflict) | Err(NativeDbError::Fenced)
        ));
        assert_eq!(f.graph_counts().await, (1, 1, 1, 2, 8, 1, 3, 1));

        let fresh = Backend::Sqlite(
            crate::db::pool::connect_sqlite_app(&f.root.join("app.sqlite"), 1)
                .await
                .unwrap(),
        );
        let selection = Selection {
            project: archive.graph.project.id,
            zotero_connectors: &[],
        };
        let mut captured = capture_backend(&fresh, f.workspace, f.actor, f.session, &selection)
            .await
            .unwrap();
        assert_eq!(captured.file_keys, keys);
        recheck_file_backend(
            &fresh,
            f.workspace,
            f.actor,
            f.session,
            &captured.archive.graph,
            &captured.archive.graph.attachments[0],
            &key,
        )
        .await
        .unwrap();
        let readback = std::fs::read(&file_path).unwrap();
        assert_eq!(digest(&readback), digest(&file_bytes));
        captured
            .archive
            .entries
            .insert(file.payload_entry.clone(), encode(&readback));
        assert_eq!(
            captured.archive.graph.documents[0].id,
            archive.graph.documents[0].id
        );
        assert_eq!(
            captured.archive.graph.documents[0].content_json,
            archive.graph.documents[0].content_json
        );
        assert_eq!(
            captured.archive.graph.tasks[0].content_json,
            archive.graph.tasks[0].content_json
        );
        for state in &archive.graph.states {
            let current = captured
                .archive
                .graph
                .states
                .iter()
                .find(|row| {
                    row.target_kind == state.target_kind && row.target_id == state.target_id
                })
                .unwrap();
            assert_eq!((current.snapshot_cutoff_seq, current.tail_seq), (1, 3));
            assert_eq!(
                current
                    .compacted_at
                    .as_deref()
                    .map(|value| chrono::DateTime::parse_from_rfc3339(value).unwrap()),
                state
                    .compacted_at
                    .as_deref()
                    .map(|value| chrono::DateTime::parse_from_rfc3339(value).unwrap())
            );
            assert_eq!(
                captured.archive.bytes(&current.state_entry).unwrap(),
                archive.bytes(&state.state_entry).unwrap()
            );
            assert_eq!(current.updates.len(), 2);
            assert_eq!(current.receipts.len(), 3);
            for expected in &state.updates {
                let actual = current
                    .updates
                    .iter()
                    .find(|row| row.op_id == expected.op_id)
                    .unwrap();
                assert_eq!(actual.seq, expected.seq);
                assert_eq!(
                    chrono::DateTime::parse_from_rfc3339(&actual.created_at).unwrap(),
                    chrono::DateTime::parse_from_rfc3339(&expected.created_at).unwrap()
                );
                assert_eq!(
                    captured.archive.bytes(&actual.payload_entry).unwrap(),
                    archive.bytes(&expected.payload_entry).unwrap()
                );
            }
            for expected in &state.receipts {
                let actual = current
                    .receipts
                    .iter()
                    .find(|row| row.op_id == expected.op_id)
                    .unwrap();
                assert_eq!(
                    (actual.seq, actual.payload_len),
                    (expected.seq, expected.payload_len)
                );
                assert_eq!(actual.payload_sha256, expected.payload_sha256);
                assert_eq!(actual.actor_user_id, f.actor);
                assert_eq!(
                    chrono::DateTime::parse_from_rfc3339(&actual.created_at).unwrap(),
                    chrono::DateTime::parse_from_rfc3339(&expected.created_at).unwrap()
                );
            }
        }
        assert_eq!(captured.archive.graph.revisions.len(), 8);
        for expected in &archive.graph.revisions {
            let actual = captured
                .archive
                .graph
                .revisions
                .iter()
                .find(|row| row.id == expected.id)
                .unwrap();
            assert_eq!(
                (&actual.target_kind, actual.target_id),
                (&expected.target_kind, expected.target_id)
            );
            assert_eq!(actual.reason, expected.reason);
            assert_eq!(actual.content_json, expected.content_json);
            assert_eq!(actual.text, expected.text);
            assert_eq!(actual.created_by, expected.created_by.map(|_| f.actor));
            assert_eq!(
                chrono::DateTime::parse_from_rfc3339(&actual.created_at).unwrap(),
                chrono::DateTime::parse_from_rfc3339(&expected.created_at).unwrap()
            );
            assert_eq!(
                (
                    actual.restored_from_id,
                    actual.restore_correlation_id,
                    actual.restore_base_tail_seq,
                    actual.restore_committed_tail_seq
                ),
                (
                    expected.restored_from_id,
                    expected.restore_correlation_id,
                    expected.restore_base_tail_seq,
                    expected.restore_committed_tail_seq
                )
            );
            assert_eq!(
                captured.archive.bytes(&actual.snapshot_entry).unwrap(),
                archive.bytes(&expected.snapshot_entry).unwrap()
            );
        }
        let actual_file = &captured.archive.graph.attachments[0];
        assert_eq!(actual_file.id, file.id);
        assert_eq!(
            (actual_file.document_id, actual_file.task_id),
            (file.document_id, file.task_id)
        );
        assert_eq!(
            (
                &actual_file.name,
                &actual_file.mime,
                &actual_file.declared_mime,
                actual_file.size_bytes,
                actual_file.image
            ),
            (
                &file.name,
                &file.mime,
                &file.declared_mime,
                file.size_bytes,
                file.image
            )
        );
        assert_eq!(actual_file.uploader_id, f.actor);
        assert_eq!(
            chrono::DateTime::parse_from_rfc3339(&actual_file.completed_at).unwrap(),
            chrono::DateTime::parse_from_rfc3339(&file.completed_at).unwrap()
        );
        let validated = crate::native_archive::validate_native(
            captured.archive.clone(),
            config,
            &CancellationToken::new(),
        )
        .await
        .unwrap();
        assert_eq!(validated.graph.native_inventory.as_ref().unwrap().len(), 2);
        recheck_delivery_backend(
            &fresh,
            f.workspace,
            f.actor,
            f.session,
            &captured.archive.graph,
            &keys,
        )
        .await
        .unwrap();
        sqlx::query("UPDATE memberships SET role='member' WHERE workspace_id=?1 AND user_id=?2")
            .bind(f.workspace.as_bytes().as_slice())
            .bind(f.actor.as_bytes().as_slice())
            .execute(&f.pool)
            .await
            .unwrap();
        assert!(matches!(
            capture_backend(&fresh, f.workspace, f.actor, f.session, &selection).await,
            Err(NativeDbError::Forbidden)
        ));
        assert!(matches!(
            recheck_delivery_backend(
                &fresh,
                f.workspace,
                f.actor,
                f.session,
                &captured.archive.graph,
                &keys
            )
            .await,
            Err(NativeDbError::Forbidden)
        ));
        assert_eq!(f.graph_counts().await, (1, 1, 1, 2, 8, 1, 3, 1));
        assert_eq!(std::fs::read(&file_path).unwrap(), file_bytes);
        fresh.close().await.unwrap();
        f.close().await;
    }

    #[tokio::test]
    async fn native_archive_selected_current_schema_graph_capture_and_replay() {
        let f = Fixture::new().await;
        let (claim, request, hash) = f.claim().await;
        let mut archive = crate::native_archive::tests::policy_fixture();
        archive.graph.statuses[0].wip_limit = Some(3);
        archive.graph.tasks[0].estimate = Some(json!("9007199254740993.000000"));
        archive.validate().unwrap();
        publish(
            &f,
            &claim,
            &archive,
            &BTreeMap::new(),
            &hash,
            &CancellationToken::new(),
        )
        .await
        .unwrap();
        assert_eq!(f.graph_counts().await, (1, 1, 1, 2, 0, 0, 3, 1));
        let status = status_backend(&f.backend, f.workspace, f.actor, f.session, claim.job_id)
            .await
            .unwrap();
        assert_eq!(status.status, "completed");
        assert_eq!(status.project_id, Some(archive.graph.project.id));
        let replay = queue_restore_backend(
            &f.backend,
            f.workspace,
            f.actor,
            f.session,
            request,
            &hash,
            b"native adapter DB policy input",
        )
        .await
        .unwrap();
        assert_eq!(replay, claim.job_id);
        assert_eq!(f.graph_counts().await, (1, 1, 1, 2, 0, 0, 3, 1));
        assert!(matches!(
            publish(
                &f,
                &claim,
                &archive,
                &BTreeMap::new(),
                &hash,
                &CancellationToken::new()
            )
            .await,
            Err(NativeDbError::Conflict) | Err(NativeDbError::Fenced)
        ));
        let captured = capture_backend(
            &f.backend,
            f.workspace,
            f.actor,
            f.session,
            &Selection {
                project: archive.graph.project.id,
                zotero_connectors: &[],
            },
        )
        .await
        .unwrap();
        captured.archive.validate().unwrap();
        assert_eq!(captured.archive.graph.project.id, archive.graph.project.id);
        assert_eq!(
            captured.archive.graph.documents[0].content_json,
            archive.graph.documents[0].content_json
        );
        assert_eq!(
            captured.archive.graph.tasks[0].due_date,
            archive.graph.tasks[0].due_date
        );
        assert_eq!(
            captured.archive.graph.comments.len(),
            archive.graph.comments.len()
        );
        assert_eq!(
            captured.archive.graph.activity.len(),
            archive.graph.activity.len()
        );
        assert_eq!(
            captured.archive.graph.collections.len(),
            archive.graph.collections.len()
        );
        assert_eq!(
            captured.archive.graph.collection_items.len(),
            archive.graph.collection_items.len()
        );
        assert_eq!(captured.archive.graph.statuses[0].wip_limit, Some(3));
        assert_eq!(
            captured.archive.graph.tasks[0].estimate,
            archive.graph.tasks[0].estimate
        );
        assert!(captured
            .archive
            .graph
            .activity
            .iter()
            .all(|a| a.actor_user_id.is_none()));
        assert_eq!(captured.archive.graph.comments[0].created_by, f.actor);
        for source in &archive.graph.states {
            let current = captured
                .archive
                .graph
                .states
                .iter()
                .find(|s| s.target_kind == source.target_kind && s.target_id == source.target_id)
                .unwrap();
            assert_eq!(
                captured.archive.bytes(&current.state_entry).unwrap(),
                archive.bytes(&source.state_entry).unwrap()
            );
            assert_eq!(current.tail_seq, source.tail_seq);
            assert_eq!(current.snapshot_cutoff_seq, source.snapshot_cutoff_seq);
        }
        recheck_delivery_backend(
            &f.backend,
            f.workspace,
            f.actor,
            f.session,
            &captured.archive.graph,
            &captured.file_keys,
        )
        .await
        .unwrap();
        sqlx::query("UPDATE sessions SET revoked_at=unixepoch()*1000000 WHERE id=?1")
            .bind(f.session.as_bytes().as_slice())
            .execute(&f.pool)
            .await
            .unwrap();
        assert!(matches!(
            recheck_delivery_backend(
                &f.backend,
                f.workspace,
                f.actor,
                f.session,
                &captured.archive.graph,
                &captured.file_keys
            )
            .await,
            Err(NativeDbError::Forbidden)
        ));
        assert!(matches!(
            status_backend(&f.backend, f.workspace, f.actor, f.session, claim.job_id).await,
            Err(NativeDbError::Forbidden)
        ));
        assert_eq!(f.graph_counts().await, (1, 1, 1, 2, 0, 0, 3, 1));
        f.close().await;
    }

    #[tokio::test]
    async fn native_archive_selected_denied_cancelled_stale_and_hash_no_effects() {
        let f = Fixture::new().await;
        let (claim, _, hash) = f.claim().await;
        let archive = crate::native_archive::tests::policy_fixture();
        let cancelled = CancellationToken::new();
        cancelled.cancel();
        assert!(matches!(
            publish(&f, &claim, &archive, &BTreeMap::new(), &hash, &cancelled).await,
            Err(NativeDbError::Archive(ArchiveError::Cancelled))
        ));
        f.unpublished(&claim).await;
        let mut stale = claim.clone();
        stale.attempt += 1;
        assert!(matches!(
            publish(
                &f,
                &stale,
                &archive,
                &BTreeMap::new(),
                &hash,
                &CancellationToken::new()
            )
            .await,
            Err(NativeDbError::Fenced)
        ));
        f.unpublished(&claim).await;
        stale = claim.clone();
        stale.lease_token = Uuid::now_v7();
        assert!(matches!(
            publish(
                &f,
                &stale,
                &archive,
                &BTreeMap::new(),
                &hash,
                &CancellationToken::new()
            )
            .await,
            Err(NativeDbError::Fenced)
        ));
        f.unpublished(&claim).await;
        assert!(matches!(
            publish(
                &f,
                &claim,
                &archive,
                &BTreeMap::new(),
                "wrong payload hash",
                &CancellationToken::new()
            )
            .await,
            Err(NativeDbError::Conflict)
        ));
        f.unpublished(&claim).await;
        sqlx::query("UPDATE memberships SET role='admin' WHERE workspace_id=?1 AND user_id=?2")
            .bind(f.workspace.as_bytes().as_slice())
            .bind(f.actor.as_bytes().as_slice())
            .execute(&f.pool)
            .await
            .unwrap();
        assert!(matches!(
            publish(
                &f,
                &claim,
                &archive,
                &BTreeMap::new(),
                &hash,
                &CancellationToken::new()
            )
            .await,
            Err(NativeDbError::Forbidden)
        ));
        f.unpublished(&claim).await;
        sqlx::query("UPDATE memberships SET role='owner' WHERE workspace_id=?1 AND user_id=?2")
            .bind(f.workspace.as_bytes().as_slice())
            .bind(f.actor.as_bytes().as_slice())
            .execute(&f.pool)
            .await
            .unwrap();
        sqlx::query("UPDATE sessions SET revoked_at=unixepoch()*1000000 WHERE id=?1")
            .bind(f.session.as_bytes().as_slice())
            .execute(&f.pool)
            .await
            .unwrap();
        assert!(matches!(
            publish(
                &f,
                &claim,
                &archive,
                &BTreeMap::new(),
                &hash,
                &CancellationToken::new()
            )
            .await,
            Err(NativeDbError::Forbidden)
        ));
        f.unpublished(&claim).await;
        f.close().await;
    }

    #[tokio::test]
    async fn native_archive_selected_two_actual_writers_one_stable_request() {
        let f = Fixture::new().await;
        let second = crate::db::pool::connect_sqlite_app(&f.root.join("app.sqlite"), 1)
            .await
            .unwrap();
        let other = Backend::Sqlite(second);
        let request = Uuid::now_v7();
        let bytes = b"same actual two-writer command";
        let hash = digest(bytes);
        let (a, b) = tokio::join!(
            queue_restore_backend(
                &f.backend,
                f.workspace,
                f.actor,
                f.session,
                request,
                &hash,
                bytes
            ),
            queue_restore_backend(
                &other,
                f.workspace,
                f.actor,
                f.session,
                request,
                &hash,
                bytes
            )
        );
        assert_eq!(a.unwrap(), b.unwrap());
        assert_eq!(
            sqlx::query_scalar::<_, i64>("SELECT count(*) FROM import_jobs")
                .fetch_one(&f.pool)
                .await
                .unwrap(),
            1
        );
        assert!(matches!(
            queue_restore_backend(
                &other,
                f.workspace,
                f.actor,
                f.session,
                request,
                &digest(b"different"),
                b"different"
            )
            .await,
            Err(NativeDbError::Conflict)
        ));
        other.close().await.unwrap();
        f.close().await;
    }

    #[tokio::test]
    async fn native_archive_selected_journal_partial_file_failure_and_exact_transfer() {
        let f = Fixture::new().await;
        let (claim, _, hash) = f.claim().await;
        let archive = archive_with_file();
        let file = &archive.graph.attachments[0];
        let key = Uuid::now_v7().to_string();
        stage_key_backend(&f.backend, &claim, file.id, &key, &CancellationToken::new())
            .await
            .unwrap();
        let staged:(String,String,i64)=sqlx::query_as("SELECT storage_key,created_refs,(SELECT count(*) FROM attachment_object_cleanups) FROM import_jobs JOIN attachment_object_cleanups ON import_jobs.workspace_id=attachment_object_cleanups.workspace_id WHERE import_jobs.id=?1").bind(claim.job_id.as_bytes().as_slice()).fetch_one(&f.pool).await.unwrap();
        assert_eq!(staged.0, key);
        assert_eq!(staged.2, 1);
        assert!(
            serde_json::from_str::<Value>(&staged.1).unwrap()["storedKeys"]
                .as_array()
                .unwrap()
                .contains(&json!(key))
        );
        // A real partial file belongs to the exact journal; a refused publish
        // must preserve both rather than performing compensation or a purge.
        let object = f.root.join(&key);
        std::fs::write(&object, b"partial").unwrap();
        let inode_bytes = std::fs::read(&object).unwrap();
        let mut wrong = BTreeMap::new();
        wrong.insert(file.id, Uuid::now_v7().to_string());
        assert!(matches!(
            publish(
                &f,
                &claim,
                &archive,
                &wrong,
                &hash,
                &CancellationToken::new()
            )
            .await,
            Err(NativeDbError::Fenced)
        ));
        f.unpublished(&claim).await;
        assert_eq!(std::fs::read(&object).unwrap(), inode_bytes);
        let keys = [(file.id, key.clone())].into();
        fault(claim.job_id, Fault::Statement);
        assert!(matches!(
            publish(
                &f,
                &claim,
                &archive,
                &keys,
                &hash,
                &CancellationToken::new()
            )
            .await,
            Err(NativeDbError::Sql(_))
        ));
        f.unpublished(&claim).await;
        assert_eq!(
            sqlx::query_scalar::<_, i64>(
                "SELECT count(*) FROM attachment_object_cleanups WHERE storage_key=?1"
            )
            .bind(&key)
            .fetch_one(&f.pool)
            .await
            .unwrap(),
            1
        );
        assert_eq!(std::fs::read(&object).unwrap(), inode_bytes);
        std::fs::write(&object, archive.bytes(&file.payload_entry).unwrap()).unwrap();
        publish(
            &f,
            &claim,
            &archive,
            &keys,
            &hash,
            &CancellationToken::new(),
        )
        .await
        .unwrap();
        assert_eq!(
            sqlx::query_scalar::<_, i64>(
                "SELECT count(*) FROM attachment_object_cleanups WHERE storage_key=?1"
            )
            .bind(&key)
            .fetch_one(&f.pool)
            .await
            .unwrap(),
            0
        );
        assert_eq!(
            sqlx::query_scalar::<_, String>("SELECT storage_key FROM attachments WHERE id=?1")
                .bind(file.id.as_bytes().as_slice())
                .fetch_one(&f.pool)
                .await
                .unwrap(),
            key
        );
        assert_eq!(
            std::fs::read(&object).unwrap(),
            archive.bytes(&file.payload_entry).unwrap()
        );
        assert_eq!(
            serde_json::from_str::<Value>(
                &sqlx::query_scalar::<_, String>(
                    "SELECT created_refs FROM import_jobs WHERE id=?1"
                )
                .bind(claim.job_id.as_bytes().as_slice())
                .fetch_one(&f.pool)
                .await
                .unwrap()
            )
            .unwrap(),
            json!({"documentIds":[],"taskIds":[],"storedKeys":[]})
        );
        f.close().await;
    }

    #[tokio::test]
    async fn native_archive_selected_late_cancel_rolls_back_and_writer_reuses() {
        let f = Fixture::new().await;
        let (claim, _, hash) = f.claim().await;
        let archive = crate::native_archive::tests::policy_fixture();
        fault(claim.job_id, Fault::Cancel);
        assert!(matches!(
            publish(
                &f,
                &claim,
                &archive,
                &BTreeMap::new(),
                &hash,
                &CancellationToken::new()
            )
            .await,
            Err(NativeDbError::Archive(ArchiveError::Cancelled))
        ));
        f.unpublished(&claim).await;
        publish(
            &f,
            &claim,
            &archive,
            &BTreeMap::new(),
            &hash,
            &CancellationToken::new(),
        )
        .await
        .unwrap();
        assert_eq!(f.graph_counts().await, (1, 1, 1, 2, 0, 0, 3, 1));
        f.close().await;
    }

    #[tokio::test]
    async fn native_archive_selected_actual_deferred_commit_error_retains_type_and_no_publication()
    {
        let f = Fixture::new().await;
        let (claim, _, hash) = f.claim().await;
        let archive = crate::native_archive::tests::policy_fixture();
        let connector = Uuid::now_v7();
        sqlx::query("INSERT INTO zotero_connectors(id,workspace_id,owner_user_id,library_type,remote_library_id,library_url) VALUES(?1,?2,?3,'user',1,'https://example.invalid/control')").bind(connector.as_bytes().as_slice()).bind(f.workspace.as_bytes().as_slice()).bind(f.actor.as_bytes().as_slice()).execute(&f.pool).await.unwrap();
        fault(claim.job_id, Fault::Deferred(connector));
        let error = publish(
            &f,
            &claim,
            &archive,
            &BTreeMap::new(),
            &hash,
            &CancellationToken::new(),
        )
        .await
        .unwrap_err();
        let NativeDbError::Sql(sqlx::Error::AnyDriverError(source)) = error else {
            panic!("publication lost the original typed COMMIT outcome")
        };
        let unknown = source
            .downcast_ref::<crate::db::backend::CommitUnknown>()
            .expect("actual COMMIT error type");
        assert!(matches!(unknown.source, sqlx::Error::Database(_)));
        // This is a separately authorized test observer AFTER the returned
        // local error, never automatic publication retry or remote proof.
        f.unpublished(&claim).await;
        assert_eq!(
            sqlx::query_scalar::<_, i64>("SELECT count(*) FROM zotero_collections")
                .fetch_one(&f.pool)
                .await
                .unwrap(),
            0
        );
        publish(
            &f,
            &claim,
            &archive,
            &BTreeMap::new(),
            &hash,
            &CancellationToken::new(),
        )
        .await
        .unwrap();
        f.close().await;
    }

    #[tokio::test]
    async fn native_archive_selected_all_tenant_current_key_and_preview_refuse_stage() {
        let f = Fixture::new().await;
        let (claim, _, _) = f.claim().await;
        let foreign = Uuid::now_v7();
        let document = Uuid::now_v7();
        let attachment = Uuid::now_v7();
        let original = Uuid::now_v7().to_string();
        let preview = Uuid::now_v7().to_string();
        sqlx::query("INSERT INTO workspaces(id,slug,name) VALUES(?1,'foreign-native','foreign current key')").bind(foreign.as_bytes().as_slice()).execute(&f.pool).await.unwrap();
        sqlx::query("INSERT INTO documents(id,workspace_id,title,path,sort_key,number,created_by) VALUES(?1,?2,'foreign',?3,'a0',1,?4)").bind(document.as_bytes().as_slice()).bind(foreign.as_bytes().as_slice()).bind(document.simple().to_string()).bind(f.actor.as_bytes().as_slice()).execute(&f.pool).await.unwrap();
        sqlx::query("INSERT INTO attachments(id,workspace_id,document_id,name,size_bytes,reserved_size_bytes,uploader_id,storage_key,status,scan_status,variants) VALUES(?1,?2,?3,'foreign.txt',1,1,?4,?5,'stored','skipped',?6)").bind(attachment.as_bytes().as_slice()).bind(foreign.as_bytes().as_slice()).bind(document.as_bytes().as_slice()).bind(f.actor.as_bytes().as_slice()).bind(&original).bind(json!({"preview":{"key":preview}}).to_string()).execute(&f.pool).await.unwrap();
        for key in [&original, &preview] {
            assert!(matches!(
                stage_key_backend(
                    &f.backend,
                    &claim,
                    Uuid::now_v7(),
                    key,
                    &CancellationToken::new()
                )
                .await,
                Err(NativeDbError::Fenced)
            ));
        }
        assert_eq!(
            sqlx::query_scalar::<_, i64>("SELECT count(*) FROM attachment_object_cleanups")
                .fetch_one(&f.pool)
                .await
                .unwrap(),
            0
        );
        assert_eq!(
            serde_json::from_str::<Value>(
                &sqlx::query_scalar::<_, String>(
                    "SELECT created_refs FROM import_jobs WHERE id=?1"
                )
                .bind(claim.job_id.as_bytes().as_slice())
                .fetch_one(&f.pool)
                .await
                .unwrap()
            )
            .unwrap(),
            json!({"documentIds":[],"taskIds":[],"storedKeys":[]})
        );
        let mut tx = f.backend.begin_write().await.unwrap();
        let mut op = tx.operation();
        op.set_tenant(f.workspace).await.unwrap();
        assert!(op
            .native_key_available(f.workspace, Uuid::now_v7(), &original, false)
            .await
            .is_err());
        let OperationTx::SqliteFamily(family) = &mut op else {
            unreachable!()
        };
        assert!(family.require_system_context().is_err());
        assert_eq!(family.tenant(), Some(f.workspace));
        tx.rollback().await.unwrap();
        f.unpublished(&claim).await;
        f.close().await;
    }

    #[tokio::test]
    async fn native_archive_selected_failed_terminal_status_is_current_claim_only() {
        let f = Fixture::new().await;
        let (claim, _, _) = f.claim().await;
        let mut stale = claim.clone();
        stale.attempt += 1;
        assert!(!fail_native_backend(&f.backend, &stale, "stale")
            .await
            .unwrap());
        f.unpublished(&claim).await;
        assert!(fail_native_backend(&f.backend, &claim, "invalid archive")
            .await
            .unwrap());
        let status = status_backend(&f.backend, f.workspace, f.actor, f.session, claim.job_id)
            .await
            .unwrap();
        assert_eq!(status.status, "failed");
        assert_eq!(status.diagnostic.as_deref(), Some("invalid archive"));
        assert!(
            !fail_native_backend(&f.backend, &claim, "replace diagnostic")
                .await
                .unwrap()
        );
        assert_eq!(
            status_backend(&f.backend, f.workspace, f.actor, f.session, claim.job_id)
                .await
                .unwrap()
                .diagnostic
                .as_deref(),
            Some("invalid archive")
        );
        assert_eq!(f.graph_counts().await, (0, 0, 0, 0, 0, 0, 0, 0));
        f.close().await;
    }
}
