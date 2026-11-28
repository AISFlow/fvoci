//! A search server, faked for the tests and for the end-to-end tests of the web app:
//! the requests the server makes of Elasticsearch and `OpenSearch`, answered as the two
//! answer them (statuses and the names of errors as recorded from real ones), over
//! documents kept in memory.
//!
//! It keeps what it is sent, and a test can make it lie: plant a document it answers
//! every search with whatever the search asked for, as an index that is behind what
//! is stored would; answer nothing in time; refuse everything; or refuse the next
//! writes. A search finds the documents that pass the request's filter and hold every
//! word asked for, as letters, in their title or text: it knows no language.
//!
//! The controls are also routes under `/_fake`, for a program that drives it from
//! outside (`examples/fake_search.rs`).
#![allow(
    dead_code,
    reason = "the example and individual tests use different controls"
)]

use std::collections::BTreeMap;
use std::fmt::Write as _;
use std::net::SocketAddr;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use axum::Router;
use axum::body::Bytes;
use axum::extract::State as Shared;
use axum::http::{HeaderMap, Method, StatusCode, Uri};
use axum::response::{IntoResponse, Response};
use fvoci_server::config::search::SearchSettings;
use serde::Serialize;
use serde_json::{Value, json};
use sha2::{Digest as _, Sha256};
use tokio::net::TcpListener;
use tokio::task::JoinHandle;

/// The marks of a stretch that matched, as the server asks for them.
const MATCH_START: char = '\u{e000}';
const MATCH_END: char = '\u{e001}';

/// Which of the two the fake says it is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Flavor {
    Elasticsearch,
    OpenSearch,
}

/// One request the fake was sent.
#[derive(Debug, Clone, Serialize)]
pub(crate) struct Recorded {
    pub(crate) method: String,
    pub(crate) path: String,
    /// The `Authorization` header, if it had one.
    pub(crate) authorization: Option<String>,
    /// The body: JSON, or for `_bulk` the list of its lines.
    pub(crate) body: Value,
}

#[derive(Debug, Clone)]
struct Index {
    definition: Value,
    uuid: String,
    /// The documents by identifier.
    documents: BTreeMap<String, Value>,
    /// The documents put since the index was last refreshed: a removal by query does
    /// not see them, as on a real search server. (A search here does: a real one
    /// refreshes by itself within a second.)
    unrefreshed: std::collections::BTreeSet<String>,
    /// The version of every document written, of those removed too: a write of a lower
    /// version is refused, as both search servers refuse one while they remember the
    /// removal (`index.gc_deletes`; here, for ever).
    versions: BTreeMap<String, i64>,
}

struct State {
    flavor: Flavor,
    /// Whether the nori plugin is installed.
    nori: AtomicBool,
    /// Whether every request is answered 503.
    down: AtomicBool,
    /// How long every request waits before it is answered.
    delay: Mutex<Duration>,
    /// Whether a search is answered as one that ran out of the search server's time.
    partial: AtomicBool,
    /// How many of the next writes of documents are refused.
    failing_writes: AtomicUsize,
    /// The `Authorization` header a request must carry, if any.
    authorization: Mutex<Option<String>>,
    indices: Mutex<BTreeMap<String, Index>>,
    /// What every search is answered with first, whatever it asked for.
    planted: Mutex<Vec<(String, Value)>>,
    recorded: Mutex<Vec<Recorded>>,
    made: AtomicUsize,
}

pub(crate) struct FakeSearch {
    pub(crate) address: SocketAddr,
    state: Arc<State>,
    serving: JoinHandle<()>,
}

impl Drop for FakeSearch {
    fn drop(&mut self) {
        self.serving.abort();
    }
}

/// The digest the server indexes a title and a text with (`search/document.rs`).
pub(crate) fn digest(title: &str, text: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(u64::try_from(title.len()).expect("a length").to_be_bytes());
    hasher.update(title.as_bytes());
    hasher.update(text.as_bytes());
    let mut digest = String::new();
    for byte in hasher.finalize().iter().take(16) {
        write!(digest, "{byte:02x}").expect("writing to a string");
    }
    digest
}

/// A document as the server sends one, of a card with this title and text.
pub(crate) fn document(
    org_id: &str,
    workspace_id: &str,
    container_id: &str,
    title: &str,
    text: &str,
) -> Value {
    json!({
        "orgId": org_id, "workspaceId": workspace_id, "containerId": container_id,
        "kind": "card", "digest": digest(title, text), "title": title, "text": text,
    })
}

/// [`document`], of a page.
pub(crate) fn page_document(
    org_id: &str,
    workspace_id: &str,
    container_id: &str,
    title: &str,
    text: &str,
) -> Value {
    let mut document = document(org_id, workspace_id, container_id, title, text);
    document["kind"] = json!("page");
    document
}

impl FakeSearch {
    pub(crate) async fn start() -> Self {
        Self::start_on(0, Flavor::OpenSearch).await
    }

    pub(crate) async fn start_as(flavor: Flavor) -> Self {
        Self::start_on(0, flavor).await
    }

    pub(crate) async fn start_on(port: u16, flavor: Flavor) -> Self {
        let listener = TcpListener::bind(("127.0.0.1", port))
            .await
            .expect("a local port is free");
        let address = listener.local_addr().expect("the listener's address");
        let state = Arc::new(State {
            flavor,
            nori: AtomicBool::new(true),
            down: AtomicBool::new(false),
            delay: Mutex::new(Duration::ZERO),
            partial: AtomicBool::new(false),
            failing_writes: AtomicUsize::new(0),
            authorization: Mutex::new(None),
            indices: Mutex::default(),
            planted: Mutex::default(),
            recorded: Mutex::default(),
            made: AtomicUsize::new(0),
        });
        let router = Router::new()
            .fallback(answer)
            .with_state(Arc::clone(&state));
        let serving = tokio::spawn(async move {
            let _ = axum::serve(listener, router).await;
        });
        Self {
            address,
            state,
            serving,
        }
    }

    /// The keys that point a server at the fake.
    pub(crate) fn settings(&self) -> SearchSettings {
        SearchSettings {
            search_url: format!("http://{}", self.address),
            // The limit has a test of its own; the others search as often as they like.
            search_requests_per_second: 0,
            // A test says when what changed is sent (`fvoci_server::search::tend`).
            upkeep_by_hand: true,
            ..SearchSettings::default()
        }
    }

    /// Takes the nori plugin away: an index that asks for it is refused.
    pub(crate) fn without_nori(&self) {
        self.state.nori.store(false, Ordering::SeqCst);
    }

    /// Answers every request with 503 from now on.
    pub(crate) fn go_down(&self) {
        self.state.down.store(true, Ordering::SeqCst);
    }

    /// Answers again.
    pub(crate) fn come_up(&self) {
        self.state.down.store(false, Ordering::SeqCst);
    }

    /// Answers every search from now on as one that ran out of the search server's own
    /// time, with what it found until then.
    pub(crate) fn answer_in_part(&self) {
        self.state.partial.store(true, Ordering::SeqCst);
    }

    /// Removes an index behind the server's back, as an operator might.
    pub(crate) fn remove_index(&self, name: &str) {
        self.state.indices.lock().expect("the indices").remove(name);
    }

    /// Makes every request wait so long before it is answered.
    pub(crate) fn set_delay(&self, delay: Duration) {
        *self.state.delay.lock().expect("the delay") = delay;
    }

    /// Refuses the next `count` writes of documents.
    pub(crate) fn fail_writes(&self, count: usize) {
        self.state.failing_writes.store(count, Ordering::SeqCst);
    }

    /// Takes only requests that carry this `Authorization` header.
    pub(crate) fn require_authorization(&self, header: &str) {
        *self.state.authorization.lock().expect("the header") = Some(header.to_owned());
    }

    /// Answers every search with this document first, whatever the search asked for.
    pub(crate) fn plant(&self, id: &str, document: Value) {
        self.state
            .planted
            .lock()
            .expect("the planted documents")
            .push((id.to_owned(), document));
    }

    /// Forgets the planted documents.
    pub(crate) fn clear_planted(&self) {
        self.state
            .planted
            .lock()
            .expect("the planted documents")
            .clear();
    }

    /// Makes an index as some other program left it: with this description.
    pub(crate) fn put_index(&self, name: &str, definition: Value) {
        let uuid = self.state.next_uuid();
        self.state.indices.lock().expect("the indices").insert(
            name.to_owned(),
            Index {
                definition,
                uuid,
                documents: BTreeMap::new(),
                unrefreshed: std::collections::BTreeSet::new(),
                versions: BTreeMap::new(),
            },
        );
    }

    /// The description an index was made with; `None` if there is none of the name.
    pub(crate) fn index_definition(&self, name: &str) -> Option<Value> {
        let indices = self.state.indices.lock().expect("the indices");
        indices.get(name).map(|index| index.definition.clone())
    }

    /// The documents of an index, by identifier; none if there is no such index.
    pub(crate) fn documents(&self, name: &str) -> BTreeMap<String, Value> {
        let indices = self.state.indices.lock().expect("the indices");
        indices
            .get(name)
            .map_or_default(|index| index.documents.clone())
    }

    /// Every request so far, the first first.
    pub(crate) fn recorded(&self) -> Vec<Recorded> {
        self.state.recorded.lock().expect("the requests").clone()
    }

    /// The requests so far whose path ends with `suffix`.
    pub(crate) fn recorded_at(&self, suffix: &str) -> Vec<Recorded> {
        self.recorded()
            .into_iter()
            .filter(|request| request.path.ends_with(suffix))
            .collect()
    }

    /// The searches the fake was sent: what asked `_search` for documents, and not for
    /// the highest version of the index alone.
    pub(crate) fn searches(&self) -> Vec<Recorded> {
        self.recorded_at("/_search")
            .into_iter()
            .filter(|request| request.body["aggs"].is_null())
            .collect()
    }

    /// The version the index holds of the document `id`, or remembers of its removal.
    pub(crate) fn version_of(&self, index: &str, id: &str) -> Option<i64> {
        let indices = self.state.indices.lock().expect("the indices");
        indices.get(index)?.versions.get(id).copied()
    }
}

impl State {
    fn next_uuid(&self) -> String {
        format!("fake-uuid-{}", self.made.fetch_add(1, Ordering::SeqCst))
    }
}

/// An answer that refuses, in the shape both search servers refuse in.
fn refusal(status: StatusCode, kind: &str, reason: &str) -> Response {
    let error = json!({"type": kind, "reason": reason});
    let body = json!({
        "error": {"root_cause": [error], "type": kind, "reason": reason},
        "status": status.as_u16(),
    });
    (status, axum::Json(body)).into_response()
}

fn no_such_index(name: &str) -> Response {
    refusal(
        StatusCode::NOT_FOUND,
        "index_not_found_exception",
        &format!("no such index [{name}]"),
    )
}

async fn answer(
    Shared(state): Shared<Arc<State>>,
    method: Method,
    uri: Uri,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    let path = uri.path().to_owned();
    if let Some(control) = path.strip_prefix("/_fake/") {
        return control_route(&state, &method, control, &body);
    }
    let authorization = headers
        .get("authorization")
        .and_then(|value| value.to_str().ok())
        .map(str::to_owned);
    let text = String::from_utf8_lossy(&body).into_owned();
    let recorded_body = if path.ends_with("/_bulk") {
        Value::Array(
            text.lines()
                .filter_map(|line| serde_json::from_str(line).ok())
                .collect(),
        )
    } else {
        serde_json::from_str(&text).unwrap_or(Value::Null)
    };
    state.recorded.lock().expect("the requests").push(Recorded {
        method: method.to_string(),
        path: path.clone(),
        authorization: authorization.clone(),
        body: recorded_body.clone(),
    });
    let delay = *state.delay.lock().expect("the delay");
    if !delay.is_zero() {
        tokio::time::sleep(delay).await;
    }
    if state.down.load(Ordering::SeqCst) {
        return refusal(
            StatusCode::SERVICE_UNAVAILABLE,
            "cluster_block_exception",
            "blocked by: [SERVICE_UNAVAILABLE/1/state not recovered / initialized];",
        );
    }
    let wanted = state.authorization.lock().expect("the header").clone();
    if wanted.is_some() && wanted != authorization {
        return refusal(
            StatusCode::UNAUTHORIZED,
            "security_exception",
            "missing authentication credentials for REST request",
        );
    }
    let segments: Vec<&str> = path.split('/').filter(|part| !part.is_empty()).collect();
    match (method.as_str(), segments.as_slice()) {
        ("GET", []) => axum::Json(root(&state)).into_response(),
        ("GET", [name]) => describe(&state, name),
        ("PUT", [name]) => create(&state, name, recorded_body),
        ("DELETE", [name]) => {
            let removed = state.indices.lock().expect("the indices").remove(*name);
            match removed {
                Some(_) => axum::Json(json!({"acknowledged": true})).into_response(),
                None => no_such_index(name),
            }
        }
        ("GET", [name, "_doc", id]) => document_of(&state, name, id),
        ("POST", [name, "_search"]) => search(&state, name, &recorded_body),
        ("POST", [name, "_bulk"]) => bulk(&state, name, &text),
        ("POST", [name, "_refresh"]) => {
            let mut indices = state.indices.lock().expect("the indices");
            match indices.get_mut(*name) {
                Some(index) => {
                    index.unrefreshed.clear();
                    axum::Json(json!({"_shards": {"total": 2, "successful": 1, "failed": 0}}))
                        .into_response()
                }
                None => no_such_index(name),
            }
        }
        ("POST", [name, "_delete_by_query"]) => delete_by_query(&state, name, &recorded_body),
        _ => refusal(
            StatusCode::BAD_REQUEST,
            "illegal_argument_exception",
            "the fake search server knows no such request",
        ),
    }
}

/// The controls, for a program outside: `POST /_fake/down` and `/_fake/up`,
/// `POST /_fake/fail-writes` (a number), `POST /_fake/plant` (`{id, document}`) and
/// `GET /_fake/requests`.
fn control_route(state: &State, method: &Method, control: &str, body: &[u8]) -> Response {
    let body: Value = serde_json::from_slice(body).unwrap_or(Value::Null);
    match (method.as_str(), control) {
        ("POST", "down") => state.down.store(true, Ordering::SeqCst),
        ("POST", "up") => state.down.store(false, Ordering::SeqCst),
        ("POST", "fail-writes") => {
            let count = body.as_u64().and_then(|count| usize::try_from(count).ok());
            state
                .failing_writes
                .store(count.unwrap_or(1), Ordering::SeqCst);
        }
        ("POST", "plant") => {
            let id = body["id"].as_str().unwrap_or_default().to_owned();
            let mut planted = state.planted.lock().expect("the planted documents");
            planted.push((id, body["document"].clone()));
        }
        ("GET", "requests") => {
            let recorded = state.recorded.lock().expect("the requests").clone();
            return axum::Json(recorded).into_response();
        }
        _ => return StatusCode::NOT_FOUND.into_response(),
    }
    StatusCode::NO_CONTENT.into_response()
}

/// What each of the two says of itself at its root.
fn root(state: &State) -> Value {
    match state.flavor {
        Flavor::OpenSearch => json!({
            "name": "fake", "cluster_name": "fake",
            "version": {"distribution": "opensearch", "number": "3.9.0"},
            "tagline": "The OpenSearch Project: https://opensearch.org/",
        }),
        Flavor::Elasticsearch => json!({
            "name": "fake", "cluster_name": "fake",
            "version": {"number": "9.5.6", "build_flavor": "default"},
            "tagline": "You Know, for Search",
        }),
    }
}

fn describe(state: &State, name: &str) -> Response {
    let indices = state.indices.lock().expect("the indices");
    let Some(index) = indices.get(name) else {
        return no_such_index(name);
    };
    let mut settings = index.definition["settings"].clone();
    if !settings.is_object() {
        settings = json!({});
    }
    settings["index"] = json!({"uuid": index.uuid, "provided_name": name});
    axum::Json(json!({name: {
        "aliases": {},
        "mappings": index.definition["mappings"],
        "settings": settings,
    }}))
    .into_response()
}

fn create(state: &State, name: &str, definition: Value) -> Response {
    if !state.nori.load(Ordering::SeqCst) && definition.to_string().contains("nori_tokenizer") {
        return refusal(
            StatusCode::BAD_REQUEST,
            "illegal_argument_exception",
            "Unknown tokenizer type [nori_tokenizer] for [fvoci_nori]",
        );
    }
    let mut indices = state.indices.lock().expect("the indices");
    if let Some(index) = indices.get(name) {
        return refusal(
            StatusCode::BAD_REQUEST,
            "resource_already_exists_exception",
            &format!("index [{name}/{}] already exists", index.uuid),
        );
    }
    indices.insert(
        name.to_owned(),
        Index {
            definition,
            uuid: state.next_uuid(),
            documents: BTreeMap::new(),
            unrefreshed: std::collections::BTreeSet::new(),
            versions: BTreeMap::new(),
        },
    );
    axum::Json(json!({"acknowledged": true, "shards_acknowledged": true, "index": name}))
        .into_response()
}

/// Whether a document passes the `term` and `terms` clauses of a filter.
fn passes(document: &Value, filter: &Value) -> bool {
    filter.as_array().is_some_and(|clauses| {
        clauses.iter().all(|clause| {
            if let Some(term) = clause["term"].as_object() {
                term.iter().all(|(field, value)| &document[field] == value)
            } else if let Some(terms) = clause["terms"].as_object() {
                terms.iter().all(|(field, values)| {
                    values
                        .as_array()
                        .is_some_and(|values| values.contains(&document[field]))
                })
            } else {
                false
            }
        })
    })
}

/// A hit as both answer it: the identifier, the digest, and the text with the first
/// word that is found in it marked, or its beginning if none is.
fn hit(id: &str, document: &Value, words: &[String]) -> Value {
    let text = document["text"].as_str().unwrap_or_default();
    let lower = text.to_lowercase();
    let marked = words
        .iter()
        .find_map(|word| {
            // Marked by the letters of the lower-cased text only where the two line up.
            let at = lower.find(word.as_str())?;
            let end = at + word.len();
            (text.is_char_boundary(at) && text.is_char_boundary(end)).then(|| {
                format!(
                    "{}{MATCH_START}{}{MATCH_END}{}",
                    &text[..at],
                    &text[at..end],
                    &text[end..]
                )
            })
        })
        .unwrap_or_else(|| text.to_owned());
    let mut hit = json!({
        "_index": "fake", "_id": id, "_score": 1.0,
        "_source": {"digest": document["digest"]},
    });
    if !marked.is_empty() {
        hit["highlight"] = json!({"text": [marked]});
    }
    hit
}

fn search(state: &State, name: &str, request: &Value) -> Response {
    let indices = state.indices.lock().expect("the indices");
    let Some(index) = indices.get(name) else {
        return no_such_index(name);
    };
    if !request["aggs"].is_null() {
        let highest = index
            .documents
            .values()
            .filter_map(|document| document["sent"].as_i64())
            .max();
        return axum::Json(json!({
            "took": 1, "timed_out": false,
            "_shards": {"total": 1, "successful": 1, "skipped": 0, "failed": 0},
            "hits": {"max_score": null, "hits": []},
            "aggregations": {"sent": {"value": highest}},
        }))
        .into_response();
    }
    let query = &request["query"]["bool"];
    let asked = query["must"][0]["bool"]["should"][0]["match"]["all"]["query"]
        .as_str()
        .unwrap_or_default();
    let words: Vec<String> = asked.split_whitespace().map(str::to_lowercase).collect();
    let count = |member: &str, default: usize| {
        request[member]
            .as_u64()
            .and_then(|count| usize::try_from(count).ok())
            .unwrap_or(default)
    };
    let (from, size) = (count("from", 0), count("size", 10));
    let planted = state.planted.lock().expect("the planted documents");
    let mut hits: Vec<Value> = planted
        .iter()
        .map(|(id, document)| hit(id, document, &words))
        .collect();
    for (id, document) in &index.documents {
        let all = format!(
            "{} {}",
            document["title"].as_str().unwrap_or_default(),
            document["text"].as_str().unwrap_or_default()
        )
        .to_lowercase();
        if passes(document, &query["filter"]) && words.iter().all(|word| all.contains(word)) {
            hits.push(hit(id, document, &words));
        }
    }
    let hits: Vec<Value> = hits.into_iter().skip(from).take(size).collect();
    axum::Json(json!({
        "took": 1, "timed_out": state.partial.load(Ordering::SeqCst),
        "_shards": {"total": 1, "successful": 1, "skipped": 0, "failed": 0},
        "hits": {"max_score": 1.0, "hits": hits},
    }))
    .into_response()
}

/// A document and its version, as `GET /{index}/_doc/{id}` answers.
fn document_of(state: &State, name: &str, id: &str) -> Response {
    let indices = state.indices.lock().expect("the indices");
    let Some(index) = indices.get(name) else {
        return no_such_index(name);
    };
    match index.documents.get(id) {
        Some(source) => axum::Json(json!({
            "_index": name, "_id": id, "found": true,
            "_version": index.versions.get(id).copied().unwrap_or(1), "_source": source,
        }))
        .into_response(),
        None => (
            StatusCode::NOT_FOUND,
            axum::Json(json!({"_index": name, "_id": id, "found": false})),
        )
            .into_response(),
    }
}

/// The refusal of a write (`action`, the member of its line) whose version is too low
/// for the version `held` of its document: `external` writes a version higher than the
/// one held, `external_gte` one as high or higher. `None` for a write it may do.
fn version_conflict(name: &str, id: &str, action: &Value, held: Option<i64>) -> Option<Value> {
    let version = action["version"].as_i64()?;
    let held = held?;
    let as_high = action["version_type"] == "external_gte";
    if version > held || (as_high && version == held) {
        return None;
    }
    let higher = if as_high {
        "higher"
    } else {
        "higher or equal to"
    };
    Some(json!({"_index": name, "_id": id, "status": 409, "error": {
        "type": "version_conflict_engine_exception",
        "reason": format!("[{id}]: version conflict, current version [{held}] is {higher} than the one provided [{version}]"),
    }}))
}

fn bulk(state: &State, name: &str, text: &str) -> Response {
    let mut indices = state.indices.lock().expect("the indices");
    // As both search servers do unless they are told otherwise: an index that is not
    // there is made by the first document put into it, with whatever mapping the
    // document suggests, and nothing says so.
    if !indices.contains_key(name) && text.contains("{\"index\":") {
        indices.insert(
            name.to_owned(),
            Index {
                definition: json!({"mappings": {"properties": {}}}),
                uuid: state.next_uuid(),
                documents: BTreeMap::new(),
                unrefreshed: std::collections::BTreeSet::new(),
                versions: BTreeMap::new(),
            },
        );
    }
    let Some(index) = indices.get_mut(name) else {
        return no_such_index(name);
    };
    let mut lines = text.lines().filter(|line| !line.trim().is_empty());
    let mut items = Vec::new();
    let mut errors = false;
    while let Some(line) = lines.next() {
        let action: Value = serde_json::from_str(line).unwrap_or(Value::Null);
        let (operation, id) = if let Some(id) = action["index"]["_id"].as_str() {
            ("index", id.to_owned())
        } else if let Some(id) = action["delete"]["_id"].as_str() {
            ("delete", id.to_owned())
        } else {
            return refusal(
                StatusCode::BAD_REQUEST,
                "illegal_argument_exception",
                "Malformed action/metadata line",
            );
        };
        let source: Option<Value> = (operation == "index")
            .then(|| {
                lines
                    .next()
                    .and_then(|line| serde_json::from_str(line).ok())
            })
            .flatten();
        let failing = state
            .failing_writes
            .try_update(Ordering::SeqCst, Ordering::SeqCst, |left| {
                left.checked_sub(1)
            })
            .is_ok();
        let version = action[operation]["version"].as_i64();
        let held = index.versions.get(&id).copied();
        let conflict = version_conflict(name, &id, &action[operation], held);
        let item = if failing {
            errors = true;
            json!({"_index": name, "_id": id, "status": 429, "error": {
                "type": "es_rejected_execution_exception",
                "reason": "rejected execution of coordinating operation",
            }})
        } else if let Some(conflict) = conflict {
            errors = true;
            conflict
        } else if operation == "index" {
            let Some(source) = source else {
                return refusal(
                    StatusCode::BAD_REQUEST,
                    "illegal_argument_exception",
                    "The bulk request must be terminated by a newline [\\n]",
                );
            };
            index.unrefreshed.insert(id.clone());
            index
                .versions
                .insert(id.clone(), version.unwrap_or(held.unwrap_or(0) + 1));
            let created = index.documents.insert(id.clone(), source).is_none();
            let (result, status) = if created {
                ("created", 201)
            } else {
                ("updated", 200)
            };
            json!({"_index": name, "_id": id, "result": result, "status": status})
        } else if index.documents.remove(&id).is_some() {
            index
                .versions
                .insert(id.clone(), version.unwrap_or(held.unwrap_or(0) + 1));
            json!({"_index": name, "_id": id, "result": "deleted", "status": 200})
        } else {
            if let Some(version) = version {
                index.versions.insert(id.clone(), version);
            }
            json!({"_index": name, "_id": id, "result": "not_found", "status": 404})
        };
        items.push(json!({operation: item}));
    }
    axum::Json(json!({"took": 1, "errors": errors, "items": items})).into_response()
}

fn delete_by_query(state: &State, name: &str, request: &Value) -> Response {
    let mut indices = state.indices.lock().expect("the indices");
    let Some(index) = indices.get_mut(name) else {
        return no_such_index(name);
    };
    let filter = &request["query"]["bool"]["filter"];
    let unrefreshed = std::mem::take(&mut index.unrefreshed);
    let removed: Vec<String> = index
        .documents
        .iter()
        .filter(|(id, document)| !unrefreshed.contains(*id) && passes(document, filter))
        .map(|(id, _)| id.clone())
        .collect();
    // As both search servers do: a document removed by query has a version one past
    // the one it was written with.
    for id in &removed {
        index.documents.remove(id);
        *index.versions.entry(id.clone()).or_insert(0) += 1;
    }
    let deleted = removed.len();
    axum::Json(json!({
        "took": 1, "timed_out": false, "total": deleted, "deleted": deleted,
        "version_conflicts": 0, "failures": [],
    }))
    .into_response()
}
