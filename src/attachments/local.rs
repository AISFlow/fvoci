use std::io;
use std::path::{Path, PathBuf};
use std::sync::LazyLock;

use futures_util::StreamExt;
use sha2::{Digest, Sha256};
use tokio::fs;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use uuid::Uuid;

use super::sniff_mime_from_bytes;

static UUID_KEY_RE: LazyLock<regex::Regex> = LazyLock::new(|| {
    regex::Regex::new(r"^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$")
        .expect("uuid regex")
});

const COPY_BUF: usize = 64 * 1024;

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
    /// Local temp file awaiting publish; `None` once the bytes already live
    /// in remote multipart storage.
    writing_path: Option<PathBuf>,
    keep: bool,
}

impl StagedPart {
    /// A part the remote store already holds under its multipart upload.
    pub(crate) fn uploaded(etag: String, size_bytes: u64) -> Self {
        Self {
            etag,
            size_bytes,
            writing_path: None,
            keep: true,
        }
    }

    pub async fn discard(&mut self) {
        if let Some(path) = &self.writing_path {
            let _ = fs::remove_file(path).await;
        }
        self.keep = true;
    }
}

struct WritingGuard {
    path: PathBuf,
    keep: bool,
}

impl WritingGuard {
    fn new(path: PathBuf) -> Self {
        Self { path, keep: false }
    }

    fn disarm(&mut self) {
        self.keep = true;
    }
}

impl Drop for WritingGuard {
    fn drop(&mut self) {
        if !self.keep {
            let _ = std::fs::remove_file(&self.path);
        }
    }
}

/// Create the temp file on the blocking pool while that same closure owns the
/// unlink guard. If the JoinHandle is dropped, the still-running closure keeps
/// the guard until IO finishes and dropping the output unlinks the path.
async fn create_writing_file(path: PathBuf) -> io::Result<(fs::File, WritingGuard)> {
    let join = tokio::task::spawn_blocking(move || -> io::Result<(std::fs::File, WritingGuard)> {
        let guard = WritingGuard::new(path.clone());
        #[cfg(test)]
        let created_tx = delayed_create::wait_before_create(&path);
        let file = std::fs::File::create(&path)?;
        #[cfg(test)]
        if let Some(tx) = created_tx {
            let _ = tx.send(());
        }
        Ok((file, guard))
    });
    let (std_file, guard) = join.await.map_err(io::Error::other)??;
    Ok((fs::File::from_std(std_file), guard))
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
        if UUID_KEY_RE.is_match(&lower) {
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
        durable_create_dir_all(&self.parts_dir(key)).await?;
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
        if !(1..=10_000).contains(&part_number) {
            return Err(StorageError::InvalidKey);
        }
        let dir = self.parts_dir(key);
        if fs::metadata(&dir).await.is_err() {
            return Err(StorageError::UploadGone);
        }
        let writing_path = dir.join(format!(
            "{part_number}.{writing}.writing",
            writing = Uuid::now_v7()
        ));
        let (mut file, mut guard) = create_writing_file(writing_path.clone()).await?;
        let mut hasher = Sha256::new();
        let mut size_bytes = 0u64;
        while let Some(chunk) = stream.next().await {
            let chunk = match chunk {
                Ok(chunk) => chunk,
                Err(err) => {
                    return Err(StorageError::Io(io::Error::other(err)));
                }
            };
            size_bytes += chunk.len() as u64;
            if size_bytes > max_bytes {
                return Err(StorageError::PartTooLarge);
            }
            hasher.update(&chunk);
            file.write_all(&chunk).await?;
        }
        file.flush().await?;
        file.sync_all().await?;
        drop(file);
        let etag = hex::encode(hasher.finalize());
        guard.disarm();
        Ok(StagedPart {
            etag,
            size_bytes,
            writing_path: Some(writing_path),
            keep: false,
        })
    }

    pub async fn publish_staged_part(
        &self,
        key: &str,
        part_number: i32,
        staged: &mut StagedPart,
    ) -> Result<PartInfo, StorageError> {
        Self::assert_key(key)?;
        let dir = self.parts_dir(key);
        let writing_path = staged
            .writing_path
            .clone()
            .ok_or(StorageError::UploadGone)?;
        if fs::metadata(&dir).await.is_err() {
            let _ = fs::remove_file(&writing_path).await;
            return Err(StorageError::UploadGone);
        }
        let final_path = dir.join(part_number.to_string());
        if let Err(err) = durable_rename(&writing_path, &final_path).await {
            let _ = fs::remove_file(&writing_path).await;
            return Err(if err.kind() == io::ErrorKind::NotFound {
                StorageError::UploadGone
            } else {
                StorageError::Io(err)
            });
        }
        staged.keep = true;
        Ok(PartInfo {
            part_number,
            etag: staged.etag.clone(),
            size_bytes: staged.size_bytes,
        })
    }

    pub async fn discard_staged_part(staged: &mut StagedPart) {
        staged.discard().await;
    }

    pub async fn list_multipart_uploads(
        &self,
        key: &str,
    ) -> Result<Vec<Option<String>>, StorageError> {
        Self::assert_key(key)?;
        match fs::metadata(self.parts_dir(key)).await {
            Ok(_) => Ok(vec![None]),
            Err(err) if err.kind() == io::ErrorKind::NotFound => Ok(Vec::new()),
            Err(err) => Err(StorageError::Io(err)),
        }
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
            let (etag, size_bytes) = hash_file(&entry.path()).await?;
            parts.push(PartInfo {
                part_number,
                etag,
                size_bytes,
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

    pub async fn discard_uncommitted_payload(&self, key: &str) -> Result<(), StorageError> {
        Self::assert_key(key)?;
        let dir = self.objects_dir(key);
        match fs::remove_dir_all(&dir).await {
            Ok(()) => {}
            Err(err) if err.kind() == io::ErrorKind::NotFound => {}
            Err(err) => return Err(StorageError::Io(err)),
        }
        // Also sync on retry after an interrupted removal: absence in memory
        // does not establish that the directory entry deletion is durable.
        match fsync_dir(&self.root.join("objects")).await {
            Ok(()) => Ok(()),
            Err(err) if err.kind() == io::ErrorKind::NotFound => Ok(()),
            Err(err) => Err(StorageError::Io(err)),
        }
    }

    pub async fn assemble_multipart(
        &self,
        key: &str,
        submitted: &[(i32, String)],
    ) -> Result<u64, StorageError> {
        Self::assert_key(key)?;
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
        if self.payload_exists(key).await? {
            return self.head(key).await?.ok_or(StorageError::UploadGone);
        }
        durable_create_dir_all(&self.objects_dir(key)).await?;
        let assembly_path = self.object_path(key).with_extension("assembly");
        let (mut out, mut assembly_guard) = create_writing_file(assembly_path.clone()).await?;
        let mut size_bytes = 0u64;
        for part in &actual {
            let path = self.parts_dir(key).join(part.part_number.to_string());
            let copied = copy_hashed(&path, &mut out).await;
            match copied {
                Ok((etag, copied_bytes)) if etag == part.etag => {
                    size_bytes += copied_bytes;
                }
                Ok(_) => {
                    drop(out);
                    return Err(StorageError::EtagMismatch);
                }
                Err(err) => {
                    drop(out);
                    return Err(err);
                }
            }
        }
        out.flush().await?;
        out.sync_all().await?;
        drop(out);
        durable_rename(&assembly_path, &self.object_path(key)).await?;
        assembly_guard.disarm();
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

async fn fsync_dir(path: &Path) -> io::Result<()> {
    #[cfg(test)]
    delayed_create::record_fsync(path);
    let dir = fs::File::open(path).await?;
    dir.sync_all().await
}

async fn durable_create_dir_all(path: &Path) -> io::Result<()> {
    fs::create_dir_all(path).await?;
    let abs = if path.is_absolute() {
        path.to_path_buf()
    } else {
        std::env::current_dir()?.join(path)
    };
    let mut cursor = fs::canonicalize(&abs).await.unwrap_or(abs);
    loop {
        if cursor.as_os_str().is_empty() {
            break;
        }
        fsync_dir(&cursor).await?;
        match cursor.parent() {
            Some(parent) if parent != cursor.as_path() && !parent.as_os_str().is_empty() => {
                cursor = parent.to_path_buf();
            }
            _ => break,
        }
    }
    Ok(())
}

async fn durable_rename(from: &Path, to: &Path) -> io::Result<()> {
    fs::rename(from, to).await?;
    if let Some(parent) = to.parent() {
        fsync_dir(parent).await?;
    }
    Ok(())
}

async fn hash_file(path: &Path) -> Result<(String, u64), StorageError> {
    let mut file = fs::File::open(path).await?;
    let mut hasher = Sha256::new();
    let mut buf = vec![0u8; COPY_BUF];
    let mut size_bytes = 0u64;
    loop {
        let n = file.read(&mut buf).await?;
        if n == 0 {
            break;
        }
        hasher.update(&buf[..n]);
        size_bytes += n as u64;
    }
    Ok((hex::encode(hasher.finalize()), size_bytes))
}

async fn copy_hashed(path: &Path, out: &mut fs::File) -> Result<(String, u64), StorageError> {
    let mut file = fs::File::open(path).await?;
    let mut hasher = Sha256::new();
    let mut buf = vec![0u8; COPY_BUF];
    let mut size_bytes = 0u64;
    loop {
        let n = file.read(&mut buf).await?;
        if n == 0 {
            break;
        }
        hasher.update(&buf[..n]);
        out.write_all(&buf[..n]).await?;
        size_bytes += n as u64;
    }
    Ok((hex::encode(hasher.finalize()), size_bytes))
}

impl Drop for StagedPart {
    fn drop(&mut self) {
        if !self.keep {
            if let Some(path) = &self.writing_path {
                let _ = std::fs::remove_file(path);
            }
        }
    }
}

#[cfg(test)]
mod delayed_create {
    use super::*;
    use std::collections::HashMap;
    use std::sync::mpsc;
    use std::sync::Mutex;
    use std::time::Duration;

    struct CreateGate {
        entered_tx: tokio::sync::oneshot::Sender<()>,
        proceed_rx: mpsc::Receiver<()>,
        created_tx: tokio::sync::oneshot::Sender<()>,
    }

    static GATES: LazyLock<Mutex<HashMap<PathBuf, CreateGate>>> =
        LazyLock::new(|| Mutex::new(HashMap::new()));
    static FSYNCS: LazyLock<Mutex<HashMap<PathBuf, Vec<PathBuf>>>> =
        LazyLock::new(|| Mutex::new(HashMap::new()));

    pub struct DelayedCreateBarrier {
        entered: Option<tokio::sync::oneshot::Receiver<()>>,
        proceed: Option<mpsc::SyncSender<()>>,
        created: Option<tokio::sync::oneshot::Receiver<()>>,
    }

    pub fn arm_for_dir(dir: PathBuf) -> DelayedCreateBarrier {
        let (entered_tx, entered_rx) = tokio::sync::oneshot::channel();
        let (proceed_tx, proceed_rx) = mpsc::sync_channel(1);
        let (created_tx, created_rx) = tokio::sync::oneshot::channel();
        GATES.lock().expect("create gate").insert(
            dir,
            CreateGate {
                entered_tx,
                proceed_rx,
                created_tx,
            },
        );
        DelayedCreateBarrier {
            entered: Some(entered_rx),
            proceed: Some(proceed_tx),
            created: Some(created_rx),
        }
    }

    pub fn wait_before_create(path: &Path) -> Option<tokio::sync::oneshot::Sender<()>> {
        let parent = path.parent()?.to_path_buf();
        let gate = GATES.lock().ok()?.remove(&parent)?;
        let _ = gate.entered_tx.send(());
        let _ = gate.proceed_rx.recv_timeout(Duration::from_secs(30));
        Some(gate.created_tx)
    }

    impl DelayedCreateBarrier {
        pub async fn wait_entered(&mut self) {
            let rx = self.entered.take().expect("entered receiver");
            rx.await
                .expect("create gate must signal entered; dropped sender is not success");
        }

        pub fn proceed(&mut self) {
            if let Some(tx) = self.proceed.take() {
                let _ = tx.send(());
            }
        }

        pub async fn wait_create_finished(&mut self) {
            let rx = self.created.take().expect("created receiver");
            rx.await
                .expect("create must complete after proceed; dropped sender is not success");
        }
    }

    impl Drop for DelayedCreateBarrier {
        fn drop(&mut self) {
            self.proceed();
        }
    }

    pub fn capture_fsyncs(root: PathBuf) {
        FSYNCS.lock().expect("fsync log").insert(root, Vec::new());
    }

    pub fn record_fsync(path: &Path) {
        let Ok(mut map) = FSYNCS.lock() else {
            return;
        };
        for (root, paths) in map.iter_mut() {
            if path.starts_with(root) || root.starts_with(path) {
                paths.push(path.to_path_buf());
            }
        }
    }

    pub fn take_fsyncs(root: &Path) -> Vec<PathBuf> {
        FSYNCS
            .lock()
            .expect("fsync log")
            .remove(root)
            .unwrap_or_default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use futures_util::stream;
    use futures_util::StreamExt;
    use std::time::Duration;

    fn temp_storage() -> (LocalStorage, PathBuf) {
        let root = std::env::temp_dir().join(format!("fvoci-att-local-{}", Uuid::now_v7()));
        std::fs::create_dir_all(&root).unwrap();
        (LocalStorage::new(root.clone()), root)
    }

    async fn writing_names(dir: &Path) -> Vec<String> {
        let mut names = Vec::new();
        let mut entries = fs::read_dir(dir).await.unwrap();
        while let Some(entry) = entries.next_entry().await.unwrap() {
            let name = entry.file_name().to_string_lossy().to_string();
            if name.ends_with(".writing") {
                names.push(name);
            }
        }
        names
    }

    #[tokio::test]
    async fn cancelled_stage_removes_writing_and_keeps_published_part() {
        let (storage, _root) = temp_storage();
        let key = Uuid::now_v7().to_string();
        storage.create_multipart(&key).await.unwrap();
        let mut first = storage
            .stage_part_stream(
                &key,
                1,
                stream::iter(vec![Ok::<bytes::Bytes, std::io::Error>(
                    bytes::Bytes::from_static(b"keep-me"),
                )]),
                32,
            )
            .await
            .unwrap();
        storage
            .publish_staged_part(&key, 1, &mut first)
            .await
            .unwrap();

        let hanging = stream::iter(vec![Ok::<bytes::Bytes, std::io::Error>(
            bytes::Bytes::from_static(b"xx"),
        )])
        .chain(futures_util::stream::pending());
        let staged = tokio::spawn({
            let storage = storage.clone();
            let key = key.clone();
            async move { storage.stage_part_stream(&key, 2, hanging, 32).await }
        });
        let dir = storage.parts_dir(&key);
        tokio::time::timeout(Duration::from_secs(5), async {
            loop {
                if !writing_names(&dir).await.is_empty() {
                    break;
                }
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("writing temp should appear");
        staged.abort();
        let _ = staged.await;
        assert!(
            writing_names(&dir).await.is_empty(),
            "cancelled stage must remove .writing"
        );
        let parts = storage.list_parts(&key).await.unwrap();
        assert_eq!(parts.len(), 1);
        assert_eq!(parts[0].part_number, 1);
        assert_eq!(parts[0].size_bytes, 7);
    }

    #[tokio::test]
    async fn stream_error_flushes_cleanup_writing() {
        let (storage, _root) = temp_storage();
        let key = Uuid::now_v7().to_string();
        storage.create_multipart(&key).await.unwrap();
        let err = storage
            .stage_part_stream(
                &key,
                1,
                stream::iter(vec![
                    Ok::<bytes::Bytes, std::io::Error>(bytes::Bytes::from_static(b"aa")),
                    Err(io::Error::other("boom")),
                ]),
                32,
            )
            .await
            .unwrap_err();
        assert!(matches!(err, StorageError::Io(_)));
        assert!(writing_names(&storage.parts_dir(&key)).await.is_empty());
        assert!(storage.list_parts(&key).await.unwrap().is_empty());
    }

    #[tokio::test]
    async fn delayed_create_cancellation_unlinks_after_pending_create() {
        let (storage, _root) = temp_storage();
        let key = Uuid::now_v7().to_string();
        storage.create_multipart(&key).await.unwrap();
        let dir = storage.parts_dir(&key);
        let mut barrier = delayed_create::arm_for_dir(dir.clone());
        let hanging = stream::iter(vec![Ok::<bytes::Bytes, std::io::Error>(
            bytes::Bytes::from_static(b"xx"),
        )])
        .chain(futures_util::stream::pending());
        let staged = tokio::spawn({
            let storage = storage.clone();
            let key = key.clone();
            async move { storage.stage_part_stream(&key, 1, hanging, 32).await }
        });
        tokio::time::timeout(Duration::from_secs(5), barrier.wait_entered())
            .await
            .expect("create should reach delayed gate");
        assert!(
            writing_names(&dir).await.is_empty(),
            "file must not exist while create is gated"
        );
        staged.abort();
        let _ = staged.await;
        barrier.proceed();
        tokio::time::timeout(Duration::from_secs(5), barrier.wait_create_finished())
            .await
            .expect("blocked create should complete after the barrier is released");
        tokio::time::timeout(Duration::from_secs(5), async {
            loop {
                if writing_names(&dir).await.is_empty() {
                    break;
                }
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("spawn_blocking output drop must unlink after create finishes");
        assert!(storage.list_parts(&key).await.unwrap().is_empty());
    }

    async fn publish_one(storage: &LocalStorage, key: &str, body: &'static [u8]) -> PartInfo {
        let mut staged = storage
            .stage_part_stream(
                key,
                1,
                stream::iter(vec![Ok::<bytes::Bytes, std::io::Error>(
                    bytes::Bytes::from_static(body),
                )]),
                body.len() as u64,
            )
            .await
            .unwrap();
        storage
            .publish_staged_part(key, 1, &mut staged)
            .await
            .unwrap()
    }

    #[tokio::test]
    async fn assemble_shortcut_rejects_wrong_etags_when_payload_exists() {
        let (storage, _root) = temp_storage();
        let key = Uuid::now_v7().to_string();
        storage.create_multipart(&key).await.unwrap();
        let part = publish_one(&storage, &key, b"payload-a").await;
        let assembled = storage
            .assemble_multipart(&key, &[(part.part_number, part.etag.clone())])
            .await
            .unwrap();
        assert_eq!(assembled, 9);
        let err = storage
            .assemble_multipart(&key, &[(1, "deadbeef".into())])
            .await
            .unwrap_err();
        assert!(matches!(err, StorageError::EtagMismatch));
        assert_eq!(storage.head(&key).await.unwrap(), Some(9));
    }

    #[tokio::test]
    async fn assemble_shortcut_reuses_payload_only_after_etag_match() {
        let (storage, _root) = temp_storage();
        let key = Uuid::now_v7().to_string();
        storage.create_multipart(&key).await.unwrap();
        let part = publish_one(&storage, &key, b"payload-b").await;
        storage
            .assemble_multipart(&key, &[(part.part_number, part.etag.clone())])
            .await
            .unwrap();
        let reused = storage
            .assemble_multipart(&key, &[(part.part_number, part.etag.clone())])
            .await
            .unwrap();
        assert_eq!(reused, 9);
        assert_eq!(storage.read_range(&key, 0, 8).await.unwrap(), b"payload-b");
    }

    #[tokio::test]
    async fn discard_uncommitted_payload_prevents_stale_object_substitution() {
        let (storage, _root) = temp_storage();
        let key = Uuid::now_v7().to_string();
        storage.create_multipart(&key).await.unwrap();
        let part = publish_one(&storage, &key, b"current").await;
        durable_create_dir_all(&storage.objects_dir(&key))
            .await
            .unwrap();
        fs::write(storage.object_path(&key), b"STALEOBJ")
            .await
            .unwrap();
        let capture = fs::canonicalize(&storage.root).await.unwrap();
        delayed_create::capture_fsyncs(capture.clone());
        storage.discard_uncommitted_payload(&key).await.unwrap();
        storage.discard_uncommitted_payload(&key).await.unwrap();
        let synced = delayed_create::take_fsyncs(&capture);
        assert_eq!(
            synced
                .iter()
                .filter(|p| **p == capture.join("objects"))
                .count(),
            2
        );
        let size = storage
            .assemble_multipart(&key, &[(part.part_number, part.etag.clone())])
            .await
            .unwrap();
        assert_eq!(size, 7);
        assert_eq!(storage.read_range(&key, 0, 6).await.unwrap(), b"current");
    }

    #[tokio::test]
    async fn durable_create_dir_all_fsyncs_new_ancestors_including_root_entry() {
        let (storage, root) = temp_storage();
        let capture = fs::canonicalize(&root).await.unwrap();
        let key = Uuid::now_v7().to_string();
        delayed_create::capture_fsyncs(capture.clone());
        storage.create_multipart(&key).await.unwrap();
        let fsyncs = delayed_create::take_fsyncs(&capture);
        let root = fs::canonicalize(&root).await.unwrap();
        let tmp = root.join("tmp");
        let key_dir = tmp.join(&key);
        assert!(
            fsyncs.iter().any(|p| p == &key_dir),
            "new tmp/key dir must be fsynced: {fsyncs:?}"
        );
        assert!(
            fsyncs.iter().any(|p| p == &tmp),
            "new tmp dir must be fsynced: {fsyncs:?}"
        );
        assert!(
            fsyncs.iter().any(|p| p == &root),
            "storage root must be fsynced for the new tmp entry: {fsyncs:?}"
        );

        delayed_create::capture_fsyncs(capture.clone());
        let key2 = Uuid::now_v7().to_string();
        storage.create_multipart(&key2).await.unwrap();
        let fsyncs = delayed_create::take_fsyncs(&capture);
        let key2_dir = tmp.join(&key2);
        assert!(
            fsyncs.iter().any(|p| p == &key2_dir),
            "existing-tree leaf must still be fsynced: {fsyncs:?}"
        );
        assert!(
            fsyncs.iter().any(|p| p == &tmp),
            "already-existing tmp ancestor must still be fsynced: {fsyncs:?}"
        );
        assert!(
            fsyncs.iter().any(|p| p == &root),
            "already-existing storage root must still be fsynced: {fsyncs:?}"
        );
    }

    #[tokio::test]
    async fn durable_create_dir_all_fsyncs_initially_missing_root_and_parent() {
        let tmp_parent = std::env::temp_dir();
        let capture = std::fs::canonicalize(&tmp_parent).unwrap();
        let parent = tmp_parent.join(format!("fvoci-att-missing-{}", Uuid::now_v7()));
        let root = parent.join("storage");
        assert!(!root.exists());
        let storage = LocalStorage::new(root.clone());
        let key = Uuid::now_v7().to_string();
        delayed_create::capture_fsyncs(capture.clone());
        storage.create_multipart(&key).await.unwrap();
        let fsyncs = delayed_create::take_fsyncs(&capture);
        let parent = fs::canonicalize(&parent).await.unwrap();
        let root = fs::canonicalize(&root).await.unwrap();
        let tmp = root.join("tmp");
        let key_dir = tmp.join(&key);
        assert!(
            fsyncs.iter().any(|p| p == &key_dir),
            "leaf must be fsynced: {fsyncs:?}"
        );
        assert!(
            fsyncs.iter().any(|p| p == &tmp),
            "tmp must be fsynced: {fsyncs:?}"
        );
        assert!(
            fsyncs.iter().any(|p| p == &root),
            "created storage root must be fsynced: {fsyncs:?}"
        );
        assert!(
            fsyncs.iter().any(|p| p == &parent),
            "parent of a newly created root must be fsynced: {fsyncs:?}"
        );
        let _ = std::fs::remove_dir_all(&parent);
    }

    #[tokio::test]
    async fn durable_create_dir_all_fsyncs_cwd_for_relative_nested_root() {
        let rel_root = PathBuf::from(format!("fvoci-att-rel-{}", Uuid::now_v7()));
        assert!(
            !rel_root.is_absolute(),
            "regression is a relative STORAGE path"
        );
        let cwd = std::fs::canonicalize(std::env::current_dir().unwrap()).unwrap();
        let storage = LocalStorage::new(rel_root.clone());
        let key = Uuid::now_v7().to_string();
        delayed_create::capture_fsyncs(cwd.clone());
        let created = storage.create_multipart(&key).await;
        let fsyncs = delayed_create::take_fsyncs(&cwd);
        let abs_root = cwd.join(&rel_root);
        let canonical_root = fs::canonicalize(&abs_root).await.ok();
        let _ = std::fs::remove_dir_all(&rel_root);
        created.unwrap();
        let canonical_root = canonical_root.expect("relative root should exist after create");
        assert!(
            fsyncs.iter().any(|p| p == &canonical_root),
            "relative storage root must be fsynced: {fsyncs:?}"
        );
        assert!(
            fsyncs.iter().any(|p| p == &cwd),
            "cwd must be fsynced so the relative root directory entry is durable: {fsyncs:?}"
        );
    }
}
