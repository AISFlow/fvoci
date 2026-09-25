//! S3-compatible attachment backend.
//!
//! Ports `packages/storage/src/s3.ts` at source SHA
//! `393795261322b916e588043cf94feca999175843` behind the Rust storage
//! methods (server-proxied parts, atomic complete, abort, ranged download,
//! delete). Request signing is delegated to `rusty-s3` (SigV4 query
//! signing); `reqwest` only carries the signed request. Credentials come only
//! from configuration and are never logged: signed URLs carry the access key
//! id and a signature, so transport errors are stripped of their URL and S3
//! error bodies are reduced to the operation, HTTP status and error code.

use std::io;
use std::pin::Pin;
use std::sync::atomic::{AtomicU8, Ordering};
use std::sync::Arc;
use std::task::{Context, Poll};
use std::time::Duration;

use bytes::Bytes;
use futures_util::Stream;
use reqwest::{redirect, Client, Response, StatusCode};
use rusty_s3::actions::{CreateMultipartUpload, ListParts, S3Action};
use rusty_s3::{Bucket, Credentials, UrlStyle};
use serde::{Deserialize, Serialize};
use url::Url;

use super::local::{LocalStorage, PartInfo, StorageError};
use super::sniff_mime_from_bytes;
use crate::config::S3Settings;

/// Pinned MinIO-compatible image used by source S3 tests (`pgsty/silo`).
pub const MINIO_TEST_IMAGE: &str = "pgsty/silo:RELEASE.2026-08-06T00-00-00Z@sha256:29a498b24669cae1fed11c1a2fb2b3d73c68829a0a9c0b14e71b386671d38fac";

/// Lifetime of each signed request URL. The URL is used immediately by this
/// process and never handed to a client, so it only has to cover clock skew.
const SIGN_TTL: Duration = Duration::from_secs(300);
const CONNECT_TIMEOUT: Duration = Duration::from_secs(10);
/// Idle read timeout. There is deliberately no total timeout: downloads are
/// streamed to the client and a slow reader must not cut a healthy transfer.
const READ_TIMEOUT: Duration = Duration::from_secs(30);
const LIST_PAGE_SIZE: u16 = 1000;
const MAX_PART_NUMBER: i32 = 10_000;

#[derive(Clone)]
pub struct S3Storage {
    client: Client,
    /// Streams proxied part bodies. reqwest's `read_timeout` also bounds the
    /// wait for response headers, which for a streamed PUT includes the whole
    /// upload of a client-paced body, so this client has none.
    upload_client: Client,
    bucket: Bucket,
    credentials: Credentials,
    endpoint: String,
}

impl std::fmt::Debug for S3Storage {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("S3Storage")
            .field("endpoint", &self.endpoint)
            .field("bucket", &self.bucket.name())
            .field("region", &self.bucket.region())
            .field("credentials", &"<redacted>")
            .finish()
    }
}

impl S3Storage {
    pub fn new(settings: S3Settings) -> Result<Self, String> {
        let mut endpoint =
            Url::parse(&settings.endpoint).map_err(|e| format!("invalid S3_ENDPOINT: {e}"))?;
        // `Bucket` joins the bucket name onto the endpoint path; without a
        // trailing slash a path prefix such as `/s3` would be replaced.
        if !endpoint.path().ends_with('/') {
            let path = format!("{}/", endpoint.path());
            endpoint.set_path(&path);
        }
        let style = if settings.force_path_style {
            UrlStyle::Path
        } else {
            UrlStyle::VirtualHost
        };
        let bucket = Bucket::new(endpoint, style, settings.bucket, settings.region)
            .map_err(|e| format!("invalid S3 bucket configuration: {e:?}"))?;
        let client = client_builder()
            .read_timeout(READ_TIMEOUT)
            .build()
            .map_err(|e| format!("s3 http client: {}", e.without_url()))?;
        let upload_client = client_builder()
            .build()
            .map_err(|e| format!("s3 http client: {}", e.without_url()))?;
        Ok(Self {
            client,
            upload_client,
            bucket,
            credentials: Credentials::new(settings.access_key_id, settings.secret_access_key),
            endpoint: settings.endpoint,
        })
    }

    pub fn bucket_name(&self) -> &str {
        self.bucket.name()
    }

    pub fn endpoint_url(&self) -> &str {
        &self.endpoint
    }

    pub async fn head_bucket(&self) -> Result<(), StorageError> {
        let url = self
            .bucket
            .head_bucket(Some(&self.credentials))
            .sign(SIGN_TTL);
        let res = self.send(self.client.head(url)).await?;
        if res.status().is_success() {
            Ok(())
        } else {
            Err(status_error("HeadBucket", res).await)
        }
    }

    /// Creates the bucket when it is missing. Used by tests and local setup;
    /// the server itself only probes with `head_bucket`.
    pub async fn ensure_bucket(&self) -> Result<(), StorageError> {
        match self.head_bucket().await {
            Ok(()) => return Ok(()),
            Err(StorageError::Io(_)) => {}
            Err(err) => return Err(err),
        }
        let url = self.bucket.create_bucket(&self.credentials).sign(SIGN_TTL);
        let res = self.send(self.client.put(url)).await?;
        let status = res.status();
        if status.is_success() {
            return Ok(());
        }
        let body = read_text(res).await?;
        match error_code(&body).as_deref() {
            Some("BucketAlreadyOwnedByYou") => Ok(()),
            code => Err(op_error("CreateBucket", status, code)),
        }
    }

    pub async fn create_multipart(&self, key: &str) -> Result<String, StorageError> {
        assert_key(key)?;
        let url = self
            .bucket
            .create_multipart_upload(Some(&self.credentials), key)
            .sign(SIGN_TTL);
        let res = self.send(self.client.post(url)).await?;
        let status = res.status();
        let body = read_text(res).await?;
        if !status.is_success() {
            return Err(op_error(
                "CreateMultipartUpload",
                status,
                error_code(&body).as_deref(),
            ));
        }
        let parsed = CreateMultipartUpload::parse_response(&body)
            .map_err(|_| malformed("CreateMultipartUpload"))?;
        let upload_id = parsed.upload_id();
        if upload_id.is_empty() {
            return Err(malformed("CreateMultipartUpload"));
        }
        Ok(upload_id.to_string())
    }

    /// Aborts one multipart upload. An upload that is already gone counts as
    /// aborted, matching the source driver.
    pub async fn abort_multipart(
        &self,
        key: &str,
        upload_ref: Option<&str>,
    ) -> Result<(), StorageError> {
        assert_key(key)?;
        let Some(upload_id) = upload_ref.filter(|id| !id.is_empty()) else {
            return Ok(());
        };
        let url = self
            .bucket
            .abort_multipart_upload(Some(&self.credentials), key, upload_id)
            .sign(SIGN_TTL);
        let res = self.send(self.client.delete(url)).await?;
        let status = res.status();
        if status.is_success() {
            return Ok(());
        }
        let body = read_text(res).await?;
        match error_code(&body).as_deref() {
            Some("NoSuchUpload") => Ok(()),
            code => Err(op_error("AbortMultipartUpload", status, code)),
        }
    }

    /// Streams one part straight to S3 with its declared length; nothing is
    /// buffered beyond the chunk in flight. A body that ends early or runs
    /// past `content_length` fails the request, so S3 never stores it.
    pub async fn upload_part_stream<S, E>(
        &self,
        key: &str,
        upload_id: &str,
        part_number: i32,
        stream: S,
        content_length: u64,
    ) -> Result<PartInfo, StorageError>
    where
        S: Stream<Item = Result<Bytes, E>> + Send + 'static,
        E: Into<Box<dyn std::error::Error + Send + Sync>> + 'static,
    {
        assert_key(key)?;
        let number = part_number_u16(part_number)?;
        if upload_id.is_empty() {
            return Err(StorageError::UploadGone);
        }
        let url = self
            .bucket
            .upload_part(Some(&self.credentials), key, number, upload_id)
            .sign(SIGN_TTL);
        let body = ExactLength::new(stream, content_length);
        let outcome = body.outcome.clone();
        let sent = self
            .upload_client
            .put(url)
            .header(reqwest::header::CONTENT_LENGTH, content_length)
            .body(reqwest::Body::wrap_stream(body))
            .send()
            .await;
        if outcome.load(Ordering::Acquire) == BODY_LENGTH_MISMATCH {
            return Err(StorageError::LengthMismatch);
        }
        let res = sent.map_err(transport)?;
        let status = res.status();
        if !status.is_success() {
            let text = read_text(res).await?;
            return Err(op_error("UploadPart", status, error_code(&text).as_deref()));
        }
        let etag = res
            .headers()
            .get(reqwest::header::ETAG)
            .and_then(|v| v.to_str().ok())
            .map(strip_quotes)
            .filter(|s| !s.is_empty())
            .ok_or_else(|| malformed("UploadPart"))?;
        Ok(PartInfo {
            part_number,
            etag,
            size_bytes: content_length,
        })
    }

    pub async fn list_parts(
        &self,
        key: &str,
        upload_ref: Option<&str>,
    ) -> Result<Vec<PartInfo>, StorageError> {
        assert_key(key)?;
        let upload_id = upload_ref
            .filter(|id| !id.is_empty())
            .ok_or(StorageError::UploadGone)?;
        let mut parts = Vec::new();
        let mut marker: Option<u16> = None;
        loop {
            let mut action = ListParts::new(&self.bucket, Some(&self.credentials), key, upload_id);
            action.set_max_parts(LIST_PAGE_SIZE);
            if let Some(marker) = marker {
                action.set_part_number_marker(marker);
            }
            let url = action.sign(SIGN_TTL);
            let res = self.send(self.client.get(url)).await?;
            let status = res.status();
            let body = read_text(res).await?;
            if !status.is_success() {
                return Err(op_error("ListParts", status, error_code(&body).as_deref()));
            }
            let page = ListParts::parse_response(&body).map_err(|_| malformed("ListParts"))?;
            for part in page.parts {
                parts.push(PartInfo {
                    part_number: i32::from(part.number),
                    etag: strip_quotes(&part.etag),
                    size_bytes: part.size,
                });
            }
            // rusty-s3 clears the marker when the page is not truncated.
            match page.next_part_number_marker {
                Some(next) if Some(next) != marker => marker = Some(next),
                Some(_) => return Err(malformed("ListParts")),
                None => break,
            }
        }
        parts.sort_by_key(|p| p.part_number);
        Ok(parts)
    }

    /// Every still-open upload that could yet publish an object at `key`.
    ///
    /// `rusty-s3` 0.8 has no `ListMultipartUploads` action, so this signs the
    /// bucket-level request with its public `signing::sign` primitive.
    pub async fn list_multipart_uploads(
        &self,
        key: &str,
    ) -> Result<Vec<Option<String>>, StorageError> {
        assert_key(key)?;
        let mut refs = Vec::new();
        let mut key_marker: Option<String> = None;
        let mut upload_marker: Option<String> = None;
        loop {
            let mut query = rusty_s3::Map::new();
            query.insert("uploads", "");
            query.insert("prefix", key);
            query.insert("max-uploads", LIST_PAGE_SIZE.to_string());
            if let Some(marker) = &key_marker {
                query.insert("key-marker", marker.clone());
            }
            if let Some(marker) = &upload_marker {
                query.insert("upload-id-marker", marker.clone());
            }
            let url = rusty_s3::signing::sign(
                &jiff::Timestamp::now(),
                rusty_s3::Method::Get,
                self.bucket.base_url().clone(),
                self.credentials.key(),
                self.credentials.secret(),
                self.credentials.token(),
                self.bucket.region(),
                SIGN_TTL.as_secs(),
                query.iter(),
                std::iter::empty(),
            );
            let res = self.send(self.client.get(url)).await?;
            let status = res.status();
            let body = read_text(res).await?;
            if !status.is_success() {
                return Err(op_error(
                    "ListMultipartUploads",
                    status,
                    error_code(&body).as_deref(),
                ));
            }
            let page: ListMultipartUploadsResult =
                quick_xml::de::from_str(&body).map_err(|_| malformed("ListMultipartUploads"))?;
            // The key is a UUID so the prefix only matches itself, but a
            // server may still return a wider prefix: filter by identity.
            refs.extend(
                page.uploads
                    .into_iter()
                    .filter(|u| u.key == key && !u.upload_id.is_empty())
                    .map(|u| Some(u.upload_id)),
            );
            if !page.is_truncated {
                break;
            }
            let next_key = page
                .next_key_marker
                .filter(|m| !m.is_empty())
                .ok_or_else(|| malformed("ListMultipartUploads"))?;
            let next_upload = page.next_upload_id_marker.filter(|m| !m.is_empty());
            if key_marker.as_deref() == Some(next_key.as_str()) && upload_marker == next_upload {
                return Err(malformed("ListMultipartUploads"));
            }
            key_marker = Some(next_key);
            upload_marker = next_upload;
        }
        Ok(refs)
    }

    /// Publishes the object atomically. A retry after a complete that already
    /// succeeded (`NoSuchUpload` with the object present) reports its size.
    pub async fn complete_multipart(
        &self,
        key: &str,
        upload_ref: Option<&str>,
        submitted: &[(i32, String)],
    ) -> Result<u64, StorageError> {
        assert_key(key)?;
        let upload_id = upload_ref
            .filter(|id| !id.is_empty())
            .ok_or(StorageError::UploadGone)?;
        let mut parts = Vec::with_capacity(submitted.len());
        for (number, etag) in submitted {
            parts.push(CompletePart {
                part_number: part_number_u16(*number)?,
                etag: format!("\"{}\"", strip_quotes(etag)),
            });
        }
        parts.sort_by_key(|p| p.part_number);
        // rusty-s3's `CompleteMultipartUpload::body` numbers parts by position;
        // the submitted part numbers are kept explicitly instead.
        let xml = quick_xml::se::to_string(&CompleteMultipartUploadBody { parts })
            .map_err(|_| malformed("CompleteMultipartUpload"))?;
        let url = self
            .bucket
            .complete_multipart_upload(
                Some(&self.credentials),
                key,
                upload_id,
                std::iter::empty::<&str>(),
            )
            .sign(SIGN_TTL);
        let res = self
            .send(
                self.client
                    .post(url)
                    .header(reqwest::header::CONTENT_TYPE, "application/xml")
                    .body(xml),
            )
            .await?;
        let status = res.status();
        let body = read_text(res).await?;
        // S3 may answer 200 OK with an <Error> document once it has started
        // streaming the response, so a success status is not enough.
        let code = error_code(&body);
        if status.is_success() && code.is_none() {
            return self
                .head(key)
                .await?
                .ok_or_else(|| malformed("CompleteMultipartUpload"));
        }
        match code.as_deref() {
            Some("NoSuchUpload") => match self.head(key).await? {
                Some(size) => Ok(size),
                None => Err(StorageError::UploadGone),
            },
            code => Err(op_error("CompleteMultipartUpload", status, code)),
        }
    }

    pub async fn delete_object(&self, key: &str) -> Result<(), StorageError> {
        assert_key(key)?;
        let url = self
            .bucket
            .delete_object(Some(&self.credentials), key)
            .sign(SIGN_TTL);
        let res = self.send(self.client.delete(url)).await?;
        let status = res.status();
        if status.is_success() {
            return Ok(());
        }
        let body = read_text(res).await?;
        let code = error_code(&body);
        // A missing key is already deleted; a missing bucket (also a 404) is
        // a misconfiguration and must not let callers drop their DB rows.
        match code.as_deref() {
            None | Some("NoSuchKey") if status == StatusCode::NOT_FOUND => Ok(()),
            code => Err(op_error("DeleteObject", status, code)),
        }
    }

    pub async fn head(&self, key: &str) -> Result<Option<u64>, StorageError> {
        assert_key(key)?;
        let url = self
            .bucket
            .head_object(Some(&self.credentials), key)
            .sign(SIGN_TTL);
        let res = self.send(self.client.head(url)).await?;
        if res.status() == StatusCode::NOT_FOUND {
            return Ok(None);
        }
        if !res.status().is_success() {
            return Err(status_error("HeadObject", res).await);
        }
        res.headers()
            .get(reqwest::header::CONTENT_LENGTH)
            .and_then(|v| v.to_str().ok())
            .and_then(|v| v.parse().ok())
            .map(Some)
            .ok_or_else(|| malformed("HeadObject"))
    }

    pub async fn read_range(
        &self,
        key: &str,
        start: u64,
        end: u64,
    ) -> Result<Vec<u8>, StorageError> {
        let res = self.get_object(key, start, end).await?;
        let bytes = res.bytes().await.map_err(transport)?;
        Ok(bytes.to_vec())
    }

    /// Opens the inclusive byte range `start..=end` of an object. The
    /// response must be the requested range: a server or proxy that ignores
    /// `Range` would otherwise stream the object from byte 0.
    pub async fn get_object(
        &self,
        key: &str,
        start: u64,
        end: u64,
    ) -> Result<Response, StorageError> {
        assert_key(key)?;
        if end < start {
            return Err(StorageError::Io(io::Error::new(
                io::ErrorKind::InvalidInput,
                "invalid byte range",
            )));
        }
        let range = format!("bytes={start}-{end}");
        let mut action = self.bucket.get_object(Some(&self.credentials), key);
        action.headers_mut().insert("range", range.clone());
        let url = action.sign(SIGN_TTL);
        let res = self
            .send(self.client.get(url).header(reqwest::header::RANGE, range))
            .await?;
        let status = res.status();
        if status == StatusCode::NOT_FOUND {
            return Err(StorageError::Io(io::Error::new(
                io::ErrorKind::NotFound,
                "object not found",
            )));
        }
        if !status.is_success() {
            return Err(status_error("GetObject", res).await);
        }
        let wanted = end - start + 1;
        let exact = match status {
            StatusCode::PARTIAL_CONTENT => res
                .headers()
                .get(reqwest::header::CONTENT_RANGE)
                .and_then(|v| v.to_str().ok())
                .is_some_and(|v| content_range_matches(v, start, end)),
            // A whole-object 200 is only the requested range when it starts
            // at 0 and has exactly the requested length.
            StatusCode::OK => start == 0 && res.content_length() == Some(wanted),
            _ => false,
        };
        if !exact {
            return Err(StorageError::Io(io::Error::other(format!(
                "GetObject returned HTTP {} without the requested range",
                status.as_u16()
            ))));
        }
        Ok(res)
    }

    pub async fn sniff_mime(&self, key: &str) -> Result<String, StorageError> {
        let size = self.head(key).await?.unwrap_or(0);
        if size == 0 {
            return Ok("application/octet-stream".to_string());
        }
        let end = (size - 1).min(4095);
        let sample = self.read_range(key, 0, end).await?;
        Ok(sniff_mime_from_bytes(&sample))
    }

    async fn send(&self, request: reqwest::RequestBuilder) -> Result<Response, StorageError> {
        request.send().await.map_err(transport)
    }
}

/// No redirects (a redirect would replay a signed PUT body elsewhere) and no
/// implicit `HTTP(S)_PROXY`: signed URLs go only to the configured endpoint.
fn client_builder() -> reqwest::ClientBuilder {
    Client::builder()
        .connect_timeout(CONNECT_TIMEOUT)
        .redirect(redirect::Policy::none())
        .no_proxy()
}

const BODY_OK: u8 = 0;
const BODY_LENGTH_MISMATCH: u8 = 1;

/// Passes a part body through while enforcing its declared length. The
/// chunk that completes the length is held back until the source ends, so
/// surplus bytes fail the upload instead of being silently dropped once the
/// HTTP client has written `Content-Length` bytes. The outcome flag lets the
/// caller tell a length violation from a transport or client-disconnect error
/// once reqwest has wrapped both.
struct ExactLength<S> {
    inner: Pin<Box<S>>,
    remaining: u64,
    held: Option<Bytes>,
    done: bool,
    outcome: Arc<AtomicU8>,
}

impl<S> ExactLength<S> {
    fn new(inner: S, length: u64) -> Self {
        Self {
            inner: Box::pin(inner),
            remaining: length,
            held: None,
            done: false,
            outcome: Arc::new(AtomicU8::new(BODY_OK)),
        }
    }

    fn mismatch(&mut self) -> Poll<Option<Result<Bytes, io::Error>>> {
        self.done = true;
        self.held = None;
        self.outcome.store(BODY_LENGTH_MISMATCH, Ordering::Release);
        Poll::Ready(Some(Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "part body does not match its declared length",
        ))))
    }
}

impl<S, E> Stream for ExactLength<S>
where
    S: Stream<Item = Result<Bytes, E>>,
    E: Into<Box<dyn std::error::Error + Send + Sync>>,
{
    type Item = Result<Bytes, io::Error>;

    fn poll_next(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        let this = self.get_mut();
        loop {
            if this.done {
                return Poll::Ready(this.held.take().map(Ok));
            }
            match this.inner.as_mut().poll_next(cx) {
                Poll::Pending => return Poll::Pending,
                Poll::Ready(Some(Ok(chunk))) if chunk.is_empty() => {}
                Poll::Ready(Some(Ok(chunk))) => {
                    let len = chunk.len() as u64;
                    if this.held.is_some() || len > this.remaining {
                        return this.mismatch();
                    }
                    this.remaining -= len;
                    if this.remaining > 0 {
                        return Poll::Ready(Some(Ok(chunk)));
                    }
                    this.held = Some(chunk);
                }
                Poll::Ready(Some(Err(err))) => {
                    this.done = true;
                    this.held = None;
                    return Poll::Ready(Some(Err(io::Error::other(err))));
                }
                Poll::Ready(None) if this.remaining > 0 => return this.mismatch(),
                Poll::Ready(None) => this.done = true,
            }
        }
    }
}

fn content_range_matches(value: &str, start: u64, end: u64) -> bool {
    value
        .trim()
        .strip_prefix("bytes ")
        .and_then(|rest| rest.split_once('/'))
        .and_then(|(range, _total)| range.split_once('-'))
        .is_some_and(|(s, e)| s.parse() == Ok(start) && e.parse() == Ok(end))
}

#[derive(Serialize)]
#[serde(rename = "CompleteMultipartUpload")]
struct CompleteMultipartUploadBody {
    #[serde(rename = "Part")]
    parts: Vec<CompletePart>,
}

#[derive(Serialize)]
struct CompletePart {
    #[serde(rename = "PartNumber")]
    part_number: u16,
    #[serde(rename = "ETag")]
    etag: String,
}

#[derive(Deserialize)]
struct ListMultipartUploadsResult {
    #[serde(rename = "Upload", default)]
    uploads: Vec<MultipartUploadEntry>,
    #[serde(rename = "IsTruncated", default)]
    is_truncated: bool,
    #[serde(rename = "NextKeyMarker", default)]
    next_key_marker: Option<String>,
    #[serde(rename = "NextUploadIdMarker", default)]
    next_upload_id_marker: Option<String>,
}

#[derive(Deserialize)]
struct MultipartUploadEntry {
    #[serde(rename = "Key")]
    key: String,
    #[serde(rename = "UploadId")]
    upload_id: String,
}

#[derive(Deserialize)]
#[serde(rename = "Error")]
struct S3ErrorBody {
    #[serde(rename = "Code")]
    code: String,
}

fn assert_key(key: &str) -> Result<(), StorageError> {
    LocalStorage::assert_key(key)
}

fn part_number_u16(n: i32) -> Result<u16, StorageError> {
    if (1..=MAX_PART_NUMBER).contains(&n) {
        u16::try_from(n).map_err(|_| StorageError::InvalidKey)
    } else {
        Err(StorageError::InvalidKey)
    }
}

fn strip_quotes(value: &str) -> String {
    value.trim().trim_matches('"').to_string()
}

/// Extracts `<Error><Code>` from an S3 error document. Any other document
/// (including a successful result) yields `None`.
fn error_code(body: &str) -> Option<String> {
    let trimmed = body.trim_start();
    if trimmed.is_empty() {
        return None;
    }
    quick_xml::de::from_str::<S3ErrorBody>(trimmed)
        .ok()
        .map(|e| e.code)
        .filter(|c| !c.is_empty())
}

fn transport(err: reqwest::Error) -> StorageError {
    // Signed URLs embed the access key id and a signature: never keep them.
    StorageError::Io(io::Error::other(err.without_url()))
}

fn malformed(op: &str) -> StorageError {
    StorageError::Io(io::Error::other(format!(
        "{op} returned a malformed response"
    )))
}

async fn read_text(res: Response) -> Result<String, StorageError> {
    res.text().await.map_err(transport)
}

async fn status_error(op: &str, res: Response) -> StorageError {
    let status = res.status();
    match read_text(res).await {
        Ok(body) => op_error(op, status, error_code(&body).as_deref()),
        Err(err) => err,
    }
}

fn op_error(op: &str, status: StatusCode, code: Option<&str>) -> StorageError {
    match code {
        Some("NoSuchUpload") => StorageError::UploadGone,
        Some("InvalidPart" | "InvalidPartOrder") => StorageError::EtagMismatch,
        Some("EntityTooLarge") => StorageError::PartTooLarge,
        Some("EntityTooSmall") => StorageError::PartTooSmall,
        _ => StorageError::Io(io::Error::other(format!(
            "{op} failed: HTTP {} code={}",
            status.as_u16(),
            code.unwrap_or("unknown")
        ))),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn settings(endpoint: &str, path_style: bool) -> S3Settings {
        S3Settings {
            endpoint: endpoint.into(),
            public_endpoint: None,
            region: "us-east-1".into(),
            bucket: "fvoci".into(),
            access_key_id: "AKIDSECRETID".into(),
            secret_access_key: "very-secret-key".into(),
            force_path_style: path_style,
        }
    }

    #[test]
    fn debug_redacts_credentials() {
        let storage = S3Storage::new(settings("http://127.0.0.1:9000", true)).unwrap();
        let rendered = format!("{storage:?}");
        assert!(rendered.contains("<redacted>"));
        assert!(!rendered.contains("AKIDSECRETID"));
        assert!(!rendered.contains("very-secret-key"));
    }

    #[test]
    fn path_style_keeps_endpoint_prefix_and_signs_bucket_host() {
        let storage = S3Storage::new(settings("http://127.0.0.1:9000/s3", true)).unwrap();
        let url = storage
            .bucket
            .head_object(
                Some(&storage.credentials),
                "aaaaaaaa-bbbb-cccc-dddd-eeeeeeeeeeee",
            )
            .sign(SIGN_TTL);
        assert_eq!(url.host_str(), Some("127.0.0.1"));
        assert_eq!(url.path(), "/s3/fvoci/aaaaaaaa-bbbb-cccc-dddd-eeeeeeeeeeee");
        assert!(!url.as_str().contains("very-secret-key"));

        let vhost = S3Storage::new(settings("https://s3.example.test", false)).unwrap();
        let url = vhost
            .bucket
            .head_object(
                Some(&vhost.credentials),
                "aaaaaaaa-bbbb-cccc-dddd-eeeeeeeeeeee",
            )
            .sign(SIGN_TTL);
        assert_eq!(url.host_str(), Some("fvoci.s3.example.test"));
    }

    #[test]
    fn complete_body_keeps_explicit_part_numbers() {
        let xml = quick_xml::se::to_string(&CompleteMultipartUploadBody {
            parts: vec![
                CompletePart {
                    part_number: 1,
                    etag: "\"a&b\"".into(),
                },
                CompletePart {
                    part_number: 3,
                    etag: "\"c\"".into(),
                },
            ],
        })
        .unwrap();
        assert!(xml.starts_with("<CompleteMultipartUpload>"), "{xml}");
        assert!(xml.contains("<PartNumber>3</PartNumber>"), "{xml}");
        assert!(xml.contains("a&amp;b"), "{xml}");
    }

    #[test]
    fn error_code_only_reads_error_documents() {
        assert_eq!(
            error_code("<?xml version=\"1.0\"?><Error><Code>NoSuchUpload</Code><Message>x</Message></Error>")
                .as_deref(),
            Some("NoSuchUpload")
        );
        assert_eq!(
            error_code(
                "<CompleteMultipartUploadResult><ETag>x</ETag></CompleteMultipartUploadResult>"
            ),
            None
        );
        assert_eq!(error_code(""), None);
    }

    #[test]
    fn list_multipart_uploads_page_parses() {
        let body = r#"<?xml version="1.0" encoding="UTF-8"?>
<ListMultipartUploadsResult><Bucket>fvoci</Bucket><KeyMarker></KeyMarker>
<NextKeyMarker>k2</NextKeyMarker><NextUploadIdMarker>u2</NextUploadIdMarker>
<MaxUploads>1000</MaxUploads><IsTruncated>true</IsTruncated>
<Upload><Key>k1</Key><UploadId>u1</UploadId><Initiated>2026-01-01T00:00:00Z</Initiated></Upload>
<Upload><Key>k2</Key><UploadId>u2</UploadId></Upload>
</ListMultipartUploadsResult>"#;
        let page: ListMultipartUploadsResult = quick_xml::de::from_str(body).unwrap();
        assert!(page.is_truncated);
        assert_eq!(page.uploads.len(), 2);
        assert_eq!(page.uploads[1].upload_id, "u2");
        assert_eq!(page.next_key_marker.as_deref(), Some("k2"));
        let empty: ListMultipartUploadsResult = quick_xml::de::from_str(
            "<ListMultipartUploadsResult><IsTruncated>false</IsTruncated></ListMultipartUploadsResult>",
        )
        .unwrap();
        assert!(empty.uploads.is_empty());
    }

    #[test]
    fn content_range_must_match_the_requested_range() {
        assert!(content_range_matches("bytes 10-20/100", 10, 20));
        assert!(!content_range_matches("bytes 0-99/100", 10, 20));
        assert!(!content_range_matches("bytes 10-21/100", 10, 20));
        assert!(!content_range_matches("items 10-20/100", 10, 20));
        assert!(!content_range_matches("bytes */100", 10, 20));
    }

    #[tokio::test]
    async fn exact_length_rejects_short_and_long_bodies() {
        use futures_util::{stream, StreamExt};

        async fn run(chunks: &[&'static [u8]], len: u64) -> (Vec<u8>, bool, u8) {
            let body = ExactLength::new(
                stream::iter(
                    chunks
                        .iter()
                        .map(|c| Ok::<_, io::Error>(Bytes::from_static(c)))
                        .collect::<Vec<_>>(),
                ),
                len,
            );
            let outcome = body.outcome.clone();
            let items: Vec<_> = body.collect().await;
            let failed = items.iter().any(Result::is_err);
            let bytes = items
                .into_iter()
                .filter_map(Result::ok)
                .flat_map(|b| b.to_vec())
                .collect();
            (bytes, failed, outcome.load(Ordering::Acquire))
        }

        assert_eq!(
            run(&[b"ab", b"cd"], 4).await,
            (b"abcd".to_vec(), false, BODY_OK)
        );
        let (_, failed, outcome) = run(&[b"ab"], 4).await;
        assert!(failed);
        assert_eq!(outcome, BODY_LENGTH_MISMATCH);
        let (bytes, failed, outcome) = run(&[b"ab", b"cde"], 4).await;
        assert_eq!(bytes, b"ab");
        assert!(failed);
        assert_eq!(outcome, BODY_LENGTH_MISMATCH);
        // Surplus after an exact-length prefix still fails: the completing
        // chunk is held until the source ends.
        let (bytes, failed, outcome) = run(&[b"ab", b"cd", b"e"], 4).await;
        assert_eq!(bytes, b"ab");
        assert!(failed);
        assert_eq!(outcome, BODY_LENGTH_MISMATCH);
        assert_eq!(run(&[], 0).await, (Vec::new(), false, BODY_OK));
        assert_eq!(
            run(&[b"", b"abcd", b""], 4).await,
            (b"abcd".to_vec(), false, BODY_OK)
        );
    }

    #[test]
    fn part_numbers_are_bounded() {
        assert!(part_number_u16(0).is_err());
        assert!(part_number_u16(10_001).is_err());
        assert_eq!(part_number_u16(10_000).unwrap(), 10_000);
    }
}
