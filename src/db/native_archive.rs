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
use crate::db::identity::{append_audit, append_event, AuditAppend, EventAppend};
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

async fn rows<T: DeserializeOwned>(
    tx: &mut Transaction<'_, Postgres>,
    sql: &str,
    scope: &Scope,
) -> Result<Vec<T>, NativeDbError> {
    let sql = sql.replace("{DOCS}", DOCS);
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
    let views = rows(&mut tx, "SELECT to_jsonb(v)-'workspace_id' FROM fvoci.views v WHERE v.project_id=$1 ORDER BY v.id LIMIT 10001", &scope).await?;
    // Either end in the project: the writer keeps both ends in one project,
    // and validation refuses an edge whose other end is not archived.
    let dependencies = rows(&mut tx, "SELECT to_jsonb(d)-'workspace_id' FROM fvoci.task_dependencies d WHERE d.blocker_id IN(SELECT id FROM fvoci.tasks WHERE project_id=$1) OR d.blocked_id IN(SELECT id FROM fvoci.tasks WHERE project_id=$1) ORDER BY d.blocker_id,d.blocked_id LIMIT 10001", &scope).await?;
    let origins = rows(&mut tx, "SELECT to_jsonb(o)-'workspace_id' FROM fvoci.task_origins o JOIN fvoci.tasks t ON t.id=o.task_id WHERE t.project_id=$1 ORDER BY o.task_id LIMIT 10001", &scope).await?;
    let activity = rows(&mut tx, "SELECT to_jsonb(a)-'workspace_id' FROM fvoci.task_activity a JOIN fvoci.tasks t ON t.id=a.task_id WHERE t.project_id=$1 ORDER BY a.id LIMIT 10001", &scope).await?;
    let collections = rows(&mut tx, "SELECT to_jsonb(c)-'workspace_id' FROM fvoci.collections c WHERE c.project_id=$1 ORDER BY c.id LIMIT 10001", &scope).await?;
    let collection_items = rows(&mut tx, "SELECT to_jsonb(i)-'workspace_id' FROM fvoci.collection_items i JOIN fvoci.collections c ON c.id=i.collection_id WHERE c.project_id=$1 ORDER BY i.id LIMIT 10001", &scope).await?;
    // Person-made collection state of the project's collections; numbers as
    // their exact numeric text. Person-scoped rows of another person are read
    // so validation refuses them, never leaves them behind.
    let collection_fields = rows(&mut tx, "SELECT to_jsonb(f)-'workspace_id' FROM fvoci.collection_fields f WHERE f.collection_id IN(SELECT id FROM fvoci.collections WHERE project_id=$1) ORDER BY f.id LIMIT 10001", &scope).await?;
    let collection_options = rows(&mut tx, "SELECT to_jsonb(o)-'workspace_id' FROM fvoci.collection_options o WHERE o.collection_id IN(SELECT id FROM fvoci.collections WHERE project_id=$1) ORDER BY o.id LIMIT 10001", &scope).await?;
    let collection_values = rows(&mut tx, "SELECT (to_jsonb(v)-'workspace_id'-'value_number')||jsonb_build_object('value_number',v.value_number::text) FROM fvoci.collection_values v WHERE v.collection_id IN(SELECT id FROM fvoci.collections WHERE project_id=$1) ORDER BY v.item_id,v.field_id LIMIT 10001", &scope).await?;
    let collection_choices = rows(&mut tx, "SELECT to_jsonb(x)-'workspace_id' FROM fvoci.collection_choices x WHERE x.collection_id IN(SELECT id FROM fvoci.collections WHERE project_id=$1) ORDER BY x.item_id,x.field_id,x.option_id LIMIT 10001", &scope).await?;
    let collection_people = rows(&mut tx, "SELECT to_jsonb(x)-'workspace_id' FROM fvoci.collection_people x WHERE x.collection_id IN(SELECT id FROM fvoci.collections WHERE project_id=$1) ORDER BY x.item_id,x.field_id,x.user_id LIMIT 10001", &scope).await?;
    let collection_views = rows(&mut tx, "SELECT to_jsonb(v)-'workspace_id' FROM fvoci.collection_views v WHERE v.collection_id IN(SELECT id FROM fvoci.collections WHERE project_id=$1) ORDER BY v.id LIMIT 10001", &scope).await?;
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
        // project's collections are typed records. Still refused here: a
        // deleted or missing/extra task collection, an item whose target is
        // outside the project, or a collection outside the project (a
        // workspace collection) holding an archived document or task.
        ("collections", "SELECT (SELECT count(*) FROM fvoci.collections WHERE project_id=$1 AND kind='task') <> 1
            OR EXISTS(SELECT 1 FROM fvoci.collections c WHERE c.project_id=$1 AND c.deleted_at IS NOT NULL)
            OR EXISTS(SELECT 1 FROM fvoci.collection_items i JOIN fvoci.collections c ON c.id=i.collection_id WHERE c.project_id=$1 AND NOT (
                (i.task_id IS NOT NULL AND EXISTS(SELECT 1 FROM fvoci.tasks t WHERE t.id=i.task_id AND t.project_id=$1))
                OR (i.document_id IS NOT NULL AND i.document_id IN {DOCS} AND EXISTS(SELECT 1 FROM fvoci.documents d WHERE d.id=i.document_id AND d.project_id=$1))))
            OR (SELECT count(*) FROM fvoci.collection_items i JOIN fvoci.collections c ON c.id=i.collection_id WHERE c.project_id=$1 AND i.task_id IS NOT NULL) <> (SELECT count(*) FROM fvoci.tasks WHERE project_id=$1)
            OR EXISTS(SELECT 1 FROM fvoci.collection_items i JOIN fvoci.collections c ON c.id=i.collection_id WHERE (c.project_id IS DISTINCT FROM $1) AND (i.document_id IN {DOCS} OR i.task_id IN(SELECT id FROM fvoci.tasks WHERE project_id=$1)))"),
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

/// The archived mirror lands disconnected in the destination actor's personal
/// workspace (destination_gate): generation 1, reconciliation required, the
/// historical completed version kept, no progress/retry/lease, no pages and
/// no credential row. Nothing here contacts Zotero.
async fn publish_zotero(
    tx: &mut Transaction<'_, Postgres>,
    g: &Graph,
    workspace: Uuid,
    actor: Uuid,
) -> Result<(), NativeDbError> {
    if g.zotero_connectors.is_empty() {
        return Ok(());
    }
    set_self_user(tx, actor).await?;
    for c in &g.zotero_connectors {
        sqlx::query("INSERT INTO fvoci.zotero_connectors(id,workspace_id,owner_user_id,library_type,remote_library_id,library_url,state,generation,completed_version,progress_version,committed_pages,retry_at,reconciliation_required,sync_id,sync_expires_at,created_at,updated_at) VALUES($1,$2,$3,$4,$5,$6,'disconnected',1,$7,NULL,0,NULL,true,NULL,NULL,$8::timestamptz,$9::timestamptz)")
            .bind(c.id).bind(workspace).bind(actor).bind(&c.library_type).bind(c.remote_library_id).bind(&c.library_url)
            .bind(c.completed_version).bind(&c.created_at).bind(&c.updated_at).execute(&mut **tx).await?;
    }
    // The collection parent FK is deferred; insertion order is irrelevant.
    for c in &g.zotero_collections {
        sqlx::query("INSERT INTO fvoci.zotero_collections(workspace_id,owner_user_id,connector_id,collection_key,remote_version,name,parent_key,availability) VALUES($1,$2,$3,$4,$5,$6,$7,$8)")
            .bind(workspace).bind(actor).bind(c.connector_id).bind(&c.collection_key).bind(c.remote_version)
            .bind(&c.name).bind(&c.parent_key).bind(&c.availability).execute(&mut **tx).await?;
    }
    for r in &g.zotero_references {
        sqlx::query("INSERT INTO fvoci.zotero_references(id,workspace_id,owner_user_id,connector_id,document_id,item_key,remote_version,local_version,bibliography,return_url,availability) VALUES($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11)")
            .bind(r.id).bind(workspace).bind(actor).bind(r.connector_id).bind(r.document_id).bind(&r.item_key)
            .bind(r.remote_version).bind(r.local_version).bind(&r.bibliography).bind(&r.return_url).bind(&r.availability)
            .execute(&mut **tx).await?;
    }
    for m in &g.zotero_memberships {
        sqlx::query("INSERT INTO fvoci.zotero_memberships(workspace_id,owner_user_id,connector_id,reference_id,collection_key) VALUES($1,$2,$3,$4,$5)")
            .bind(workspace).bind(actor).bind(m.connector_id).bind(m.reference_id).bind(&m.collection_key)
            .execute(&mut **tx).await?;
    }
    for l in &g.zotero_links {
        sqlx::query("INSERT INTO fvoci.zotero_links(id,workspace_id,owner_user_id,connector_id,reference_id,document_id,task_id,anchor) VALUES($1,$2,$3,$4,$5,$6,$7,$8)")
            .bind(l.id).bind(workspace).bind(actor).bind(l.connector_id).bind(l.reference_id).bind(l.document_id)
            .bind(l.task_id).bind(&l.anchor).execute(&mut **tx).await?;
    }
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
async fn publish_task_time(
    tx: &mut Transaction<'_, Postgres>,
    g: &Graph,
    workspace: Uuid,
    actor: Uuid,
    job: Uuid,
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
    for row in &g.time_entries {
        insert_record(tx, "time_entries", mapped(row, workspace, actor)?).await?;
    }
    for entry in g.time_entries.iter().filter(|e| e.ended_at.is_none()) {
        if !g
            .timer_legacy_open
            .iter()
            .any(|l| l.time_entry_id == entry.id)
        {
            sqlx::query(
                "DELETE FROM fvoci.task_timer_legacy_open WHERE time_entry_id=$1 AND user_id=$2",
            )
            .bind(entry.id)
            .bind(actor)
            .execute(&mut **tx)
            .await?;
        }
    }
    for row in &g.timer_runs {
        insert_record(tx, "task_timer_runs", mapped(row, workspace, actor)?).await?;
    }
    for row in &g.timer_segments {
        insert_record(tx, "task_timer_segments", mapped(row, workspace, actor)?).await?;
    }
    for c in &g.timer_commands {
        sqlx::query("INSERT INTO fvoci.task_timer_commands(user_id,request_id,request_hash,run_id,result,created_at,restored_from_archive) VALUES($1,$2,$3,$4,$5,$6::timestamptz,$7)")
            .bind(actor).bind(c.request_id).bind(&c.request_hash).bind(c.run_id).bind(&c.result)
            .bind(&c.created_at).bind(job).execute(&mut **tx).await?;
    }
    // Explicit columns: a JSON null before/after value (time.manual has no
    // before state) is a jsonb null, which jsonb_populate_record would turn
    // into SQL NULL against the NOT NULL columns.
    // The source workspace locator becomes the restore workspace (where the
    // selected task now lives); an older historical locator stays as it was.
    for a in &g.timer_audit {
        sqlx::query("INSERT INTO fvoci.task_timer_audit(id,user_id,request_id,workspace_id,task_id,time_entry_id,verb,before_value,after_value,reason,created_at) VALUES($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11::timestamptz)")
            .bind(a.id).bind(actor).bind(a.request_id)
            .bind(a.workspace_id.map(|w| if w == g.source_workspace_id { workspace } else { w }))
            .bind(a.task_id).bind(a.time_entry_id).bind(&a.verb).bind(&a.before_value)
            .bind(&a.after_value).bind(&a.reason).bind(&a.created_at).execute(&mut **tx).await?;
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
    archive.validate()?;
    let mut tx = pool.begin().await?;
    set_tenant(&mut tx, claim.workspace_id).await?;
    crate::db::quota::acquire_admission_lock(&mut tx).await?;
    destination_gate(
        &mut tx,
        claim.workspace_id,
        claim.created_by,
        claim.session_id,
        true,
        true,
    )
    .await?;
    lock_tree(&mut tx, claim.workspace_id).await?;
    crate::db::attachments::lock_workspace_storage(&mut tx, claim.workspace_id).await?;
    // Tree/storage locks precede the job row fence, preserving the existing
    // document/task/attachment operation ordering.
    destination_gate(
        &mut tx,
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
    if !hold_import_fence(&mut tx, claim.workspace_id, fence).await? {
        return Err(NativeDbError::Fenced);
    }
    let stored_hash: String = sqlx::query_scalar(
        "SELECT native_archive_hash FROM fvoci.import_jobs WHERE id=$1 AND workspace_id=$2",
    )
    .bind(claim.job_id)
    .bind(claim.workspace_id)
    .fetch_one(&mut *tx)
    .await?;
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
    let mut project = mapped(&g.project, workspace, actor)?;
    project["root_document_id"] = Value::Null;
    project["next_number"] = json!(g.project.next_number.max(
        g.tasks
            .iter()
            .map(|t| t.number)
            .chain(g.documents.iter().map(|d| d.number))
            .max()
            .unwrap_or(0)
            .checked_add(1)
            .ok_or(ArchiveError::Limit)?
    ));
    insert_record(&mut tx, "projects", project).await?;
    // A new destination lead grant, not an archived source grant.
    sqlx::query("INSERT INTO fvoci.project_members(id,workspace_id,project_id,user_id,role) VALUES($1,$2,$3,$4,'lead')")
        .bind(Uuid::now_v7()).bind(workspace).bind(g.project.id).bind(actor).execute(&mut *tx).await?;
    // The destination's project trigger created a fresh baseline collection in
    // this transaction; give it the archived identity and timestamps. A global
    // UUID collision fails the whole graph like every other content ID.
    for c in g.collections.iter().filter(|c| c.kind == "task") {
        let changed = sqlx::query("UPDATE fvoci.collections SET id=$3, name=$4, version=$5, created_at=$6::timestamptz, updated_at=$7::timestamptz WHERE workspace_id=$1 AND project_id=$2 AND kind='task'")
            .bind(workspace).bind(g.project.id).bind(c.id).bind(&c.name).bind(c.version)
            .bind(&c.created_at).bind(&c.updated_at).execute(&mut *tx).await?;
        if changed.rows_affected() != 1 {
            return Err(ArchiveError::Invalid("baseline collection reconciliation".into()).into());
        }
    }
    for row in &g.workflows {
        insert_record(&mut tx, "workflows", mapped(row, workspace, actor)?).await?;
    }
    for row in &g.statuses {
        insert_record(&mut tx, "statuses", mapped(row, workspace, actor)?).await?;
    }
    let mut remaining: Vec<_> = g.documents.iter().collect();
    let mut inserted = std::collections::BTreeSet::new();
    while !remaining.is_empty() {
        let Some(index) = remaining
            .iter()
            .position(|d| d.parent_id.is_none_or(|id| inserted.contains(&id)))
        else {
            return Err(ArchiveError::Invalid("document parent cycle".into()).into());
        };
        let row = remaining.remove(index);
        insert_record(&mut tx, "documents", mapped(row, workspace, actor)?).await?;
        inserted.insert(row.id);
    }
    // Workspace tags of the archived documents, then their assignments (the
    // documents are inserted above). A destination tag with the same lower
    // name, or a global id collision, is an ordinary unique violation.
    for row in &g.document_tags {
        insert_record(&mut tx, "document_tags", mapped(row, workspace, actor)?).await?;
    }
    for row in &g.document_tag_assignments {
        insert_record(
            &mut tx,
            "document_tag_assignments",
            mapped(row, workspace, actor)?,
        )
        .await?;
    }
    // Before tasks: tasks.milestone_id references a milestone.
    for row in &g.milestones {
        insert_record(&mut tx, "milestones", mapped(row, workspace, actor)?).await?;
    }
    let mut remaining: Vec<_> = g.tasks.iter().collect();
    let mut inserted = std::collections::BTreeSet::new();
    while !remaining.is_empty() {
        let Some(index) = remaining
            .iter()
            .position(|t| t.parent_id.is_none_or(|id| inserted.contains(&id)))
        else {
            return Err(ArchiveError::Invalid("task parent cycle".into()).into());
        };
        let row = remaining.remove(index);
        insert_record(&mut tx, "tasks", mapped(row, workspace, actor)?).await?;
        inserted.insert(row.id);
    }
    // Document collections of the project, then their items (the documents
    // are inserted above); each task insert's trigger created one fresh item
    // of the task collection, whose identity is restored.
    for c in g.collections.iter().filter(|c| c.kind != "task") {
        insert_record(&mut tx, "collections", mapped(c, workspace, actor)?).await?;
    }
    for item in g
        .collection_items
        .iter()
        .filter(|i| i.document_id.is_some())
    {
        insert_record(&mut tx, "collection_items", mapped(item, workspace, actor)?).await?;
    }
    for item in g.collection_items.iter().filter(|i| i.task_id.is_some()) {
        let changed = sqlx::query("UPDATE fvoci.collection_items SET id=$4, version=$5, created_at=$6::timestamptz, updated_at=$7::timestamptz WHERE workspace_id=$1 AND collection_id=$2 AND task_id=$3")
            .bind(workspace).bind(item.collection_id).bind(item.task_id).bind(item.id).bind(item.version)
            .bind(&item.created_at).bind(&item.updated_at).execute(&mut *tx).await?;
        if changed.rows_affected() != 1 {
            return Err(
                ArchiveError::Invalid("baseline collection item reconciliation".into()).into(),
            );
        }
    }
    for row in &g.collection_fields {
        insert_record(&mut tx, "collection_fields", mapped(row, workspace, actor)?).await?;
    }
    for row in &g.collection_options {
        insert_record(
            &mut tx,
            "collection_options",
            mapped(row, workspace, actor)?,
        )
        .await?;
    }
    for row in &g.collection_values {
        insert_record(&mut tx, "collection_values", mapped(row, workspace, actor)?).await?;
    }
    for row in &g.collection_choices {
        insert_record(
            &mut tx,
            "collection_choices",
            mapped(row, workspace, actor)?,
        )
        .await?;
    }
    for row in &g.collection_people {
        insert_record(&mut tx, "collection_people", mapped(row, workspace, actor)?).await?;
    }
    // People fields by collection, for typed view-query mapping.
    let people_of = |collection: Option<Uuid>| {
        let fields: std::collections::BTreeSet<Uuid> = g
            .collection_fields
            .iter()
            .filter(|f| {
                Some(f.collection_id) == collection
                    && crate::collections::FieldType::parse(&f.r#type)
                        .is_some_and(|t| t.is_people())
            })
            .map(|f| f.id)
            .collect();
        move |field: Uuid| fields.contains(&field)
    };
    for row in &g.collection_views {
        let mut value = mapped(row, workspace, actor)?;
        value["owner_id"] = json!(actor);
        let people = people_of(Some(row.collection_id));
        value["config"] =
            mapped_collection_view_config(&row.config, &people, g.source_actor_id, actor);
        insert_record(&mut tx, "collection_views", value).await?;
    }
    sqlx::query("UPDATE fvoci.projects SET root_document_id=$3 WHERE workspace_id=$1 AND id=$2")
        .bind(workspace)
        .bind(g.project.id)
        .bind(g.project.root_document_id)
        .execute(&mut *tx)
        .await?;
    sqlx::query("UPDATE fvoci.workspaces SET next_document_number=GREATEST(next_document_number,$2) WHERE id=$1")
        .bind(workspace).bind(g.documents.iter().map(|d| d.number).max().unwrap_or(0).checked_add(1).ok_or(ArchiveError::Limit)?).execute(&mut *tx).await?;
    for row in &g.assignees {
        insert_record(&mut tx, "task_assignees", mapped(row, workspace, actor)?).await?;
    }
    for row in &g.dependencies {
        insert_record(&mut tx, "task_dependencies", mapped(row, workspace, actor)?).await?;
    }
    let task_people = people_of(
        g.collections
            .iter()
            .find(|c| c.kind == "task")
            .map(|c| c.id),
    );
    for row in &g.views {
        let mut value = mapped(row, workspace, actor)?;
        value["config"] =
            mapped_project_view_config(&row.config, &task_people, g.source_actor_id, actor);
        insert_record(&mut tx, "views", value).await?;
    }
    for row in &g.labels {
        insert_record(&mut tx, "labels", mapped(row, workspace, actor)?).await?;
    }
    for row in &g.task_labels {
        insert_record(&mut tx, "task_labels", mapped(row, workspace, actor)?).await?;
    }
    for row in &g.origins {
        insert_record(&mut tx, "task_origins", mapped(row, workspace, actor)?).await?;
    }
    for row in &g.activity {
        let mut value = mapped(row, workspace, actor)?;
        value["actor_user_id"] = Value::Null;
        value["changes"] = mapped_activity_changes(&row.changes, g.source_actor_id, actor);
        insert_record(&mut tx, "task_activity", value).await?;
    }
    // Comments: parents before replies (validated acyclic); the author and
    // the single actor's reactions become the destination actor.
    let mut remaining: Vec<_> = g.comments.iter().collect();
    let mut inserted = std::collections::BTreeSet::new();
    while !remaining.is_empty() {
        let Some(index) = remaining
            .iter()
            .position(|c| c.parent_id.is_none_or(|id| inserted.contains(&id)))
        else {
            return Err(ArchiveError::Invalid("comment parent cycle".into()).into());
        };
        let row = remaining.remove(index);
        let mut value = mapped(row, workspace, actor)?;
        value["reactions"] = mapped_comment_reactions(&row.reactions, g.source_actor_id, actor);
        insert_record(&mut tx, "comments", value).await?;
        inserted.insert(row.id);
    }
    for row in &g.states {
        let table = if row.target_kind == "document" {
            "document"
        } else {
            "task"
        };
        let mut value = mapped(row, workspace, actor)?;
        for field in [
            "target_kind",
            "target_id",
            "state_entry",
            "updates",
            "receipts",
        ] {
            value.as_object_mut().expect("typed record").remove(field);
        }
        value[format!("{table}_id")] = json!(row.target_id);
        value["state"] = json!(format!(
            "\\x{}",
            hex::encode(archive.bytes(&row.state_entry)?)
        ));
        value["writer_generation"] = json!(0);
        insert_record(&mut tx, &format!("{table}_states"), value).await?;
        for update in &row.updates {
            let mut value = mapped(update, workspace, actor)?;
            value
                .as_object_mut()
                .expect("typed record")
                .remove("payload_entry");
            value[format!("{table}_id")] = json!(row.target_id);
            value["payload"] = json!(format!(
                "\\x{}",
                hex::encode(archive.bytes(&update.payload_entry)?)
            ));
            insert_record(&mut tx, &format!("{table}_collab_updates"), value).await?;
        }
        for receipt in &row.receipts {
            let mut value = mapped(receipt, workspace, actor)?;
            value[format!("{table}_id")] = json!(row.target_id);
            value["payload_sha256"] = json!(format!("\\x{}", receipt.payload_sha256));
            insert_record(&mut tx, &format!("{table}_collab_op_receipts"), value).await?;
        }
    }
    for row in &g.revisions {
        let mut value = mapped(row, workspace, actor)?;
        value
            .as_object_mut()
            .expect("typed record")
            .remove("snapshot_entry");
        value["y_snapshot"] = json!(format!(
            "\\x{}",
            hex::encode(archive.bytes(&row.snapshot_entry)?)
        ));
        insert_record(&mut tx, "revisions", value).await?;
    }
    for row in &g.attachments {
        let key = keys
            .get(&row.id)
            .ok_or_else(|| ArchiveError::Invalid("missing staged key".into()))?;
        let mut value = mapped(row, workspace, actor)?;
        value
            .as_object_mut()
            .expect("typed record")
            .remove("payload_entry");
        value["storage_key"] = json!(key);
        value["reserved_size_bytes"] = json!(row.size_bytes);
        value["status"] = json!("stored");
        // An unauthenticated archive cannot assert a scan: the destination did
        // not scan these bytes, exactly like its own ordinary uploads.
        value["scan_status"] = json!("skipped");
        value["extract_status"] = json!(crate::attachments::initial_extract_status(
            &row.name, &row.mime
        ));
        value["extract_text"] = json!("");
        value["variants"] = json!({});
        insert_record(&mut tx, "attachments", value).await?;
        let removed=sqlx::query("DELETE FROM fvoci.attachment_object_cleanups WHERE workspace_id=$1 AND attachment_id=$2 AND storage_key=$3")
            .bind(workspace).bind(row.id).bind(key).execute(&mut *tx).await?;
        if removed.rows_affected() != 1 {
            return Err(NativeDbError::Fenced);
        }
    }
    publish_zotero(&mut tx, g, workspace, actor).await?;
    publish_task_time(&mut tx, g, workspace, actor, claim.job_id).await?;
    // Retired: destination tenant/actor, every target NULL, key/hash kept.
    for r in &g.personal_input_commands {
        sqlx::query("INSERT INTO fvoci.personal_input_commands(workspace_id,actor_user_id,request_id,request_hash,intent,document_id,task_id,project_id,created_at) VALUES($1,$2,$3,$4,$5,NULL,NULL,NULL,$6::timestamptz)")
            .bind(workspace).bind(actor).bind(r.request_id).bind(&r.request_hash).bind(&r.intent).bind(&r.created_at)
            .execute(&mut *tx).await?;
    }
    let result = json!({"projectId":g.project.id,"archiveHash":stored_hash,"sourceWorkspaceId":g.source_workspace_id,
        "sourceActorId":g.source_actor_id,"destinationActorId":actor,"attribution":"inert source provenance",
        "documentIds":g.documents.iter().map(|d|d.id).collect::<Vec<_>>(),"taskIds":g.tasks.iter().map(|t|t.id).collect::<Vec<_>>(),
        "activityIds":g.activity.iter().map(|a|a.id).collect::<Vec<_>>(),
        "zoteroConnectorIds":g.zotero_connectors.iter().map(|c|c.id).collect::<Vec<_>>()});
    sqlx::query("UPDATE fvoci.import_jobs SET native_result=$3,created_refs='{\"documentIds\":[],\"taskIds\":[],\"storedKeys\":[]}'::jsonb WHERE workspace_id=$1 AND id=$2")
        .bind(workspace).bind(claim.job_id).bind(&result).execute(&mut *tx).await?;
    for (kind, id) in g
        .documents
        .iter()
        .map(|d| ("document", d.id))
        .chain(g.tasks.iter().map(|t| ("task", t.id)))
    {
        append_event(
            &mut tx,
            EventAppend {
                id: Uuid::now_v7(),
                workspace_id: Some(workspace),
                actor_user_id: Some(actor),
                verb: format!("{kind}.created"),
                target_type: Some(kind.into()),
                target_id: Some(id),
                payload: json!({"nativeRestoreJobId":claim.job_id}),
            },
        )
        .await?;
    }
    append_event(
        &mut tx,
        EventAppend {
            id: Uuid::now_v7(),
            workspace_id: Some(workspace),
            actor_user_id: Some(actor),
            verb: "native_archive.restored".into(),
            target_type: Some("project".into()),
            target_id: Some(g.project.id),
            payload: result.clone(),
        },
    )
    .await?;
    append_audit(
        &mut tx,
        AuditAppend {
            id: Uuid::now_v7(),
            workspace_id: Some(workspace),
            actor_user_id: Some(actor),
            verb: "native_archive.restored".into(),
            target_type: Some("project".into()),
            target_id: Some(g.project.id),
            payload: result,
            ip: None,
        },
    )
    .await?;
    let live:bool=sqlx::query_scalar("SELECT lease_until>clock_timestamp() FROM fvoci.import_jobs WHERE workspace_id=$1 AND id=$2 AND lease_token=$3 AND status='running'")
        .bind(workspace).bind(claim.job_id).bind(claim.lease_token).fetch_optional(&mut *tx).await?.unwrap_or(false);
    if !live
        || !crate::db::import_jobs::finish_import_job_in_tx(
            &mut tx,
            claim,
            crate::db::import_jobs::ImportStatus::Completed,
        )
        .await?
    {
        return Err(NativeDbError::Fenced);
    }
    tx.commit().await?;
    Ok(())
}
