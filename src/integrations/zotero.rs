//! Zotero v3 GET-only adapter. No arbitrary URL, remote content fetch or scheduler.
//! JSON/HTTP/URL/crypto remain with the locked dependencies; limits and identity
//! validation are FVOCI policy. Schema45 public SHA80014a0b is frozen below.
use crate::api::zotero_dto::{Bibliography, Creator, LibraryType, Tag};
use crate::integrations::outbound::{self, Outbound};
use futures_util::StreamExt;
use serde::de::{self, MapAccess, SeqAccess, Visitor};
use serde::{Deserialize, Deserializer};
use serde_json::Value;
use std::collections::{BTreeMap, BTreeSet};
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use std::time::Duration;
use url::Url;
use uuid::Uuid;

pub const BODY_MAX: usize = 256 * 1024;
pub const KEY_MAX: usize = 2000;
pub const PAGE_SIZE: usize = 25;
pub const REQUEST_MAX: usize = 256;
pub const REQUEST_TIMEOUT: Duration = Duration::from_secs(10);
pub const CYCLE_TIMEOUT: Duration = Duration::from_secs(60);

#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum ZoteroError {
    #[error("invalid Zotero metadata or library binding")]
    Invalid,
    #[error("Zotero library changed during collection")]
    VersionChanged,
    #[error("Zotero access denied")]
    Denied,
    #[error("Zotero temporarily unavailable")]
    Transient,
    #[error("Zotero retry delayed")]
    Delayed,
    #[error("Zotero read limit exceeded")]
    Limit,
    #[error("Zotero connection retired")]
    Retired,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Library {
    pub kind: LibraryType,
    pub remote_id: i64,
    pub website: String,
}
pub fn decimal(raw: &str) -> Result<i64, ZoteroError> {
    if raw.is_empty()
        || raw.len() > 19
        || (raw.len() > 1 && raw.starts_with('0'))
        || !raw.bytes().all(|v| v.is_ascii_digit())
    {
        return Err(ZoteroError::Invalid);
    }
    raw.parse::<i64>().map_err(|_| ZoteroError::Invalid)
}
pub fn key_valid(key: &str) -> bool {
    key.len() == 8
        && key
            .bytes()
            .all(|v| b"23456789ABCDEFGHIJKLMNPQRSTUVWXYZ".contains(&v))
}
pub fn secret_context(workspace: Uuid, owner: Uuid, connector: Uuid) -> String {
    format!("zotero:{workspace}:{owner}:{connector}")
}
impl Library {
    pub fn new(kind: LibraryType, remote_id: &str, website: &str) -> Result<Self, ZoteroError> {
        let remote_id = decimal(remote_id)?;
        if remote_id == 0 {
            return Err(ZoteroError::Invalid);
        }
        let url = safe_website(website)?;
        let parts: Vec<_> = url.path().trim_matches('/').split('/').collect();
        let valid = match kind {
            LibraryType::Group => {
                parts.len() >= 2
                    && parts.len() <= 3
                    && parts[0] == "groups"
                    && parts[1] == remote_id.to_string()
            }
            LibraryType::User => {
                (parts.len() == 2 && parts[0] == "users" && parts[1] == remote_id.to_string())
                    || (parts.len() == 1
                        && !parts[0].is_empty()
                        && !["groups", "users", "settings", "support"].contains(&parts[0]))
            }
        };
        if !valid {
            return Err(ZoteroError::Invalid);
        }
        Ok(Self {
            kind,
            remote_id,
            website: url.as_str().trim_end_matches('/').to_owned(),
        })
    }
    pub fn url(&self, resource: &str, params: &[(&str, String)]) -> Result<Url, ZoteroError> {
        if !["items", "collections", "deleted"].contains(&resource)
            && !resource.strip_prefix("collections/").is_some_and(key_valid)
        {
            return Err(ZoteroError::Invalid);
        }
        let mut url = Url::parse(&format!(
            "https://api.zotero.org/{}/{}/{}",
            self.kind.path(),
            self.remote_id,
            resource
        ))
        .map_err(|_| ZoteroError::Invalid)?;
        if !params.is_empty() {
            url.query_pairs_mut()
                .extend_pairs(params.iter().map(|(k, v)| (*k, v.as_str())));
        }
        Ok(url)
    }
    /// Validate a saved URL against the selected stored library descriptor.
    /// This performs no remote lookup or ownership verification. Live imports
    /// additionally verify the upstream library descriptor in `return_url`.
    pub fn validate_saved_return_url(&self, key: &str, href: &str) -> Result<String, ZoteroError> {
        if !key_valid(key)
            || Self::new(self.kind, &self.remote_id.to_string(), &self.website)? != *self
        {
            return Err(ZoteroError::Invalid);
        }
        let target = safe_website(href)?;
        let expected = format!("{}/items/{key}", self.website);
        if target.as_str().trim_end_matches('/') != expected {
            return Err(ZoteroError::Invalid);
        }
        Ok(target.into())
    }
    fn return_url(
        &self,
        library: &RemoteLibrary,
        key: &str,
        href: &str,
    ) -> Result<String, ZoteroError> {
        let target = self.validate_saved_return_url(key, href)?;
        let numeric = format!(
            "https://www.zotero.org/{}/{}",
            self.kind.path(),
            self.remote_id
        );
        if self.website != numeric {
            let remote_base = library
                .links
                .as_ref()
                .and_then(|links| links.alternate.as_ref())
                .ok_or(ZoteroError::Invalid)?;
            if safe_website(&remote_base.href)?
                .as_str()
                .trim_end_matches('/')
                != self.website
            {
                return Err(ZoteroError::Invalid);
            }
        }
        Ok(target)
    }
}
fn safe_website(raw: &str) -> Result<Url, ZoteroError> {
    let url = Url::parse(raw).map_err(|_| ZoteroError::Invalid)?;
    if url.scheme() != "https"
        || url.host_str() != Some("www.zotero.org")
        || url.port().is_some()
        || !url.username().is_empty()
        || url.password().is_some()
        || url.query().is_some()
        || url.fragment().is_some()
        || raw.len() > 1024
        || url.path().contains('%')
    {
        return Err(ZoteroError::Invalid);
    }
    Ok(url)
}

/// The test seam exposes only a fixed-origin GET. Production never reads a
/// test endpoint from environment, API input, a link header or metadata URL.
pub struct ReadReply {
    pub status: u16,
    pub headers: reqwest::header::HeaderMap,
    pub body: Vec<u8>,
}
pub type ReadFuture<'a> = Pin<Box<dyn Future<Output = Result<ReadReply, ZoteroError>> + Send + 'a>>;
pub trait ReadClient: Send + Sync {
    fn get<'a>(&'a self, url: &'a Url, key: &'a str) -> ReadFuture<'a>;
}
struct HttpRead {
    outbound: Outbound,
}
impl ReadClient for HttpRead {
    fn get<'a>(&'a self, url: &'a Url, key: &'a str) -> ReadFuture<'a> {
        Box::pin(async move {
            let pinned = self
                .outbound
                .without_allow_list()
                .pin(url)
                .await
                .map_err(|_| ZoteroError::Transient)?;
            let client = outbound::pinned_client(url, pinned, REQUEST_TIMEOUT)
                .map_err(|_| ZoteroError::Transient)?;
            let mut secret =
                reqwest::header::HeaderValue::from_str(key).map_err(|_| ZoteroError::Invalid)?;
            secret.set_sensitive(true);
            let reply = client
                .get(url.clone())
                .header("Zotero-API-Version", "3")
                .header("Zotero-API-Key", secret)
                .header("Accept", "application/json")
                .send()
                .await
                .map_err(|_| ZoteroError::Transient)?;
            let status = reply.status().as_u16();
            let mut headers = reqwest::header::HeaderMap::new();
            for name in [
                "last-modified-version",
                "total-results",
                "backoff",
                "retry-after",
                "link",
                "zotero-api-version",
            ] {
                for value in reply.headers().get_all(name) {
                    headers.append(name, value.clone());
                }
            }
            // Never consume or retain an error echo (including a key or HTML).
            if status != 200 {
                return Ok(ReadReply {
                    status,
                    headers,
                    body: Vec::new(),
                });
            }
            let mut body = Vec::new();
            let mut stream = reply.bytes_stream();
            while let Some(chunk) = stream.next().await {
                let chunk = chunk.map_err(|_| ZoteroError::Transient)?;
                if chunk.len() > BODY_MAX.saturating_sub(body.len()) {
                    return Err(ZoteroError::Limit);
                }
                body.extend_from_slice(&chunk);
            }
            Ok(ReadReply {
                status,
                headers,
                body,
            })
        })
    }
}
#[derive(Clone)]
pub struct ZoteroClient {
    reader: Arc<dyn ReadClient>,
}
impl ZoteroClient {
    pub fn system(outbound: Outbound) -> Self {
        Self {
            reader: Arc::new(HttpRead { outbound }),
        }
    }
    pub fn with_reader(reader: Arc<dyn ReadClient>) -> Self {
        Self { reader }
    }
    pub async fn read(
        &self,
        library: &Library,
        key: &str,
        resource: &str,
        params: &[(&str, String)],
    ) -> Result<ReadReply, ZoteroError> {
        let url = library.url(resource, params)?;
        tokio::time::timeout(REQUEST_TIMEOUT, self.reader.get(&url, key))
            .await
            .map_err(|_| ZoteroError::Transient)?
    }
}
pub fn header<'a>(
    reply: &'a ReadReply,
    name: &'static str,
) -> Result<Option<&'a str>, ZoteroError> {
    if reply.headers.get_all(name).iter().count() > 1 {
        return Err(ZoteroError::Invalid);
    }
    reply
        .headers
        .get(name)
        .map(|v| v.to_str().map_err(|_| ZoteroError::Invalid))
        .transpose()
}
pub fn delay(reply: &ReadReply) -> Result<i64, ZoteroError> {
    let mut seconds = 0;
    for name in ["backoff", "retry-after"] {
        if let Some(value) = header(reply, name)? {
            seconds = seconds.max(decimal(value)?);
        }
    }
    if seconds > 86400 {
        return Err(ZoteroError::Limit);
    }
    if [429, 503].contains(&reply.status) {
        seconds = seconds.max(30);
    }
    Ok(seconds)
}
pub fn version(reply: &ReadReply, expected: Option<i64>) -> Result<i64, ZoteroError> {
    match reply.status {
        200 => (),
        401 | 403 => return Err(ZoteroError::Denied),
        429 => return Err(ZoteroError::Delayed),
        500..=599 => return Err(ZoteroError::Transient),
        _ => return Err(ZoteroError::Invalid),
    }
    if reply.body.len() > BODY_MAX {
        return Err(ZoteroError::Limit);
    }
    if header(reply, "zotero-api-version")?.is_some_and(|v| v != "3") {
        return Err(ZoteroError::Invalid);
    }
    let version = decimal(header(reply, "last-modified-version")?.ok_or(ZoteroError::Invalid)?)?;
    if expected.is_some_and(|v| v != version) {
        return Err(ZoteroError::VersionChanged);
    }
    Ok(version)
}

// Serde parses JSON syntax and numeric values. This visitor only adds the
// duplicate-key rejection policy that serde_json::Value otherwise lacks.
struct UniqueJson(Value);
impl<'de> Deserialize<'de> for UniqueJson {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        struct UniqueVisitor;
        impl<'de> Visitor<'de> for UniqueVisitor {
            type Value = UniqueJson;
            fn expecting(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                f.write_str("JSON with unique object keys")
            }
            fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> Result<Self::Value, A::Error> {
                let mut value = serde_json::Map::new();
                while let Some((key, item)) = map.next_entry::<String, UniqueJson>()? {
                    if value.insert(key, item.0).is_some() {
                        return Err(de::Error::custom("duplicate object key"));
                    }
                }
                Ok(UniqueJson(Value::Object(value)))
            }
            fn visit_seq<A: SeqAccess<'de>>(self, mut seq: A) -> Result<Self::Value, A::Error> {
                let mut value = Vec::new();
                while let Some(item) = seq.next_element::<UniqueJson>()? {
                    value.push(item.0);
                }
                Ok(UniqueJson(Value::Array(value)))
            }
            fn visit_bool<E: de::Error>(self, v: bool) -> Result<Self::Value, E> {
                Ok(UniqueJson(Value::Bool(v)))
            }
            fn visit_i64<E: de::Error>(self, v: i64) -> Result<Self::Value, E> {
                Ok(UniqueJson(v.into()))
            }
            fn visit_u64<E: de::Error>(self, v: u64) -> Result<Self::Value, E> {
                Ok(UniqueJson(v.into()))
            }
            fn visit_f64<E: de::Error>(self, v: f64) -> Result<Self::Value, E> {
                serde_json::Number::from_f64(v)
                    .map(|n| UniqueJson(Value::Number(n)))
                    .ok_or_else(|| de::Error::custom("invalid number"))
            }
            fn visit_str<E: de::Error>(self, v: &str) -> Result<Self::Value, E> {
                Ok(UniqueJson(v.into()))
            }
            fn visit_string<E: de::Error>(self, v: String) -> Result<Self::Value, E> {
                Ok(UniqueJson(v.into()))
            }
            fn visit_unit<E: de::Error>(self) -> Result<Self::Value, E> {
                Ok(UniqueJson(Value::Null))
            }
            fn visit_none<E: de::Error>(self) -> Result<Self::Value, E> {
                Ok(UniqueJson(Value::Null))
            }
        }
        deserializer.deserialize_any(UniqueVisitor)
    }
}
pub fn parse<T: serde::de::DeserializeOwned>(body: &[u8]) -> Result<T, ZoteroError> {
    if body.len() > BODY_MAX {
        return Err(ZoteroError::Limit);
    }
    let value: UniqueJson = serde_json::from_slice(body).map_err(|_| ZoteroError::Invalid)?;
    serde_json::from_value(value.0).map_err(|_| ZoteroError::Invalid)
}
/// Imported upstream bytes only; authored document bodies use their own path.
pub fn body_contains_credential(body: &[u8], credential: &str) -> Result<bool, ZoteroError> {
    if credential.is_empty() {
        return Err(ZoteroError::Invalid);
    }
    if body.len() > BODY_MAX {
        return Err(ZoteroError::Limit);
    }
    if body
        .windows(credential.len())
        .any(|value| value == credential.as_bytes())
    {
        return Ok(true);
    }
    // JSON escapes must not turn an echoed key into persistable metadata.
    // Use the same bounded serde parser as the import, including duplicate-key
    // rejection. This guard never touches locally authored document content.
    let decoded: Value = parse(body)?;
    let mut pending = vec![&decoded];
    while let Some(value) = pending.pop() {
        match value {
            Value::String(value) if value.contains(credential) => return Ok(true),
            Value::Array(values) => pending.extend(values),
            Value::Object(values) => {
                if values.keys().any(|key| key.contains(credential)) {
                    return Ok(true);
                }
                pending.extend(values.values());
            }
            _ => {}
        }
    }
    Ok(false)
}
pub fn versions(body: &[u8], library_version: i64) -> Result<BTreeMap<String, i64>, ZoteroError> {
    let versions: BTreeMap<String, i64> = parse(body)?;
    if versions.len() > KEY_MAX {
        return Err(ZoteroError::Limit);
    }
    if versions
        .iter()
        .any(|(key, v)| !key_valid(key) || *v < 0 || *v > library_version)
    {
        return Err(ZoteroError::Invalid);
    }
    Ok(versions)
}

#[derive(Deserialize)]
struct RemoteLibrary {
    #[serde(rename = "type")]
    kind: LibraryType,
    id: i64,
    links: Option<RemoteLinks>,
}
#[derive(Deserialize)]
struct RemoteLinks {
    alternate: Option<RemoteLink>,
}
#[derive(Deserialize)]
struct RemoteLink {
    href: String,
}
#[derive(Deserialize)]
struct Envelope {
    key: String,
    version: i64,
    library: RemoteLibrary,
    links: Option<RemoteLinks>,
    data: BTreeMap<String, Value>,
}
impl Envelope {
    fn validate(
        &self,
        library: &Library,
        inventory: &BTreeMap<String, i64>,
    ) -> Result<(), ZoteroError> {
        if self.library.kind != library.kind
            || self.library.id != library.remote_id
            || inventory.get(&self.key) != Some(&self.version)
            || !key_valid(&self.key)
            || self.data.get("key").and_then(Value::as_str) != Some(self.key.as_str())
            || self.data.get("version").and_then(Value::as_i64) != Some(self.version)
        {
            return Err(ZoteroError::Invalid);
        }
        Ok(())
    }
}
#[derive(Debug, Clone)]
pub struct Item {
    pub key: String,
    pub version: i64,
    pub bibliography: Bibliography,
    pub return_url: String,
    pub collections: Vec<String>,
    pub trashed: bool,
}
#[derive(Debug, Clone)]
pub struct Collection {
    pub key: String,
    pub version: i64,
    pub name: String,
    pub parent: Option<String>,
}
fn bounded(value: &str, max: usize) -> Result<(), ZoteroError> {
    if value.len() > max || value.contains('\0') {
        Err(ZoteroError::Limit)
    } else {
        Ok(())
    }
}
/// Validate normalized, stored bibliography with the same schema45 policy as
/// live imports. The caller owns archive parsing, size limits, identity and
/// authorization; this validates no envelope, history or credential material.
pub fn validate_bibliography(value: &Bibliography) -> Result<(), ZoteroError> {
    let (allowed_fields, allowed_creators) =
        schema(&value.item_type).ok_or(ZoteroError::Invalid)?;
    bounded(&value.title, 4096)?;
    if value.creators.len() > 100 || value.tags.len() > 100 || value.relations.len() > 40 {
        return Err(ZoteroError::Limit);
    }
    for creator in &value.creators {
        if !allowed_creators.contains(&creator.creator_type.as_str())
            || creator.name.is_some()
                == (creator.first_name.is_some() || creator.last_name.is_some())
        {
            return Err(ZoteroError::Invalid);
        }
        for name in [&creator.name, &creator.first_name, &creator.last_name]
            .into_iter()
            .flatten()
        {
            bounded(name, 1024)?;
        }
    }
    for tag in &value.tags {
        bounded(&tag.tag, 1024)?;
        if ![0, 1].contains(&tag.tag_type) {
            return Err(ZoteroError::Invalid);
        }
    }
    for (key, values) in &value.relations {
        bounded(key, 128)?;
        if values.len() > 20 {
            return Err(ZoteroError::Limit);
        }
        for value in values {
            bounded(value, 2048)?;
        }
    }
    for (key, value) in &value.fields {
        // Live normalization extracts title into the dedicated top-level field.
        if key == "title"
            || (!allowed_fields.contains(&key.as_str())
                && !["dateAdded", "dateModified"].contains(&key.as_str()))
        {
            return Err(ZoteroError::Invalid);
        }
        bounded(value, 16384)?;
    }
    Ok(())
}
pub fn items(
    body: &[u8],
    library: &Library,
    requested: &BTreeMap<String, i64>,
) -> Result<Vec<Item>, ZoteroError> {
    let objects: Vec<Envelope> = parse(body)?;
    if objects.len() != requested.len() || objects.len() > PAGE_SIZE {
        return Err(ZoteroError::Invalid);
    }
    let mut seen = BTreeSet::new();
    let mut output = Vec::new();
    for mut object in objects {
        object.validate(library, requested)?;
        if !seen.insert(object.key.clone()) {
            return Err(ZoteroError::Invalid);
        }
        let item_type = object
            .data
            .remove("itemType")
            .and_then(|v| v.as_str().map(str::to_string))
            .ok_or(ZoteroError::Invalid)?;
        schema(&item_type).ok_or(ZoteroError::Invalid)?;
        let title = match object.data.remove("title") {
            None => String::new(),
            Some(Value::String(title)) => title,
            Some(_) => return Err(ZoteroError::Invalid),
        };
        let creators: Vec<Creator> = serde_json::from_value(
            object
                .data
                .remove("creators")
                .unwrap_or_else(|| serde_json::json!([])),
        )
        .map_err(|_| ZoteroError::Invalid)?;
        let tags: Vec<Tag> = serde_json::from_value(
            object
                .data
                .remove("tags")
                .unwrap_or_else(|| serde_json::json!([])),
        )
        .map_err(|_| ZoteroError::Invalid)?;
        let collections: Vec<String> = serde_json::from_value(
            object
                .data
                .remove("collections")
                .unwrap_or_else(|| serde_json::json!([])),
        )
        .map_err(|_| ZoteroError::Invalid)?;
        if collections.len() > 100
            || collections.iter().any(|v| !key_valid(v))
            || collections.iter().collect::<BTreeSet<_>>().len() != collections.len()
        {
            return Err(ZoteroError::Invalid);
        }
        let trashed = match object.data.remove("deleted") {
            None => false,
            Some(Value::Number(v)) if v.as_i64() == Some(1) => true,
            Some(Value::Bool(v)) => v,
            _ => return Err(ZoteroError::Invalid),
        };
        object.data.remove("key");
        object.data.remove("version");
        let mut relations = BTreeMap::new();
        if let Some(value) = object.data.remove("relations") {
            let map = value.as_object().ok_or(ZoteroError::Invalid)?;
            if map.len() > 40 {
                return Err(ZoteroError::Limit);
            }
            for (key, value) in map {
                let values: Vec<&str> = if let Some(single) = value.as_str() {
                    vec![single]
                } else {
                    value
                        .as_array()
                        .ok_or(ZoteroError::Invalid)?
                        .iter()
                        .map(|v| v.as_str().ok_or(ZoteroError::Invalid))
                        .collect::<Result<_, _>>()?
                };
                if values.len() > 20 {
                    return Err(ZoteroError::Limit);
                }
                relations.insert(key.clone(), values.into_iter().map(str::to_owned).collect());
            }
        }
        let mut fields = BTreeMap::new();
        for (key, value) in object.data {
            let value = value.as_str().ok_or(ZoteroError::Invalid)?;
            fields.insert(key, value.to_owned());
        }
        let bibliography = Bibliography {
            item_type,
            title,
            fields,
            creators,
            tags,
            relations,
        };
        validate_bibliography(&bibliography)?;
        let href = object
            .links
            .as_ref()
            .and_then(|v| v.alternate.as_ref())
            .ok_or(ZoteroError::Invalid)?;
        let return_url = library.return_url(&object.library, &object.key, &href.href)?;
        output.push(Item {
            key: object.key,
            version: object.version,
            bibliography,
            return_url,
            collections,
            trashed,
        });
    }
    Ok(output)
}
pub fn collection(
    body: &[u8],
    library: &Library,
    inventory: &BTreeMap<String, i64>,
    requested: &str,
) -> Result<Collection, ZoteroError> {
    let mut object: Envelope = parse(body)?;
    object.validate(library, inventory)?;
    if object.key != requested {
        return Err(ZoteroError::Invalid);
    }
    let name = object
        .data
        .remove("name")
        .and_then(|v| v.as_str().map(str::to_owned))
        .ok_or(ZoteroError::Invalid)?;
    bounded(&name, 4096)?;
    let parent = match object.data.remove("parentCollection") {
        Some(Value::Bool(false)) => None,
        Some(Value::String(v)) if key_valid(&v) => Some(v),
        _ => return Err(ZoteroError::Invalid),
    };
    object.data.remove("key");
    object.data.remove("version");
    for (key, value) in object.data {
        if key != "relations" || !value.is_object() {
            return Err(ZoteroError::Invalid);
        }
    }
    Ok(Collection {
        key: object.key,
        version: object.version,
        name,
        parent,
    })
}
pub fn collection_graph(collections: &[Collection]) -> Result<(), ZoteroError> {
    let graph: BTreeMap<_, _> = collections
        .iter()
        .map(|c| (c.key.as_str(), c.parent.as_deref()))
        .collect();
    if graph.len() != collections.len() {
        return Err(ZoteroError::Invalid);
    }
    for key in graph.keys() {
        let mut path = BTreeSet::new();
        let mut current = Some(*key);
        while let Some(key) = current {
            if !path.insert(key) {
                return Err(ZoteroError::Invalid);
            }
            current = *graph.get(key).ok_or(ZoteroError::Invalid)?;
        }
    }
    Ok(())
}
#[derive(Debug, Deserialize)]
pub struct Deleted {
    pub items: Vec<String>,
    pub collections: Vec<String>,
    pub searches: Vec<String>,
    pub tags: Vec<String>,
}
impl Deleted {
    pub fn validate(&self) -> Result<(), ZoteroError> {
        if self.items.len() + self.collections.len() + self.searches.len() + self.tags.len()
            > KEY_MAX
        {
            return Err(ZoteroError::Limit);
        }
        for keys in [&self.items, &self.collections, &self.searches] {
            if keys.iter().any(|v| !key_valid(v))
                || keys.iter().collect::<BTreeSet<_>>().len() != keys.len()
            {
                return Err(ZoteroError::Invalid);
            }
        }
        Ok(())
    }
}

// Frozen Zotero schema45; only type-specific bibliographic fields and creators.
pub(crate) fn schema(
    item_type: &str,
) -> Option<(&'static [&'static str], &'static [&'static str])> {
    match item_type {
        "artwork" => Some((
            &[
                "title",
                "abstractNote",
                "artworkMedium",
                "artworkSize",
                "date",
                "eventPlace",
                "DOI",
                "citationKey",
                "url",
                "accessDate",
                "archive",
                "archiveLocation",
                "shortTitle",
                "language",
                "libraryCatalog",
                "callNumber",
                "rights",
                "extra",
            ],
            &["artist", "contributor"],
        )),
        "audioRecording" => Some((
            &[
                "title",
                "abstractNote",
                "audioRecordingFormat",
                "seriesTitle",
                "volume",
                "numberOfVolumes",
                "label",
                "place",
                "date",
                "runningTime",
                "ISBN",
                "DOI",
                "citationKey",
                "url",
                "accessDate",
                "archive",
                "archiveLocation",
                "shortTitle",
                "language",
                "libraryCatalog",
                "callNumber",
                "rights",
                "extra",
            ],
            &[
                "performer",
                "originalCreator",
                "composer",
                "wordsBy",
                "translator",
                "contributor",
            ],
        )),
        "bill" => Some((
            &[
                "title",
                "abstractNote",
                "billNumber",
                "code",
                "codeVolume",
                "section",
                "codePages",
                "legislativeBody",
                "session",
                "history",
                "date",
                "DOI",
                "citationKey",
                "url",
                "accessDate",
                "shortTitle",
                "language",
                "rights",
                "extra",
            ],
            &["sponsor", "cosponsor", "contributor"],
        )),
        "blogPost" => Some((
            &[
                "title",
                "abstractNote",
                "blogTitle",
                "websiteType",
                "date",
                "DOI",
                "citationKey",
                "url",
                "accessDate",
                "ISSN",
                "shortTitle",
                "language",
                "rights",
                "extra",
            ],
            &["author", "translator", "commenter", "contributor"],
        )),
        "book" => Some((
            &[
                "title",
                "abstractNote",
                "series",
                "seriesNumber",
                "volume",
                "numberOfVolumes",
                "edition",
                "date",
                "publisher",
                "place",
                "originalDate",
                "originalPublisher",
                "originalPlace",
                "format",
                "numPages",
                "ISBN",
                "DOI",
                "citationKey",
                "url",
                "accessDate",
                "ISSN",
                "archive",
                "archiveLocation",
                "shortTitle",
                "language",
                "libraryCatalog",
                "callNumber",
                "rights",
                "extra",
            ],
            &[
                "author",
                "contributor",
                "editor",
                "translator",
                "seriesEditor",
            ],
        )),
        "bookSection" => Some((
            &[
                "title",
                "abstractNote",
                "bookTitle",
                "series",
                "seriesNumber",
                "volume",
                "numberOfVolumes",
                "edition",
                "date",
                "publisher",
                "place",
                "originalDate",
                "originalPublisher",
                "originalPlace",
                "format",
                "pages",
                "ISBN",
                "DOI",
                "citationKey",
                "url",
                "accessDate",
                "ISSN",
                "archive",
                "archiveLocation",
                "shortTitle",
                "language",
                "libraryCatalog",
                "callNumber",
                "rights",
                "extra",
            ],
            &[
                "author",
                "contributor",
                "editor",
                "bookAuthor",
                "translator",
                "seriesEditor",
            ],
        )),
        "case" => Some((
            &[
                "caseName",
                "abstractNote",
                "court",
                "dateDecided",
                "docketNumber",
                "reporter",
                "reporterVolume",
                "firstPage",
                "history",
                "DOI",
                "citationKey",
                "url",
                "accessDate",
                "shortTitle",
                "language",
                "rights",
                "extra",
            ],
            &["author", "counsel", "contributor"],
        )),
        "computerProgram" => Some((
            &[
                "title",
                "abstractNote",
                "seriesTitle",
                "versionNumber",
                "date",
                "system",
                "company",
                "place",
                "programmingLanguage",
                "rights",
                "citationKey",
                "url",
                "accessDate",
                "DOI",
                "ISBN",
                "archive",
                "archiveLocation",
                "libraryCatalog",
                "callNumber",
                "shortTitle",
                "extra",
            ],
            &["programmer", "contributor"],
        )),
        "conferencePaper" => Some((
            &[
                "title",
                "abstractNote",
                "proceedingsTitle",
                "conferenceName",
                "publisher",
                "place",
                "date",
                "eventPlace",
                "volume",
                "issue",
                "numberOfVolumes",
                "pages",
                "series",
                "seriesNumber",
                "DOI",
                "ISBN",
                "citationKey",
                "url",
                "accessDate",
                "ISSN",
                "archive",
                "archiveLocation",
                "shortTitle",
                "language",
                "libraryCatalog",
                "callNumber",
                "rights",
                "extra",
            ],
            &[
                "author",
                "contributor",
                "editor",
                "translator",
                "seriesEditor",
            ],
        )),
        "dataset" => Some((
            &[
                "title",
                "abstractNote",
                "identifier",
                "type",
                "versionNumber",
                "date",
                "repository",
                "repositoryLocation",
                "format",
                "DOI",
                "citationKey",
                "url",
                "accessDate",
                "archive",
                "archiveLocation",
                "shortTitle",
                "language",
                "libraryCatalog",
                "callNumber",
                "rights",
                "extra",
            ],
            &["author", "contributor"],
        )),
        "dictionaryEntry" => Some((
            &[
                "title",
                "abstractNote",
                "dictionaryTitle",
                "series",
                "seriesNumber",
                "volume",
                "numberOfVolumes",
                "edition",
                "date",
                "publisher",
                "place",
                "pages",
                "ISBN",
                "DOI",
                "citationKey",
                "url",
                "accessDate",
                "archive",
                "archiveLocation",
                "shortTitle",
                "language",
                "libraryCatalog",
                "callNumber",
                "rights",
                "extra",
            ],
            &[
                "author",
                "contributor",
                "editor",
                "translator",
                "seriesEditor",
            ],
        )),
        "document" => Some((
            &[
                "title",
                "abstractNote",
                "type",
                "date",
                "publisher",
                "place",
                "DOI",
                "citationKey",
                "url",
                "accessDate",
                "archive",
                "archiveLocation",
                "shortTitle",
                "language",
                "libraryCatalog",
                "callNumber",
                "rights",
                "extra",
            ],
            &[
                "author",
                "contributor",
                "editor",
                "translator",
                "reviewedAuthor",
            ],
        )),
        "email" => Some((
            &[
                "subject",
                "abstractNote",
                "date",
                "DOI",
                "citationKey",
                "url",
                "accessDate",
                "shortTitle",
                "language",
                "rights",
                "extra",
            ],
            &["author", "translator", "contributor", "recipient"],
        )),
        "encyclopediaArticle" => Some((
            &[
                "title",
                "abstractNote",
                "encyclopediaTitle",
                "series",
                "seriesNumber",
                "volume",
                "numberOfVolumes",
                "edition",
                "date",
                "publisher",
                "place",
                "pages",
                "ISBN",
                "DOI",
                "citationKey",
                "url",
                "accessDate",
                "archive",
                "archiveLocation",
                "shortTitle",
                "language",
                "libraryCatalog",
                "callNumber",
                "rights",
                "extra",
            ],
            &[
                "author",
                "contributor",
                "editor",
                "translator",
                "seriesEditor",
            ],
        )),
        "film" => Some((
            &[
                "title",
                "abstractNote",
                "distributor",
                "place",
                "date",
                "genre",
                "videoRecordingFormat",
                "runningTime",
                "DOI",
                "citationKey",
                "url",
                "accessDate",
                "archive",
                "archiveLocation",
                "shortTitle",
                "language",
                "libraryCatalog",
                "callNumber",
                "rights",
                "extra",
            ],
            &[
                "director",
                "producer",
                "scriptwriter",
                "castMember",
                "host",
                "guest",
                "narrator",
                "translator",
                "contributor",
            ],
        )),
        "forumPost" => Some((
            &[
                "title",
                "abstractNote",
                "forumTitle",
                "postType",
                "date",
                "DOI",
                "citationKey",
                "url",
                "accessDate",
                "shortTitle",
                "language",
                "rights",
                "extra",
            ],
            &["author", "contributor"],
        )),
        "hearing" => Some((
            &[
                "title",
                "abstractNote",
                "committee",
                "publisher",
                "numberOfVolumes",
                "documentNumber",
                "pages",
                "legislativeBody",
                "session",
                "history",
                "date",
                "place",
                "DOI",
                "citationKey",
                "url",
                "accessDate",
                "shortTitle",
                "language",
                "rights",
                "extra",
            ],
            &["contributor"],
        )),
        "instantMessage" => Some((
            &[
                "title",
                "abstractNote",
                "date",
                "DOI",
                "citationKey",
                "url",
                "accessDate",
                "shortTitle",
                "language",
                "rights",
                "extra",
            ],
            &["author", "contributor", "recipient"],
        )),
        "interview" => Some((
            &[
                "title",
                "abstractNote",
                "interviewMedium",
                "date",
                "publisher",
                "place",
                "DOI",
                "citationKey",
                "url",
                "accessDate",
                "archive",
                "archiveLocation",
                "shortTitle",
                "language",
                "libraryCatalog",
                "callNumber",
                "rights",
                "extra",
            ],
            &["interviewee", "contributor", "interviewer", "translator"],
        )),
        "journalArticle" => Some((
            &[
                "title",
                "abstractNote",
                "publicationTitle",
                "publisher",
                "place",
                "date",
                "volume",
                "issue",
                "section",
                "partNumber",
                "partTitle",
                "pages",
                "series",
                "seriesTitle",
                "seriesText",
                "journalAbbreviation",
                "DOI",
                "citationKey",
                "url",
                "accessDate",
                "PMID",
                "PMCID",
                "ISSN",
                "archive",
                "archiveLocation",
                "shortTitle",
                "language",
                "libraryCatalog",
                "callNumber",
                "rights",
                "extra",
            ],
            &[
                "author",
                "contributor",
                "editor",
                "translator",
                "reviewedAuthor",
            ],
        )),
        "letter" => Some((
            &[
                "title",
                "abstractNote",
                "letterType",
                "date",
                "eventPlace",
                "DOI",
                "citationKey",
                "url",
                "accessDate",
                "archive",
                "archiveLocation",
                "shortTitle",
                "language",
                "libraryCatalog",
                "callNumber",
                "rights",
                "extra",
            ],
            &["author", "recipient", "contributor", "translator"],
        )),
        "magazineArticle" => Some((
            &[
                "title",
                "abstractNote",
                "publicationTitle",
                "publisher",
                "place",
                "date",
                "volume",
                "issue",
                "pages",
                "ISSN",
                "DOI",
                "citationKey",
                "url",
                "accessDate",
                "archive",
                "archiveLocation",
                "shortTitle",
                "language",
                "libraryCatalog",
                "callNumber",
                "rights",
                "extra",
            ],
            &["author", "contributor", "translator", "reviewedAuthor"],
        )),
        "manuscript" => Some((
            &[
                "title",
                "abstractNote",
                "manuscriptType",
                "institution",
                "place",
                "date",
                "numPages",
                "number",
                "DOI",
                "citationKey",
                "url",
                "accessDate",
                "archive",
                "archiveLocation",
                "shortTitle",
                "language",
                "libraryCatalog",
                "callNumber",
                "rights",
                "extra",
            ],
            &["author", "contributor", "translator"],
        )),
        "map" => Some((
            &[
                "title",
                "abstractNote",
                "mapType",
                "scale",
                "seriesTitle",
                "edition",
                "publisher",
                "place",
                "date",
                "DOI",
                "ISBN",
                "citationKey",
                "url",
                "accessDate",
                "archive",
                "archiveLocation",
                "shortTitle",
                "language",
                "libraryCatalog",
                "callNumber",
                "rights",
                "extra",
            ],
            &["cartographer", "contributor", "seriesEditor"],
        )),
        "newspaperArticle" => Some((
            &[
                "title",
                "abstractNote",
                "publicationTitle",
                "publisher",
                "place",
                "date",
                "volume",
                "issue",
                "edition",
                "section",
                "pages",
                "ISSN",
                "DOI",
                "citationKey",
                "url",
                "accessDate",
                "archive",
                "archiveLocation",
                "shortTitle",
                "language",
                "libraryCatalog",
                "callNumber",
                "rights",
                "extra",
            ],
            &["author", "contributor", "translator", "reviewedAuthor"],
        )),
        "patent" => Some((
            &[
                "title",
                "abstractNote",
                "place",
                "country",
                "assignee",
                "issuingAuthority",
                "patentNumber",
                "filingDate",
                "pages",
                "applicationNumber",
                "priorityNumbers",
                "issueDate",
                "priorityDate",
                "references",
                "legalStatus",
                "DOI",
                "citationKey",
                "url",
                "accessDate",
                "shortTitle",
                "language",
                "rights",
                "extra",
            ],
            &["inventor", "attorneyAgent", "contributor"],
        )),
        "podcast" => Some((
            &[
                "title",
                "abstractNote",
                "seriesTitle",
                "episodeNumber",
                "audioFileType",
                "date",
                "publisher",
                "place",
                "runningTime",
                "DOI",
                "citationKey",
                "url",
                "accessDate",
                "shortTitle",
                "language",
                "rights",
                "extra",
            ],
            &[
                "podcaster",
                "guest",
                "producer",
                "executiveProducer",
                "seriesCreator",
                "director",
                "scriptwriter",
                "castMember",
                "translator",
                "contributor",
            ],
        )),
        "preprint" => Some((
            &[
                "title",
                "abstractNote",
                "genre",
                "repository",
                "archiveID",
                "place",
                "date",
                "series",
                "seriesNumber",
                "DOI",
                "citationKey",
                "url",
                "accessDate",
                "archive",
                "archiveLocation",
                "shortTitle",
                "language",
                "libraryCatalog",
                "callNumber",
                "rights",
                "extra",
            ],
            &[
                "author",
                "contributor",
                "editor",
                "translator",
                "reviewedAuthor",
            ],
        )),
        "presentation" => Some((
            &[
                "title",
                "abstractNote",
                "presentationType",
                "date",
                "meetingName",
                "place",
                "series",
                "sessionTitle",
                "DOI",
                "citationKey",
                "url",
                "accessDate",
                "shortTitle",
                "language",
                "rights",
                "extra",
            ],
            &[
                "presenter",
                "chair",
                "organizer",
                "contributor",
                "translator",
            ],
        )),
        "radioBroadcast" => Some((
            &[
                "title",
                "abstractNote",
                "programTitle",
                "episodeNumber",
                "audioRecordingFormat",
                "network",
                "place",
                "date",
                "runningTime",
                "DOI",
                "citationKey",
                "url",
                "accessDate",
                "archive",
                "archiveLocation",
                "shortTitle",
                "language",
                "libraryCatalog",
                "callNumber",
                "rights",
                "extra",
            ],
            &[
                "creator",
                "host",
                "guest",
                "producer",
                "executiveProducer",
                "seriesCreator",
                "director",
                "scriptwriter",
                "castMember",
                "translator",
                "contributor",
            ],
        )),
        "report" => Some((
            &[
                "title",
                "abstractNote",
                "reportNumber",
                "reportType",
                "institution",
                "place",
                "date",
                "seriesTitle",
                "seriesNumber",
                "pages",
                "DOI",
                "ISBN",
                "citationKey",
                "url",
                "accessDate",
                "ISSN",
                "archive",
                "archiveLocation",
                "shortTitle",
                "language",
                "libraryCatalog",
                "callNumber",
                "rights",
                "extra",
            ],
            &[
                "author",
                "editor",
                "contributor",
                "translator",
                "seriesEditor",
            ],
        )),
        "standard" => Some((
            &[
                "title",
                "abstractNote",
                "organization",
                "committee",
                "type",
                "number",
                "versionNumber",
                "edition",
                "status",
                "date",
                "publisher",
                "place",
                "partNumber",
                "partTitle",
                "ISBN",
                "DOI",
                "citationKey",
                "url",
                "accessDate",
                "archive",
                "archiveLocation",
                "shortTitle",
                "numPages",
                "language",
                "libraryCatalog",
                "callNumber",
                "rights",
                "extra",
            ],
            &["author", "editor", "contributor"],
        )),
        "statute" => Some((
            &[
                "nameOfAct",
                "abstractNote",
                "code",
                "codeNumber",
                "publicLawNumber",
                "dateEnacted",
                "pages",
                "section",
                "session",
                "history",
                "DOI",
                "citationKey",
                "url",
                "accessDate",
                "shortTitle",
                "language",
                "rights",
                "extra",
            ],
            &["author", "contributor"],
        )),
        "thesis" => Some((
            &[
                "title",
                "abstractNote",
                "thesisType",
                "university",
                "place",
                "date",
                "series",
                "seriesNumber",
                "numPages",
                "DOI",
                "ISBN",
                "citationKey",
                "url",
                "accessDate",
                "ISSN",
                "archive",
                "archiveLocation",
                "shortTitle",
                "language",
                "libraryCatalog",
                "callNumber",
                "rights",
                "extra",
            ],
            &["author", "contributor"],
        )),
        "tvBroadcast" => Some((
            &[
                "title",
                "abstractNote",
                "programTitle",
                "episodeNumber",
                "videoRecordingFormat",
                "network",
                "place",
                "date",
                "runningTime",
                "DOI",
                "citationKey",
                "url",
                "accessDate",
                "archive",
                "archiveLocation",
                "shortTitle",
                "language",
                "libraryCatalog",
                "callNumber",
                "rights",
                "extra",
            ],
            &[
                "director",
                "producer",
                "executiveProducer",
                "seriesCreator",
                "scriptwriter",
                "castMember",
                "host",
                "guest",
                "narrator",
                "translator",
                "contributor",
            ],
        )),
        "videoRecording" => Some((
            &[
                "title",
                "abstractNote",
                "videoRecordingFormat",
                "seriesTitle",
                "volume",
                "numberOfVolumes",
                "studio",
                "place",
                "date",
                "runningTime",
                "ISBN",
                "DOI",
                "citationKey",
                "url",
                "accessDate",
                "archive",
                "archiveLocation",
                "shortTitle",
                "language",
                "libraryCatalog",
                "callNumber",
                "rights",
                "extra",
            ],
            &[
                "creator",
                "director",
                "producer",
                "scriptwriter",
                "executiveProducer",
                "castMember",
                "host",
                "guest",
                "narrator",
                "translator",
                "contributor",
            ],
        )),
        "webpage" => Some((
            &[
                "title",
                "abstractNote",
                "websiteTitle",
                "websiteType",
                "date",
                "publisher",
                "place",
                "DOI",
                "citationKey",
                "url",
                "accessDate",
                "shortTitle",
                "language",
                "rights",
                "extra",
            ],
            &["author", "contributor", "translator"],
        )),
        _ => None,
    }
}

pub fn bibliographic_types() -> &'static [&'static str] {
    &[
        "artwork",
        "audioRecording",
        "bill",
        "blogPost",
        "book",
        "bookSection",
        "case",
        "computerProgram",
        "conferencePaper",
        "dataset",
        "dictionaryEntry",
        "document",
        "email",
        "encyclopediaArticle",
        "film",
        "forumPost",
        "hearing",
        "instantMessage",
        "interview",
        "journalArticle",
        "letter",
        "magazineArticle",
        "manuscript",
        "map",
        "newspaperArticle",
        "patent",
        "podcast",
        "preprint",
        "presentation",
        "radioBroadcast",
        "report",
        "standard",
        "statute",
        "thesis",
        "tvBroadcast",
        "videoRecording",
        "webpage",
    ]
}

/// Synthetic upstream used by the registered integration/browser fixtures.
/// Absent from default product builds; no environment/API target switch.
#[cfg(feature = "db-tests")]
pub mod fixtures {
    use super::*;
    use axum::{
        extract::{OriginalUri, State},
        http::StatusCode,
        response::IntoResponse,
        routing::get,
        Router,
    };
    use std::sync::{
        atomic::{AtomicU8, Ordering},
        Mutex,
    };
    use tokio::sync::Notify;
    pub const KEY: &str = "SYNTHETIC_ONLY_NEVER_A_ZOTERO_KEY";
    #[derive(Default)]
    pub struct Model {
        /// 0 normal, 1 second-page503, 2 delete13, 3 denied403, 4 backoff200,
        /// 5 version drift, 6 held body, 7 foreign Link, 8 redirect, 9 missing key,
        /// 10 rate429, 11 transient503, 12 error echo, 13 excluded type,
        /// 14 bibliography update, 15 above browser precision, 16..19 bad title,
        /// 20 Unicode-escaped credential in imported creator metadata,
        /// 21 absent-title positive control, 22 one-item historical C99 source.
        pub mode: AtomicU8,
        pub many: AtomicU8,
        pub log: Mutex<Vec<(String, String)>>,
        pub entered: Notify,
        pub release: Notify,
    }
    pub struct Upstream {
        pub addr: std::net::SocketAddr,
        pub model: Arc<Model>,
        stop: Option<tokio::sync::oneshot::Sender<()>>,
        join: tokio::task::JoinHandle<()>,
    }
    impl Upstream {
        pub async fn start() -> Self {
            let model = Arc::new(Model::default());
            let app = Router::new()
                .route("/{*path}", get(reply))
                .with_state(model.clone());
            let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
            let addr = listener.local_addr().unwrap();
            let (stop, rx) = tokio::sync::oneshot::channel();
            let join = tokio::spawn(async move {
                axum::serve(listener, app)
                    .with_graceful_shutdown(async {
                        let _ = rx.await;
                    })
                    .await
                    .unwrap();
            });
            Self {
                addr,
                model,
                stop: Some(stop),
                join,
            }
        }
        pub fn reader(&self) -> ZoteroClient {
            ZoteroClient::with_reader(Arc::new(LocalRead(self.addr)))
        }
        pub async fn shutdown(mut self) {
            if let Some(stop) = self.stop.take() {
                let _ = stop.send(());
            }
            self.join.await.unwrap();
        }
    }
    struct LocalRead(std::net::SocketAddr);
    impl ReadClient for LocalRead {
        fn get<'a>(&'a self, url: &'a Url, key: &'a str) -> ReadFuture<'a> {
            Box::pin(async move {
                if url.scheme() != "https" || url.host_str() != Some("api.zotero.org") {
                    return Err(ZoteroError::Invalid);
                }
                let endpoint = format!(
                    "http://{}{}{}",
                    self.0,
                    url.path(),
                    url.query().map(|q| format!("?{q}")).unwrap_or_default()
                );
                let mut header = reqwest::header::HeaderValue::from_str(key)
                    .map_err(|_| ZoteroError::Invalid)?;
                header.set_sensitive(true);
                let response = reqwest::Client::builder()
                    .no_proxy()
                    .redirect(reqwest::redirect::Policy::none())
                    .build()
                    .unwrap()
                    .get(endpoint)
                    .header("Zotero-API-Key", header)
                    .send()
                    .await
                    .map_err(|_| ZoteroError::Transient)?;
                let status = response.status().as_u16();
                let headers = response.headers().clone();
                let body = response
                    .bytes()
                    .await
                    .map_err(|_| ZoteroError::Transient)?
                    .to_vec();
                Ok(ReadReply {
                    status,
                    headers,
                    body,
                })
            })
        }
    }
    pub fn item_keys(many: bool) -> Vec<String> {
        if !many {
            return vec!["ABCD2345".into(), "EFGH4567".into()];
        }
        // Independent legal literal fixture keys; first page=25, last page=2.
        let mut keys = vec!["ABCD2345".into()];
        keys.extend(
            "23456789ABCDEFGHJKLMNPQRST"
                .chars()
                .map(|c| format!("JKLM234{c}")),
        );
        keys
    }
    fn item(key: &str, kind: &str, mode: u8) -> Value {
        let item_version = if mode == 14 && key == "ABCD2345" {
            14
        } else {
            7
        };
        let title = if mode == 14 && key == "ABCD2345" {
            "Revised synthetic bibliography"
        } else if key == "ABCD2345" {
            "합성 연구 자료 🙂"
        } else {
            "Second synthetic reference"
        };
        serde_json::json!({"key":key,"version":item_version,"library":{"type":kind,"id":42},"links":{"alternate":{"href":format!("https://www.zotero.org/{}/42/items/{key}",if kind=="user"{"users"}else{"groups"})}},"data":{"key":key,"version":item_version,"itemType":"book","title":title,"creators":[{"creatorType":"author","firstName":"민","lastName":"김"},{"creatorType":"editor","name":"Synthetic Research Group"}],"date":"2026","publisher":"Synthetic Press","ISBN":"9780000000000","collections":["BCDE3456","CDEF4567"],"tags":[{"tag":"연구","type":0}],"relations":{}}})
    }
    async fn reply(
        State(model): State<Arc<Model>>,
        OriginalUri(uri): OriginalUri,
    ) -> axum::response::Response {
        let url = Url::parse(&format!("http://fixture{uri}")).unwrap();
        model
            .log
            .lock()
            .unwrap()
            .push(("GET".into(), uri.to_string()));
        let query: BTreeMap<_, _> = url
            .query_pairs()
            .map(|(k, v)| (k.into_owned(), v.into_owned()))
            .collect();
        let mode = model.mode.load(Ordering::SeqCst);
        let path = url.path();
        let kind = if path.starts_with("/groups/42/") {
            "group"
        } else {
            "user"
        };
        let version: i64 = match mode {
            2 | 13 => 13,
            14 => 14,
            15 => 9007199254740993,
            22 => 99,
            _ => 12,
        };
        let mut headers = reqwest::header::HeaderMap::new();
        headers.insert(
            "last-modified-version",
            version.to_string().parse().unwrap(),
        );
        headers.insert("zotero-api-version", "3".parse().unwrap());
        if mode == 3 {
            return (StatusCode::FORBIDDEN, headers, "denied").into_response();
        }
        if [10, 11, 12].contains(&mode) {
            headers.insert("retry-after", "30".parse().unwrap());
            return (
                if mode == 10 {
                    StatusCode::TOO_MANY_REQUESTS
                } else {
                    StatusCode::SERVICE_UNAVAILABLE
                },
                headers,
                if mode == 12 { KEY } else { "temporary" },
            )
                .into_response();
        }
        if mode == 4 {
            headers.insert("backoff", "60".parse().unwrap());
        }
        if mode == 7 {
            headers.insert(
                "link",
                "<https://example.invalid/steal?key=never>; rel=\"next\""
                    .parse()
                    .unwrap(),
            );
        }
        if mode == 8 {
            headers.insert("location", "https://example.invalid/steal".parse().unwrap());
            return (StatusCode::FOUND, headers, "redirect").into_response();
        }
        let collection_inventory = if mode == 2 {
            serde_json::json!({"BCDE3456":6})
        } else {
            serde_json::json!({"BCDE3456":6,"CDEF4567":7})
        };
        let value = if path.ends_with("/collections") {
            collection_inventory
        } else if path.contains("/collections/") {
            let key = path.rsplit('/').next().unwrap();
            let object_version = if key == "BCDE3456" { 6 } else { 7 };
            headers.insert(
                "last-modified-version",
                object_version.to_string().parse().unwrap(),
            );
            serde_json::json!({"key":key,"version":object_version,"library":{"type":kind,"id":42},"data":{"key":key,"version":object_version,"name":if key=="BCDE3456"{"Study"}else{"Evidence"},"parentCollection":if key=="BCDE3456"{Value::Bool(false)}else{Value::String("BCDE3456".into())}}})
        } else if path.ends_with("/deleted") {
            serde_json::json!({"items":if mode==2{vec!["ABCD2345"]}else{vec![]},"collections":if mode==2{vec!["CDEF4567"]}else{vec![]},"searches":[],"tags":[]})
        } else if query.get("format").is_some_and(|v| v == "versions") {
            if mode == 5 {
                headers.insert("last-modified-version", "13".parse().unwrap());
            }
            let mut versions = BTreeMap::new();
            let excluded = query
                .get("itemType")
                .is_some_and(|v| v == "note || attachment || annotation");
            let bibliography = query.contains_key("itemType") && !excluded;
            let since = query
                .get("since")
                .and_then(|s| s.parse::<i64>().ok())
                .unwrap_or(0);
            {
                let keys = if mode == 22 {
                    vec!["ABCD2345".to_owned()]
                } else {
                    item_keys(model.many.load(Ordering::SeqCst) > 0)
                };
                for key in keys {
                    let item_version = if mode == 13 && key == "ABCD2345" {
                        13
                    } else if mode == 14 && key == "ABCD2345" {
                        14
                    } else {
                        7
                    };
                    if since >= item_version {
                        continue;
                    }
                    if mode == 2 && key == "ABCD2345" {
                        continue;
                    }
                    if mode == 13 && key == "ABCD2345" && bibliography {
                        continue;
                    }
                    if excluded && !(mode == 13 && key == "ABCD2345") {
                        continue;
                    }
                    versions.insert(key, item_version);
                }
            }
            serde_json::to_value(versions).unwrap()
        } else if let Some(keys) = query.get("itemKey") {
            if mode == 22 && keys.split(',').any(|key| key != "ABCD2345") {
                return (
                    StatusCode::BAD_REQUEST,
                    headers,
                    "unrequested historical fixture key",
                )
                    .into_response();
            }
            if mode == 6 {
                model.entered.notify_one();
                model.release.notified().await;
            }
            if mode == 1 && !keys.contains("ABCD2345") {
                return (StatusCode::SERVICE_UNAVAILABLE, headers, "page two failed")
                    .into_response();
            }
            let mut values: Vec<Value> = keys.split(',').map(|key| item(key, kind, mode)).collect();
            for item in &mut values {
                if item["key"] != "ABCD2345" {
                    continue;
                }
                match mode {
                    16 => item["data"]["title"] = Value::Null,
                    17 => item["data"]["title"] = serde_json::json!(42),
                    18 => item["data"]["title"] = serde_json::json!({"wrong":"type"}),
                    19 => item["data"]["title"] = serde_json::json!(["wrong", "type"]),
                    21 => {
                        item["data"].as_object_mut().unwrap().remove("title");
                    }
                    20 => item["data"]["creators"][1]["name"] = Value::String(KEY.to_owned()),
                    _ => (),
                }
            }
            if mode == 9 {
                values.pop();
            }
            if mode == 0 {
                headers.insert(
                    "link",
                    format!(
                        "<https://api.zotero.org/{}/42/items?start=25&limit=25>; rel=\"next\"",
                        if kind == "user" { "users" } else { "groups" }
                    )
                    .parse()
                    .unwrap(),
                );
            }
            headers.insert(
                "total-results",
                keys.split(',').count().to_string().parse().unwrap(),
            );
            Value::Array(values)
        } else {
            return (StatusCode::NOT_FOUND, headers, "unsupported fixture path").into_response();
        };
        if mode == 20 {
            let escaped: String = KEY
                .chars()
                .map(|value| format!("\\u{:04x}", value as u32))
                .collect();
            let body = value.to_string().replace(KEY, &escaped);
            headers.insert("content-type", "application/json".parse().unwrap());
            return (StatusCode::OK, headers, body).into_response();
        }
        (StatusCode::OK, headers, axum::Json(value)).into_response()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    const ITEM: &str = r#"[{"key":"ABCD2345","version":7,"library":{"type":"user","id":42},"links":{"alternate":{"href":"https://www.zotero.org/users/42/items/ABCD2345"}},"data":{"key":"ABCD2345","version":7,"itemType":"book","title":"합성 연구 자료 🙂","creators":[{"creatorType":"author","firstName":"민","lastName":"김"},{"creatorType":"editor","name":"Synthetic Research Group"}],"date":"Spring 2026","ISBN":"9780000000000","collections":["BCDE3456","CDEF4567"],"tags":[{"tag":"연구","type":0}],"relations":{}}}]"#;
    fn library() -> Library {
        Library::new(LibraryType::User, "42", "https://www.zotero.org/users/42").unwrap()
    }
    fn requested() -> BTreeMap<String, i64> {
        BTreeMap::from([("ABCD2345".into(), 7)])
    }
    #[test]
    fn bibliography_keeps_literal_unicode_creators_dates_and_two_memberships() {
        let items = items(ITEM.as_bytes(), &library(), &requested()).unwrap();
        assert_eq!(items.len(), 1);
        let item = &items[0];
        assert_eq!(item.key, "ABCD2345");
        assert_eq!(item.version, 7);
        assert_eq!(item.bibliography.title, "합성 연구 자료 🙂");
        assert_eq!(item.bibliography.fields["date"], "Spring 2026");
        assert_eq!(
            item.bibliography.creators[0].last_name.as_deref(),
            Some("김")
        );
        assert_eq!(
            item.bibliography.creators[1].name.as_deref(),
            Some("Synthetic Research Group")
        );
        assert_eq!(item.collections, ["BCDE3456", "CDEF4567"]);
        assert_eq!(
            item.return_url,
            "https://www.zotero.org/users/42/items/ABCD2345"
        );
        assert_eq!(bibliographic_types().len(), 37);
    }
    const SAVED_BIBLIOGRAPHY: &str = r#"{"itemType":"book","title":"합성 연구 자료 🙂","fields":{"date":"Spring 2026","ISBN":"9780000000000","dateAdded":"2026-10-01T00:00:00Z"},"creators":[{"creatorType":"author","firstName":"민","lastName":"김"},{"creatorType":"editor","name":"Synthetic Research Group"}],"tags":[{"tag":"연구","type":0}],"relations":{"dc:relation":["https://www.zotero.org/users/42/items/EFGH4567"]}}"#;
    fn saved_bibliography() -> Bibliography {
        serde_json::from_str(SAVED_BIBLIOGRAPHY).unwrap()
    }
    #[test]
    fn stored_bibliography_validates_literal_metadata_without_an_upstream_envelope() {
        let value = saved_bibliography();
        assert_eq!(validate_bibliography(&value), Ok(()));
        assert_eq!(value.title, "합성 연구 자료 🙂");
        assert_eq!(value.fields["date"], "Spring 2026");
        assert_eq!(value.creators[0].last_name.as_deref(), Some("김"));
        assert_eq!(
            value.creators[1].name.as_deref(),
            Some("Synthetic Research Group")
        );
        for literal in [
            r#"{"itemType":"case","title":"","fields":{"caseName":"Literal case name"},"creators":[],"tags":[]}"#,
            r#"{"itemType":"email","title":"","fields":{"subject":"Literal subject"},"creators":[],"tags":[]}"#,
        ] {
            let value: Bibliography = serde_json::from_str(literal).unwrap();
            assert_eq!(validate_bibliography(&value), Ok(()));
            assert_eq!(value.title, "");
        }
        assert!(serde_json::from_str::<Bibliography>(
            r#"{"itemType":"book","title":null,"fields":{},"creators":[],"tags":[]}"#
        )
        .is_err());
        assert!(serde_json::from_str::<Bibliography>(
            r#"{"itemType":"book","title":"","fields":{},"creators":[],"tags":[],"unexpected":"x"}"#
        )
        .is_err());
    }
    #[test]
    fn stored_bibliography_rejects_nonbibliographic_shapes_and_existing_policy_bounds() {
        for item_type in ["note", "attachment", "annotation", "unknownFutureType"] {
            let mut value = saved_bibliography();
            value.item_type = item_type.into();
            assert_eq!(validate_bibliography(&value), Err(ZoteroError::Invalid));
        }
        for field in ["unknownField", "title", "collections", "key", "version"] {
            let mut value = saved_bibliography();
            value
                .fields
                .insert(field.into(), "ambiguous stored field".into());
            assert_eq!(validate_bibliography(&value), Err(ZoteroError::Invalid));
        }
        let mut mixed_creator = saved_bibliography();
        mixed_creator.creators[0].name = Some("Institution and person".into());
        assert_eq!(
            validate_bibliography(&mixed_creator),
            Err(ZoteroError::Invalid)
        );
        let mut absent_creator = saved_bibliography();
        absent_creator.creators[0].first_name = None;
        absent_creator.creators[0].last_name = None;
        assert_eq!(
            validate_bibliography(&absent_creator),
            Err(ZoteroError::Invalid)
        );
        let mut wrong_creator_role = saved_bibliography();
        wrong_creator_role.creators[0].creator_type = "unknownCreatorRole".into();
        assert_eq!(
            validate_bibliography(&wrong_creator_role),
            Err(ZoteroError::Invalid)
        );
        let mut wrong_tag_type = saved_bibliography();
        wrong_tag_type.tags[0].tag_type = 2;
        assert_eq!(
            validate_bibliography(&wrong_tag_type),
            Err(ZoteroError::Invalid)
        );

        let base = saved_bibliography();
        let mut too_long_title = base.clone();
        too_long_title.title = "x".repeat(4097);
        let mut nul_title = base.clone();
        nul_title.title = "literal\0title".into();
        let mut too_many_creators = base.clone();
        too_many_creators.creators = vec![base.creators[0].clone(); 101];
        let mut too_long_name = base.clone();
        too_long_name.creators[0].last_name = Some("x".repeat(1025));
        let mut too_many_tags = base.clone();
        too_many_tags.tags = vec![base.tags[0].clone(); 101];
        let mut too_long_tag = base.clone();
        too_long_tag.tags[0].tag = "x".repeat(1025);
        let mut too_long_field = base.clone();
        too_long_field
            .fields
            .insert("date".into(), "x".repeat(16385));
        let mut nul_field = base.clone();
        nul_field
            .fields
            .insert("date".into(), "literal\0date".into());
        let mut too_many_relations = base.clone();
        too_many_relations.relations = (0..41)
            .map(|i| (format!("relation{i}"), Vec::new()))
            .collect();
        let mut too_long_relation_key = base.clone();
        too_long_relation_key.relations = BTreeMap::from([("x".repeat(129), Vec::new())]);
        let mut too_many_relation_values = base.clone();
        too_many_relation_values.relations =
            BTreeMap::from([("dc:relation".into(), vec!["x".into(); 21])]);
        let mut too_long_relation_value = base.clone();
        too_long_relation_value.relations =
            BTreeMap::from([("dc:relation".into(), vec!["x".repeat(2049)])]);
        for value in [
            too_long_title,
            nul_title,
            too_many_creators,
            too_long_name,
            too_many_tags,
            too_long_tag,
            too_long_field,
            nul_field,
            too_many_relations,
            too_long_relation_key,
            too_many_relation_values,
            too_long_relation_value,
        ] {
            assert_eq!(validate_bibliography(&value), Err(ZoteroError::Limit));
        }
    }
    #[test]
    fn saved_return_url_binds_to_stored_descriptor_without_weakening_live_verification() {
        let user = library();
        let href = "https://www.zotero.org/users/42/items/ABCD2345";
        assert_eq!(
            user.validate_saved_return_url("ABCD2345", href).unwrap(),
            href
        );
        for href in [
            "https://example.invalid/users/42/items/ABCD2345",
            "http://www.zotero.org/users/42/items/ABCD2345",
            "https://www.zotero.org/users/43/items/ABCD2345",
            "https://www.zotero.org/groups/42/items/ABCD2345",
            "https://www.zotero.org/users/42/items/EFGH4567",
            "https://www.zotero.org/users/42/items/ABCD2345?key=SYNTHETIC_ONLY",
            "https://www.zotero.org/users/42/items/ABCD2345#secret",
            "https://user:secret@www.zotero.org/users/42/items/ABCD2345",
            "https://www.zotero.org:8443/users/42/items/ABCD2345",
            "https://www.zotero.org/users/42/items/%41BCD2345",
        ] {
            assert_eq!(
                user.validate_saved_return_url("ABCD2345", href),
                Err(ZoteroError::Invalid)
            );
        }
        assert!(user.validate_saved_return_url("ABCD1234", href).is_err());
        let group = Library::new(
            LibraryType::Group,
            "42",
            "https://www.zotero.org/groups/42/study",
        )
        .unwrap();
        assert!(group
            .validate_saved_return_url(
                "ABCD2345",
                "https://www.zotero.org/groups/42/study/items/ABCD2345"
            )
            .is_ok());
        assert!(group.validate_saved_return_url("ABCD2345", href).is_err());
        let named = Library::new(
            LibraryType::User,
            "42",
            "https://www.zotero.org/archive_reader",
        )
        .unwrap();
        let named_href = "https://www.zotero.org/archive_reader/items/ABCD2345";
        assert_eq!(
            named
                .validate_saved_return_url("ABCD2345", named_href)
                .unwrap(),
            named_href
        );
        // Saved metadata can validate its bound URL offline; live imports still
        // require the upstream library's verified alternate base for this name.
        let missing_live_descriptor = ITEM.replace(href, named_href);
        assert_eq!(
            items(missing_live_descriptor.as_bytes(), &named, &requested()).unwrap_err(),
            ZoteroError::Invalid
        );
        let verified_live_descriptor = missing_live_descriptor.replace(
            r#""library":{"type":"user","id":42}"#,
            r#""library":{"type":"user","id":42,"links":{"alternate":{"href":"https://www.zotero.org/archive_reader"}}}"#,
        );
        assert_eq!(
            items(verified_live_descriptor.as_bytes(), &named, &requested()).unwrap()[0].return_url,
            named_href
        );
        let invalid_descriptor = Library {
            remote_id: 0,
            ..user
        };
        assert!(invalid_descriptor
            .validate_saved_return_url("ABCD2345", href)
            .is_err());
    }
    #[test]
    fn rejects_duplicate_json_keys_missing_extra_wrong_library_versions_and_content_types() {
        for input in [
            ITEM.replace("\"version\":7", "\"version\":7,\"version\":8"),
            ITEM.replace("\"id\":42", "\"id\":43"),
            ITEM.replace("\"type\":\"user\"", "\"type\":\"group\""),
            ITEM.replace("\"version\":7", "\"version\":6"),
            ITEM.replace("\"itemType\":\"book\"", "\"itemType\":\"note\""),
            ITEM.replace("\"itemType\":\"book\"", "\"itemType\":\"newUnknownType\""),
            ITEM.replace("\"ISBN\"", "\"unrecognizedField\""),
            "[]".into(),
            format!(
                "[{},{}]",
                &ITEM[1..ITEM.len() - 1],
                &ITEM[1..ITEM.len() - 1]
            ),
        ] {
            assert_eq!(
                items(input.as_bytes(), &library(), &requested()).unwrap_err(),
                ZoteroError::Invalid
            );
        }
        assert!(versions(br#"{"ABCD2345":7,"ABCD2345":7}"#, 12).is_err());
        assert!(versions(br#"{"ABCD2345":13}"#, 12).is_err());
        assert!(versions(br#"{"ABCD2345":7.0}"#, 12).is_err());
    }
    #[test]
    fn decimal_storage_browser_precision_and_bounds_are_explicit() {
        assert_eq!(decimal("9007199254740993").unwrap(), 9007199254740993);
        assert_eq!(decimal("9223372036854775807").unwrap(), i64::MAX);
        for value in ["9223372036854775808", "-1", "1.0", "01", "1e2", ""] {
            assert!(decimal(value).is_err());
        }
        assert!(!key_valid("ABCD1234"));
        assert!(!key_valid("C1"));
        assert_eq!(
            parse::<Value>(&vec![b' '; BODY_MAX + 1]).unwrap_err(),
            ZoteroError::Limit
        );
    }
    #[test]
    fn alternate_library_binding_queries_foreign_urls_and_cycles_fail() {
        for target in [
            "https://example.invalid/users/42/items/ABCD2345",
            "https://www.zotero.org/users/43/items/ABCD2345",
            "https://www.zotero.org/users/42/items/ABCD2345?key=secret",
        ] {
            let input = ITEM.replace("https://www.zotero.org/users/42/items/ABCD2345", target);
            assert!(items(input.as_bytes(), &library(), &requested()).is_err());
        }
        assert!(library().url("items/ABCD2345/file", &[]).is_err());
        assert!(library().url("keys/secret", &[]).is_err());
        let mut graph = vec![
            Collection {
                key: "BCDE3456".into(),
                version: 6,
                name: "Study".into(),
                parent: None,
            },
            Collection {
                key: "CDEF4567".into(),
                version: 7,
                name: "Evidence".into(),
                parent: Some("BCDE3456".into()),
            },
        ];
        collection_graph(&graph).unwrap();
        graph[0].parent = Some("CDEF4567".into());
        assert!(collection_graph(&graph).is_err());
        graph[0].parent = Some("EFGH4567".into());
        assert!(collection_graph(&graph).is_err());
    }
    #[test]
    fn rate_success_denied_transient_and_redirect_are_distinct() {
        let mut reply = ReadReply {
            status: 200,
            headers: reqwest::header::HeaderMap::new(),
            body: b"{}".to_vec(),
        };
        reply
            .headers
            .insert("last-modified-version", "12".parse().unwrap());
        reply.headers.insert("backoff", "60".parse().unwrap());
        assert_eq!(delay(&reply).unwrap(), 60);
        assert_eq!(version(&reply, Some(12)).unwrap(), 12);
        assert_eq!(
            version(&reply, Some(13)).unwrap_err(),
            ZoteroError::VersionChanged
        );
        reply.headers.remove("backoff");
        reply.status = 429;
        reply.headers.insert("retry-after", "30".parse().unwrap());
        assert_eq!(delay(&reply).unwrap(), 30);
        assert_eq!(version(&reply, None).unwrap_err(), ZoteroError::Delayed);
        reply.status = 503;
        reply.headers.insert("retry-after", "45".parse().unwrap());
        assert_eq!(delay(&reply).unwrap(), 45);
        assert_eq!(version(&reply, None).unwrap_err(), ZoteroError::Transient);
        reply.status = 403;
        assert_eq!(version(&reply, None).unwrap_err(), ZoteroError::Denied);
        reply.status = 302;
        assert_eq!(version(&reply, None).unwrap_err(), ZoteroError::Invalid);
    }
    #[test]
    fn present_imported_title_rejects_nonstring_literals_and_keeps_absent_control() {
        let needle = "\"title\":\"합성 연구 자료 🙂\"";
        assert!(ITEM.contains(needle));
        let results: Vec<_> = ["null", "42", "[\"bad\"]", "{\"bad\":\"type\"}"]
            .into_iter()
            .map(|value| {
                items(
                    ITEM.replace(needle, &format!("\"title\":{value}"))
                        .as_bytes(),
                    &library(),
                    &requested(),
                )
                .err()
            })
            .collect();
        assert_eq!(results, vec![Some(ZoteroError::Invalid); 4]);
        assert_eq!(
            items(
                ITEM.replace(needle, "\"title\":\"\"").as_bytes(),
                &library(),
                &requested()
            )
            .unwrap()[0]
                .bibliography
                .title,
            ""
        );
        assert_eq!(
            items(
                ITEM.replace(&format!("{needle},"), "").as_bytes(),
                &library(),
                &requested()
            )
            .unwrap()[0]
                .bibliography
                .title,
            ""
        );
        let case=br#"[{"key":"ABCD2345","version":7,"library":{"type":"user","id":42},"links":{"alternate":{"href":"https://www.zotero.org/users/42/items/ABCD2345"}},"data":{"key":"ABCD2345","version":7,"itemType":"case","caseName":"Literal v. Literal","creators":[],"collections":[],"tags":[],"relations":{}}}]"#;
        let imported = items(case, &library(), &requested()).unwrap();
        assert_eq!(
            imported[0].bibliography.fields["caseName"],
            "Literal v. Literal"
        );
        assert_eq!(imported[0].bibliography.title, "");
    }
    #[test]
    fn decoded_imported_credential_sentinel_is_detected() {
        let sentinel = "SYNTHETIC_ONLY_NEVER_A_ZOTERO_KEY";
        let escaped: String = sentinel
            .chars()
            .map(|value| format!("\\u{:04x}", value as u32))
            .collect();
        let body = format!("{{\"creator\":\"{escaped}\"}}");
        assert!(!body.contains(sentinel));
        assert!(body_contains_credential(body.as_bytes(), sentinel).unwrap());
    }
}
