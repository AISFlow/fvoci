use std::io;
use std::path::{Path, PathBuf};

use futures_util::StreamExt;
use sha2::{Digest, Sha256};
use tokio::fs;
use tokio::io::AsyncWriteExt;
use uuid::Uuid;

use super::sniff_mime_from_bytes;

const UUID_KEY_RE: &str = r"^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$";

#[derive(Debug, Clone)]
pub struct PartInfo {
    pub part_number: i32,
    pub etag: String,
    pub size_bytes: u64,
}

#[derive(Debug)]
pub struct StagedPart {
    pub etag: String,
    pub size_bytes: u64,
    writing_path: PathBuf,
}

#[derive(Debug, thiserror::Error)]
pub enum StorageError {
    #[error("storage key rejected")]
    InvalidKey,
    #[error("upload gone")]
    UploadGone,
    #[error("part too large")]
    PartTooLarge,
    #[error("etag mismatch")]
    EtagMismatch,
    #[error("io error: {0}")]
    Io(#[from] io::Error),
}

#[derive(Clone)]
pub struct LocalStorage {
    root: PathBuf,
}

impl LocalStorage {
    pub fn new(root: PathBuf) -> Self {
        Self { root }
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    pub fn assert_key(key: &str) -> Result<(), StorageError> {
        let lower = key.to_ascii_lowercase();
        let re = regex::Regex::new(UUID_KEY_RE).expect("uuid regex");
        if re.is_match(&lower) {
            Ok(())
        } else {
            Err(StorageError::InvalidKey)
        }
    }

    fn objects_dir(&self, key: &str) -> PathBuf {
        self.root.join("objects").join(key)
    }

    fn parts_dir(&self, key: &str) -> PathBuf {
        self.root.join("tmp").join(key)
    }

    fn object_path(&self, key: &str) -> PathBuf {
        self.objects_dir(key).join("payload")
    }

    pub async fn create_multipart(&self, key: &str) -> Result<(), StorageError> {
        Self::assert_key(key)?;
        fs::create_dir_all(self.parts_dir(key)).await?;
        Ok(())
    }

    pub async fn abort_multipart(&self, key: &str) -> Result<(), StorageError> {
        Self::assert_key(key)?;
        let dir = self.parts_dir(key);
        if fs::metadata(&dir).await.is_ok() {
            fs::remove_dir_all(&dir).await?;
        }
        Ok(())
    }

    pub async fn delete_object(&self, key: &str) -> Result<(), StorageError> {
        Self::assert_key(key)?;
        let dir = self.objects_dir(key);
        if fs::metadata(&dir).await.is_ok() {
            fs::remove_dir_all(&dir).await?;
        }
        let parts = self.parts_dir(key);
        if fs::metadata(&parts).await.is_ok() {
            fs::remove_dir_all(&parts).await?;
        }
        Ok(())
    }

    pub async fn stage_part_stream<S, E>(
        &self,
        key: &str,
        part_number: i32,
        mut stream: S,
        max_bytes: u64,
    ) -> Result<StagedPart, StorageError>
    where
        S: futures_util::Stream<Item = Result<bytes::Bytes, E>> + Unpin,
        E: Into<Box<dyn std::error::Error + Send + Sync>>,
    {
        Self::assert_key(key)?;
        if part_number < 1 || part_number > 10_000 {
            return Err(StorageError::InvalidKey);
        }
        let dir = self.parts_dir(key);
        if fs::metadata(&dir).await.is_err() {
            return Err(StorageError::UploadGone);
        }
        let writing_path = dir.join(format!("{part_number}.{writing}.writing", writing = Uuid::now_v7()));
        let mut file = fs::File::create(&writing_path).await?;
        let mut hasher = Sha256::new();
        let mut size_bytes = 0u64;
        while let Some(chunk) = stream.next().await {
            let chunk = match chunk {
                Ok(chunk) => chunk,
                Err(err) => {
                    let _ = fs::remove_file(&writing_path).await;
                    return Err(StorageError::Io(io::Error::other(err)));
                }
            };
            size_bytes += chunk.len() as u64;
            if size_bytes > max_bytes {
                let _ = fs::remove_file(&writing_path).await;
                return Err(StorageError::PartTooLarge);
            }
            hasher.update(&chunk);
            if let Err(err) = file.write_all(&chunk).await {
                let _ = fs::remove_file(&writing_path).await;
                return Err(StorageError::Io(err));
            }
        }
        file.flush().await?;
        let etag = hex::encode(hasher.finalize());
        Ok(StagedPart {
            etag,
            size_bytes,
            writing_path,
        })
    }

    pub async fn publish_staged_part(
        &self,
        key: &str,
        part_number: i32,
        staged: &StagedPart,
    ) -> Result<PartInfo, StorageError> {
        Self::assert_key(key)?;
        let dir = self.parts_dir(key);
        if fs::metadata(&dir).await.is_err() {
            let _ = fs::remove_file(&staged.writing_path).await;
            return Err(StorageError::UploadGone);
        }
        let final_path = dir.join(part_number.to_string());
        if let Err(err) = fs::rename(&staged.writing_path, &final_path).await {
            let _ = fs::remove_file(&staged.writing_path).await;
            return Err(if err.kind() == io::ErrorKind::NotFound {
                StorageError::UploadGone
            } else {
                StorageError::Io(err)
            });
        }
        Ok(PartInfo {
            part_number,
            etag: staged.etag.clone(),
            size_bytes: staged.size_bytes,
        })
    }

    pub async fn discard_staged_part(staged: &StagedPart) {
        let _ = fs::remove_file(&staged.writing_path).await;
    }

    pub async fn list_parts(&self, key: &str) -> Result<Vec<PartInfo>, StorageError> {
        Self::assert_key(key)?;
        let dir = self.parts_dir(key);
        if fs::metadata(&dir).await.is_err() {
            return Err(StorageError::UploadGone);
        }
        let mut parts = Vec::new();
        let mut entries = fs::read_dir(&dir).await?;
        while let Some(entry) = entries.next_entry().await? {
            let name = entry.file_name().to_string_lossy().to_string();
            if !name.chars().all(|c| c.is_ascii_digit()) {
                continue;
            }
            let part_number = name.parse::<i32>().unwrap_or(0);
            if part_number < 1 {
                continue;
            }
            let data = fs::read(entry.path()).await?;
            let etag = hex::encode(Sha256::digest(&data));
            parts.push(PartInfo {
                part_number,
                etag,
                size_bytes: data.len() as u64,
            });
        }
        parts.sort_by_key(|p| p.part_number);
        Ok(parts)
    }

    pub async fn payload_exists(&self, key: &str) -> Result<bool, StorageError> {
        Self::assert_key(key)?;
        match fs::metadata(self.object_path(key)).await {
            Ok(_) => Ok(true),
            Err(err) if err.kind() == io::ErrorKind::NotFound => Ok(false),
            Err(err) => Err(StorageError::Io(err)),
        }
    }

    pub async fn assemble_multipart(
        &self,
        key: &str,
        submitted: &[(i32, String)],
    ) -> Result<u64, StorageError> {
        Self::assert_key(key)?;
        if self.payload_exists(key).await? {
            return self.head(key).await?.ok_or(StorageError::UploadGone);
        }
        let actual = self.list_parts(key).await?;
        let mut submitted = submitted.to_vec();
        submitted.sort_by_key(|(n, _)| *n);
        let matches = actual.len() == submitted.len()
            && actual
                .iter()
                .zip(submitted.iter())
                .all(|(a, (n, etag))| a.part_number == *n && a.etag == *etag);
        if !matches {
            return Err(StorageError::EtagMismatch);
        }
        fs::create_dir_all(self.objects_dir(key)).await?;
        let assembly_path = self.object_path(key).with_extension("assembly");
        let mut out = fs::File::create(&assembly_path).await?;
        let mut size_bytes = 0u64;
        for part in &actual {
            let path = self.parts_dir(key).join(part.part_number.to_string());
            let data = fs::read(&path).await?;
            let etag = hex::encode(Sha256::digest(&data));
            if etag != part.etag {
                let _ = fs::remove_file(&assembly_path).await;
                return Err(StorageError::EtagMismatch);
            }
            size_bytes += data.len() as u64;
            out.write_all(&data).await?;
        }
        out.flush().await?;
        out.sync_all().await?;
        fs::rename(&assembly_path, self.object_path(key)).await?;
        Ok(size_bytes)
    }

    pub async fn finalize_multipart(&self, key: &str) -> Result<(), StorageError> {
        Self::assert_key(key)?;
        let parts = self.parts_dir(key);
        if fs::metadata(&parts).await.is_ok() {
            fs::remove_dir_all(&parts).await?;
        }
        Ok(())
    }

    pub async fn head(&self, key: &str) -> Result<Option<u64>, StorageError> {
        Self::assert_key(key)?;
        match fs::metadata(self.object_path(key)).await {
            Ok(meta) => Ok(Some(meta.len())),
            Err(err) if err.kind() == io::ErrorKind::NotFound => Ok(None),
            Err(err) => Err(StorageError::Io(err)),
        }
    }

    pub async fn read_range(
        &self,
        key: &str,
        start: u64,
        end: u64,
    ) -> Result<Vec<u8>, StorageError> {
        Self::assert_key(key)?;
        let len = end - start + 1;
        let mut file = self.open_payload_at(key, start).await?;
        let mut buf = vec![0u8; len as usize];
        use tokio::io::AsyncReadExt;
        file.read_exact(&mut buf).await?;
        Ok(buf)
    }

    pub async fn open_payload_at(
        &self,
        key: &str,
        start: u64,
    ) -> Result<tokio::fs::File, StorageError> {
        Self::assert_key(key)?;
        let path = self.object_path(key);
        let mut file = fs::File::open(&path).await?;
        use tokio::io::{AsyncSeekExt, SeekFrom};
        file.seek(SeekFrom::Start(start)).await?;
        Ok(file)
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
}
