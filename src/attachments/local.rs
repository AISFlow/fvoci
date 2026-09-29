use std::io;
use std::path::{Path, PathBuf};
use std::sync::LazyLock;

use futures_util::StreamExt;
use sha2::{Digest, Sha256};
use tokio::fs;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use uuid::Uuid;

static UUID_KEY_RE: LazyLock<regex::Regex> = LazyLock::new(|| {
    regex::Regex::new(r"^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$")
        .expect("uuid regex")
});

const COPY_BUF: usize = 64 * 1024;
/// Length of the longest well-formed `{n}.etag` sidecar,
/// `<64 hex> <u64> <u64> <i64>.<i64>`: 64 hex digits, three spaces and a dot,
/// and four numbers of at most 20 characters each (`u64::MAX` and `i64::MIN`
/// both print as 20), so 64 + 4 + 4 × 20 = 148 bytes. Anything longer is not
/// trusted.
const ETAG_SIDECAR_MAX_BYTES: u64 = 64 + 4 + 4 * 20;

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
    /// Inode and mtime of the local file that staging wrote and hashed.
    identity: Option<PartIdentity>,
    keep: bool,
}

impl StagedPart {
    /// A part the remote store already holds under its multipart upload.
    pub(crate) fn uploaded(etag: String, size_bytes: u64) -> Self {
        Self {
            etag,
            size_bytes,
            writing_path: None,
            identity: None,
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

/// Which file a cached etag describes. A rename keeps the inode and mtime,
/// while any republish (this version's or v0.1.0's after a rollback) renames
/// a new inode over `{n}`, so a sidecar left behind by an older part, a
/// cancelled publish or a restore never matches the current file.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct PartIdentity {
    ino: u64,
    mtime_sec: i64,
    mtime_nsec: i64,
}

impl PartIdentity {
    fn of(meta: &std::fs::Metadata) -> Self {
        use std::os::unix::fs::MetadataExt;
        Self {
            ino: meta.ino(),
            mtime_sec: meta.mtime(),
            mtime_nsec: meta.mtime_nsec(),
        }
    }
}

/// Advisory cache of a published part's staging-time SHA-256, stored as
/// `{n}.etag` next to part `{n}`: `<sha256 hex> <size> <inode> <mtime s>.<ns>`.
/// It only saves the resume listing and the assembly pre-check from reading
/// every part; `copy_hashed` still verifies the bytes at assembly. It is
/// trusted only while size, inode and mtime match the part on disk, so a
/// missing, torn or stale sidecar costs a re-hash and nothing else. Its name
/// (and its temp name) is never all digits, which is what v0.1.0's
/// `list_parts` requires of a part, so a rollback ignores it.
#[derive(Debug, Clone, PartialEq, Eq)]
struct EtagSidecar {
    etag: String,
    size_bytes: u64,
    identity: PartIdentity,
}

impl EtagSidecar {
    fn encode(&self) -> String {
        format!(
            "{} {} {} {}.{}",
            self.etag,
            self.size_bytes,
            self.identity.ino,
            self.identity.mtime_sec,
            self.identity.mtime_nsec
        )
    }

    fn parse(text: &str) -> Option<Self> {
        let mut fields = text.split(' ');
        let etag = fields.next()?;
        let size_bytes = fields.next()?.parse().ok()?;
        let ino = fields.next()?.parse().ok()?;
        let (mtime_sec, mtime_nsec) = fields.next()?.split_once('.')?;
        if fields.next().is_some()
            || etag.len() != 64
            || !etag.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f'))
        {
            return None;
        }
        Some(Self {
            etag: etag.to_string(),
            size_bytes,
            identity: PartIdentity {
                ino,
                mtime_sec: mtime_sec.parse().ok()?,
                mtime_nsec: mtime_nsec.parse().ok()?,
            },
        })
    }

    fn describes(&self, meta: &std::fs::Metadata) -> bool {
        meta.is_file() && meta.len() == self.size_bytes && PartIdentity::of(meta) == self.identity
    }
}

fn etag_sidecar_path(dir: &Path, part_number: i32) -> PathBuf {
    dir.join(format!("{part_number}.etag"))
}

fn etag_sidecar_tmp_path(dir: &Path, part_number: i32) -> PathBuf {
    dir.join(format!("{part_number}.etag.{}.tmp", Uuid::now_v7()))
}

/// Writes the sidecar through a temp file and a rename, in one blocking
/// task so a cancelled caller cannot leave the temp file behind. No fsync:
/// after a crash a lost or torn sidecar fails validation and is re-hashed.
async fn write_etag_sidecar(dir: &Path, part_number: i32, sidecar: &EtagSidecar) -> io::Result<()> {
    let tmp = etag_sidecar_tmp_path(dir, part_number);
    let path = etag_sidecar_path(dir, part_number);
    let content = sidecar.encode();
    tokio::task::spawn_blocking(move || {
        let written = std::fs::write(&tmp, content).and_then(|()| std::fs::rename(&tmp, &path));
        if written.is_err() {
            let _ = std::fs::remove_file(&tmp);
        }
        written
    })
    .await
    .map_err(io::Error::other)?
}

/// The cached etag and size of part `{n}` at `part`, or `None` (hash the
/// part) unless its sidecar parses and describes the file on disk now.
async fn cached_part_etag(dir: &Path, part_number: i32, part: &Path) -> Option<(String, u64)> {
    use std::io::Read;

    let sidecar = etag_sidecar_path(dir, part_number);
    let part = part.to_path_buf();
    tokio::task::spawn_blocking(move || {
        let mut text = String::new();
        std::fs::File::open(&sidecar)
            .ok()?
            .take(ETAG_SIDECAR_MAX_BYTES + 1)
            .read_to_string(&mut text)
            .ok()?;
        if text.len() as u64 > ETAG_SIDECAR_MAX_BYTES {
            return None;
        }
        let cached = EtagSidecar::parse(&text)?;
        let meta = std::fs::metadata(&part).ok()?;
        cached
            .describes(&meta)
            .then_some((cached.etag, cached.size_bytes))
    })
    .await
    .ok()
    .flatten()
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
    /// A non-final part is below the backend's minimum part size
    /// (S3 `EntityTooSmall`).
    #[error("part too small (EntityTooSmall)")]
    PartTooSmall,
    /// The part body has no declared length and the backend needs one.
    #[error("part length required")]
    LengthRequired,
    /// The part body ended before, or ran past, its declared length.
    #[error("part body does not match its declared length")]
    LengthMismatch,
    /// The client's request body failed mid-stream (disconnect or reset).
    /// Nothing is stored; this is the client's failure, not the server's.
    #[error("client request body failed")]
    ClientBody,
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
                Err(_) => return Err(StorageError::ClientBody),
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
        // The fd is the file just hashed; without it no sidecar is written.
        let identity = file
            .metadata()
            .await
            .ok()
            .map(|meta| PartIdentity::of(&meta));
        drop(file);
        let etag = hex::encode(hasher.finalize());
        guard.disarm();
        Ok(StagedPart {
            etag,
            size_bytes,
            writing_path: Some(writing_path),
            identity,
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
        // Drop the old part's cached etag before its bytes are replaced; the
        // rename's directory fsync makes the removal durable with it.
        match fs::remove_file(etag_sidecar_path(&dir, part_number)).await {
            Ok(()) => {}
            Err(err) if err.kind() == io::ErrorKind::NotFound => {}
            Err(err) => {
                let _ = fs::remove_file(&writing_path).await;
                return Err(StorageError::Io(err));
            }
        }
        if let Err(err) = durable_rename(&writing_path, &final_path).await {
            let _ = fs::remove_file(&writing_path).await;
            return Err(if err.kind() == io::ErrorKind::NotFound {
                StorageError::UploadGone
            } else {
                StorageError::Io(err)
            });
        }
        staged.keep = true;
        if let Some(identity) = staged.identity {
            let sidecar = EtagSidecar {
                etag: staged.etag.clone(),
                size_bytes: staged.size_bytes,
                identity,
            };
            // The part is published either way; without a sidecar the
            // listing hashes it as before.
            if let Err(err) = write_etag_sidecar(&dir, part_number, &sidecar).await {
                tracing::warn!(
                    error = %err,
                    key,
                    part_number,
                    "attachment.part_etag_cache_failed"
                );
            }
        }
        Ok(PartInfo {
            part_number,
            etag: staged.etag.clone(),
            size_bytes: staged.size_bytes,
        })
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

    /// Published parts with their etags. A part's etag comes from its
    /// `{n}.etag` sidecar when that still describes the file, so a resume
    /// does not read the upload; otherwise (legacy v0.1.0 parts, crash
    /// leftovers, replaced parts) the part is hashed. Never writes sidecars:
    /// it runs without the upload's transaction lock.
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
            let path = entry.path();
            let (etag, size_bytes) = match cached_part_etag(&dir, part_number, &path).await {
                Some(cached) => cached,
                None => hash_file(&path).await?,
            };
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

        // First yielded body chunk is reachable only after create_writing_file
        // returns, so the outer stage future owns WritingGuard. File
        // appearance alone can precede that transfer while spawn_blocking
        // still holds the guard; that earlier boundary is covered by the
        // delayed-create test.
        let (first_chunk_tx, first_chunk_rx) = tokio::sync::oneshot::channel();
        let mut first_chunk_tx = Some(first_chunk_tx);
        let hanging = stream::iter(vec![Ok::<bytes::Bytes, std::io::Error>(
            bytes::Bytes::from_static(b"xx"),
        )])
        .inspect(move |_| {
            if let Some(tx) = first_chunk_tx.take() {
                let _ = tx.send(());
            }
        })
        .chain(futures_util::stream::pending());
        let staged = tokio::spawn({
            let storage = storage.clone();
            let key = key.clone();
            async move { storage.stage_part_stream(&key, 2, hanging, 32).await }
        });
        let dir = storage.parts_dir(&key);
        tokio::time::timeout(Duration::from_secs(5), first_chunk_rx)
            .await
            .expect("stage must consume the first body chunk after owning the writing guard")
            .expect("first-chunk witness must not be dropped");
        assert!(
            !writing_names(&dir).await.is_empty(),
            "writing temp should appear"
        );
        staged.abort();
        let join_err = staged
            .await
            .expect_err("cancelled stage must not complete successfully");
        assert!(
            join_err.is_cancelled(),
            "cancelled stage must be a cancellation, not a panic: {join_err:?}"
        );
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
        assert!(matches!(err, StorageError::ClientBody), "{err:?}");
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

    fn sha256_hex(bytes: &[u8]) -> String {
        hex::encode(Sha256::digest(bytes))
    }

    /// Rewrites a published part's bytes in place (same inode, same length)
    /// and puts its mtime back, so only a payload read can notice.
    fn overwrite_in_place_keeping_mtime(path: &Path, bytes: &[u8]) {
        let before = std::fs::metadata(path).unwrap();
        assert_eq!(before.len(), bytes.len() as u64, "same-length overwrite");
        let mtime = before.modified().unwrap();
        let mut file = std::fs::OpenOptions::new().write(true).open(path).unwrap();
        std::io::Write::write_all(&mut file, bytes).unwrap();
        file.set_modified(mtime).unwrap();
        file.sync_all().unwrap();
        drop(file);
        let after = std::fs::metadata(path).unwrap();
        assert_eq!(after.modified().unwrap(), mtime);
    }

    #[tokio::test]
    async fn list_parts_trusts_the_staged_etag_without_reading_the_payload() {
        let (storage, _root) = temp_storage();
        let key = Uuid::now_v7().to_string();
        storage.create_multipart(&key).await.unwrap();
        let part = publish_one(&storage, &key, b"payload-a").await;
        assert_eq!(part.etag, sha256_hex(b"payload-a"));
        let path = storage.parts_dir(&key).join("1");
        overwrite_in_place_keeping_mtime(&path, b"PAYLOAD-X");

        // The listing reports what staging hashed: it did not read the bytes.
        let listed = storage.list_parts(&key).await.unwrap();
        assert_eq!(listed.len(), 1);
        assert_eq!(listed[0].etag, part.etag);
        assert_eq!(listed[0].size_bytes, 9);

        // copy_hashed still checks the real bytes at assembly.
        let err = storage
            .assemble_multipart(&key, &[(1, part.etag.clone())])
            .await
            .unwrap_err();
        assert!(matches!(err, StorageError::EtagMismatch), "{err:?}");
        assert_eq!(storage.head(&key).await.unwrap(), None);
    }

    #[tokio::test]
    async fn list_parts_hashes_when_the_sidecar_is_missing_or_does_not_match() {
        let (storage, _root) = temp_storage();
        let key = Uuid::now_v7().to_string();
        storage.create_multipart(&key).await.unwrap();
        let part = publish_one(&storage, &key, b"payload-a").await;
        let dir = storage.parts_dir(&key);
        let path = dir.join("1");
        let sidecar = dir.join("1.etag");
        overwrite_in_place_keeping_mtime(&path, b"PAYLOAD-X");
        let real = sha256_hex(b"PAYLOAD-X");
        let valid = std::fs::read_to_string(&sidecar).expect("publish writes 1.etag");
        let fields: Vec<&str> = valid.split(' ').collect();
        assert_eq!(fields.len(), 4, "{valid:?}");
        assert_eq!(fields[0], part.etag);
        let (size, ino, mtime) = (fields[1], fields[2], fields[3]);
        let other_ino = (ino.parse::<u64>().unwrap() + 1).to_string();

        let bad: Vec<(&str, Option<String>)> = vec![
            ("missing", None),
            ("empty", Some(String::new())),
            (
                "short etag",
                Some(format!("{} {size} {ino} {mtime}", &part.etag[1..])),
            ),
            (
                "uppercase etag",
                Some(format!("{} {size} {ino} {mtime}", part.etag.to_uppercase())),
            ),
            (
                "non-hex etag",
                Some(format!("{} {size} {ino} {mtime}", "g".repeat(64))),
            ),
            (
                "size mismatch",
                Some(format!("{} 10 {ino} {mtime}", part.etag)),
            ),
            (
                "inode mismatch",
                Some(format!("{} {size} {other_ino} {mtime}", part.etag)),
            ),
            (
                "mtime mismatch",
                Some(format!("{} {size} {ino} 1.0", part.etag)),
            ),
            ("extra field", Some(format!("{valid} x"))),
            ("oversized", Some(format!("{valid}{}", " ".repeat(4096)))),
        ];
        for (case, content) in bad {
            match content {
                Some(content) => std::fs::write(&sidecar, content).unwrap(),
                None => {
                    let _ = std::fs::remove_file(&sidecar);
                }
            }
            let listed = storage.list_parts(&key).await.unwrap();
            assert_eq!(listed.len(), 1, "{case}");
            assert_eq!(listed[0].etag, real, "{case}: must hash the payload");
            assert_eq!(listed[0].size_bytes, 9, "{case}");
        }

        // A payload changed in place (mtime moves) is hashed as well.
        std::fs::write(&sidecar, &valid).unwrap();
        assert_eq!(storage.list_parts(&key).await.unwrap()[0].etag, part.etag);
        let file = std::fs::OpenOptions::new().write(true).open(&path).unwrap();
        file.set_modified(std::time::SystemTime::UNIX_EPOCH + Duration::from_secs(7))
            .unwrap();
        drop(file);
        assert_eq!(storage.list_parts(&key).await.unwrap()[0].etag, real);
    }

    #[tokio::test]
    async fn part_replaced_behind_the_sidecar_is_hashed() {
        // v0.1.0 (after a rollback) republishes by renaming a new `{n}` and
        // leaves `{n}.etag` alone; the new inode must not match it.
        let (storage, _root) = temp_storage();
        let key = Uuid::now_v7().to_string();
        storage.create_multipart(&key).await.unwrap();
        let part = publish_one(&storage, &key, b"payload-a").await;
        let dir = storage.parts_dir(&key);
        let replacement = dir.join("1.legacy.writing");
        std::fs::write(&replacement, b"PAYLOAD-X").unwrap();
        let old_mtime = std::fs::metadata(dir.join("1"))
            .unwrap()
            .modified()
            .unwrap();
        std::fs::File::options()
            .write(true)
            .open(&replacement)
            .unwrap()
            .set_modified(old_mtime)
            .unwrap();
        std::fs::rename(&replacement, dir.join("1")).unwrap();
        assert!(dir.join("1.etag").exists());
        let listed = storage.list_parts(&key).await.unwrap();
        assert_ne!(listed[0].etag, part.etag);
        assert_eq!(listed[0].etag, sha256_hex(b"PAYLOAD-X"));
    }

    #[tokio::test]
    async fn republishing_a_part_replaces_its_cached_etag() {
        let (storage, _root) = temp_storage();
        let key = Uuid::now_v7().to_string();
        storage.create_multipart(&key).await.unwrap();
        publish_one(&storage, &key, b"payload-a").await;
        let second = publish_one(&storage, &key, b"payload-b").await;
        assert_eq!(second.etag, sha256_hex(b"payload-b"));
        let sidecar = std::fs::read_to_string(storage.parts_dir(&key).join("1.etag")).unwrap();
        assert!(
            sidecar.starts_with(&format!("{} 9 ", second.etag)),
            "{sidecar:?}"
        );
        let listed = storage.list_parts(&key).await.unwrap();
        assert_eq!(listed[0].etag, second.etag);
        let size = storage
            .assemble_multipart(&key, &[(1, second.etag.clone())])
            .await
            .unwrap();
        assert_eq!(size, 9);
        assert_eq!(storage.read_range(&key, 0, 8).await.unwrap(), b"payload-b");
    }

    #[test]
    fn widest_sidecar_fits_the_size_cap() {
        let widest = EtagSidecar {
            etag: "f".repeat(64),
            size_bytes: u64::MAX,
            identity: PartIdentity {
                ino: u64::MAX,
                mtime_sec: i64::MIN,
                mtime_nsec: i64::MIN,
            },
        };
        let text = widest.encode();
        assert_eq!(text.len() as u64, ETAG_SIDECAR_MAX_BYTES, "{text:?}");
        assert_eq!(EtagSidecar::parse(&text), Some(widest));
    }

    #[tokio::test]
    async fn sidecar_names_are_never_part_names() {
        let (storage, _root) = temp_storage();
        let key = Uuid::now_v7().to_string();
        storage.create_multipart(&key).await.unwrap();
        publish_one(&storage, &key, b"payload-a").await;
        let dir = storage.parts_dir(&key);
        // A temp sidecar left by a crash between write and rename.
        let stray = etag_sidecar_tmp_path(&dir, 1);
        std::fs::write(&stray, "garbage").unwrap();
        for n in [1, 42, 10_000] {
            for path in [etag_sidecar_path(&dir, n), etag_sidecar_tmp_path(&dir, n)] {
                let name = path.file_name().unwrap().to_string_lossy().to_string();
                assert!(
                    !name.chars().all(|c| c.is_ascii_digit()),
                    "{name} would be read as a part by v0.1.0"
                );
            }
        }
        // The v0.1.0 listing filter sees only the part itself.
        let mut digit_names = Vec::new();
        for entry in std::fs::read_dir(&dir).unwrap() {
            let name = entry.unwrap().file_name().to_string_lossy().to_string();
            if name.chars().all(|c| c.is_ascii_digit()) {
                digit_names.push(name);
            }
        }
        assert_eq!(digit_names, vec!["1".to_string()]);
        let listed = storage.list_parts(&key).await.unwrap();
        assert_eq!(listed.len(), 1);
        assert_eq!(listed[0].etag, sha256_hex(b"payload-a"));
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
