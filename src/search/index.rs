//! Outbox search indexer: state-based refresh under a per-workspace session lock.

use std::collections::{HashMap, HashSet};
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use std::time::Duration;

use sqlx::PgPool;
use uuid::Uuid;

use crate::db::context::{
    lock_key_from_uuid, SEARCH_INDEX_LOCK_NAMESPACE, SEARCH_REBUILD_LOCK_KEY,
};
use crate::db::outbox::OutboxEvent;
use crate::db::search_index::{
    cursor_of, list_live_workspace_ids, list_sources, load_sources, related_page_limit,
    SearchIndexCursor, SearchIndexRow, SourceScope,
};
use crate::outbox::{DeliveryMode, OutboxConsumer, OutboxProcessError};
use crate::search::meili::{
    delete_all_meili_documents, delete_meili_by_filter, enqueue_delete_meili_by_filter,
    enqueue_delete_meili_sources, enqueue_upsert_meili_sources, ensure_meili_index, meili_eq,
    search_source_id, upsert_meili_sources, wait_meili_tasks, MeiliConfig, MeiliError,
    SearchSource, SearchSourceKind, MEILI_OP_TIMEOUT_MS,
};
use crate::search::text::index_document_text;

pub const SEARCH_INDEX_CONSUMER: &str = "search-index";
const REBUILD_PAGE: i64 = 200;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct SearchResourceRef {
    pub kind: SearchSourceKind,
    pub workspace_id: Uuid,
    pub id: Uuid,
}

pub struct SearchIndexConsumer {
    meili: MeiliConfig,
}

impl SearchIndexConsumer {
    pub fn new(meili: MeiliConfig) -> Self {
        Self { meili }
    }
}

impl OutboxConsumer for SearchIndexConsumer {
    fn name(&self) -> &str {
        SEARCH_INDEX_CONSUMER
    }

    fn delivery_mode(&self) -> DeliveryMode {
        DeliveryMode::External
    }

    fn deliver<'a>(
        &'a self,
        pool: &'a PgPool,
        _lease_owner: Uuid,
        event: &'a OutboxEvent,
    ) -> Pin<Box<dyn Future<Output = Result<(), OutboxProcessError>> + Send + 'a>> {
        Box::pin(async move {
            process_search_index_event(pool, &self.meili, event)
                .await
                .map_err(|err| OutboxProcessError::Delivery(err.to_string()))
        })
    }

    fn deliver_batch<'a>(
        &'a self,
        pool: &'a PgPool,
        _lease_owner: Uuid,
        events: &'a [OutboxEvent],
    ) -> Pin<Box<dyn Future<Output = (usize, Option<OutboxProcessError>)> + Send + 'a>> {
        Box::pin(async move {
            match deliver_search_index_batch(pool, &self.meili, events).await {
                Ok(()) => (events.len(), None),
                Err(err) => (0, Some(OutboxProcessError::Delivery(err.to_string()))),
            }
        })
    }

    fn batch_time_budget(&self) -> Option<Duration> {
        Some(Duration::from_millis(MEILI_OP_TIMEOUT_MS))
    }
}

struct MeiliBatchSink<'a> {
    meili: &'a MeiliConfig,
    uids: Vec<u64>,
}

impl<'a> MeiliBatchSink<'a> {
    async fn upsert_sources(&mut self, docs: &[SearchSource]) -> Result<(), SearchIndexError> {
        let new = enqueue_upsert_meili_sources(self.meili, docs).await?;
        self.uids.extend(new);
        Ok(())
    }

    async fn delete_sources(&mut self, ids: &[String]) -> Result<(), SearchIndexError> {
        let uid = enqueue_delete_meili_sources(self.meili, ids).await?;
        if uid != 0 {
            self.uids.push(uid);
        }
        Ok(())
    }

    async fn delete_by_filter(&mut self, filter: &str) -> Result<(), SearchIndexError> {
        let uid = enqueue_delete_meili_by_filter(self.meili, filter).await?;
        self.uids.push(uid);
        Ok(())
    }

    async fn finish(self) -> Result<(), SearchIndexError> {
        wait_meili_tasks(self.meili, &self.uids).await?;
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
enum BatchCoalesceKey {
    WorkspaceDelete(Uuid),
    ProjectLifecycle {
        workspace_id: Uuid,
        project_id: Uuid,
    },
    Resource(SearchResourceRef),
}

fn batch_coalesce_key(event: &OutboxEvent) -> Option<BatchCoalesceKey> {
    if event.verb == "workspace.deleted" {
        return event.workspace_id.map(BatchCoalesceKey::WorkspaceDelete);
    }
    if event.verb == "project.deleted" || event.verb == "project.restored" {
        return event
            .workspace_id
            .zip(event.target_id)
            .map(|(ws, project)| BatchCoalesceKey::ProjectLifecycle {
                workspace_id: ws,
                project_id: project,
            });
    }
    resource_from_event(event).map(BatchCoalesceKey::Resource)
}

struct CoalescedSearchEvent {
    event: OutboxEvent,
    subtree: bool,
    body_only: bool,
}

fn event_refresh_flags(event: &OutboxEvent) -> (bool, bool) {
    let subtree = moved_across_project(event);
    let body_only = resource_from_event(event)
        .is_some_and(|resource| resource.kind == SearchSourceKind::Document)
        && document_body_only(event);
    (subtree, body_only)
}

fn coalesce_search_index_events(events: &[OutboxEvent]) -> Vec<CoalescedSearchEvent> {
    let mut order: Vec<BatchCoalesceKey> = Vec::new();
    let mut latest: HashMap<BatchCoalesceKey, CoalescedSearchEvent> = HashMap::new();
    for event in events {
        let Some(key) = batch_coalesce_key(event) else {
            continue;
        };
        let (subtree, body_only) = event_refresh_flags(event);
        if let Some(existing) = latest.get_mut(&key) {
            existing.event = event.clone();
            existing.subtree |= subtree;
            existing.body_only &= body_only;
            continue;
        }
        order.push(key.clone());
        latest.insert(
            key,
            CoalescedSearchEvent {
                event: event.clone(),
                subtree,
                body_only,
            },
        );
    }

    let deleted_workspaces: HashSet<Uuid> = order
        .iter()
        .filter_map(|key| match key {
            BatchCoalesceKey::WorkspaceDelete(ws) => Some(*ws),
            _ => None,
        })
        .collect();

    order
        .into_iter()
        .filter(|key| match key {
            BatchCoalesceKey::WorkspaceDelete(_) => true,
            BatchCoalesceKey::ProjectLifecycle { workspace_id, .. } => {
                !deleted_workspaces.contains(workspace_id)
            }
            BatchCoalesceKey::Resource(resource) => {
                !deleted_workspaces.contains(&resource.workspace_id)
            }
        })
        .filter_map(|key| latest.remove(&key))
        .collect()
}

async fn deliver_search_index_batch(
    pool: &PgPool,
    meili: &MeiliConfig,
    events: &[OutboxEvent],
) -> Result<(), SearchIndexError> {
    if events.is_empty() {
        return Ok(());
    }
    ensure_meili_index(meili).await?;
    let coalesced = coalesce_search_index_events(events);
    let mut sink = MeiliBatchSink {
        meili,
        uids: Vec::new(),
    };
    for item in &coalesced {
        apply_search_index_event_batch(pool, &mut sink, item).await?;
    }
    sink.finish().await?;
    Ok(())
}

async fn apply_search_index_event_batch(
    pool: &PgPool,
    sink: &mut MeiliBatchSink<'_>,
    item: &CoalescedSearchEvent,
) -> Result<(), SearchIndexError> {
    let event = &item.event;
    if event.verb == "workspace.deleted" {
        if let Some(workspace_id) = event.workspace_id {
            with_workspace_lock(pool, workspace_id, || async {
                sink.delete_by_filter(&meili_eq("workspaceId", &workspace_id.to_string())?)
                    .await?;
                Ok(())
            })
            .await?;
        }
        return Ok(());
    }
    if event.verb == "project.deleted" || event.verb == "project.restored" {
        if let (Some(workspace_id), Some(project_id)) = (event.workspace_id, event.target_id) {
            with_workspace_lock(pool, workspace_id, || async {
                sink.delete_by_filter(&meili_eq("projectId", &project_id.to_string())?)
                    .await?;
                upsert_pages_batch(
                    pool,
                    sink,
                    workspace_id,
                    SourceScope {
                        project_id: Some(project_id),
                        ..SourceScope::default()
                    },
                    None,
                )
                .await?;
                Ok(())
            })
            .await?;
            return Ok(());
        }
    }
    if let Some(resource) = resource_from_event(event) {
        if resource.kind == SearchSourceKind::Document && item.body_only {
            refresh_document_body_only_batch(pool, sink, resource).await?;
        } else {
            refresh_search_resource_batch(pool, sink, resource, item.subtree).await?;
        }
    }
    Ok(())
}

async fn upsert_pages_batch(
    pool: &PgPool,
    sink: &mut MeiliBatchSink<'_>,
    workspace_id: Uuid,
    scope: SourceScope,
    parent: Option<SearchResourceRef>,
) -> Result<&'static str, SearchIndexError> {
    let mut after: Option<SearchIndexCursor> = None;
    let mut any = false;
    loop {
        let batch = list_sources(
            pool,
            workspace_id,
            after.as_ref(),
            related_page_limit(),
            &scope,
        )
        .await?;
        if batch.is_empty() {
            return Ok(if any { "ok" } else { "empty" });
        }
        any = true;
        let docs: Vec<SearchSource> = batch.iter().map(to_meili).collect();
        sink.upsert_sources(&docs).await?;
        if let Some(parent) = parent {
            let live = load_sources(pool, parent.workspace_id, parent.kind, parent.id).await?;
            if live.is_empty() {
                delete_absent_batch(sink, parent).await?;
                return Ok("gone");
            }
        }
        let last = batch.last().expect("non-empty");
        if (batch.len() as i64) < related_page_limit() {
            return Ok("ok");
        }
        after = Some(cursor_of(last));
    }
}

async fn delete_absent_batch(
    sink: &mut MeiliBatchSink<'_>,
    resource: SearchResourceRef,
) -> Result<(), SearchIndexError> {
    if resource.kind == SearchSourceKind::Comment {
        sink.delete_sources(&[search_source_id(
            SearchSourceKind::Comment,
            &resource.id.to_string(),
            None,
        )])
        .await?;
        return Ok(());
    }
    sink.delete_by_filter(&absence_filter(resource)?).await?;
    Ok(())
}

async fn refresh_document_body_only_batch(
    pool: &PgPool,
    sink: &mut MeiliBatchSink<'_>,
    resource: SearchResourceRef,
) -> Result<(), SearchIndexError> {
    with_workspace_lock(pool, resource.workspace_id, || async {
        let current = load_sources(
            pool,
            resource.workspace_id,
            SearchSourceKind::Document,
            resource.id,
        )
        .await?;
        if current.is_empty() {
            delete_absent_batch(sink, resource).await?;
            return Ok(());
        }
        let docs: Vec<SearchSource> = current.iter().map(to_meili).collect();
        sink.upsert_sources(&docs).await?;
        Ok(())
    })
    .await
}

async fn refresh_search_resource_batch(
    pool: &PgPool,
    sink: &mut MeiliBatchSink<'_>,
    resource: SearchResourceRef,
    subtree: bool,
) -> Result<(), SearchIndexError> {
    with_workspace_lock(pool, resource.workspace_id, || async {
        if resource.kind == SearchSourceKind::Document || resource.kind == SearchSourceKind::Task {
            let scope = if resource.kind == SearchSourceKind::Document {
                SourceScope {
                    document_id: Some(resource.id),
                    subtree,
                    ..SourceScope::default()
                }
            } else {
                SourceScope {
                    task_id: Some(resource.id),
                    ..SourceScope::default()
                }
            };
            let outcome =
                upsert_pages_batch(pool, sink, resource.workspace_id, scope, Some(resource))
                    .await?;
            if outcome == "empty" {
                delete_absent_batch(sink, resource).await?;
            }
            return Ok(());
        }
        let current = load_sources(pool, resource.workspace_id, resource.kind, resource.id).await?;
        if current.is_empty() {
            delete_absent_batch(sink, resource).await?;
            return Ok(());
        }
        let docs: Vec<SearchSource> = current.iter().map(to_meili).collect();
        sink.upsert_sources(&docs).await?;
        if resource.kind == SearchSourceKind::Attachment {
            let chunks: Vec<i32> = current.iter().filter_map(|row| row.chunk_no).collect();
            if chunks.iter().any(|n| *n < 0) {
                return Err(SearchIndexError::Other(
                    "invalid attachment search chunk number".into(),
                ));
            }
            let obsolete = if chunks.is_empty() {
                "chunkNo IS NOT NULL".to_string()
            } else {
                let list = chunks
                    .iter()
                    .map(|n| n.to_string())
                    .collect::<Vec<_>>()
                    .join(",");
                format!("(chunkNo NOT IN [{list}] OR chunkNo IS NULL)")
            };
            let filter = format!(
                "{} AND {} AND {obsolete}",
                meili_eq("workspaceId", &resource.workspace_id.to_string())?,
                meili_eq("attachmentId", &resource.id.to_string())?,
            );
            sink.delete_by_filter(&filter).await?;
        }
        let again = load_sources(pool, resource.workspace_id, resource.kind, resource.id).await?;
        if again.is_empty() {
            delete_absent_batch(sink, resource).await?;
        }
        Ok(())
    })
    .await
}

#[derive(Debug, thiserror::Error)]
pub enum SearchIndexError {
    #[error("database error: {0}")]
    Db(#[from] sqlx::Error),
    #[error("{0}")]
    Meili(MeiliError),
    #[error("{0}")]
    Other(String),
}

impl From<MeiliError> for SearchIndexError {
    fn from(value: MeiliError) -> Self {
        Self::Meili(value)
    }
}

fn to_meili(row: &SearchIndexRow) -> SearchSource {
    let text = index_document_text(&row.title, &row.body, &row.chosung);
    SearchSource {
        id: search_source_id(
            row.kind,
            &row.resource_id.to_string(),
            row.chunk_no.map(i64::from),
        ),
        kind: row.kind,
        workspace_id: row.workspace_id.to_string(),
        project_id: row.project_id.map(|id| id.to_string()),
        document_id: row.document_id.map(|id| id.to_string()),
        task_id: row.task_id.map(|id| id.to_string()),
        comment_id: row.comment_id.map(|id| id.to_string()),
        attachment_id: row.attachment_id.map(|id| id.to_string()),
        chunk_no: row.chunk_no.map(i64::from),
        title: text.title,
        body: text.body,
        chosung: text.chosung,
        stem: text.stem,
        updated_at: row.updated_at.timestamp_millis(),
        embedding: if row.kind == SearchSourceKind::Attachment {
            row.embedding.clone()
        } else {
            None
        },
    }
}

fn absence_filter(resource: SearchResourceRef) -> Result<String, MeiliError> {
    match resource.kind {
        SearchSourceKind::Document => meili_eq("documentId", &resource.id.to_string()),
        SearchSourceKind::Task => meili_eq("taskId", &resource.id.to_string()),
        SearchSourceKind::Attachment => meili_eq("attachmentId", &resource.id.to_string()),
        SearchSourceKind::Comment => meili_eq("commentId", &resource.id.to_string()),
    }
}

fn resource_from_event(event: &OutboxEvent) -> Option<SearchResourceRef> {
    let workspace_id = event.workspace_id?;
    let id = event.target_id?;
    let prefix = event.verb.split('.').next()?;
    let kind = match prefix {
        "document" => SearchSourceKind::Document,
        "task" => SearchSourceKind::Task,
        "comment" => SearchSourceKind::Comment,
        "attachment" => SearchSourceKind::Attachment,
        _ => return None,
    };
    Some(SearchResourceRef {
        kind,
        workspace_id,
        id,
    })
}

fn moved_across_project(event: &OutboxEvent) -> bool {
    if event.verb != "document.moved" {
        return false;
    }
    if event.payload.get("kind").and_then(|v| v.as_str()) == Some("reorder") {
        return false;
    }
    event.payload.get("oldProjectId") != event.payload.get("newProjectId")
}

async fn with_workspace_lock<T, F, Fut>(
    pool: &PgPool,
    workspace_id: Uuid,
    f: F,
) -> Result<T, SearchIndexError>
where
    F: FnOnce() -> Fut,
    Fut: Future<Output = Result<T, SearchIndexError>>,
{
    // Transaction-scoped lock: if this future is cancelled (e.g. the batch
    // timeout), the dropped transaction rolls back before the connection is
    // reused, which releases the lock. A session lock would stay held on an idle
    // pooled connection and stall this workspace's indexing.
    let mut lock_tx = pool.begin().await?;
    sqlx::query("SELECT pg_advisory_xact_lock($1, $2)")
        .bind(SEARCH_INDEX_LOCK_NAMESPACE)
        .bind(lock_key_from_uuid(workspace_id))
        .execute(&mut *lock_tx)
        .await?;
    let result = f().await;
    lock_tx.rollback().await?;
    result
}

fn document_body_only(event: &OutboxEvent) -> bool {
    if event.verb == "document.collab_update_appended"
        || event.verb == "document.collab_snapshot_compacted"
    {
        return true;
    }
    if event.verb == "document.updated" {
        if event.payload.get("collab").and_then(|v| v.as_bool()) == Some(true) {
            return true;
        }
        // Title/project/visibility changes must refresh comments and attachment chunks.
        return event.payload.get("title").is_none()
            && event.payload.get("icon").is_none()
            && event.payload.get("status").is_none();
    }
    false
}

pub async fn process_search_index_event(
    pool: &PgPool,
    meili: &MeiliConfig,
    event: &OutboxEvent,
) -> Result<(), SearchIndexError> {
    // One refresh implementation: a single event is a batch of one.
    deliver_search_index_batch(pool, meili, std::slice::from_ref(event)).await
}

pub struct RebuildOutcome {
    pub workspaces: usize,
    pub pages: usize,
}

/// Pool for [`rebuild_search_index`]: the per-workspace lock transaction and one
/// page transaction are held at the same time; the rebuild lock takes a third
/// connection that is detached from the pool (3 connections in total).
pub async fn rebuild_pool(url: &str) -> Result<PgPool, sqlx::Error> {
    sqlx::postgres::PgPoolOptions::new()
        .max_connections(2)
        .connect(url)
        .await
}

pub async fn rebuild_search_index(
    pool: &PgPool,
    meili: &MeiliConfig,
    workspace_id: Option<Uuid>,
) -> Result<RebuildOutcome, SearchIndexError> {
    // The rebuild lock lives on a connection detached from the pool: if this
    // future is cancelled or the unlock fails, the connection is closed rather
    // than returned to the pool still holding the session lock.
    let mut lock_conn = pool.acquire().await?.detach();
    sqlx::query("SELECT pg_advisory_lock($1)")
        .bind(SEARCH_REBUILD_LOCK_KEY)
        .execute(&mut lock_conn)
        .await?;
    let result = rebuild_search_index_inner(pool, meili, workspace_id).await;
    // Closing the session releases the lock whether or not an explicit unlock ran.
    let _ = sqlx::Connection::close(lock_conn).await;
    result
}

async fn rebuild_search_index_inner(
    pool: &PgPool,
    meili: &MeiliConfig,
    workspace_id: Option<Uuid>,
) -> Result<RebuildOutcome, SearchIndexError> {
    ensure_meili_index(meili).await?;
    // Clear before listing so a workspace created meanwhile is still rebuilt.
    if let Some(wanted) = workspace_id {
        delete_meili_by_filter(meili, &meili_eq("workspaceId", &wanted.to_string())?).await?;
    } else {
        delete_all_meili_documents(meili).await?;
    }
    let mut ids = list_live_workspace_ids(pool).await?;
    if let Some(wanted) = workspace_id {
        ids.retain(|id| *id == wanted);
        if ids.is_empty() {
            return Err(SearchIndexError::Other("workspace not found".into()));
        }
    }
    let mut pages = 0usize;
    for id in &ids {
        let mut after: Option<SearchIndexCursor> = None;
        loop {
            let page = rebuild_search_page(pool, meili, *id, after.as_ref(), REBUILD_PAGE).await?;
            pages += 1;
            if page.done {
                break;
            }
            if page.after.as_ref().map(|c| (c.kind, c.id, c.chunk_no))
                == after.as_ref().map(|c| (c.kind, c.id, c.chunk_no))
            {
                return Err(SearchIndexError::Other(
                    "search rebuild cursor did not advance".into(),
                ));
            }
            after = page.after;
        }
    }
    Ok(RebuildOutcome {
        workspaces: ids.len(),
        pages,
    })
}

struct RebuildPage {
    after: Option<SearchIndexCursor>,
    done: bool,
}

async fn rebuild_search_page(
    pool: &PgPool,
    meili: &MeiliConfig,
    workspace_id: Uuid,
    after: Option<&SearchIndexCursor>,
    limit: i64,
) -> Result<RebuildPage, SearchIndexError> {
    ensure_meili_index(meili).await?;
    with_workspace_lock(pool, workspace_id, || async {
        let batch = list_sources(pool, workspace_id, after, limit, &SourceScope::default()).await?;
        let docs: Vec<SearchSource> = batch.iter().map(to_meili).collect();
        upsert_meili_sources(meili, &docs).await?;
        Ok(RebuildPage {
            done: (batch.len() as i64) < limit,
            after: batch.last().map(cursor_of),
        })
    })
    .await
}

pub fn search_index_consumer(meili: MeiliConfig) -> Arc<dyn OutboxConsumer> {
    Arc::new(SearchIndexConsumer::new(meili))
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Utc;
    use serde_json::{json, Value};

    fn test_event(
        verb: &str,
        payload: Value,
        resource_id: Uuid,
        workspace_id: Uuid,
    ) -> OutboxEvent {
        OutboxEvent {
            snapshot_xmin: "1".into(),
            id: Uuid::now_v7(),
            seq: 1,
            xact: "1".into(),
            workspace_id: Some(workspace_id),
            actor_user_id: None,
            verb: verb.into(),
            target_type: Some("document".into()),
            target_id: Some(resource_id),
            payload,
            channel: "system".into(),
            created_at: Utc::now(),
        }
    }

    #[test]
    fn coalesce_move_then_update_keeps_subtree() {
        let workspace_id = Uuid::now_v7();
        let document_id = Uuid::now_v7();
        let old_project = Uuid::now_v7();
        let new_project = Uuid::now_v7();
        let events = [
            test_event(
                "document.moved",
                json!({
                    "oldProjectId": old_project,
                    "newProjectId": new_project,
                }),
                document_id,
                workspace_id,
            ),
            test_event(
                "document.updated",
                json!({ "collab": true }),
                document_id,
                workspace_id,
            ),
        ];
        let coalesced = coalesce_search_index_events(&events);
        assert_eq!(coalesced.len(), 1);
        assert!(coalesced[0].subtree);
        assert!(!coalesced[0].body_only);
    }

    #[test]
    fn coalesce_title_then_collab_is_full_refresh() {
        let workspace_id = Uuid::now_v7();
        let document_id = Uuid::now_v7();
        let events = [
            test_event(
                "document.updated",
                json!({ "title": "renamed" }),
                document_id,
                workspace_id,
            ),
            test_event(
                "document.collab_update_appended",
                json!({ "seq": 1 }),
                document_id,
                workspace_id,
            ),
        ];
        let coalesced = coalesce_search_index_events(&events);
        assert_eq!(coalesced.len(), 1);
        assert!(!coalesced[0].subtree);
        assert!(!coalesced[0].body_only);
    }

    #[test]
    fn coalesce_two_collab_events_stay_body_only() {
        let workspace_id = Uuid::now_v7();
        let document_id = Uuid::now_v7();
        let events = [
            test_event(
                "document.collab_update_appended",
                json!({ "seq": 1 }),
                document_id,
                workspace_id,
            ),
            test_event(
                "document.collab_update_appended",
                json!({ "seq": 2 }),
                document_id,
                workspace_id,
            ),
        ];
        let coalesced = coalesce_search_index_events(&events);
        assert_eq!(coalesced.len(), 1);
        assert!(!coalesced[0].subtree);
        assert!(coalesced[0].body_only);
    }
}
