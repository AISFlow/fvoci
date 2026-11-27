//! The S3 file backend.

use std::ops::Range;
use std::sync::Arc;
use std::time::Duration;

use bytes::Bytes;
use futures_util::{StreamExt as _, TryStreamExt as _};
use object_store::aws::{AmazonS3, AmazonS3Builder, AmazonS3ConfigKey};
use object_store::buffered::BufWriter;
use object_store::multipart::{MultipartStore, PartId};
use object_store::path::Path;
use object_store::{
    ClientOptions, GetOptions, GetRange, ObjectStore, ObjectStoreExt as _, PutPayload,
};
use tokio::io::{AsyncRead, AsyncWriteExt as _};

use super::measure::measured;
use super::{FileStream, ObjectListing, StorageError, StoredObject};
use crate::config::S3Config;
use crate::metrics::{FilesBackend, FilesOperation, Metrics};

/// Files kept in a bucket of an S3-compatible object store, the `file_storage_s3` setting.
///
/// A stored path such as `<organization>/<board>/<secret>` ([`fvoci_model::FileKey`]) is
/// the object key, after the configured path prefix: the same key as on the disk.
#[derive(Debug, Clone)]
pub struct S3Files {
    store: Arc<dyn ObjectStore>,
    /// The same bucket, for uploads made part by part across requests.
    multipart: Arc<AmazonS3>,
    prefix: String,
    /// What the operations are measured into ([`super::FileBackend::measured`]).
    pub(super) metrics: Metrics,
    /// Metrics for multipart operations (`multipart_create`, `multipart_part`,
    /// `multipart_complete`, `multipart_abort`), kept populated even when [`Self`] sits
    /// inside `FileBackend::Faulty` whose outer wrapper measures non-multipart calls.
    multipart_metrics: Metrics,
}

impl S3Files {
    /// Creates a backend from the S3 settings.
    ///
    /// With an empty access key the credentials come from the environment: the standard
    /// `AWS_*` variables, a web identity token, or the instance metadata service.
    ///
    /// # Errors
    ///
    /// Returns [`StorageError::Config`] if no bucket is set, or the object store client
    /// cannot be built from the settings.
    pub fn new(config: &S3Config) -> Result<Self, StorageError> {
        if config.bucket.is_empty() {
            return Err(StorageError::Config(
                "file_storage_s3.bucket is empty".to_owned(),
            ));
        }

        let mut builder = if config.access_key_id.is_empty() {
            AmazonS3Builder::from_env()
        } else {
            AmazonS3Builder::new()
                .with_access_key_id(&config.access_key_id)
                .with_secret_access_key(&config.secret_access_key)
        };
        builder = builder.with_bucket_name(&config.bucket);
        if !config.region.is_empty() {
            builder = builder.with_region(&config.region);
        }
        if !config.endpoint.is_empty() {
            // A custom endpoint means an S3-compatible server, which is addressed by path
            // (`host/bucket/key`), not by a bucket subdomain.
            let scheme = if config.https { "https" } else { "http" };
            builder = builder
                .with_endpoint(format!("{scheme}://{}", config.endpoint))
                .with_virtual_hosted_style_request(false);
        }
        builder = builder.with_allow_http(!config.https);
        if config.server_side_encryption {
            // Server-side encryption with keys managed by the object store (SSE-S3).
            let key: AmazonS3ConfigKey = "aws_server_side_encryption"
                .parse()
                .map_err(|error: object_store::Error| StorageError::Config(error.to_string()))?;
            builder = builder.with_config(key, "AES256");
        }
        if let Ok(millis @ 1..) = u64::try_from(config.timeout_ms) {
            builder = builder.with_client_options(
                ClientOptions::new().with_timeout(Duration::from_millis(millis)),
            );
        }

        let store = builder
            .build()
            .map_err(|error| StorageError::Config(error.to_string()))?;
        let multipart = Arc::new(store);
        let store: Arc<dyn ObjectStore> = Arc::<AmazonS3>::clone(&multipart);
        Ok(Self {
            store,
            multipart,
            prefix: config.key_prefix.trim_matches('/').to_owned(),
            metrics: Metrics::none(),
            multipart_metrics: Metrics::none(),
        })
    }

    /// The same backend, measuring its operations into `metrics`.
    pub(super) fn measured(self, metrics: Metrics) -> Self {
        Self {
            multipart_metrics: metrics.clone(),
            metrics,
            ..self
        }
    }

    /// Sets multipart metrics alone when an outer `FileBackend::Faulty` wrapper owns
    /// non-multipart operation measurement.
    #[cfg(feature = "test-util")]
    pub(super) fn measured_multipart(self, multipart_metrics: Metrics) -> Self {
        Self {
            multipart_metrics,
            ..self
        }
    }

    /// Runs `work`, the step `operation` of an upload in parts, measured.
    async fn measure<T>(
        &self,
        operation: FilesOperation,
        work: impl Future<Output = Result<T, StorageError>>,
    ) -> Result<T, StorageError> {
        measured(&self.multipart_metrics, FilesBackend::S3, operation, work).await
    }

    /// The object key of a stored path: the configured prefix followed by the path.
    ///
    /// Characters that are not allowed in a key segment are percent-encoded by `object_store`.
    fn key(&self, path: &str) -> Result<Path, StorageError> {
        // One spelling of a place: the key of `a//b` would be that of `a/b`.
        if !fvoci_model::is_storage_path(path) {
            return Err(StorageError::InvalidPath(path.to_owned()));
        }
        Ok(if self.prefix.is_empty() {
            Path::from(path)
        } else {
            Path::from(format!("{}/{path}", self.prefix))
        })
    }

    pub(super) async fn write(
        &self,
        path: &str,
        reader: &mut (dyn AsyncRead + Send + Unpin),
    ) -> Result<u64, StorageError> {
        let mut guard = UnfinishedMultipartWrite {
            writer: Some(BufWriter::new(Arc::clone(&self.store), self.key(path)?)),
            path: path.to_owned(),
        };
        let writer = guard.writer.as_mut().expect("writer just initialized");
        let written = match tokio::io::copy(reader, writer).await {
            Ok(written) => written,
            Err(error) => {
                // A large object is uploaded in parts; without this they would stay in
                // the bucket.
                if let Some(mut w) = guard.writer.take()
                    && let Err(abort) = w.abort().await
                {
                    tracing::warn!(error = %abort, path, "cannot abort the unfinished upload");
                }
                return Err(error.into());
            }
        };
        let mut writer = guard.writer.take().expect("writer still held");
        if let Err(error) = writer.shutdown().await {
            if let Err(abort) = writer.abort().await {
                tracing::warn!(error = %abort, path, "cannot abort the unfinished upload");
            }
            return Err(error.into());
        }
        Ok(written)
    }

    /// Writes `bytes` at `path`, reads them back and removes them: whether the bucket is
    /// there and takes, gives and removes objects with these credentials. Returns what
    /// was read.
    pub(super) async fn check(
        &self,
        path: &str,
        bytes: &'static [u8],
    ) -> Result<Bytes, StorageError> {
        let key = self.key(path)?;
        self.store
            .put(&key, PutPayload::from_static(bytes))
            .await
            .map_err(StorageError::ObjectStore)?;
        let read = async { self.store.get(&key).await?.bytes().await }
            .await
            .map_err(StorageError::ObjectStore);
        // Removed also when it could not be read.
        let removed = self.delete(path).await;
        let read = read?;
        removed?;
        Ok(read)
    }

    pub(super) async fn size(&self, path: &str) -> Result<Option<u64>, StorageError> {
        match self.store.head(&self.key(path)?).await {
            Ok(meta) => Ok(Some(meta.size)),
            Err(object_store::Error::NotFound { .. }) => Ok(None),
            Err(error) => Err(StorageError::ObjectStore(error)),
        }
    }

    pub(super) async fn read(
        &self,
        path: &str,
        range: Option<Range<u64>>,
    ) -> Result<FileStream, StorageError> {
        let options = GetOptions {
            range: range.map(GetRange::Bounded),
            ..GetOptions::default()
        };
        let result = self
            .store
            .get_opts(&self.key(path)?, options)
            .await
            .map_err(|error| convert(error, path))?;
        Ok(result
            .into_stream()
            .map_err(StorageError::ObjectStore)
            .boxed())
    }

    pub(super) async fn copy(&self, from: &str, to: &str) -> Result<(), StorageError> {
        self.store
            .copy(&self.key(from)?, &self.key(to)?)
            .await
            .map_err(|error| convert(error, from))
    }

    pub(super) async fn delete(&self, path: &str) -> Result<(), StorageError> {
        match self.store.delete(&self.key(path)?).await {
            // What is gone is deleted.
            Ok(()) | Err(object_store::Error::NotFound { .. }) => Ok(()),
            Err(error) => Err(StorageError::ObjectStore(error)),
        }
    }

    pub(super) fn list(&self) -> ObjectListing {
        let prefix = (!self.prefix.is_empty()).then(|| Path::from(self.prefix.as_str()));
        // The listing names keys as the object store encodes them: the prefix too.
        let strip = prefix
            .as_ref()
            .map_or_else(String::new, |prefix| format!("{}/", prefix.as_ref()));
        self.store
            .list(prefix.as_ref())
            .map(move |meta| {
                let meta = meta.map_err(StorageError::ObjectStore)?;
                let key = meta.location.as_ref();
                Ok(StoredObject {
                    path: key.strip_prefix(&strip).unwrap_or(key).to_owned(),
                    modified_at: meta.last_modified.timestamp_millis(),
                })
            })
            .boxed()
    }

    pub(super) async fn rename(&self, from: &str, to: &str) -> Result<(), StorageError> {
        self.store
            .rename(&self.key(from)?, &self.key(to)?)
            .await
            .map_err(|error| convert(error, from))
    }
}

/// Uploads made part by part across requests. The object
/// is not there until the upload is completed. Each step is measured here, with
/// `S3Files::measure`, as the operations of every backend are in `FileBackend`.
impl S3Files {
    /// Starts an upload of the file at `path`, and returns its identifier.
    ///
    /// # Errors
    ///
    /// Returns [`StorageError::InvalidPath`] for a path that is no place of the
    /// storage, or the object store's error.
    pub async fn create_multipart(&self, path: &str) -> Result<String, StorageError> {
        self.measure(FilesOperation::MultipartCreate, async {
            self.multipart
                .create_multipart(&self.key(path)?)
                .await
                .map_err(StorageError::ObjectStore)
        })
        .await
    }

    /// Sends part `index` (from 0) of the upload, made of `chunks` in order, replacing
    /// what was sent under it, and returns the part's identifier. The object store checks
    /// the part against the SHA-256 the request is signed with.
    ///
    /// # Errors
    ///
    /// Returns [`StorageError::InvalidPath`] for a path that is no place of the
    /// storage, or the object store's error.
    pub async fn put_part(
        &self,
        path: &str,
        upload_id: &str,
        index: usize,
        chunks: Vec<Bytes>,
    ) -> Result<String, StorageError> {
        self.measure(FilesOperation::MultipartPart, async {
            let part = self
                .multipart
                .put_part(
                    &self.key(path)?,
                    &upload_id.to_owned(),
                    index,
                    chunks.into_iter().collect::<PutPayload>(),
                )
                .await
                .map_err(StorageError::ObjectStore)?;
            Ok(part.content_id)
        })
        .await
    }

    /// Completes the upload from its parts, whose identifiers are given in order, and
    /// makes the object.
    ///
    /// # Errors
    ///
    /// Returns [`StorageError::InvalidPath`] for a path that is no place of the
    /// storage, or the object store's error.
    pub async fn complete_multipart(
        &self,
        path: &str,
        upload_id: &str,
        parts: Vec<String>,
    ) -> Result<(), StorageError> {
        let parts = parts
            .into_iter()
            .map(|content_id| PartId { content_id })
            .collect();
        self.measure(FilesOperation::MultipartComplete, async {
            self.multipart
                .complete_multipart(&self.key(path)?, &upload_id.to_owned(), parts)
                .await
                .map(|_| ())
                .map_err(StorageError::ObjectStore)
        })
        .await
    }

    /// Throws the upload away with the parts sent. One that is gone is left gone.
    ///
    /// # Errors
    ///
    /// Returns [`StorageError::InvalidPath`] for a path that is no place of the
    /// storage, or the object store's error.
    pub async fn abort_multipart(&self, path: &str, upload_id: &str) -> Result<(), StorageError> {
        self.measure(FilesOperation::MultipartAbort, async {
            match self
                .multipart
                .abort_multipart(&self.key(path)?, &upload_id.to_owned())
                .await
            {
                Ok(()) | Err(object_store::Error::NotFound { .. }) => Ok(()),
                Err(error) => Err(StorageError::ObjectStore(error)),
            }
        })
        .await
    }
}

fn convert(error: object_store::Error, path: &str) -> StorageError {
    match error {
        object_store::Error::NotFound { .. } => StorageError::NotFound(path.to_owned()),
        other => StorageError::ObjectStore(other),
    }
}

/// Aborts an incomplete `object_store::buffered::BufWriter` multipart upload in the
/// background if the write future is cancelled or timed out mid-stream.
struct UnfinishedMultipartWrite {
    writer: Option<BufWriter>,
    path: String,
}

impl Drop for UnfinishedMultipartWrite {
    fn drop(&mut self) {
        let Some(mut writer) = self.writer.take() else {
            return;
        };
        let path = std::mem::take(&mut self.path);
        if let Ok(handle) = tokio::runtime::Handle::try_current() {
            handle.spawn(async move {
                if let Err(abort) = writer.abort().await {
                    tracing::warn!(error = %abort, path, "cannot abort cancelled multipart upload");
                }
            });
        }
    }
}
