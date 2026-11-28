//! The requests to the search server, and what its answers mean.
//!
//! Elasticsearch and `OpenSearch` take the same requests here: making and removing an
//! index, describing it, and `_search`. Where the two differ, in how the server signs
//! in and in what an answer that refuses says, this file reads both. Nothing of an
//! answer is kept but its status, the name of its error, and what a search found:
//! never its text, which may repeat what a card or a page says.

use std::time::Duration;

use fvoci_model::SnippetPart;
use fvoci_store::SearchDocument;
use serde_json::{Value, json};

use super::query::{MATCH_END, MATCH_START, MatchBy, Scope};
use super::version::DocumentVersion;
use super::{document, index};
use crate::config::search::{SearchCredentials, SearchServer};

/// The longest the connection to the search server may take to open.
const CONNECT_TIMEOUT: Duration = Duration::from_secs(2);
/// How long a request that makes, removes or describes the index, or writes or
/// removes documents, may take.
const INDEX_TIMEOUT: Duration = Duration::from_secs(30);
/// About the most one request that writes documents carries: both search servers
/// take 100 MB by default.
const MOST_WRITTEN: usize = 16 * 1024 * 1024;
/// The most of an answer that is read. An answer to a search holds as many pieces of
/// text as it was asked for, each about a line long.
const MOST_BODY: usize = 8 * 1024 * 1024;
/// The most of the name of an error that is kept.
const MOST_KIND: usize = 64;

/// Why no answer came from the search server.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum NoAnswer {
    /// The connection could not be opened: nothing listens there, or its certificate
    /// does not hold.
    Connection,
    /// The answer did not come in the time the request had.
    Timeout,
    /// Anything else: the connection broke, or the request could not be sent.
    Other,
}

impl std::fmt::Display for NoAnswer {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::Connection => "no connection",
            Self::Timeout => "no answer in time",
            Self::Other => "the request failed",
        })
    }
}

impl From<&reqwest::Error> for NoAnswer {
    fn from(error: &reqwest::Error) -> Self {
        if error.is_timeout() {
            Self::Timeout
        } else if error.is_connect() {
            Self::Connection
        } else {
            Self::Other
        }
    }
}

/// Why a request to the search server came to nothing.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub(crate) enum ClientError {
    /// No answer: the connection failed, or the answer did not come in time.
    #[error("the search server did not answer ({0})")]
    Unreachable(NoAnswer),
    /// A search was answered with a part of what matches, or a removal removed a
    /// part: the search server ran out of its own time, or some of the index did not
    /// answer.
    #[error("the search server did a request in part")]
    Partial,
    /// The search server has no analyser of Korean text.
    #[error("the search server has no nori analysis plugin")]
    NoNori,
    /// The search server holds, under the name of the index, what this server may
    /// not make its index of: an index it did not make, or one a newer server made.
    #[error("the search server holds another index under the name")]
    NoIndex,
    /// The search server refused the request: its status, and the name of its error.
    #[error("the search server answered {status} ({kind})")]
    Refused {
        /// The status of the answer.
        status: u16,
        /// The `type` of its error, or `unknown`.
        kind: String,
    },
    /// An answer that is not what the search server answers such a request with.
    #[error("the search server's answer could not be read")]
    Unreadable,
}

impl ClientError {
    /// Whether the search server said that the index is not there.
    pub(crate) fn is_missing_index(&self) -> bool {
        matches!(self, Self::Refused { status: 404, kind } if kind == "index_not_found_exception")
    }
}

/// What is at the name of the index.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum IndexFound {
    /// Nothing.
    Absent,
    /// An index with this server's mark, made from the definition of some version.
    Ours {
        /// The version of [`index::definition`] it was made from.
        version: i64,
        /// The identifier the search server gave it when it was made.
        uuid: String,
    },
    /// An index that does not say it is this server's.
    Foreign,
}

/// A write of one document, of what was read from the database at `version`
/// ([`super::version::Versions`]). The search server does it unless it did a write of
/// the same document of a higher version: then it refuses it (409,
/// `version_conflict_engine_exception`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Operation {
    /// The document is put, in place of what the search server holds of its card or
    /// page.
    Put {
        document: SearchDocument,
        version: DocumentVersion,
    },
    /// The document of the card of this identifier is removed, if there is one.
    Remove {
        id: String,
        version: DocumentVersion,
    },
}

/// Why the search server did not do a write of one document.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Refused {
    /// The `type` of its error, or `unknown`.
    pub(crate) kind: String,
    /// For a write of a version lower than what it holds of the document (or remembers
    /// of its removal): that version, as the search server says it.
    pub(crate) holds: Option<i64>,
}

impl Operation {
    /// The lines of the operation in a `_bulk` request.
    fn lines(&self) -> String {
        match self {
            Self::Put { document, version } => format!(
                "{}\n{}\n",
                json!({"index": versioned(&document.id, *version)}),
                document::source(document, *version)
            ),
            Self::Remove { id, version } => {
                format!("{}\n", json!({"delete": versioned(id, *version)}))
            }
        }
    }

    /// The member of an item of the answer that says what became of the operation.
    fn answered_under(&self) -> &'static str {
        match self {
            Self::Put { .. } => "index",
            Self::Remove { .. } => "delete",
        }
    }
}

/// What a line of `_bulk` says of the document it writes: its identifier and its
/// version. `external_gte` and not `external`: a removal by query
/// ([`Client::remove_documents`]) leaves a document's version one past the one it was
/// written with, and what is put after it, read later, may have just that one.
fn versioned(id: &str, version: DocumentVersion) -> Value {
    json!({"_id": id, "version": version.get(), "version_type": "external_gte"})
}

/// The version a refusal of a write for its version says the search server holds of
/// the document: both say `… current version [N] is higher …` under the type
/// `version_conflict_engine_exception`. `None` for any other refusal.
fn version_held(outcome: &Value) -> Option<i64> {
    if outcome.pointer("/error/type")?.as_str()? != "version_conflict_engine_exception" {
        return None;
    }
    let reason = outcome.pointer("/error/reason")?.as_str()?;
    let (_, after) = reason.split_once("current version [")?;
    let (digits, _) = after.split_once(']')?;
    digits.parse().ok()
}

/// What a search found of one document.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Hit {
    /// The identifier of the document, which is its card's or page's.
    pub(crate) id: String,
    /// The digest the document was indexed with.
    pub(crate) digest: String,
    /// A piece of the text it was indexed with.
    pub(crate) snippet: Vec<SnippetPart>,
}

/// The client of the search server: no redirect is followed and no proxy of the
/// environment is used. A request carries the server's credentials and what cards and pages
/// say, and goes where `search_url` says, nowhere else.
#[derive(Debug, Clone)]
pub(crate) struct Client {
    http: reqwest::Client,
    /// `scheme://host[:port]`.
    base: String,
    index: String,
    credentials: SearchCredentials,
}

/// An answer of the search server, as much as is read of it.
struct Answer {
    status: u16,
    body: Value,
}

impl Answer {
    /// The refusal this answer is, if its status says it is one.
    fn refusal(&self) -> Option<ClientError> {
        if (200..300).contains(&self.status) {
            return None;
        }
        Some(ClientError::Refused {
            status: self.status,
            kind: kind_of(&self.body),
        })
    }
}

/// The name of the error of an answer: letters, digits and underscores, as both
/// search servers name theirs, and `unknown` for anything else.
fn kind_of(body: &Value) -> String {
    body.pointer("/error/type")
        .and_then(Value::as_str)
        .filter(|kind| {
            !kind.is_empty()
                && kind.len() <= MOST_KIND
                && kind.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
        })
        .unwrap_or("unknown")
        .to_owned()
}

impl Client {
    /// The client of the search server the configuration names.
    ///
    /// # Errors
    ///
    /// Returns an error if the TLS of the client cannot be set up.
    pub(crate) fn new(server: &SearchServer) -> anyhow::Result<Self> {
        let mut builder = reqwest::Client::builder()
            .redirect(reqwest::redirect::Policy::none())
            .no_proxy()
            .connect_timeout(CONNECT_TIMEOUT);
        if let Some(ca_path) = &server.ca_file {
            let pem = std::fs::read(ca_path).map_err(|error| {
                anyhow::anyhow!("unable to read search_ca_file {ca_path:?}: {error}")
            })?;
            for cert in reqwest::Certificate::from_pem_bundle(&pem).map_err(|error| {
                anyhow::anyhow!("invalid PEM certificate in search_ca_file {ca_path:?}: {error}")
            })? {
                builder = builder.add_root_certificate(cert);
            }
        }
        let http = builder.build()?;
        Ok(Self {
            http,
            base: server.url.clone(),
            index: server.index.clone(),
            credentials: server.credentials.clone(),
        })
    }

    /// The address of the index, or of `under` it (`/_search`).
    fn url(&self, under: &str) -> String {
        format!("{}/{}{under}", self.base, self.index)
    }

    /// Sends `request` as the server signs in, and reads the answer.
    async fn send(
        &self,
        request: reqwest::RequestBuilder,
        timeout: Duration,
    ) -> Result<Answer, ClientError> {
        let request = match &self.credentials {
            SearchCredentials::None => request,
            SearchCredentials::Basic { username, password } => {
                request.basic_auth(username, Some(password))
            }
            SearchCredentials::ApiKey(key) => {
                request.header("authorization", format!("ApiKey {key}"))
            }
        };
        let exchange = async {
            let mut response = request
                .send()
                .await
                .map_err(|error| ClientError::Unreachable(NoAnswer::from(&error)))?;
            let status = response.status().as_u16();
            let mut body = Vec::new();
            while let Some(chunk) = response
                .chunk()
                .await
                .map_err(|error| ClientError::Unreachable(NoAnswer::from(&error)))?
            {
                if body.len() + chunk.len() > MOST_BODY {
                    return Err(ClientError::Unreadable);
                }
                body.extend_from_slice(&chunk);
            }
            // An answer without a body, or with one that is no JSON (a proxy's page),
            // still has its status.
            let body = serde_json::from_slice(&body).unwrap_or(Value::Null);
            Ok(Answer { status, body })
        };
        tokio::time::timeout(timeout, exchange)
            .await
            .unwrap_or(Err(ClientError::Unreachable(NoAnswer::Timeout)))
    }

    /// What is at the name of the index.
    ///
    /// # Errors
    ///
    /// Returns why the search server did not say.
    pub(crate) async fn describe_index(&self) -> Result<IndexFound, ClientError> {
        let answer = self
            .send(self.http.get(self.url("")), INDEX_TIMEOUT)
            .await?;
        if answer.status == 404 {
            return Ok(IndexFound::Absent);
        }
        if let Some(refusal) = answer.refusal() {
            return Err(refusal);
        }
        // The description is under the name of the index, or under the name of the
        // index an alias of this name stands for: one index either way.
        let described = answer
            .body
            .as_object()
            .filter(|indices| indices.len() == 1)
            .and_then(|indices| indices.values().next())
            .ok_or(ClientError::Unreadable)?;
        let Some(version) = index::version_of(described) else {
            return Ok(IndexFound::Foreign);
        };
        let uuid = described
            .pointer("/settings/index/uuid")
            .and_then(Value::as_str)
            .ok_or(ClientError::Unreadable)?;
        Ok(IndexFound::Ours {
            version,
            uuid: uuid.to_owned(),
        })
    }

    /// Makes the index, from [`index::definition`].
    ///
    /// # Errors
    ///
    /// Returns [`ClientError::NoNori`] if the search server knows no nori, and
    /// otherwise why the index was not made: one that is there already is a refusal.
    pub(crate) async fn create_index(&self) -> Result<(), ClientError> {
        let request = self.http.put(self.url("")).json(&index::definition());
        let answer = self.send(request, INDEX_TIMEOUT).await?;
        let Some(refusal) = answer.refusal() else {
            return Ok(());
        };
        // Both say `Unknown tokenizer type [nori_tokenizer] for [...]`, or the same of
        // a filter of nori's, with the status 400.
        let names_nori = answer
            .body
            .pointer("/error/reason")
            .and_then(Value::as_str)
            .is_some_and(|reason| reason.contains("nori_"));
        if answer.status == 400 && names_nori {
            return Err(ClientError::NoNori);
        }
        Err(refusal)
    }

    /// Removes the index; one that is not there is removed.
    ///
    /// # Errors
    ///
    /// Returns why the index was not removed.
    pub(crate) async fn delete_index(&self) -> Result<(), ClientError> {
        let answer = self
            .send(self.http.delete(self.url("")), INDEX_TIMEOUT)
            .await?;
        match answer.refusal() {
            Some(refusal) if answer.status != 404 => Err(refusal),
            _ => Ok(()),
        }
    }

    /// Writes documents, in the order given. Returns what became of each: `None` for
    /// one that was written (a removal of what was not there too), or the name of the
    /// search server's error for one it refused.
    ///
    /// # Errors
    ///
    /// Returns why a request was not taken at all: then none of it was written, and
    /// what earlier requests of this call wrote stays.
    pub(crate) async fn write(
        &self,
        operations: &[Operation],
    ) -> Result<Vec<Option<Refused>>, ClientError> {
        let mut refusals = Vec::with_capacity(operations.len());
        let mut sent = 0;
        while sent < operations.len() {
            let mut body = String::new();
            let mut count = 0;
            for operation in &operations[sent..] {
                let lines = operation.lines();
                if count > 0 && body.len() + lines.len() > MOST_WRITTEN {
                    break;
                }
                body.push_str(&lines);
                count += 1;
            }
            let batch = &operations[sent..sent + count];
            refusals.extend(self.write_batch(batch, body).await?);
            sent += count;
        }
        Ok(refusals)
    }

    /// One `_bulk` request of [`Client::write`].
    async fn write_batch(
        &self,
        operations: &[Operation],
        body: String,
    ) -> Result<Vec<Option<Refused>>, ClientError> {
        let request = self
            .http
            .post(self.url("/_bulk"))
            .header("content-type", "application/x-ndjson")
            .body(body);
        let answer = self.send(request, INDEX_TIMEOUT).await?;
        if let Some(refusal) = answer.refusal() {
            return Err(refusal);
        }
        let items = answer
            .body
            .get("items")
            .and_then(Value::as_array)
            .filter(|items| items.len() == operations.len())
            .ok_or(ClientError::Unreadable)?;
        Ok(operations
            .iter()
            .zip(items)
            .map(|(operation, item)| {
                let outcome = &item[operation.answered_under()];
                let status = outcome["status"].as_u64().unwrap_or(0);
                let removed_nothing =
                    matches!(operation, Operation::Remove { .. }) && status == 404;
                if (200..300).contains(&status) || removed_nothing {
                    None
                } else {
                    Some(Refused {
                        kind: kind_of(outcome),
                        holds: version_held(outcome),
                    })
                }
            })
            .collect())
    }

    /// Makes what was written until now visible to searches and removals by query.
    ///
    /// # Errors
    ///
    /// Returns why the index was not refreshed.
    pub(crate) async fn refresh(&self) -> Result<(), ClientError> {
        let answer = self
            .send(self.http.post(self.url("/_refresh")), INDEX_TIMEOUT)
            .await?;
        answer.refusal().map_or(Ok(()), Err)
    }

    /// Removes every document of `scope` that searches see.
    ///
    /// # Errors
    ///
    /// Returns why the documents were not removed, all of them or some.
    pub(crate) async fn remove_documents(&self, scope: &Scope<'_>) -> Result<(), ClientError> {
        let request = self
            .http
            .post(self.url("/_delete_by_query?refresh=true&conflicts=proceed"))
            .json(&scope.every_document());
        let answer = self.send(request, INDEX_TIMEOUT).await?;
        if let Some(refusal) = answer.refusal() {
            return Err(refusal);
        }
        // What it did not get to, or left for a conflict, is still there.
        let count = |member: &str| answer.body[member].as_u64().unwrap_or(0);
        let failed = answer.body["failures"]
            .as_array()
            .is_some_and(|failures| !failures.is_empty());
        let timed_out = answer.body["timed_out"].as_bool().unwrap_or(false);
        if failed || timed_out || count("version_conflicts") > 0 {
            return Err(ClientError::Partial);
        }
        Ok(())
    }

    /// The name of the index.
    pub(crate) fn index(&self) -> &str {
        &self.index
    }

    /// The highest version a document of the index was written with (`sent`), if it
    /// holds one.
    ///
    /// # Errors
    ///
    /// Returns why the search server did not say.
    pub(crate) async fn highest_version(&self) -> Result<Option<i64>, ClientError> {
        // An aggregation reads what was refreshed.
        self.refresh().await?;
        let request = self.http.post(self.url("/_search")).json(&json!({
            "size": 0,
            "track_total_hits": false,
            "aggs": {"sent": {"max": {"field": "sent"}}},
        }));
        let answer = self.send(request, INDEX_TIMEOUT).await?;
        if let Some(refusal) = answer.refusal() {
            return Err(refusal);
        }
        let highest = &answer.body["aggregations"]["sent"]["value"];
        if highest.is_null() {
            return Ok(None);
        }
        if let Some(highest) = highest.as_i64() {
            return Ok(Some(highest));
        }
        // Both answer the maximum as a JSON number, a float for a long: a version, a
        // whole number of microseconds, is well within a float's whole numbers, and is
        // read rounded up so that what follows it is past it.
        let highest = highest.as_f64().ok_or(ClientError::Unreadable)?.ceil();
        format!("{highest:.0}")
            .parse::<i64>()
            .map(Some)
            .map_err(|error| {
                tracing::warn!(%error, highest, "the highest version of the index could not be read");
                ClientError::Unreadable
            })
    }

    /// Finds up to `size` documents of `scope` by `match_by`, past the `from` that match
    /// best, the best match first, within `timeout`.
    ///
    /// # Errors
    ///
    /// Returns why nothing was found out: an index that is not there is a refusal, and
    /// an answer that holds a part of what matches is none.
    pub(crate) async fn search(
        &self,
        scope: &Scope<'_>,
        match_by: MatchBy<'_>,
        from: usize,
        size: usize,
        timeout: Duration,
    ) -> Result<Vec<Hit>, ClientError> {
        let request = self
            .http
            .post(self.url("/_search"))
            .json(&scope.search(match_by, from, size));
        let answer = self.send(request, timeout).await?;
        if let Some(refusal) = answer.refusal() {
            return Err(refusal);
        }
        let timed_out = answer.body["timed_out"].as_bool().unwrap_or(false);
        let failed_shards = answer
            .body
            .pointer("/_shards/failed")
            .and_then(Value::as_u64)
            .unwrap_or(0);
        if timed_out || failed_shards > 0 {
            return Err(ClientError::Partial);
        }
        let hits = answer
            .body
            .pointer("/hits/hits")
            .and_then(Value::as_array)
            .ok_or(ClientError::Unreadable)?;
        hits.iter()
            .map(|hit| {
                let id = hit.get("_id").and_then(Value::as_str);
                Ok(Hit {
                    id: id.ok_or(ClientError::Unreadable)?.to_owned(),
                    digest: hit
                        .pointer("/_source/digest")
                        .and_then(Value::as_str)
                        .unwrap_or_default()
                        .to_owned(),
                    snippet: hit
                        .pointer("/highlight/text/0")
                        .and_then(Value::as_str)
                        .map_or_default(snippet_parts),
                })
            })
            .collect()
    }
}

/// A piece of text as the search server marked it, in stretches: those between
/// [`MATCH_START`] and [`MATCH_END`] matched. Marks that do not pair change nothing
/// but which stretch a character falls in; no mark is ever text.
fn snippet_parts(marked: &str) -> Vec<SnippetPart> {
    let mut parts: Vec<SnippetPart> = Vec::new();
    let mut matched = false;
    for c in marked.chars() {
        match c {
            MATCH_START => matched = true,
            MATCH_END => matched = false,
            c => match parts.last_mut() {
                Some(part) if part.matched == matched => part.text.push(c),
                _ => parts.push(SnippetPart {
                    text: c.to_string(),
                    matched,
                }),
            },
        }
    }
    parts
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    fn plain(text: &str) -> SnippetPart {
        SnippetPart {
            text: text.to_owned(),
            matched: false,
        }
    }

    fn matched(text: &str) -> SnippetPart {
        SnippetPart {
            matched: true,
            ..plain(text)
        }
    }

    #[test]
    fn a_marked_piece_becomes_stretches_and_neighbouring_matches_one() {
        let marked =
            format!("지나면 {MATCH_START}환불{MATCH_END}{MATCH_START}규정{MATCH_END}을 따라");
        assert_eq!(
            snippet_parts(&marked),
            [plain("지나면 "), matched("환불규정"), plain("을 따라")]
        );
        assert_eq!(snippet_parts("no match"), [plain("no match")]);
        assert_eq!(snippet_parts(""), []);
        // Marks a body held itself, or that do not pair.
        assert_eq!(
            snippet_parts(&format!("a{MATCH_END}b{MATCH_START}c")),
            [plain("ab"), matched("c")]
        );
    }

    #[test]
    fn a_refusal_for_a_lower_version_says_the_version_held() {
        let refused = |kind: &str, reason: &str| json!({"status": 409, "error": {"type": kind, "reason": reason}});
        // As Elasticsearch and OpenSearch say it, for external_gte and for external.
        for reason in [
            "[c1]: version conflict, current version [1794747538116988] is higher than the one provided [17]",
            "[c1]: version conflict, current version [1794747538116988] is higher or equal to the one provided [17]",
        ] {
            assert_eq!(
                version_held(&refused("version_conflict_engine_exception", reason)),
                Some(1_794_747_538_116_988),
                "{reason}"
            );
        }
        assert_eq!(
            version_held(&refused("mapper_parsing_exception", "current version [3]")),
            None
        );
        assert_eq!(
            version_held(&refused("version_conflict_engine_exception", "no number")),
            None
        );
        assert_eq!(version_held(&json!({"status": 429})), None);
    }

    #[test]
    fn every_write_says_its_version_and_that_one_as_high_wins() {
        let version = crate::search::version::Versions::default().next();
        let removal = Operation::Remove {
            id: "c1".to_owned(),
            version,
        };
        let line: Value = serde_json::from_str(removal.lines().trim()).expect("a line");
        assert_eq!(
            line,
            json!({"delete": {"_id": "c1", "version": version.get(), "version_type": "external_gte"}})
        );
    }

    #[test]
    fn an_error_is_named_by_its_type_and_never_by_its_text() {
        let named = json!({"error": {"type": "index_not_found_exception", "reason": "no such index [secret-text]"}});
        assert_eq!(kind_of(&named), "index_not_found_exception");
        for unnamed in [
            json!(null),
            json!({"error": "a string"}),
            json!({"error": {"type": "has spaces and <text>"}}),
            json!({"error": {"type": "x".repeat(MOST_KIND + 1)}}),
        ] {
            assert_eq!(kind_of(&unnamed), "unknown", "{unnamed}");
        }
        let refused = ClientError::Refused {
            status: 404,
            kind: kind_of(&named),
        };
        assert!(!refused.to_string().contains("secret-text"), "{refused}");
    }
}
