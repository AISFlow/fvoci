//! Meilisearch CE client for FVOCI search.
//!
//! Ports `packages/search/src/meili.ts` (lexical path) at source SHA
//! `393795261322b916e588043cf94feca999175843`. Semantic/vector search
//! (`searchMeiliVector`, embedding upsert besides a null `_vectors` slot)
//! is intentionally not ported.

use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::Path;
use std::sync::LazyLock;
use std::time::{Duration, Instant};

use regex::Regex;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use tokio::sync::Mutex;
use url::Url;

use crate::search::text::{is_chosung_query, normalize_search_text};

/// Official CE v1.53.2 multiarch digest. EE/BUSL is forbidden.
pub const MEILI_CE_IMAGE: &str =
    "getmeili/meilisearch:v1.53.2@sha256:c94e58ca09662dd6e65e8f1b0fd145767be3da7d5422a863a27b8d2b68e090c9";
pub const MEILI_INDEX_UID: &str = "fvoci";
pub const MEILI_MAX_TOTAL_HITS: u32 = 1000;
pub const MEILI_OP_TIMEOUT_MS: u64 = 30_000;
const MEILI_MAX_RESPONSE_BYTES: usize = 1_048_576;
const MEILI_UPSERT_MAX_BYTES: usize = 8 * 1024 * 1024;
const TASK_POLL_MIN_MS: u64 = 5;
const TASK_POLL_MAX_MS: u64 = 100;
pub const ATTACHMENT_EMBEDDER: &str = "attachments";
/// Source `@fvoci/contracts` `EMBEDDING_DIMENSIONS`. Required by index settings.
pub const EMBEDDING_DIMENSIONS: u32 = 1536;
const SCOPED_KEY_NAME: &str = "fvoci";
const KEY_FILE_UID: u32 = 1000;

static UUID_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$")
        .expect("uuid regex")
});
static ATTR_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^[a-zA-Z][a-zA-Z0-9]*$").expect("meili attr regex"));
static SOURCE_ID_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)^(document|task|comment|attachment)_([0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12})(?:_(\d+))?$")
        .expect("source id regex")
});

static ENSURE: LazyLock<Mutex<HashSet<String>>> = LazyLock::new(|| Mutex::new(HashSet::new()));

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MeiliError {
    FilterAttr,
    FilterValue,
    SourceId,
    ResponseTooLarge,
    DocumentTooLarge,
    Http(u16),
    TaskFailed,
    Timeout,
    Unavailable,
    Config,
    Io,
    Protocol,
}

impl std::fmt::Display for MeiliError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::FilterAttr => write!(f, "meili filter attr"),
            Self::FilterValue => write!(f, "meili filter value"),
            Self::SourceId => write!(f, "meili source id"),
            Self::ResponseTooLarge => write!(f, "meili response exceeds byte limit"),
            Self::DocumentTooLarge => write!(f, "meili document exceeds upsert byte limit"),
            Self::Http(status) => write!(f, "meili HTTP {status}"),
            Self::TaskFailed => write!(f, "meili task failed"),
            Self::Timeout => write!(f, "meili operation timed out"),
            Self::Unavailable => write!(f, "meili unavailable (connection failed)"),
            Self::Config => write!(f, "meili config"),
            Self::Io => write!(f, "meili key file"),
            Self::Protocol => write!(f, "meili protocol"),
        }
    }
}

impl std::error::Error for MeiliError {}

impl From<reqwest::Error> for MeiliError {
    fn from(err: reqwest::Error) -> Self {
        if err.is_timeout() {
            Self::Timeout
        } else if err.is_connect() {
            Self::Unavailable
        } else {
            Self::Protocol
        }
    }
}

impl From<std::io::Error> for MeiliError {
    fn from(_: std::io::Error) -> Self {
        Self::Io
    }
}

#[derive(Clone)]
pub struct MeiliConfig {
    pub url: String,
    api_key: String,
    pub index_uid: String,
    // Owned per config rather than process-wide: pooled connections belong to the
    // runtime that opened them.
    http: reqwest::Client,
}

impl std::fmt::Debug for MeiliConfig {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("MeiliConfig")
            .field("url", &self.url)
            .field("api_key", &"<redacted>")
            .field("index_uid", &self.index_uid)
            .finish()
    }
}

impl MeiliConfig {
    pub fn new(url: String, api_key: String, index_uid: String) -> Self {
        Self {
            url: url.trim_end_matches('/').to_string(),
            api_key,
            index_uid,
            http: reqwest::Client::builder()
                .timeout(Duration::from_millis(MEILI_OP_TIMEOUT_MS))
                .redirect(reqwest::redirect::Policy::none())
                .build()
                .expect("meili http client"),
        }
    }

    pub fn api_key(&self) -> &str {
        &self.api_key
    }

    fn origin(&self) -> &str {
        &self.url
    }

    fn index_path(&self, suffix: &str) -> String {
        format!("/indexes/{}{suffix}", self.index_uid)
    }

    fn ensure_key(&self) -> String {
        format!("{}\0{}", self.origin(), self.index_uid)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum SearchSourceKind {
    Document,
    Task,
    Comment,
    Attachment,
}

impl SearchSourceKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Document => "document",
            Self::Task => "task",
            Self::Comment => "comment",
            Self::Attachment => "attachment",
        }
    }
}

impl std::fmt::Display for SearchSourceKind {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

fn parse_kind(value: &str) -> Option<SearchSourceKind> {
    match value {
        "document" => Some(SearchSourceKind::Document),
        "task" => Some(SearchSourceKind::Task),
        "comment" => Some(SearchSourceKind::Comment),
        "attachment" => Some(SearchSourceKind::Attachment),
        _ => None,
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct SearchSource {
    pub id: String,
    pub kind: SearchSourceKind,
    pub workspace_id: String,
    pub project_id: Option<String>,
    pub document_id: Option<String>,
    pub task_id: Option<String>,
    pub comment_id: Option<String>,
    pub attachment_id: Option<String>,
    pub chunk_no: Option<i64>,
    pub title: String,
    pub body: String,
    pub chosung: String,
    pub stem: String,
    pub updated_at: i64,
}

#[derive(Debug, Clone, PartialEq)]
pub struct MeiliHit {
    pub id: String,
    pub kind: SearchSourceKind,
    pub workspace_id: String,
    pub resource_id: String,
    pub chunk_no: Option<i64>,
    pub document_id: Option<String>,
    pub task_id: Option<String>,
    pub comment_id: Option<String>,
    pub attachment_id: Option<String>,
    pub project_id: Option<String>,
    pub score: f64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MeiliSearchScope {
    pub workspace_id: String,
    pub project_ids: Vec<String>,
    pub include_wiki: bool,
    pub wiki_document_ids: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MeiliSearchInput {
    pub q: String,
    pub stem: String,
    pub scopes: Vec<MeiliSearchScope>,
    pub kind: Option<SearchSourceKind>,
    /// API-token domain narrowing by parent (source `parentKindClause`); `None` is unrestricted.
    pub parent_kinds: Option<ParentKinds>,
    pub limit: u32,
    pub offset: u32,
}

/// Which parents (document / task) an API token may read through mixed content.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ParentKinds {
    pub document: bool,
    pub task: bool,
}

/// Source `parentKindClause`: narrows hits by their parent before pagination.
pub fn parent_kind_clause(kinds: Option<ParentKinds>) -> Option<&'static str> {
    let kinds = kinds?;
    match (kinds.document, kinds.task) {
        (true, true) => None,
        (true, false) => Some("documentId IS NOT NULL"),
        (false, true) => Some("taskId IS NOT NULL"),
        (false, false) => Some("documentId IS NULL AND taskId IS NULL"),
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct MeiliSearchPage {
    pub hits: Vec<MeiliHit>,
    pub next_offset: Option<u32>,
}

pub fn search_source_id(
    kind: SearchSourceKind,
    resource_id: &str,
    chunk_no: Option<i64>,
) -> String {
    if kind == SearchSourceKind::Attachment {
        if let Some(chunk) = chunk_no {
            return format!("attachment_{resource_id}_{chunk}");
        }
    }
    format!("{}_{resource_id}", kind.as_str())
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParsedSourceId {
    pub kind: SearchSourceKind,
    pub resource_id: String,
    pub chunk_no: Option<i64>,
}

pub fn parse_search_source_id(id: &str) -> Option<ParsedSourceId> {
    let caps = SOURCE_ID_RE.captures(id)?;
    let kind = parse_kind(caps.get(1)?.as_str())?;
    let resource_id = caps.get(2)?.as_str().to_string();
    let chunk_no = caps.get(3).and_then(|m| m.as_str().parse::<i64>().ok());
    if kind != SearchSourceKind::Attachment && chunk_no.is_some() {
        return None;
    }
    Some(ParsedSourceId {
        kind,
        resource_id,
        chunk_no: if kind == SearchSourceKind::Attachment {
            chunk_no
        } else {
            None
        },
    })
}

fn is_uuid(value: &str) -> bool {
    UUID_RE.is_match(value)
}

fn is_kind_token(value: &str) -> bool {
    parse_kind(value).is_some()
}

fn filter_literal(value: &str) -> Result<String, MeiliError> {
    if is_uuid(value) || is_kind_token(value) {
        Ok(serde_json::to_string(value).expect("json string"))
    } else {
        Err(MeiliError::FilterValue)
    }
}

pub fn meili_eq(attr: &str, value: &str) -> Result<String, MeiliError> {
    if !ATTR_RE.is_match(attr) {
        return Err(MeiliError::FilterAttr);
    }
    Ok(format!("{attr} = {}", filter_literal(value)?))
}

fn source_resource_key(id: &str) -> Result<String, MeiliError> {
    let parsed = parse_search_source_id(id).ok_or(MeiliError::SourceId)?;
    Ok(format!("{}_{}", parsed.kind.as_str(), parsed.resource_id))
}

fn meili_document(doc: &SearchSource) -> Result<Value, MeiliError> {
    let resource_key = source_resource_key(&doc.id)?;
    Ok(json!({
        "id": doc.id,
        "kind": doc.kind.as_str(),
        "workspaceId": doc.workspace_id,
        "projectId": doc.project_id,
        "documentId": doc.document_id,
        "taskId": doc.task_id,
        "commentId": doc.comment_id,
        "attachmentId": doc.attachment_id,
        "chunkNo": doc.chunk_no,
        "title": doc.title,
        "body": doc.body,
        "chosung": doc.chosung,
        "stem": doc.stem,
        "updatedAt": doc.updated_at,
        "resourceKey": resource_key,
        "_vectors": { ATTACHMENT_EMBEDDER: Value::Null },
    }))
}

fn index_settings() -> Value {
    json!({
        "searchCutoffMs": MEILI_OP_TIMEOUT_MS,
        "searchableAttributes": ["title", "body", "chosung", "stem"],
        "filterableAttributes": [
            "kind",
            "workspaceId",
            "projectId",
            "documentId",
            "taskId",
            "commentId",
            "attachmentId",
            "chunkNo",
            "resourceKey",
        ],
        "displayedAttributes": [
            "id",
            "kind",
            "workspaceId",
            "projectId",
            "documentId",
            "taskId",
            "commentId",
            "attachmentId",
            "chunkNo",
            "updatedAt",
        ],
        "pagination": { "maxTotalHits": MEILI_MAX_TOTAL_HITS },
        "embedders": {
            ATTACHMENT_EMBEDDER: {
                "source": "userProvided",
                "dimensions": EMBEDDING_DIMENSIONS,
            }
        }
    })
}

struct MeiliResponse {
    status: u16,
    json: Value,
}

async fn read_capped_json(mut resp: reqwest::Response) -> Result<Value, MeiliError> {
    let status = resp.status().as_u16();
    let mut total = 0usize;
    let mut buf = Vec::new();
    while let Some(chunk) = resp.chunk().await? {
        total = total.saturating_add(chunk.len());
        if total > MEILI_MAX_RESPONSE_BYTES {
            return Err(MeiliError::ResponseTooLarge);
        }
        buf.extend_from_slice(&chunk);
    }
    if buf.is_empty() {
        return Ok(Value::Null);
    }
    serde_json::from_slice(&buf).map_err(|_| MeiliError::Http(status))
}

async fn meili_request(
    config: &MeiliConfig,
    method: reqwest::Method,
    path: &str,
    body: Option<&Value>,
) -> Result<MeiliResponse, MeiliError> {
    let url = format!("{}{path}", config.origin());
    let mut req = config
        .http
        .request(method, &url)
        .header("accept", "application/json");
    if !config.api_key.is_empty() {
        req = req.bearer_auth(&config.api_key);
    }
    if let Some(body) = body {
        req = req.header("content-type", "application/json").json(body);
    }
    let resp = req.send().await?;
    let status = resp.status().as_u16();
    if (300..400).contains(&status) {
        return Err(MeiliError::Http(status));
    }
    let json = read_capped_json(resp).await?;
    Ok(MeiliResponse { status, json })
}

fn task_uid_of(json: &Value) -> Result<u64, MeiliError> {
    json.get("taskUid")
        .and_then(Value::as_u64)
        .ok_or(MeiliError::Protocol)
}

fn task_error_is_index_already_exists(json: &Value) -> bool {
    json.get("error")
        .and_then(|e| e.get("code"))
        .and_then(Value::as_str)
        .is_some_and(|c| c.contains("index_already_exists"))
}

async fn wait_meili_task(config: &MeiliConfig, task_uid: u64) -> Result<(), MeiliError> {
    wait_meili_task_inner(config, task_uid, false).await
}

async fn wait_meili_task_inner(
    config: &MeiliConfig,
    task_uid: u64,
    tolerate_index_exists: bool,
) -> Result<(), MeiliError> {
    let deadline = Instant::now() + Duration::from_millis(MEILI_OP_TIMEOUT_MS);
    let mut poll_ms = TASK_POLL_MIN_MS;
    loop {
        if Instant::now() >= deadline {
            return Err(MeiliError::Timeout);
        }
        let got = meili_request(
            config,
            reqwest::Method::GET,
            &format!("/tasks/{task_uid}"),
            None,
        )
        .await?;
        if got.status != 200 {
            return Err(MeiliError::Http(got.status));
        }
        let status = got.json.get("status").and_then(Value::as_str).unwrap_or("");
        match status {
            "succeeded" => return Ok(()),
            "failed" | "canceled" => {
                if tolerate_index_exists && task_error_is_index_already_exists(&got.json) {
                    return Ok(());
                }
                let code = got
                    .json
                    .get("error")
                    .and_then(|e| e.get("code"))
                    .and_then(Value::as_str)
                    .unwrap_or("unknown");
                tracing::warn!(task_uid, code, "meili task {status}");
                return Err(MeiliError::TaskFailed);
            }
            _ => {
                tokio::time::sleep(Duration::from_millis(poll_ms)).await;
                poll_ms = (poll_ms * 2).min(TASK_POLL_MAX_MS);
            }
        }
    }
}

async fn enqueue_meili(
    config: &MeiliConfig,
    method: reqwest::Method,
    path: &str,
    body: Option<&Value>,
) -> Result<u64, MeiliError> {
    let got = meili_request(config, method, path, body).await?;
    if got.status != 200 && got.status != 201 && got.status != 202 {
        return Err(MeiliError::Http(got.status));
    }
    task_uid_of(&got.json)
}

async fn enqueue_and_wait(
    config: &MeiliConfig,
    method: reqwest::Method,
    path: &str,
    body: Option<&Value>,
) -> Result<(), MeiliError> {
    let uid = enqueue_meili(config, method, path, body).await?;
    wait_meili_task(config, uid).await
}

async fn wait_meili_tasks_inner(config: &MeiliConfig, uids: &[u64]) -> Result<(), MeiliError> {
    if uids.is_empty() {
        return Ok(());
    }
    let deadline = Instant::now() + Duration::from_millis(MEILI_OP_TIMEOUT_MS);
    let wanted: HashSet<u64> = uids.iter().copied().collect();
    let uids_param = wanted
        .iter()
        .map(|uid| uid.to_string())
        .collect::<Vec<_>>()
        .join(",");
    let page_limit = wanted.len().clamp(1, 1000);
    let mut poll_ms = TASK_POLL_MIN_MS;
    loop {
        if Instant::now() >= deadline {
            return Err(MeiliError::Timeout);
        }
        let mut by_uid: HashMap<u64, String> = HashMap::new();
        let mut from: Option<u64> = None;
        loop {
            let mut path = format!("/tasks?uids={uids_param}&limit={page_limit}");
            if let Some(from_uid) = from {
                path.push_str(&format!("&from={from_uid}"));
            }
            let got = meili_request(config, reqwest::Method::GET, &path, None).await?;
            if got.status != 200 {
                return Err(MeiliError::Http(got.status));
            }
            let results = got
                .json
                .get("results")
                .and_then(Value::as_array)
                .ok_or(MeiliError::Protocol)?;
            for task in results {
                let Some(uid) = task.get("uid").and_then(Value::as_u64) else {
                    continue;
                };
                if !wanted.contains(&uid) {
                    continue;
                }
                let status = task
                    .get("status")
                    .and_then(Value::as_str)
                    .unwrap_or("")
                    .to_string();
                by_uid.insert(uid, status);
            }
            let next = got.json.get("next").and_then(Value::as_u64);
            match next {
                Some(next_uid) if by_uid.len() < wanted.len() && !results.is_empty() => {
                    from = Some(next_uid);
                }
                _ => break,
            }
        }
        let mut pending = false;
        for uid in &wanted {
            match by_uid.get(uid).map(String::as_str) {
                Some("succeeded") => {}
                Some("failed") | Some("canceled") => {
                    tracing::warn!(task_uid = uid, "meili task failed");
                    return Err(MeiliError::TaskFailed);
                }
                Some(_) | None => pending = true,
            }
        }
        if !pending {
            return Ok(());
        }
        tokio::time::sleep(Duration::from_millis(poll_ms)).await;
        poll_ms = (poll_ms * 2).min(TASK_POLL_MAX_MS);
    }
}

async fn create_index_if_missing(config: &MeiliConfig) -> Result<(), MeiliError> {
    let got = meili_request(config, reqwest::Method::GET, &config.index_path(""), None).await?;
    if got.status == 200 {
        return Ok(());
    }
    if got.status != 404 {
        return Err(MeiliError::Http(got.status));
    }
    let body = json!({ "uid": config.index_uid, "primaryKey": "id" });
    let created = meili_request(config, reqwest::Method::POST, "/indexes", Some(&body)).await?;
    if created.status == 409 {
        return Ok(());
    }
    if created.status == 200 || created.status == 201 || created.status == 202 {
        wait_meili_task_inner(config, task_uid_of(&created.json)?, true).await
    } else {
        Err(MeiliError::Http(created.status))
    }
}

async fn ensure_meili_index_op(config: &MeiliConfig) -> Result<(), MeiliError> {
    let key = config.ensure_key();
    {
        let done = ENSURE.lock().await;
        if done.contains(&key) {
            return Ok(());
        }
    }
    create_index_if_missing(config).await?;
    let settings = index_settings();
    enqueue_and_wait(
        config,
        reqwest::Method::PATCH,
        &config.index_path("/settings"),
        Some(&settings),
    )
    .await?;
    ENSURE.lock().await.insert(key);
    Ok(())
}

async fn upsert_meili_sources_enqueue(
    config: &MeiliConfig,
    docs: &[SearchSource],
) -> Result<Vec<u64>, MeiliError> {
    if docs.is_empty() {
        return Ok(Vec::new());
    }
    let encoded: Vec<Value> = docs.iter().map(meili_document).collect::<Result<_, _>>()?;
    let mut batch: Vec<Value> = Vec::new();
    let mut bytes = 2usize;
    let mut uids = Vec::new();
    for doc in encoded {
        let item = serde_json::to_vec(&doc)
            .map_err(|_| MeiliError::Protocol)?
            .len();
        if item.saturating_add(2) > MEILI_UPSERT_MAX_BYTES {
            return Err(MeiliError::DocumentTooLarge);
        }
        let extra = item + if batch.is_empty() { 0 } else { 1 };
        if !batch.is_empty() && bytes.saturating_add(extra) > MEILI_UPSERT_MAX_BYTES {
            let uid = enqueue_meili(
                config,
                reqwest::Method::POST,
                &config.index_path("/documents"),
                Some(&Value::Array(std::mem::take(&mut batch))),
            )
            .await?;
            uids.push(uid);
            bytes = 2;
        }
        batch.push(doc);
        bytes = bytes.saturating_add(extra);
    }
    if !batch.is_empty() {
        let uid = enqueue_meili(
            config,
            reqwest::Method::POST,
            &config.index_path("/documents"),
            Some(&Value::Array(batch)),
        )
        .await?;
        uids.push(uid);
    }
    Ok(uids)
}

async fn upsert_meili_sources_op(
    config: &MeiliConfig,
    docs: &[SearchSource],
) -> Result<(), MeiliError> {
    let uids = upsert_meili_sources_enqueue(config, docs).await?;
    wait_meili_tasks_inner(config, &uids).await
}

async fn delete_meili_sources_op(config: &MeiliConfig, ids: &[String]) -> Result<(), MeiliError> {
    if ids.is_empty() {
        return Ok(());
    }
    enqueue_and_wait(
        config,
        reqwest::Method::POST,
        &config.index_path("/documents/delete-batch"),
        Some(&json!(ids)),
    )
    .await
}

async fn delete_meili_by_filter_op(config: &MeiliConfig, filter: &str) -> Result<(), MeiliError> {
    enqueue_and_wait(
        config,
        reqwest::Method::POST,
        &config.index_path("/documents/delete"),
        Some(&json!({ "filter": filter })),
    )
    .await
}

async fn delete_all_meili_documents_op(config: &MeiliConfig) -> Result<(), MeiliError> {
    enqueue_and_wait(
        config,
        reqwest::Method::DELETE,
        &config.index_path("/documents"),
        None,
    )
    .await
}

fn string_or_null(value: &Value) -> Option<String> {
    value.as_str().map(str::to_string)
}

fn number_or_null(value: &Value) -> Option<i64> {
    value.as_i64().or_else(|| value.as_u64().map(|n| n as i64))
}

fn hit_of(row: &Value) -> Option<MeiliHit> {
    let id = row.get("id")?.as_str()?.to_string();
    let parsed = parse_search_source_id(&id)?;
    let workspace_id = string_or_null(row.get("workspaceId")?)?;
    if !is_uuid(&workspace_id) {
        return None;
    }
    let kind = row
        .get("kind")
        .and_then(Value::as_str)
        .and_then(parse_kind)
        .unwrap_or(parsed.kind);
    let score = row
        .get("_rankingScore")
        .and_then(Value::as_f64)
        .filter(|n| n.is_finite())
        .unwrap_or(0.0);
    Some(MeiliHit {
        id,
        kind,
        workspace_id,
        resource_id: parsed.resource_id,
        chunk_no: parsed
            .chunk_no
            .or_else(|| row.get("chunkNo").and_then(number_or_null)),
        document_id: row.get("documentId").and_then(string_or_null),
        task_id: row.get("taskId").and_then(string_or_null),
        comment_id: row.get("commentId").and_then(string_or_null),
        attachment_id: row.get("attachmentId").and_then(string_or_null),
        project_id: row.get("projectId").and_then(string_or_null),
        score,
    })
}

fn meili_search_query(q: &str, stem: &str) -> Option<(String, Vec<&'static str>)> {
    let trimmed = q.trim();
    if trimmed.is_empty() {
        return None;
    }
    if is_chosung_query(trimmed) {
        return Some((trimmed.to_string(), vec!["chosung"]));
    }
    let qn = normalize_search_text(trimmed);
    let st = stem.trim();
    let q = if st.is_empty() { qn } else { st.to_string() };
    Some((q, vec!["title", "body", "stem"]))
}

fn scope_clause(scope: &MeiliSearchScope) -> Result<Option<String>, MeiliError> {
    let ws = meili_eq("workspaceId", &scope.workspace_id)?;
    let ids: Result<Vec<String>, MeiliError> = scope
        .project_ids
        .iter()
        .map(|id| {
            if !is_uuid(id) {
                return Err(MeiliError::FilterValue);
            }
            Ok(serde_json::to_string(id).expect("json string"))
        })
        .collect();
    let ids = ids?;
    let mut parts: Vec<String> = Vec::new();
    if !ids.is_empty() {
        parts.push(format!("projectId IN [{}]", ids.join(", ")));
    }
    if scope.include_wiki {
        parts.push("projectId IS NULL".to_string());
    }
    let wiki_ids: Result<Vec<String>, MeiliError> = scope
        .wiki_document_ids
        .iter()
        .map(|id| {
            if !is_uuid(id) {
                return Err(MeiliError::FilterValue);
            }
            Ok(serde_json::to_string(id).expect("json string"))
        })
        .collect();
    let wiki_ids = wiki_ids?;
    if !wiki_ids.is_empty() {
        parts.push(format!("documentId IN [{}]", wiki_ids.join(", ")));
    }
    if parts.is_empty() {
        return Ok(None);
    }
    Ok(Some(format!("({ws} AND ({}))", parts.join(" OR "))))
}

const RETRIEVE: &[&str] = &[
    "id",
    "kind",
    "workspaceId",
    "projectId",
    "documentId",
    "taskId",
    "commentId",
    "attachmentId",
    "chunkNo",
];

async fn search_meili_op(
    config: &MeiliConfig,
    input: &MeiliSearchInput,
) -> Result<MeiliSearchPage, MeiliError> {
    let Some((q, attributes)) = meili_search_query(&input.q, &input.stem) else {
        return Ok(MeiliSearchPage {
            hits: Vec::new(),
            next_offset: None,
        });
    };
    let mut scopes = Vec::new();
    for scope in &input.scopes {
        if let Some(clause) = scope_clause(scope)? {
            scopes.push(clause);
        }
    }
    if scopes.is_empty() {
        return Ok(MeiliSearchPage {
            hits: Vec::new(),
            next_offset: None,
        });
    }
    ensure_meili_index(config).await?;
    let mut filter = scopes.join(" OR ");
    if let Some(kind) = input.kind {
        filter = format!("({filter}) AND {}", meili_eq("kind", kind.as_str())?);
    }
    if let Some(parent) = parent_kind_clause(input.parent_kinds) {
        filter = format!("({filter}) AND {parent}");
    }
    let body = json!({
        "q": q,
        "filter": filter,
        "limit": input.limit,
        "offset": input.offset,
        "attributesToRetrieve": RETRIEVE,
        "attributesToSearchOn": attributes,
        "matchingStrategy": "all",
        "showRankingScore": true,
        "retrieveVectors": false,
        "distinct": "resourceKey",
    });
    let got = meili_request(
        config,
        reqwest::Method::POST,
        &config.index_path("/search"),
        Some(&body),
    )
    .await?;
    if got.status != 200 {
        return Err(MeiliError::Http(got.status));
    }
    let hits: Vec<MeiliHit> = got
        .json
        .get("hits")
        .and_then(Value::as_array)
        .map(|rows| rows.iter().filter_map(hit_of).collect())
        .unwrap_or_default();
    let next_offset = if hits.len() as u32 == input.limit {
        Some(input.offset + hits.len() as u32)
    } else {
        None
    };
    Ok(MeiliSearchPage { hits, next_offset })
}

fn write_key_file(path: &Path, key: &str) -> Result<(), MeiliError> {
    use std::io::Write;
    use std::os::unix::fs::OpenOptionsExt;
    if let Some(parent) = path.parent() {
        if !parent.as_os_str().is_empty() {
            fs::create_dir_all(parent)?;
        }
    }
    let tmp = path.with_extension("tmp");
    let _ = fs::remove_file(&tmp);
    let written = (|| -> Result<(), MeiliError> {
        let mut file = fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(&tmp)?;
        file.write_all(key.trim().as_bytes())?;
        file.sync_all()?;
        // When run as root, hand the file to the image's service account; as any
        // other user the file already belongs to the process that will read it.
        // A wrong owner surfaces later as an unreadable FVOCI_MEILI_KEY_FILE.
        let _ = std::os::unix::fs::chown(&tmp, Some(KEY_FILE_UID), Some(KEY_FILE_UID));
        fs::rename(&tmp, path)?;
        Ok(())
    })();
    if written.is_err() {
        let _ = fs::remove_file(&tmp);
    }
    written
}

/// A reusable server key must work on its own index and must NOT be able to
/// manage keys (a master or admin key in the file would otherwise be kept).
async fn key_is_scoped_for_index(url: &str, key: &str, index_uid: &str) -> bool {
    let config = MeiliConfig::new(url.to_string(), key.to_string(), index_uid.to_string());
    let own = meili_request(
        &config,
        reqwest::Method::GET,
        &format!("/indexes/{index_uid}"),
        None,
    )
    .await;
    // 404: key is valid before the index exists.
    let works = matches!(own, Ok(ref got) if got.status == 200 || got.status == 404);
    if !works {
        return false;
    }
    matches!(
        meili_request(&config, reqwest::Method::GET, "/keys", None).await,
        Ok(got) if got.status == 403
    )
}

const SCOPED_KEY_ACTIONS: &[&str] = &[
    "search",
    "documents.*",
    "indexes.get",
    "indexes.create",
    "settings.get",
    "settings.update",
    "tasks.get",
];

fn listed_key_is_scoped(row: &Value, index_uid: &str) -> bool {
    let indexes_ok = row
        .get("indexes")
        .and_then(Value::as_array)
        .is_some_and(|indexes| indexes.len() == 1 && indexes[0].as_str() == Some(index_uid));
    let actions_ok = row
        .get("actions")
        .and_then(Value::as_array)
        .is_some_and(|actions| {
            actions
                .iter()
                .all(|a| a.as_str().is_some_and(|a| SCOPED_KEY_ACTIONS.contains(&a)))
        });
    indexes_ok && actions_ok
}

/// Create or reuse a scoped API key limited to `index_uid` and write it to `dest`.
pub async fn ensure_scoped_meili_key(
    url: &str,
    master_key: &str,
    index_uid: &str,
    dest: &Path,
) -> Result<(), MeiliError> {
    let url = url.trim_end_matches('/');
    if dest.is_file() {
        if let Ok(existing) = fs::read_to_string(dest) {
            let existing = existing.trim();
            if existing.len() >= 16 && key_is_scoped_for_index(url, existing, index_uid).await {
                return Ok(());
            }
        }
    }
    let master = MeiliConfig::new(
        url.to_string(),
        master_key.to_string(),
        index_uid.to_string(),
    );
    let listed = meili_request(&master, reqwest::Method::GET, "/keys?limit=1000", None).await?;
    if listed.status != 200 {
        return Err(MeiliError::Http(listed.status));
    }
    if let Some(results) = listed.json.get("results").and_then(Value::as_array) {
        for row in results {
            let name = row.get("name").and_then(Value::as_str).unwrap_or("");
            let matches_index = listed_key_is_scoped(row, index_uid);
            if name == SCOPED_KEY_NAME && matches_index {
                if let Some(key) = row.get("key").and_then(Value::as_str) {
                    if key.len() >= 16 {
                        write_key_file(dest, key)?;
                        return Ok(());
                    }
                }
            }
        }
    }
    let body = json!({
        "name": SCOPED_KEY_NAME,
        "description": "FVOCI index-scoped API key",
        "actions": SCOPED_KEY_ACTIONS,
        "indexes": [index_uid],
        "expiresAt": Value::Null,
    });
    let created = meili_request(&master, reqwest::Method::POST, "/keys", Some(&body)).await?;
    if created.status != 201 && created.status != 200 {
        return Err(MeiliError::Http(created.status));
    }
    let key = created
        .json
        .get("key")
        .and_then(Value::as_str)
        .ok_or(MeiliError::Protocol)?;
    write_key_file(dest, key)?;
    Ok(())
}

pub fn parse_meili_url(raw: &str) -> Result<String, String> {
    let parsed = Url::parse(raw.trim()).map_err(|e| format!("invalid FVOCI_MEILI_URL: {e}"))?;
    if parsed.scheme() != "http" && parsed.scheme() != "https" {
        return Err("FVOCI_MEILI_URL must be an HTTP(S) origin without credentials".into());
    }
    if !parsed.username().is_empty() || parsed.password().is_some() {
        return Err("FVOCI_MEILI_URL must be an HTTP(S) origin without credentials".into());
    }
    if parsed.query().is_some() || parsed.fragment().is_some() {
        return Err("FVOCI_MEILI_URL must be an HTTP(S) origin without credentials".into());
    }
    let path = parsed.path();
    if path != "/" && !path.is_empty() {
        return Err("FVOCI_MEILI_URL must be an HTTP(S) origin without credentials".into());
    }
    Ok(raw.trim().trim_end_matches('/').to_string())
}

pub fn meili_config_from_values(
    url: Option<&str>,
    key: Option<&str>,
    key_file: Option<&str>,
    index: Option<&str>,
) -> Result<Option<MeiliConfig>, String> {
    let url = url.map(str::trim).filter(|v| !v.is_empty());
    let Some(url) = url else {
        return Ok(None);
    };
    let url = parse_meili_url(url)?;
    let from_file = match key_file.map(str::trim).filter(|v| !v.is_empty()) {
        Some(path) => {
            let raw = fs::read_to_string(path)
                .map_err(|e| format!("failed to read FVOCI_MEILI_KEY_FILE: {e}"))?;
            Some(raw.trim().to_string())
        }
        None => None,
    };
    let from_env = key
        .map(str::trim)
        .filter(|v| !v.is_empty())
        .map(str::to_string);
    let api_key = from_file.or(from_env).ok_or_else(|| {
        "FVOCI_MEILI_URL is set but FVOCI_MEILI_KEY or FVOCI_MEILI_KEY_FILE is missing".to_string()
    })?;
    if api_key.len() < 16 {
        return Err("FVOCI_MEILI_KEY must be at least 16 characters".into());
    }
    let index_uid = meili_index_uid_from(index)?;
    Ok(Some(MeiliConfig::new(url, api_key, index_uid)))
}

fn meili_index_uid_from(index: Option<&str>) -> Result<String, String> {
    let index_uid = index
        .map(str::trim)
        .filter(|v| !v.is_empty())
        .unwrap_or(MEILI_INDEX_UID)
        .to_string();
    if !index_uid
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
    {
        return Err("FVOCI_MEILI_INDEX contains unsupported characters".into());
    }
    Ok(index_uid)
}

pub fn meili_config_from_env() -> Result<Option<MeiliConfig>, String> {
    meili_config_from_values(
        std::env::var("FVOCI_MEILI_URL").ok().as_deref(),
        std::env::var("FVOCI_MEILI_KEY").ok().as_deref(),
        std::env::var("FVOCI_MEILI_KEY_FILE").ok().as_deref(),
        std::env::var("FVOCI_MEILI_INDEX").ok().as_deref(),
    )
}

pub fn read_meili_master_key_from_env() -> Result<String, String> {
    let key = std::env::var("MEILI_MASTER_KEY")
        .or_else(|_| std::env::var("FVOCI_MEILI_MASTER_KEY"))
        .map_err(|_| "MEILI_MASTER_KEY is required".to_string())?;
    let trimmed = key.trim();
    if trimmed.len() < 16 {
        return Err("MEILI_MASTER_KEY must be at least 16 characters".into());
    }
    Ok(trimmed.to_string())
}

pub async fn ensure_meili_key_file(dest: &Path) -> Result<(), String> {
    let url =
        std::env::var("FVOCI_MEILI_URL").map_err(|_| "FVOCI_MEILI_URL is required".to_string())?;
    let url = parse_meili_url(&url)?;
    let master = read_meili_master_key_from_env()?;
    let index_uid = meili_index_uid_from(std::env::var("FVOCI_MEILI_INDEX").ok().as_deref())?;
    ensure_scoped_meili_key(&url, &master, &index_uid, dest)
        .await
        .map_err(|e| e.to_string())?;
    let key = fs::read_to_string(dest).map_err(|e| format!("meili key file: {e}"))?;
    let config = MeiliConfig::new(url, key.trim().to_string(), index_uid.to_string());
    ensure_meili_index(&config).await.map_err(|e| e.to_string())
}

/// One deadline per public operation (the source's single `opSignal`): request
/// retries, batches and task polling all share it, so a stalled Meili cannot hold
/// a caller for multiples of the per-request timeout.
async fn with_op_deadline<T>(
    fut: impl std::future::Future<Output = Result<T, MeiliError>>,
) -> Result<T, MeiliError> {
    tokio::time::timeout(Duration::from_millis(MEILI_OP_TIMEOUT_MS), fut)
        .await
        .unwrap_or(Err(MeiliError::Timeout))
}

/// Read-only reachability and key check (`fvoci-migrate --doctor`): the
/// scoped key must be able to read its index. A missing index is not an error
/// here — the server creates it lazily.
pub async fn probe_meili_index(config: &MeiliConfig) -> Result<(), MeiliError> {
    with_op_deadline(async {
        let path = format!("/indexes/{}", config.index_uid);
        let resp = meili_request(config, reqwest::Method::GET, &path, None).await?;
        match resp.status {
            200 | 404 => Ok(()),
            status => Err(MeiliError::Http(status)),
        }
    })
    .await
}

pub async fn ensure_meili_index(config: &MeiliConfig) -> Result<(), MeiliError> {
    with_op_deadline(ensure_meili_index_op(config)).await
}

pub async fn delete_meili_sources(config: &MeiliConfig, ids: &[String]) -> Result<(), MeiliError> {
    with_op_deadline(delete_meili_sources_op(config, ids)).await
}

pub async fn delete_meili_by_filter(config: &MeiliConfig, filter: &str) -> Result<(), MeiliError> {
    with_op_deadline(delete_meili_by_filter_op(config, filter)).await
}

pub async fn delete_all_meili_documents(config: &MeiliConfig) -> Result<(), MeiliError> {
    with_op_deadline(delete_all_meili_documents_op(config)).await
}

pub async fn upsert_meili_sources(
    config: &MeiliConfig,
    docs: &[SearchSource],
) -> Result<(), MeiliError> {
    with_op_deadline(upsert_meili_sources_op(config, docs)).await
}

pub async fn enqueue_upsert_meili_sources(
    config: &MeiliConfig,
    docs: &[SearchSource],
) -> Result<Vec<u64>, MeiliError> {
    with_op_deadline(upsert_meili_sources_enqueue(config, docs)).await
}

pub async fn enqueue_delete_meili_sources(
    config: &MeiliConfig,
    ids: &[String],
) -> Result<u64, MeiliError> {
    if ids.is_empty() {
        return Ok(0);
    }
    with_op_deadline(enqueue_meili(
        config,
        reqwest::Method::POST,
        &config.index_path("/documents/delete-batch"),
        Some(&json!(ids)),
    ))
    .await
}

pub async fn enqueue_delete_meili_by_filter(
    config: &MeiliConfig,
    filter: &str,
) -> Result<u64, MeiliError> {
    with_op_deadline(enqueue_meili(
        config,
        reqwest::Method::POST,
        &config.index_path("/documents/delete"),
        Some(&json!({ "filter": filter })),
    ))
    .await
}

pub async fn enqueue_delete_all_meili_documents(config: &MeiliConfig) -> Result<u64, MeiliError> {
    with_op_deadline(enqueue_meili(
        config,
        reqwest::Method::DELETE,
        &config.index_path("/documents"),
        None,
    ))
    .await
}

pub async fn wait_meili_tasks(config: &MeiliConfig, uids: &[u64]) -> Result<(), MeiliError> {
    with_op_deadline(wait_meili_tasks_inner(config, uids)).await
}

pub async fn search_meili(
    config: &MeiliConfig,
    input: &MeiliSearchInput,
) -> Result<MeiliSearchPage, MeiliError> {
    with_op_deadline(search_meili_op(config, input)).await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn meili_eq_rejects_filter_injection() {
        assert!(meili_eq("workspaceId", "a OR 1=1").is_err());
        assert!(meili_eq(
            "workspaceId",
            "11111111-1111-4111-8111-111111111111\" OR kind = \"document"
        )
        .is_err());
        assert!(meili_eq("kind); DROP", "document").is_err());
        assert!(meili_eq(
            "workspaceId;commentId",
            "11111111-1111-4111-8111-111111111111"
        )
        .is_err());
        assert_eq!(
            meili_eq("workspaceId", "11111111-1111-4111-8111-111111111111").unwrap(),
            r#"workspaceId = "11111111-1111-4111-8111-111111111111""#
        );
        assert_eq!(
            meili_eq("kind", "document").unwrap(),
            r#"kind = "document""#
        );
    }

    #[test]
    fn search_source_id_round_trip() {
        let doc = "22222222-2222-4222-8222-222222222222";
        let att = "44444444-4444-4444-8444-444444444444";
        assert_eq!(
            search_source_id(SearchSourceKind::Document, doc, None),
            format!("document_{doc}")
        );
        assert_eq!(
            search_source_id(SearchSourceKind::Attachment, att, None),
            format!("attachment_{att}")
        );
        assert_eq!(
            search_source_id(SearchSourceKind::Attachment, att, Some(0)),
            format!("attachment_{att}_0")
        );
        let parsed = parse_search_source_id(&format!("document_{doc}")).unwrap();
        assert_eq!(parsed.kind, SearchSourceKind::Document);
        assert_eq!(parsed.chunk_no, None);
        assert!(parse_search_source_id(&format!("document:{doc}")).is_none());
        assert!(parse_search_source_id(&format!("document|{doc}")).is_none());
        assert!(parse_search_source_id(&format!("document_{doc}_0")).is_none());
    }

    #[test]
    fn meili_config_disabled_when_url_unset() {
        assert!(
            meili_config_from_values(None, Some("x".repeat(16).as_str()), None, None)
                .unwrap()
                .is_none()
        );
        assert!(
            meili_config_from_values(Some("   "), Some("x".repeat(16).as_str()), None, None)
                .unwrap()
                .is_none()
        );
        let err =
            meili_config_from_values(Some("http://127.0.0.1:7700"), None, None, None).unwrap_err();
        assert!(err.contains("FVOCI_MEILI_KEY"));
    }

    #[test]
    fn meili_debug_redacts_key() {
        let cfg = MeiliConfig::new(
            "http://127.0.0.1:7700".into(),
            "super-secret-meili-key".into(),
            "fvoci".into(),
        );
        let rendered = format!("{cfg:?}");
        assert!(rendered.contains("<redacted>"));
        assert!(!rendered.contains("super-secret-meili-key"));
    }

    #[test]
    fn empty_query_builds_no_search() {
        assert!(meili_search_query("   ", "").is_none());
        let (q, attrs) = meili_search_query("ㄱㅅ", "").unwrap();
        assert_eq!(q, "ㄱㅅ");
        assert_eq!(attrs, vec!["chosung"]);
        let (q, attrs) = meili_search_query("ＦＶＯＣＩ", "").unwrap();
        assert_eq!(q, "FVOCI");
        assert_eq!(attrs, vec!["title", "body", "stem"]);
    }
}
