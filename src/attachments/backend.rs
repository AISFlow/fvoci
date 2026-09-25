use std::io;
use std::path::PathBuf;
use std::pin::Pin;
use std::sync::Arc;
use std::task::{Context, Poll};

use bytes::Bytes;
use futures_util::{Stream, StreamExt};
use tokio::io::AsyncReadExt;
use tokio_util::io::ReaderStream;

use super::local::{LocalStorage, PartInfo, StagedPart, StorageError};
use super::s3::S3Storage;
use crate::config::StorageSettings;

/// Local or S3-compatible attachment bytes, sharing the multipart contract
/// used by `LocalStorage`.
#[derive(Clone)]
pub enum ObjectStorage {
    Local(LocalStorage),
    S3(Arc<S3Storage>),
}

pub struct ObjectBody {
    inner: Pin<Box<dyn Stream<Item = Result<Bytes, io::Error>> + Send>>,
}

impl Stream for ObjectBody {
    type Item = Result<Bytes, io::Error>;

    fn poll_next(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        self.inner.as_mut().poll_next(cx)
    }
}

impl ObjectStorage {
    pub fn local(root: PathBuf) -> Self {
        Self::Local(LocalStorage::new(root))
    }

    pub fn from_settings(settings: &StorageSettings) -> Result<Self, String> {
        match settings {
            StorageSettings::Local { root } => Ok(Self::local(root.clone())),
            StorageSettings::S3(s3) => Ok(Self::S3(Arc::new(S3Storage::new(s3.clone())?))),
        }
    }

    pub async fn create_multipart(&self, key: &str) -> Result<Option<String>, StorageError> {
        match self {
            Self::Local(local) => {
                local.create_multipart(key).await?;
                Ok(None)
            }
            Self::S3(s3) => Ok(Some(s3.create_multipart(key).await?)),
        }
    }

    pub async fn abort_multipart(
        &self,
        key: &str,
        upload_ref: Option<&str>,
    ) -> Result<(), StorageError> {
        match self {
            Self::Local(local) => local.abort_multipart(key).await,
            Self::S3(s3) => s3.abort_multipart(key, upload_ref).await,
        }
    }

    pub async fn delete_object(&self, key: &str) -> Result<(), StorageError> {
        match self {
            Self::Local(local) => local.delete_object(key).await,
            Self::S3(s3) => s3.delete_object(key).await,
        }
    }

    /// Stages one part body. `declared_len` is the request's declared body
    /// length; a length above `max_bytes` is refused before any byte is read.
    pub async fn stage_part_stream<S, E>(
        &self,
        key: &str,
        upload_ref: Option<&str>,
        part_number: i32,
        stream: S,
        declared_len: Option<u64>,
        max_bytes: u64,
    ) -> Result<StagedPart, StorageError>
    where
        S: Stream<Item = Result<Bytes, E>> + Unpin + Send + 'static,
        E: Into<Box<dyn std::error::Error + Send + Sync>> + 'static,
    {
        if declared_len.is_some_and(|len| len > max_bytes) {
            return Err(StorageError::PartTooLarge);
        }
        match self {
            Self::Local(local) => {
                let _ = upload_ref;
                local
                    .stage_part_stream(key, part_number, stream, max_bytes)
                    .await
            }
            Self::S3(s3) => {
                // The body streams straight into UploadPart with its declared
                // length, so no part is held in memory. Like the source's
                // presigned PUTs, an uploaded part stays invisible until
                // `CompleteMultipartUpload` names it.
                let len = declared_len.ok_or(StorageError::LengthRequired)?;
                let upload_id = upload_ref
                    .filter(|id| !id.is_empty())
                    .ok_or(StorageError::UploadGone)?;
                let part = s3
                    .upload_part_stream(key, upload_id, part_number, stream, len)
                    .await?;
                Ok(StagedPart::uploaded(part.etag, part.size_bytes))
            }
        }
    }

    pub async fn publish_staged_part(
        &self,
        key: &str,
        part_number: i32,
        staged: &mut StagedPart,
    ) -> Result<PartInfo, StorageError> {
        match self {
            Self::Local(local) => local.publish_staged_part(key, part_number, staged).await,
            Self::S3(_) => Ok(PartInfo {
                part_number,
                etag: staged.etag.clone(),
                size_bytes: staged.size_bytes,
            }),
        }
    }

    pub async fn discard_staged_part(staged: &mut StagedPart) {
        staged.discard().await;
    }

    pub async fn list_parts(
        &self,
        key: &str,
        upload_ref: Option<&str>,
    ) -> Result<Vec<PartInfo>, StorageError> {
        match self {
            Self::Local(local) => local.list_parts(key).await,
            Self::S3(s3) => s3.list_parts(key, upload_ref).await,
        }
    }

    pub async fn list_multipart_uploads(
        &self,
        key: &str,
    ) -> Result<Vec<Option<String>>, StorageError> {
        match self {
            Self::Local(local) => local.list_multipart_uploads(key).await,
            Self::S3(s3) => s3.list_multipart_uploads(key).await,
        }
    }

    /// Aborts every open multipart upload that could still publish `key`
    /// (the row's own and any orphan whose id was never persisted), then
    /// deletes the object. A missing upload or object counts as done; any
    /// other storage error is returned so the caller keeps its DB row.
    pub async fn purge_key(&self, key: &str) -> Result<(), StorageError> {
        for upload_ref in self.list_multipart_uploads(key).await? {
            self.abort_multipart(key, upload_ref.as_deref()).await?;
        }
        self.delete_object(key).await
    }

    pub async fn payload_exists(&self, key: &str) -> Result<bool, StorageError> {
        Ok(self.head(key).await?.is_some())
    }

    pub async fn discard_uncommitted_payload(&self, key: &str) -> Result<(), StorageError> {
        match self {
            Self::Local(local) => local.discard_uncommitted_payload(key).await,
            // S3 complete publishes the object atomically; deleting first would
            // destroy a successful complete that has not yet marked the row stored.
            Self::S3(_) => {
                let _ = key;
                Ok(())
            }
        }
    }

    pub async fn assemble_multipart(
        &self,
        key: &str,
        upload_ref: Option<&str>,
        submitted: &[(i32, String)],
    ) -> Result<u64, StorageError> {
        match self {
            Self::Local(local) => local.assemble_multipart(key, submitted).await,
            Self::S3(s3) => s3.complete_multipart(key, upload_ref, submitted).await,
        }
    }

    pub async fn finalize_multipart(&self, key: &str) -> Result<(), StorageError> {
        match self {
            Self::Local(local) => local.finalize_multipart(key).await,
            Self::S3(_) => {
                let _ = key;
                Ok(())
            }
        }
    }

    pub async fn head(&self, key: &str) -> Result<Option<u64>, StorageError> {
        match self {
            Self::Local(local) => local.head(key).await,
            Self::S3(s3) => s3.head(key).await,
        }
    }

    pub async fn read_range(
        &self,
        key: &str,
        start: u64,
        end: u64,
    ) -> Result<Vec<u8>, StorageError> {
        match self {
            Self::Local(local) => local.read_range(key, start, end).await,
            Self::S3(s3) => s3.read_range(key, start, end).await,
        }
    }

    pub async fn open_payload_stream(
        &self,
        key: &str,
        start: u64,
        end: u64,
    ) -> Result<ObjectBody, StorageError> {
        let len = end.saturating_sub(start).saturating_add(1);
        match self {
            Self::Local(local) => {
                let file = local.open_payload_at(key, start).await?;
                let stream = ReaderStream::with_capacity(file.take(len), 64 * 1024);
                Ok(ObjectBody {
                    inner: Box::pin(stream),
                })
            }
            Self::S3(s3) => {
                let response = s3.get_object(key, start, end).await?;
                let stream = response
                    .bytes_stream()
                    .map(|chunk| chunk.map_err(io::Error::other));
                Ok(ObjectBody {
                    inner: Box::pin(stream),
                })
            }
        }
    }

    pub async fn sniff_mime(&self, key: &str) -> Result<String, StorageError> {
        match self {
            Self::Local(local) => local.sniff_mime(key).await,
            Self::S3(s3) => s3.sniff_mime(key).await,
        }
    }

    pub async fn probe(&self) -> Result<(), String> {
        match self {
            Self::Local(_) => Ok(()),
            Self::S3(s3) => s3.head_bucket().await.map_err(|err| {
                format!(
                    "storage probe failed: HeadBucket on \"{}\" at {} — {err}",
                    s3.bucket_name(),
                    s3.endpoint_url()
                )
            }),
        }
    }
}

impl From<S3Storage> for ObjectStorage {
    fn from(s3: S3Storage) -> Self {
        Self::S3(Arc::new(s3))
    }
}

impl From<LocalStorage> for ObjectStorage {
    fn from(local: LocalStorage) -> Self {
        Self::Local(local)
    }
}
