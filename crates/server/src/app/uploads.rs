//! Uploads in parts.
//!
//! A user opens a session for a file of a board, sends the file in numbered parts of a
//! size the server sets, and completes the session, which makes the file. Nothing of the
//! parts is a file before.
//!
//! With the local file storage, the parts are kept on the local disk under the temporary
//! directory, `uploads/<session>/`, each in a file of its own that the record of the
//! part names (`<number>.<random>`), until the session is completed or thrown away. With
//! S3, a session that has bytes to send is a multipart upload of the object store, made
//! at the file's final place when the session is opened: each part is received into a
//! file of that directory and checked as on the local disk, then sent to the bucket and
//! the file removed, and the record of the part names the object store's identifier of
//! it.

use std::collections::{HashMap, HashSet};
use std::io;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use bytes::Bytes;
use futures_util::Stream;
use fvoci_model::{Upload, UploadPart, UploadPurpose};
use fvoci_store::PartRecord;
use tokio::fs;
use tokio::sync::{OwnedMutexGuard, OwnedSemaphorePermit, Semaphore};

use super::doomed::Doomed;
use super::files::FilePlace;
use super::{App, Imported};
use crate::auth::now_millis;
use crate::auth::secret::new_secret;
use crate::error::{AppError, Reason};
use crate::storage::S3Files;

/// How long a session lasts after it is opened.
pub const UPLOAD_LIFETIME_MILLIS: i64 = 24 * 60 * 60 * 1000;

/// How many sessions one user may have open at once.
pub const MAX_OPEN_UPLOADS: i64 = 8;

/// How long a closed session that is still there is left to whoever closed it before
/// the clean-up throws it away: a completion takes far less, and one that takes this
/// long has failed with the server.
pub const CLOSED_UPLOAD_GRACE_MILLIS: i64 = 60 * 60 * 1000;

/// The longest file name a session takes, in characters.
pub const MAX_UPLOAD_NAME_CHARS: usize = 255;

/// How many parts of the configured size the uploads to the object store hold in memory
/// at once, while each is read from its file and sent to the bucket. A part that would
/// take more waits on the disk until the memory is there.
pub const PARTS_IN_MEMORY: u64 = 8;

/// Maximum concurrent part streams received on local disk for one user at a time.
pub(super) const MAX_RECEIVING_PARTS_PER_USER: usize = 8;

/// Maximum idle gap between incoming body chunks while receiving an upload.
pub(crate) const PART_IDLE_TIMEOUT: Duration = Duration::from_secs(30);

/// What a refusal says a session is that is not there: none of that identifier, another
/// user's, or one that has ended.
const UPLOAD: &str = "upload";

/// The SHA-256 digest of a part, as the client sent it.
pub type PartDigest = [u8; 32];

/// What a clean-up of the upload sessions did.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct CleanedUp {
    /// The sessions thrown away.
    pub removed: usize,
    /// The sessions that could not wholly be thrown away.
    pub left: usize,
}

/// The memory of [`PARTS_IN_MEMORY`] parts of `part_bytes`, in KiB.
pub(crate) fn part_memory_kib(part_bytes: u64) -> usize {
    let kib = part_bytes.div_ceil(1024).saturating_mul(PARTS_IN_MEMORY);
    usize::try_from(kib)
        .unwrap_or(usize::MAX)
        .min(Semaphore::MAX_PERMITS)
}

/// A turn for each session. A part of an upload to the object store is checked against
/// its session, sent to the bucket and recorded in the session's turn, and the session is
/// closed and completed, abandoned or thrown away in it: what the bucket holds under a
/// number is the part recorded under it, and nothing reaches the bucket once the session
/// is closed. The object store keeps one part to a number, where the local disk keeps a
/// file to each part that comes in. Kept in the server's memory: one server serves a
/// database (`docs/configuration.md`). A turn is there while someone holds or waits for
/// it, and no longer.
#[derive(Debug, Default)]
pub(crate) struct UploadTurns(Mutex<HashMap<String, Arc<tokio::sync::Mutex<()>>>>);

impl UploadTurns {
    /// Waits for the turn of a session.
    async fn take(&self, upload_id: &str) -> Turn<'_> {
        let turn = {
            let mut turns = self.0.lock().expect("the upload turns are not poisoned");
            Arc::clone(turns.entry(upload_id.to_owned()).or_default())
        };
        Turn {
            turns: self,
            upload_id: upload_id.to_owned(),
            guard: Some(turn.lock_owned().await),
        }
    }
}

/// The turn of a session, which is given up when this is dropped. The guard is an
/// `Option` only so that the drop can look at the count while it still holds it.
#[derive(Debug)]
struct Turn<'turns> {
    turns: &'turns UploadTurns,
    upload_id: String,
    guard: Option<OwnedMutexGuard<()>>,
}

impl Drop for Turn<'_> {
    fn drop(&mut self) {
        let Some(guard) = self.guard.take() else {
            return;
        };
        let mut turns = self
            .turns
            .0
            .lock()
            .expect("the upload turns are not poisoned");
        // The map's and this one: nobody else holds or waits for the turn, and nobody
        // can start to while the map is locked.
        if Arc::strong_count(OwnedMutexGuard::mutex(&guard)) == 2 {
            turns.remove(&self.upload_id);
        }
        drop(guard);
    }
}

/// Guards in-flight `putUploadPart` streams so two requests never stream the same
/// `(upload_id, part_number)` concurrently (`409 Conflict`) and one user holds at most
/// [`MAX_RECEIVING_PARTS_PER_USER`] active part receivers at once.
#[derive(Debug, Default)]
pub(crate) struct ReceivingParts {
    active: Mutex<HashSet<(String, i64)>>,
    per_user: Mutex<HashMap<String, Arc<Semaphore>>>,
}

impl ReceivingParts {
    async fn enter(
        &self,
        user_id: &str,
        upload_id: &str,
        number: i64,
    ) -> Result<Receiving<'_>, AppError> {
        let key = (upload_id.to_owned(), number);
        {
            let mut active = self.active.lock().expect("receiving parts lock poisoned");
            if !active.insert(key.clone()) {
                return Err(AppError::conflict(format_args!(
                    "part {number} of the upload is already being received"
                )));
            }
        }
        let part_guard = ActivePart {
            parts: self,
            key: Some(key),
        };
        let semaphore = {
            let mut per_user = self.per_user.lock().expect("receiving users lock poisoned");
            Arc::clone(
                per_user
                    .entry(user_id.to_owned())
                    .or_insert_with(|| Arc::new(Semaphore::new(MAX_RECEIVING_PARTS_PER_USER))),
            )
        };
        let permit = Arc::clone(&semaphore)
            .acquire_owned()
            .await
            .map_err(|_| AppError::internal("receiving part semaphore closed"))?;
        Ok(Receiving {
            _part: part_guard,
            parts: self,
            user_id: user_id.to_owned(),
            semaphore,
            permit: Some(permit),
        })
    }
}

struct ActivePart<'a> {
    parts: &'a ReceivingParts,
    key: Option<(String, i64)>,
}

impl Drop for ActivePart<'_> {
    fn drop(&mut self) {
        if let Some(key) = self.key.take() {
            self.parts
                .active
                .lock()
                .expect("receiving parts lock poisoned")
                .remove(&key);
        }
    }
}

struct Receiving<'a> {
    _part: ActivePart<'a>,
    parts: &'a ReceivingParts,
    user_id: String,
    semaphore: Arc<Semaphore>,
    permit: Option<OwnedSemaphorePermit>,
}

impl Drop for Receiving<'_> {
    fn drop(&mut self) {
        let Some(permit) = self.permit.take() else {
            return;
        };
        drop(permit);
        let mut per_user = self
            .parts
            .per_user
            .lock()
            .expect("receiving users lock poisoned");
        // Map entry + `self.semaphore` = 2 strong refs when nobody else holds or waits.
        if Arc::strong_count(&self.semaphore) == 2
            && self.semaphore.available_permits() == MAX_RECEIVING_PARTS_PER_USER
        {
            per_user.remove(&self.user_id);
        }
    }
}

impl App {
    /// Opens a session for `user_id` to upload a file to `container_id`. The caller has
    /// checked that the user may upload files to the board.
    ///
    /// # Errors
    ///
    /// Returns a bad request for an empty or too long name, a negative size or a content
    /// type that is too long; a 413 with the reason `file_too_large` for a file larger than
    /// `max_file_bytes`; a 429 if the user has [`MAX_OPEN_UPLOADS`] sessions open, or the
    /// open sessions would hold back more bytes than the user's share (twice the largest
    /// upload, a file or an archive) or the server's (`max_pending_upload_bytes`); and
    /// the store's error.
    pub async fn create_upload(
        &self,
        user_id: &str,
        container_id: &str,
        new: FileToUpload,
    ) -> Result<Upload, AppError> {
        self.open_upload(user_id, UploadPurpose::File, container_id, new)
            .await
    }

    /// Opens a session for `user_id` to upload an archive to import into `workspace_id`.
    /// Its parts are kept on the local disk whatever the file storage: the import reads the
    /// archive at random. The caller has checked that the user may import into the
    /// workspace.
    ///
    /// # Errors
    ///
    /// Those of [`App::create_upload`], with a 413 with the reason `archive_too_large`
    /// for an archive larger than `max_import_bytes`.
    pub async fn create_import_upload(
        &self,
        user_id: &str,
        workspace_id: &str,
        new: FileToUpload,
    ) -> Result<Upload, AppError> {
        self.open_upload(user_id, UploadPurpose::Import, workspace_id, new)
            .await
    }

    /// Opens a session for a purpose, to a board (a file) or a workspace (an import).
    async fn open_upload(
        &self,
        user_id: &str,
        purpose: UploadPurpose,
        target: &str,
        new: FileToUpload,
    ) -> Result<Upload, AppError> {
        new.check()?;
        let limit = match purpose {
            UploadPurpose::File => self.limits().largest_file,
            UploadPurpose::Import => self.limits().largest_import,
        };
        // Checked above: the size is no less than zero.
        if new.size.unsigned_abs() > limit.get() {
            return Err(file_to_upload::too_large(purpose, limit));
        }

        // One server serves a database (`docs/configuration.md`): a lock in it is
        // enough to keep the check and the insertion together.
        let turn = self.opening_upload_turn().await;
        if purpose == UploadPurpose::File {
            self.check_org_storage_room(u64::try_from(new.size).unwrap_or(0))
                .await?;
        }
        let now = now_millis();
        let usage = self.store().upload_usage(user_id, now).await?;
        if usage.user_sessions >= MAX_OPEN_UPLOADS {
            return Err(AppError::limited(
                format_args!("{MAX_OPEN_UPLOADS} uploads are open; complete or abandon one first"),
                Reason::TooManyUploads,
            ));
        }
        let user_share = self.limits().open_upload_bytes_of_a_user();
        if usage.user_bytes.saturating_add(new.size) > user_share {
            return Err(AppError::limited(
                format_args!(
                    "the open uploads would hold back more than {user_share} bytes; \
                     complete or abandon one first"
                ),
                Reason::UploadsPending,
            ));
        }
        let server_share =
            i64::try_from(self.config().max_pending_upload_bytes).unwrap_or(i64::MAX);
        if usage.server_bytes.saturating_add(new.size) > server_share {
            return Err(AppError::limited(
                "the server holds back as many bytes of uploads as it may; try again later",
                Reason::ServerUploadsPending,
            ));
        }
        let part_size = i64::try_from(self.config().upload_part_bytes).unwrap_or(i64::MAX);
        // An empty file has no part to send: its parts, none, are on the disk, as an archive's.
        let multipart = self
            .files()
            .multipart()
            .filter(|_| new.size > 0 && purpose == UploadPurpose::File);
        let path = match multipart {
            Some(_) => self.new_file_place(target, &new.name).await?.path,
            None => String::new(),
        };
        let (container_id, workspace_id) = match purpose {
            UploadPurpose::File => (target.to_owned(), String::new()),
            UploadPurpose::Import => (String::new(), target.to_owned()),
        };
        let upload = Upload {
            id: new_secret(),
            user_id: user_id.to_owned(),
            purpose,
            container_id,
            workspace_id,
            name: new.name,
            size: new.size,
            content_type: new.content_type,
            part_size,
            created_at: now,
            expires_at: now.saturating_add(UPLOAD_LIFETIME_MILLIS),
            closed_at: 0,
            multipart_id: String::new(),
            path,
        };
        self.store().create_upload(&upload).await?;
        drop(turn);
        match multipart {
            Some(files) => self.start_multipart(upload, files).await,
            None => Ok(upload),
        }
    }

    /// Starts the multipart upload of a session in the object store and records it. Done
    /// once the session is stored, out of the turn of opening: the object store may be
    /// slow to answer, and the session holds its place in the limits meanwhile. A session
    /// that cannot be started is thrown away.
    async fn start_multipart(
        &self,
        mut upload: Upload,
        files: &S3Files,
    ) -> Result<Upload, AppError> {
        let multipart_id = match files.create_multipart(&upload.path).await {
            Ok(multipart_id) => multipart_id,
            Err(error) => {
                self.discard_upload(&upload).await;
                return Err(AppError::internal(format_args!(
                    "the upload could not be started in the files storage: {error}"
                )));
            }
        };
        upload.multipart_id = multipart_id;
        let recorded = self
            .store()
            .set_upload_multipart(&upload.id, &upload.multipart_id)
            .await;
        match recorded {
            Ok(true) => Ok(upload),
            Ok(false) => {
                self.discard_upload(&upload).await;
                Err(AppError::internal("the upload went away as it was started"))
            }
            Err(error) => {
                self.discard_upload(&upload).await;
                Err(error.into())
            }
        }
    }

    /// Returns the user's session and the parts it has received.
    ///
    /// # Errors
    ///
    /// Returns a not-found error if the user has no such session that is still going:
    /// none of that identifier, another user's, or one that has ended. And the store's
    /// error.
    pub async fn upload(
        &self,
        upload_id: &str,
        user_id: &str,
    ) -> Result<(Upload, Vec<UploadPart>), AppError> {
        let upload = match self
            .store()
            .get_upload(upload_id, user_id, now_millis())
            .await
        {
            Ok(upload) => upload,
            Err(error) if error.is_not_found() => return Err(AppError::not_found(UPLOAD)),
            Err(error) => return Err(error.into()),
        };
        let parts = self.store().get_upload_parts(&upload.id).await?;
        Ok((upload, parts))
    }

    /// Receives part `number` of an upload from `body`, whose length the request
    /// declared as `length`, and records it, replacing what was received under that
    /// number before. The part is checked as it is read: its length against the part's
    /// size, its bytes against `digest`. A part that fails a check is not kept.
    ///
    /// # Errors
    ///
    /// Returns a bad request for a number the upload has no part of, a declared length
    /// other than the part's size, a body that is longer or shorter than that, or one
    /// whose digest differs; a conflict if the session was closed meanwhile; and a
    /// server error if the part cannot be written.
    pub async fn put_upload_part<E>(
        &self,
        upload: &Upload,
        number: i64,
        length: u64,
        digest: &PartDigest,
        body: impl Stream<Item = Result<Bytes, E>> + Unpin,
    ) -> Result<(), AppError>
    where
        E: std::fmt::Display,
    {
        let Some(size) = upload.part_len(number) else {
            return Err(AppError::bad_request(format_args!(
                "the upload has parts 1 to {}",
                upload.part_count()
            ))
            .with_parameter("partNumber"));
        };
        let size = u64::try_from(size).unwrap_or_default();
        if length != size {
            return Err(AppError::bad_request(format_args!(
                "part {number} has {size} bytes; the request declares {length}"
            )));
        }
        if upload.closed_at != 0 {
            return Err(closed());
        }
        let _receiving = self
            .receiving_parts()
            .enter(&upload.user_id, &upload.id, number)
            .await?;

        let directory = self.upload_dir(&upload.id);
        fs::create_dir_all(&directory)
            .await
            .map_err(|error| part_error(&error))?;
        // Every part that comes in gets a file of its own, which the record names: a part
        // sent again, or a request that is late, never touches a file that is recorded.
        let name = format!("{number}.{}", new_secret());
        let file = directory.join(&name);
        if let Err(error) = receive_part(&file, size, digest, body).await {
            remove_quietly(&file).await;
            return Err(error);
        }
        if !upload.multipart_id.is_empty() {
            // The part was received at the client's pace on the disk; the memory and the
            // turn are held only while it goes to the bucket.
            // The directory stays for the other parts that come in; it goes with the
            // session.
            let sent = self.send_multipart_part(upload, number, size, &file).await;
            remove_quietly(&file).await;
            return sent;
        }
        let record = self
            .store()
            .put_upload_part(
                &upload.id,
                &UploadPart {
                    number,
                    size: i64::try_from(size).unwrap_or(i64::MAX),
                    part_id: name,
                },
            )
            .await;
        match record {
            Ok(PartRecord::Recorded { replaced }) => {
                if let Some(replaced) = replaced.filter(|replaced| !replaced.is_empty()) {
                    remove_quietly(&directory.join(replaced)).await;
                }
                Ok(())
            }
            Ok(PartRecord::Closed) => {
                // The session was closed while the part came in. Whoever closed it owns
                // the directory, or has moved it away and this request made it again.
                remove_quietly(&file).await;
                if let Err(error) = fs::remove_dir(&directory).await {
                    // Not empty: what is left is the closer's, or another late part's.
                    tracing::debug!(%error, "the directory of a closed upload is left");
                }
                Err(closed())
            }
            Err(error) => {
                remove_quietly(&file).await;
                Err(error.into())
            }
        }
    }

    /// Sends a part of a multipart upload, received and checked in `file`, to the object
    /// store, where it replaces what was sent under its number, and records it.
    async fn send_multipart_part(
        &self,
        upload: &Upload,
        number: i64,
        size: u64,
        file: &Path,
    ) -> Result<(), AppError> {
        let Some(files) = self.files().multipart() else {
            return Err(AppError::internal(
                "the upload is in an object store that is not configured",
            ));
        };
        // The turn first: a part waiting for it holds no memory.
        let _turn = self.upload_turns().take(&upload.id).await;
        // Checked in the turn: a session closed meanwhile takes nothing more.
        self.check_open(upload).await?;
        let kib = u32::try_from(size.div_ceil(1024))
            .expect("a part is at most 5 GiB, checked when the server starts");
        let _memory = Arc::clone(self.part_memory())
            .acquire_many_owned(kib)
            .await
            .map_err(|_| AppError::internal("the memory for parts is closed"))?;
        let bytes = fs::read(file).await.map_err(|error| {
            // The session's directory went with it while the part came in.
            if error.kind() == io::ErrorKind::NotFound {
                closed()
            } else {
                part_error(&error)
            }
        })?;
        let bytes = Bytes::from(bytes);
        // Numbers start at 1, the object store's indexes at 0.
        let index = usize::try_from(number.saturating_sub(1)).unwrap_or_default();
        let part_id = match files
            .put_part(&upload.path, &upload.multipart_id, index, vec![bytes])
            .await
        {
            Ok(part_id) => part_id,
            Err(error) => {
                // The bucket may have taken it with its answer lost.
                self.forget_part(upload, number).await;
                return Err(AppError::internal(format_args!(
                    "the part could not be stored in the files storage: {error}"
                )));
            }
        };
        let part = UploadPart {
            number,
            size: i64::try_from(size).unwrap_or(i64::MAX),
            part_id,
        };
        match self.store().put_upload_part(&upload.id, &part).await {
            Ok(PartRecord::Recorded { .. }) => Ok(()),
            // Not in the turn, which closes it in its turn; left as an answer all the same.
            Ok(PartRecord::Closed) => Err(closed()),
            Err(error) => {
                self.forget_part(upload, number).await;
                Err(error.into())
            }
        }
    }

    /// Forgets the record of a part when the bucket may hold another part under its
    /// number, so that the number is sent again.
    async fn forget_part(&self, upload: &Upload, number: i64) {
        if let Err(error) = self.store().delete_upload_part(&upload.id, number).await {
            tracing::error!(%error, "the record of a part the bucket may have replaced could not be removed");
        }
    }

    /// Checks that a session is still open: a conflict if it was closed or has gone, as by
    /// a completion, an abandonment or the clean-up.
    async fn check_open(&self, upload: &Upload) -> Result<(), AppError> {
        match self
            .store()
            .get_upload(&upload.id, &upload.user_id, now_millis())
            .await
        {
            Ok(current) if current.closed_at == 0 => Ok(()),
            Ok(_) => Err(closed()),
            Err(error) if error.is_not_found() => Err(closed()),
            Err(error) => Err(error.into()),
        }
    }

    /// Completes an upload: checks that every part is there, makes the file of the
    /// board from the parts in order, and throws the session away. Returns the file's
    /// identifier. The caller has checked again that the user may upload files to the
    /// board.
    ///
    /// # Errors
    ///
    /// Returns a bad request naming the parts that are missing, with the session left
    /// as it is; a conflict if the session is being completed or thrown away already;
    /// and a server error if the file cannot be made, with the session thrown away.
    pub async fn complete_upload(&self, upload: &Upload) -> Result<String, AppError> {
        if upload.purpose != UploadPurpose::File {
            return Err(AppError::bad_request(
                "the upload is of an archive; import it with importArchive",
            )
            .with_parameter("uploadId"));
        }
        // Before the session is closed, so that a missing part leaves it open.
        self.check_parts(upload).await?;
        let _turn = self.upload_turns().take(&upload.id).await;
        if !self.store().close_upload(&upload.id, now_millis()).await? {
            return Err(closed());
        }
        // Read again once closed: a part recorded in between replaced one read above,
        // and none is recorded now.
        let parts = match self.check_parts(upload).await {
            Ok(parts) => parts,
            Err(error) => {
                self.discard_in_turn(upload).await;
                return Err(error);
            }
        };
        if !upload.multipart_id.is_empty() {
            return self.complete_multipart(upload, &parts).await;
        }
        // The parts are moved out of the way first: a part that is still coming in, or an
        // abandonment, finds no directory to change under the completion.
        let directory = self.upload_dir(&upload.id);
        let working = directory.with_extension(format!("completing-{}", new_secret()));
        // An empty file has no part, and so no directory yet.
        let moved = match fs::create_dir_all(&directory).await {
            Ok(()) => fs::rename(&directory, &working).await,
            Err(error) => Err(error),
        };
        let saved = match moved {
            Ok(()) => self.save_parts(upload, &parts, &working).await,
            Err(error) => Err(part_error(&error)),
        };
        self.discard_in_turn(upload).await;
        remove_dir_quietly(&working).await;
        saved
    }

    /// Completes the multipart upload of a closed session in the object store, which
    /// makes the file at its place, and records the file.
    async fn complete_multipart(
        &self,
        upload: &Upload,
        parts: &[UploadPart],
    ) -> Result<String, AppError> {
        let size = u64::try_from(upload.size).unwrap_or_default();
        if let Err(error) = self.check_org_storage_room(size).await {
            self.discard_in_turn(upload).await;
            return Err(error);
        }
        let Some(files) = self.files().multipart() else {
            self.discard_in_turn(upload).await;
            return Err(AppError::internal(
                "the upload is in an object store that is not configured",
            ));
        };
        let part_ids = parts.iter().map(|part| part.part_id.clone()).collect();
        if let Err(error) = files
            .complete_multipart(&upload.path, &upload.multipart_id, part_ids)
            .await
        {
            // The completion may have taken effect with its answer lost: the object, which
            // has no record, goes with the session.
            self.discard_in_turn(upload).await;
            return Err(AppError::internal(format_args!(
                "the file could not be made from its parts: {error}"
            )));
        }
        // Recorded before the session is forgotten: a server that stops in between leaves
        // a closed session, which the clean-up throws away with the object if it has no
        // record.
        let place = FilePlace::at(&upload.path, &upload.name);
        let recorded = self.record_file(upload, place, size).await;
        // A record whose answer was lost may be there all the same.
        if recorded.is_err() && self.remove_if_unrecorded(upload).await.is_none() {
            // Left closed for the clean-up to look at again.
            return recorded;
        }
        // The upload is the file now, or gone: there is nothing left to abort.
        self.forget_upload(upload).await;
        recorded
    }

    /// Checks that every part of an upload was received with its size, and returns the
    /// parts by number.
    async fn check_parts(&self, upload: &Upload) -> Result<Vec<UploadPart>, AppError> {
        let parts = self.store().get_upload_parts(&upload.id).await?;
        let missing: Vec<i64> = (1..=upload.part_count())
            .filter(|number| {
                !parts.iter().any(|part| {
                    part.number == *number && Some(part.size) == upload.part_len(*number)
                })
            })
            .collect();
        if !missing.is_empty() {
            let shown: Vec<String> = missing.iter().take(20).map(i64::to_string).collect();
            let more = if missing.len() > shown.len() {
                ", …"
            } else {
                ""
            };
            return Err(AppError::bad_request(format_args!(
                "parts missing: {}{more}",
                shown.join(", ")
            )));
        }
        // By number, as the store reads them, and none but these: a part of another
        // number is refused before it is recorded.
        Ok(parts)
    }

    /// Makes the file of an upload from its parts in `directory`, which must be the size
    /// they were received with.
    async fn save_parts(
        &self,
        upload: &Upload,
        parts: &[UploadPart],
        directory: &Path,
    ) -> Result<String, AppError> {
        let mut reader = parts_reader(parts, directory).await?;
        self.save_upload(&mut reader, upload).await
    }

    /// Imports the archive of an upload into its workspace, as its user, and throws the
    /// upload away: checks that every part is there, joins them into one file on the
    /// local disk, and imports from that. The caller has checked again that the user may
    /// import into the workspace. Returns what the import left out.
    ///
    /// # Errors
    ///
    /// Returns a bad request for an upload of a file, or one that misses parts (left as
    /// it is); a conflict if it is being completed or thrown away already; and the
    /// errors of the import, with the upload thrown away.
    pub async fn import_upload(&self, upload: &Upload) -> Result<Imported, AppError> {
        let summary = self
            .import_upload_with_format(upload, super::ImportFormat::Fvoci)
            .await?;
        Ok(Imported {
            members_left_out: summary.members_left_out,
        })
    }

    /// Imports an upload in parts as `format` (`fvoci`, `trello`, or `notion`) into its
    /// workspace as its user, throwing the upload away once all parts are present.
    ///
    /// # Errors
    ///
    /// Returns the errors of [`App::import_upload`] and [`App::import_external_archive`].
    pub async fn import_upload_with_format(
        &self,
        upload: &Upload,
        format: super::ImportFormat,
    ) -> Result<super::ImportSummary, AppError> {
        if upload.purpose != UploadPurpose::Import {
            return Err(AppError::bad_request(
                "the upload is of a file; complete it with completeUpload",
            )
            .with_field("uploadId"));
        }
        // Before the session is closed, so that a missing part leaves it open.
        self.check_parts(upload).await?;
        let turn = self.upload_turns().take(&upload.id).await;
        if !self.store().close_upload(&upload.id, now_millis()).await? {
            return Err(closed());
        }
        let parts = match self.check_parts(upload).await {
            Ok(parts) => parts,
            Err(error) => {
                self.discard_in_turn(upload).await;
                return Err(error);
            }
        };
        if let Err(error) = format.validate_upload(&upload.name, upload.size) {
            self.discard_in_turn(upload).await;
            return Err(error);
        }
        // Moved aside, as for a completion: the session's directory goes with it, the
        // archive stays until it is imported. A server that stops meanwhile leaves a
        // directory that is no session's, which the start-up sweep removes.
        let directory = self.upload_dir(&upload.id);
        // Removed however the import ends, also when its request is dropped meanwhile.
        let working =
            Doomed::directory(directory.with_extension(format!("importing-{}", new_secret())));
        let moved = match fs::create_dir_all(&directory).await {
            Ok(()) => fs::rename(&directory, working.path()).await,
            Err(error) => Err(error),
        };
        let archive = working.path().join("archive.fvoci");
        let joined = match moved {
            Ok(()) => join_parts(&parts, working.path(), &archive).await,
            Err(error) => Err(part_error(&error)),
        };
        self.discard_in_turn(upload).await;
        drop(turn);
        let imported = match joined {
            Ok(()) => {
                self.import_external_archive(
                    &archive,
                    format,
                    &upload.workspace_id,
                    &upload.user_id,
                )
                .await
            }
            Err(error) => Err(error),
        };
        working.remove().await;
        imported
    }

    /// Abandons a session at the request of its user, and throws it away.
    ///
    /// # Errors
    ///
    /// Returns a conflict if the session is being completed or thrown away already, and
    /// the store's error.
    pub async fn abandon_upload(&self, upload: &Upload) -> Result<(), AppError> {
        let _turn = self.upload_turns().take(&upload.id).await;
        if !self.store().close_upload(&upload.id, now_millis()).await? {
            return Err(closed());
        }
        self.discard_in_turn(upload).await;
        Ok(())
    }

    /// Throws a session away: its record, and the parts on the disk or in the object
    /// store. What is gone already is left gone; what cannot be removed is logged, and
    /// found again by the clean-up of the temporary directory, or by the bucket's rule
    /// for incomplete multipart uploads (`docs/fvoci-cdn.md`). Returns whether all of it
    /// went: false if something was logged.
    pub async fn discard_upload(&self, upload: &Upload) -> bool {
        let _turn = self.upload_turns().take(&upload.id).await;
        self.discard_in_turn(upload).await
    }

    /// [`App::discard_upload`] in the session's turn, which the caller holds.
    async fn discard_in_turn(&self, upload: &Upload) -> bool {
        let mut settled = true;
        // Closed first: a part still coming in is refused rather than recorded.
        if let Err(error) = self.store().close_upload(&upload.id, now_millis()).await {
            tracing::warn!(%error, "an upload could not be closed before it was thrown away");
            settled = false;
        }
        if !upload.multipart_id.is_empty()
            && let Some(files) = self.files().multipart()
            && let Err(error) = files
                .abort_multipart(&upload.path, &upload.multipart_id)
                .await
        {
            tracing::warn!(%error, "the parts of an upload could not be thrown away in the files storage");
            settled = false;
        }
        if !upload.multipart_id.is_empty() {
            match self.remove_if_unrecorded(upload).await {
                // Left closed for the clean-up to look at again, once its grace is over.
                None => return false,
                Some(removed) => settled &= removed,
            }
        }
        self.forget_upload(upload).await && settled
    }

    /// Removes the object at the place of a multipart upload unless something of any
    /// organization holds it (`Store::held_objects`): a completion that took effect
    /// without its record, as when the answer was lost, leaves an object nothing gives
    /// out. Doomed first, it is removed by the housekeeping if not now. Returns `None`
    /// if that is not settled (the holds or the doom failed); else whether it went now.
    async fn remove_if_unrecorded(&self, upload: &Upload) -> Option<bool> {
        let paths = [upload.path.as_str()];
        match self.store().held_objects(&paths, Some(&upload.id)).await {
            Ok(held) if !held.is_empty() => Some(true),
            Ok(_) => match self.store().doom_unrecorded(&paths).await {
                Ok(_) => Some(self.remove_doomed_now(&upload.path).await),
                Err(error) => {
                    tracing::warn!(%error, "an uploaded file without a record could not be doomed");
                    None
                }
            },
            Err(error) => {
                tracing::warn!(%error, "whether an uploaded file has a record could not be read");
                None
            }
        }
    }

    /// Removes the record of a closed session and its directory of parts. Returns
    /// whether both went.
    async fn forget_upload(&self, upload: &Upload) -> bool {
        let forgotten = match self.store().delete_upload(&upload.id).await {
            Ok(()) => true,
            Err(error) => {
                tracing::error!(%error, "the record of an upload could not be removed");
                false
            }
        };
        let removed = remove_dir_quietly(&self.upload_dir(&upload.id)).await;
        forgotten && removed
    }

    /// Throws away the sessions that have ended at `now`, and those closed for longer
    /// than [`CLOSED_UPLOAD_GRACE_MILLIS`] that are still there. Returns how many went,
    /// and how many could not wholly go (logged; the next clean-up finds them again).
    ///
    /// # Errors
    ///
    /// Returns a 503 error if this process does not hold the lock of the server, and
    /// the store's error if the sessions cannot be read.
    pub async fn clean_up_uploads(&self, now: i64) -> Result<CleanedUp, AppError> {
        self.require_server_lock()?;
        let uploads = self
            .store()
            .uploads_to_clean_up(now, now.saturating_sub(CLOSED_UPLOAD_GRACE_MILLIS))
            .await?;
        let mut cleaned = CleanedUp::default();
        for upload in &uploads {
            // A server that lost its lock leaves the rest to the next.
            self.require_server_lock()?;
            if self.discard_upload(upload).await {
                cleaned.removed += 1;
            } else {
                cleaned.left += 1;
            }
        }
        Ok(cleaned)
    }

    /// The directory that holds the parts of an upload.
    pub(crate) fn upload_dir(&self, upload_id: &str) -> PathBuf {
        self.config().tmp_dir().join("uploads").join(upload_id)
    }
}

mod file_to_upload;
mod parts_io;

pub use file_to_upload::FileToUpload;
use parts_io::{
    closed, join_parts, part_error, parts_reader, receive_part, remove_dir_quietly, remove_quietly,
};
