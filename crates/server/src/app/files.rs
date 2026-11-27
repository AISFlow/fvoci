//! Files attached to boards: storing uploads, finding them again and copying them when
//! cards are duplicated.
//!
//! A file is one board's. It is stored at a key the server makes up
//! ([`FileKey`]: the organization, the board, a secret), is asked for by a stored name,
//! the secret with the upload's extension, and its record names the board, the stored
//! name and the key. A file is given out, copied and exported only through the board
//! of its record: whoever may read one board cannot fetch another board's file by
//! knowing its name. A file without a record is given out through no board.

use std::collections::{BTreeMap, HashMap, HashSet};

use fvoci_model::{
    Block, BlockError, BlockPatch, BlockPatchBatch, Body, Container, FileKey, FileRecord, JsonMap,
    ListedFile, Upload, new_id,
};
use serde_json::Value;
use tokio::io::AsyncRead;

use super::App;
use super::loose_objects::WrittenPlace;
use crate::auth::now_millis;
use crate::auth::secret::new_secret;
use crate::error::{AppError, Reason};
use crate::storage::StorageError;

/// What a request misses that asks for a file that is not there.
pub(crate) const FILE: &str = "file";

/// Where a new file of a board goes in the file storage, and what its record says of
/// its name.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct FilePlace {
    /// The key in the file storage.
    pub(crate) path: String,
    /// The name blocks refer to the file by: the secret of the key, with `extension`.
    stored_name: String,
    /// The name the client gave, without directories.
    name: String,
    /// The extension of the stored name, with its dot, or empty.
    extension: String,
}

impl FilePlace {
    /// A new place for a file of `container_id`, of the organization `org_id`, that the
    /// client calls `filename`: a key of a new secret, and the stored name of that secret
    /// with the lower-cased extension of `filename` (`.jpeg` becomes `.jpg`). Nothing of
    /// the client's name ends up in the key.
    pub(crate) fn new(org_id: &str, container_id: &str, filename: &str) -> Self {
        let secret = new_secret();
        let key = FileKey {
            org_id,
            container_id,
            secret: &secret,
        };
        Self::at(&key.to_string(), filename)
    }

    /// The place chosen with [`FilePlace::new`] at `path`, for the file the client calls
    /// `filename`.
    pub(crate) fn at(path: &str, filename: &str) -> Self {
        let name = base_name(filename);
        let extension = stored_extension(name);
        let secret = path.rsplit('/').next().unwrap_or(path);
        Self {
            path: path.to_owned(),
            stored_name: format!("{secret}{extension}"),
            name: name.to_owned(),
            extension,
        }
    }
}

impl App {
    /// The files uploaded to a board or wiki, oldest first, by the name its bodies give
    /// them: those no body or attachment shows any more too, until they are let go.
    ///
    /// # Errors
    ///
    /// Returns the store's error if the query fails.
    pub async fn files_of_container(
        &self,
        container_id: &str,
    ) -> Result<Vec<ListedFile>, AppError> {
        Ok(self
            .store()
            .get_files_of_container(container_id)
            .await?
            .into_iter()
            .map(ListedFile::from)
            .collect())
    }

    /// Stores an uploaded file for a board and returns the name it is stored under,
    /// which is what blocks refer to it by.
    ///
    /// The stored name is a new secret followed by the lower-cased extension of
    /// `filename` (`.jpeg` becomes `.jpg`). Nothing of the client's name ends up in a
    /// path: it is kept in the record only.
    ///
    /// The place is marked as being written before anything is written there, and the
    /// record takes the mark away in its transaction: a write or a record that fails,
    /// or a server that stops in between, leaves a mark that is doomed and removed,
    /// now or by the housekeeping, and no object that nothing names.
    ///
    /// # Errors
    ///
    /// Returns a server error if the place cannot be marked, if the file cannot be
    /// written within a day or its record cannot be written, and if the housekeeping
    /// gave the write up meanwhile.
    pub async fn save_file(
        &self,
        reader: &mut (dyn AsyncRead + Send + Unpin),
        container_id: &str,
        filename: &str,
    ) -> Result<String, AppError> {
        self.save_file_by(reader, container_id, filename, "").await
    }

    /// Verifies that storing `added_bytes` more in this handle's organization stays
    /// within `max_org_storage_bytes`.
    pub(crate) async fn check_org_storage_room(&self, added_bytes: u64) -> Result<(), AppError> {
        let max_bytes = self.limits().max_storage_bytes;
        let current = self.store().organization_storage_bytes().await?;
        let added = i64::try_from(added_bytes).unwrap_or(i64::MAX);
        if current.saturating_add(added) > max_bytes {
            let limit_u64 = u64::try_from(max_bytes).unwrap_or(u64::MAX);
            return Err(AppError::too_large(
                format_args!(
                    "storing the file would exceed the organization's storage limit of {max_bytes} bytes"
                ),
                limit_u64,
            )
            .with_reason(Reason::StorageFull));
        }
        Ok(())
    }

    /// [`App::save_file`], with who uploads the file: who its first version is by.
    /// Empty where nobody is known to.
    ///
    /// # Errors
    ///
    /// As [`App::save_file`].
    pub async fn save_file_by(
        &self,
        reader: &mut (dyn AsyncRead + Send + Unpin),
        container_id: &str,
        filename: &str,
        user_id: &str,
    ) -> Result<String, AppError> {
        self.check_org_storage_room(0).await?;
        let place = self.new_file_place(container_id, filename).await?;
        let written = self.mark_place(&place.path).await?.write(reader).await?;
        let size = written.size();
        if let Err(error) = self.check_org_storage_room(size).await {
            written.give_up().await;
            return Err(error);
        }
        let record = file_record(container_id, place, size, user_id);
        written.record(&record).await?;
        Ok(record.stored_name)
    }

    /// Pre-copies storage objects for `file_ids` from `source_app`'s `source_container_id`
    /// into `target_container_id` of `target_org_id` under `loose_objects` marks without
    /// writing any `files` rows yet.
    ///
    /// Checks `self.check_org_storage_room` before copying, and if any storage copy
    /// fails, gives up all already-copied objects and returns a 500 error so no partial
    /// state remains.
    pub(crate) async fn pre_copy_files_to_container(
        &self,
        source_app: &Self,
        source_container_id: &str,
        target_org_id: &str,
        target_container_id: &str,
        file_ids: &[String],
        created_by: &str,
    ) -> Result<(HashMap<String, String>, Vec<FileRecord>, Vec<WrittenPlace>), AppError> {
        let mut unique_ids = Vec::with_capacity(file_ids.len());
        let mut seen = HashSet::with_capacity(file_ids.len());
        for file_id in file_ids {
            if !file_id.is_empty() && seen.insert(file_id.as_str()) {
                unique_ids.push(file_id.as_str());
            }
        }
        if unique_ids.is_empty() {
            return Ok((HashMap::new(), Vec::new(), Vec::new()));
        }

        let mut sources = Vec::with_capacity(unique_ids.len());
        let mut total_bytes = 0_u64;
        for file_id in unique_ids {
            if let Some(source) = source_app.file_record(source_container_id, file_id).await? {
                total_bytes = total_bytes.saturating_add(u64::try_from(source.size).unwrap_or(0));
                sources.push((file_id.to_owned(), source));
            }
        }
        self.check_org_storage_room(total_bytes).await?;

        let mut file_map = HashMap::with_capacity(sources.len());
        let mut records = Vec::with_capacity(sources.len());
        let mut written_places: Vec<WrittenPlace> = Vec::with_capacity(sources.len());
        let now = now_millis();

        for (old_stored_name, source) in sources {
            let secret = new_secret();
            let key = FileKey {
                org_id: target_org_id,
                container_id: target_container_id,
                secret: &secret,
            };
            let copy = FileRecord {
                id: new_id(),
                container_id: target_container_id.to_owned(),
                stored_name: format!("{secret}{}", source.extension),
                path: key.to_string(),
                created_at: now,
                version: 1,
                created_by: created_by.to_owned(),
                ..source.clone()
            };
            let marked = match self.mark_place(&copy.path).await {
                Ok(marked) => marked,
                Err(error) => {
                    for place in written_places {
                        place.give_up().await;
                    }
                    return Err(error);
                }
            };
            let size = u64::try_from(source.size).unwrap_or(0);
            match marked.copy_from(&source.path, size).await {
                Ok(written) => {
                    file_map.insert(old_stored_name, copy.stored_name.clone());
                    records.push(copy);
                    written_places.push(written);
                }
                Err(error) => {
                    for place in written_places {
                        place.give_up().await;
                    }
                    return Err(AppError::internal(format_args!(
                        "unable to copy file in storage: {error}"
                    )));
                }
            }
        }

        Ok((file_map, records, written_places))
    }

    /// A new place for a file of `container_id` that the client calls `filename`
    /// ([`FilePlace::new`]), under the organization of the board's record.
    ///
    /// # Errors
    ///
    /// Returns a not-found error if the handle reaches no such board, and the store's
    /// error if the lookup fails.
    pub(crate) async fn new_file_place(
        &self,
        container_id: &str,
        filename: &str,
    ) -> Result<FilePlace, AppError> {
        let org_id = self.org_of_container(container_id).await?;
        Ok(FilePlace::new(&org_id, container_id, filename))
    }

    /// The organization of a board, which the keys of its files begin with.
    async fn org_of_container(&self, container_id: &str) -> Result<String, AppError> {
        self.store()
            .org_of_container(container_id)
            .await
            .map_err(|error| {
                if error.is_not_found() {
                    AppError::not_found("board")
                } else {
                    error.into()
                }
            })
    }

    /// A new key in the file storage for bytes of a file of `container_id`: where a
    /// version made by going back to an older one goes. A key says nothing of the
    /// file's name or extension.
    ///
    /// # Errors
    ///
    /// As [`App::new_file_place`].
    pub(super) async fn new_version_place(&self, container_id: &str) -> Result<String, AppError> {
        let org_id = self.org_of_container(container_id).await?;
        let secret = new_secret();
        let key = FileKey {
            org_id: &org_id,
            container_id,
            secret: &secret,
        };
        Ok(key.to_string())
    }

    /// Records the file of `size` bytes stored at `place` as the file of `upload`: of
    /// its board or wiki, by who opened it. Returns its stored name. For an object whose
    /// place the upload session holds, which is its mark: the place is not marked as
    /// being written.
    ///
    /// # Errors
    ///
    /// Returns the store's error if the record cannot be written.
    pub(crate) async fn record_file(
        &self,
        upload: &Upload,
        place: FilePlace,
        size: u64,
    ) -> Result<String, AppError> {
        let record = file_record(&upload.container_id, place, size, &upload.user_id);
        self.store().save_file_record(&record).await?;
        Ok(record.stored_name)
    }

    /// Stores the bytes of `upload`, read from `reader`, as [`App::save_file_by`] stores
    /// a file: of its board or wiki, under its name, by who opened it.
    ///
    /// # Errors
    ///
    /// As [`App::save_file`].
    pub(crate) async fn save_upload(
        &self,
        reader: &mut (dyn AsyncRead + Send + Unpin),
        upload: &Upload,
    ) -> Result<String, AppError> {
        let Upload {
            container_id,
            name,
            user_id,
            ..
        } = upload;
        self.save_file_by(reader, container_id, name, user_id).await
    }

    /// Returns the record of the file a board has under a stored name, or `None` if the
    /// board has no such file: there is none of that name, or it is another board's.
    ///
    /// # Errors
    ///
    /// Returns the store's error if the lookup fails.
    pub async fn file_record(
        &self,
        container_id: &str,
        stored_name: &str,
    ) -> Result<Option<FileRecord>, AppError> {
        if stored_name.is_empty() {
            return Ok(None);
        }
        match self
            .store()
            .get_file_record(container_id, stored_name)
            .await
        {
            Ok(record) => Ok(Some(record)),
            Err(error) if error.is_not_found() => Ok(None),
            Err(error) => Err(error.into()),
        }
    }

    /// Checks that every image and file block of a body names a file of the board. The write
    /// that stores the body asks again of the files it makes a block show newly, and holds
    /// their records until it commits ([`fvoci_store::NewBodyFiles`]): a file collected in
    /// between refuses it the same way.
    ///
    /// # Errors
    ///
    /// Returns a bad request that names `body` for a node naming a file the board does not
    /// have, and the store's error if a lookup fails.
    pub async fn check_body_files(
        &self,
        container_id: &str,
        body: Option<&Body>,
    ) -> Result<(), AppError> {
        if self.has_body_files(container_id, body).await? {
            Ok(())
        } else {
            Err(BlockError::BodyFileNotOfContainer.into())
        }
    }

    /// Whether a board has the file of every image and file block of a body.
    ///
    /// # Errors
    ///
    /// Returns the store's error if a lookup fails.
    pub(crate) async fn has_body_files(
        &self,
        container_id: &str,
        body: Option<&Body>,
    ) -> Result<bool, AppError> {
        let Some(body) = body else {
            return Ok(true);
        };
        for file_id in body.file_ids() {
            if self.file_record(container_id, file_id).await?.is_none() {
                return Ok(false);
            }
        }
        Ok(true)
    }

    /// Opens a board's file for reading, or returns `None` if the board has no file of
    /// that stored name or the file is gone from the storage.
    ///
    /// # Errors
    ///
    /// Returns a server error if the storage or the record lookup fails.
    pub(crate) async fn open_stored_file(
        &self,
        container_id: &str,
        stored_name: &str,
    ) -> Result<Option<crate::storage::FileStream>, AppError> {
        let Some(record) = self.file_record(container_id, stored_name).await? else {
            return Ok(None);
        };
        if !self.files().exists(&record.path).await? {
            return Ok(None);
        }
        Ok(Some(self.files().read(&record.path, None).await?))
    }

    /// Copies the files that duplicated blocks show or attach, and points each block at
    /// its copies.
    ///
    /// `container_id` is the board the blocks were copied from; `blocks` are the copies, as
    /// they are stored already, and are changed along with what is stored. A block whose
    /// file could not be copied ends up naming no file. The bodies pointed at their copies
    /// are part of the copy, not a change of it: a copy begins without versions of its
    /// body, as a copied page does. For blocks that are not stored yet, see
    /// [`App::copy_card_files_into`].
    ///
    /// # Errors
    ///
    /// Returns the store's error if the updated blocks cannot be written. Copy failures
    /// are logged and do not fail the call.
    pub async fn copy_and_update_card_files(
        &self,
        container_id: &str,
        user_id: &str,
        blocks: &mut [Block],
    ) -> Result<(), AppError> {
        // A failure to copy does not stop the duplication: it is logged, and the copied
        // blocks end up naming no file.
        let new_file_names = match self.copy_card_files(container_id, blocks).await {
            Ok(names) => names,
            Err(error) => {
                tracing::error!(
                    container_id,
                    %error,
                    "Could not copy files while duplicating board"
                );
                HashMap::new()
            }
        };
        let not_written = |error| {
            AppError::from(error).context(format_args!(
                "could not point copies at their files, of blocks of board {container_id}"
            ))
        };

        // The names of the copies, for the stored blocks: the bodies by the boards of
        // the copies, the attachments as a patch. Caller `blocks` are updated only
        // after size validation and store writes succeed.
        let mut bodies: BTreeMap<String, Vec<(String, Option<Body>)>> = BTreeMap::new();
        let mut renamed_bodies: HashMap<String, Option<Body>> = HashMap::new();
        let mut block_ids = Vec::new();
        let mut block_patches = Vec::new();
        for block in blocks.iter() {
            if block
                .body
                .as_ref()
                .is_some_and(|body| !body.file_ids().is_empty())
            {
                let renamed = block
                    .body
                    .clone()
                    .and_then(|body| rename_body_files(body, &new_file_names));
                if let Some(body) = &renamed {
                    body.check_size()
                        .map_err(|error| AppError::from_body_error("body", error))?;
                }
                renamed_bodies.insert(block.id.clone(), renamed.clone());
                bodies
                    .entry(block.container_id.clone())
                    .or_default()
                    .push((block.id.clone(), renamed));
                continue;
            }
            let Some(file_id) = block.attachment_file() else {
                continue;
            };
            let new_name = new_file_names.get(file_id).cloned().unwrap_or_default();
            block_ids.push(block.id.clone());
            let mut updated_fields = JsonMap::new();
            updated_fields.insert("fileId".to_owned(), Value::String(new_name));
            block_patches.push(BlockPatch {
                updated_fields,
                deleted_fields: vec!["attachmentId".to_owned()],
                ..BlockPatch::default()
            });
        }

        for (copies_container, renamed) in bodies {
            self.store()
                .replace_bodies_as_written(&copies_container, renamed)
                .await
                .map_err(not_written)?;
        }
        for block in blocks.iter_mut() {
            if let Some(renamed) = renamed_bodies.remove(&block.id) {
                block.body = renamed;
            }
        }
        if !block_ids.is_empty() {
            let batch = BlockPatchBatch {
                block_ids,
                block_patches,
            };
            self.store()
                .patch_blocks(&batch, user_id)
                .await
                .map_err(not_written)?;
            // The patch moved `updatedAt` on: the caller's copies are what is answered and
            // announced, and a client edits them with that `updatedAt` as `readAt`.
            let patched: HashSet<&str> = batch.block_ids.iter().map(String::as_str).collect();
            for block in blocks.iter_mut() {
                if patched.contains(block.id.as_str()) {
                    *block = self.store().get_block(&block.id).await?;
                }
            }
        }
        Ok(())
    }

    /// Copies the files that attachment blocks refer to, for blocks that are
    /// about to be stored as copies of blocks of `source_container_id`, and points each block
    /// at its copy. Nothing is written to the blocks' store: the caller stores them.
    ///
    /// A block whose file is not the source board's, or could not be copied, ends up
    /// naming no file. Failures are logged and do not fail the call.
    pub async fn copy_card_files_into(&self, source_container_id: &str, blocks: &mut [Block]) {
        let new_file_names = match self.copy_card_files(source_container_id, blocks).await {
            Ok(names) => names,
            Err(error) => {
                tracing::error!(
                    source_container_id,
                    %error,
                    "could not copy the files of new blocks"
                );
                HashMap::new()
            }
        };
        for block in blocks.iter_mut() {
            block.body = block
                .body
                .take()
                .and_then(|body| rename_body_files(body, &new_file_names));
            let Some(file_id) = block.attachment_file() else {
                continue;
            };
            let new_name = new_file_names.get(file_id).cloned().unwrap_or_default();
            block
                .fields
                .insert("fileId".to_owned(), Value::String(new_name));
            block.fields.remove("attachmentId");
        }
    }

    /// Copies each file referred to by an attachment block and returns a map
    /// from the original stored names to those of the copies. Only files of the source
    /// board are copied.
    async fn copy_card_files(
        &self,
        source_container_id: &str,
        blocks: &[Block],
    ) -> Result<HashMap<String, String>, AppError> {
        let source_container = self.container(source_container_id).await?;

        let mut new_file_names = HashMap::new();
        let mut destination: Option<Container> = None;
        // The organization of the board copies went to last, by its identifier.
        let mut destination_org: Option<(String, String)> = None;
        for (block, file_id) in blocks
            .iter()
            .flat_map(|block| block.shown_files().into_iter().map(move |id| (block, id)))
        {
            // A file that two blocks show, or one body twice, is copied once.
            if new_file_names.contains_key(&file_id) {
                continue;
            }

            // Most copies stay on the source board; a destination is looked up once.
            let destination_container = if block.container_id == source_container.id {
                &source_container
            } else {
                if destination
                    .as_ref()
                    .is_none_or(|container: &Container| container.id != block.container_id)
                {
                    // Of the workspace a copy goes to, which may be another than the
                    // handle's (checked by the caller): read so, not by `App::container`.
                    let container = self.store().get_container(&block.container_id).await;
                    destination = Some(container.map_err(|error| {
                        if error.is_not_found() {
                            AppError::not_found("board")
                        } else {
                            error.into()
                        }
                    })?);
                }
                destination
                    .as_ref()
                    .expect("the destination board was fetched just above")
            };

            // A block can name any file. One that is not the source board's is not
            // copied; the copied block ends up naming no file.
            let Some(source) = self.file_record(&source_container.id, &file_id).await? else {
                tracing::warn!(
                    source_container_id,
                    block_id = %block.id,
                    "not copying a file that the source board does not have"
                );
                continue;
            };

            let org_id = match &destination_org {
                Some((container, org)) if *container == destination_container.id => org.clone(),
                _ => {
                    let org = self.org_of_container(&destination_container.id).await?;
                    destination_org = Some((destination_container.id.clone(), org.clone()));
                    org
                }
            };
            if let Some(stored_name) = self
                .copy_file(&source, &org_id, &destination_container.id)
                .await
            {
                new_file_names.insert(file_id, stored_name);
            }
        }
        Ok(new_file_names)
    }

    /// Copies the file of the record `source` to a new key of `container_id`, of the
    /// organization `org_id`, and records the copy; returns its stored name, which keeps
    /// the extension of its source's record.
    ///
    /// The mark, the bytes, then the record that says where they are and takes the mark
    /// away. A file that could not be copied gets no record and no name (`None`, the
    /// failure logged): the block that pointed at it ends up naming no file, not a file
    /// that is not there.
    async fn copy_file(
        &self,
        source: &FileRecord,
        org_id: &str,
        container_id: &str,
    ) -> Option<String> {
        let secret = new_secret();
        let key = FileKey {
            org_id,
            container_id,
            secret: &secret,
        };
        let copy = FileRecord {
            id: new_id(),
            container_id: container_id.to_owned(),
            stored_name: format!("{secret}{}", source.extension),
            path: key.to_string(),
            created_at: now_millis(),
            // A file of its own, with the newest bytes of the one it copies as its
            // only version, by nobody.
            version: 1,
            created_by: String::new(),
            ..source.clone()
        };
        let marked = match self.mark_place(&copy.path).await {
            Ok(marked) => marked,
            Err(error) => {
                tracing::error!(%error, "card file copy could not mark destination");
                return None;
            }
        };
        let size = u64::try_from(source.size).unwrap_or(0);
        let written = match marked.copy_from(&source.path, size).await {
            Ok(written) => written,
            Err(error) => {
                tracing::error!(
                    %error,
                    source_path = source.path,
                    destination_path = copy.path,
                    "card file copy failed to copy file"
                );
                return None;
            }
        };
        if let Err(error) = written.record(&copy).await {
            tracing::error!(%error, "card file copy could not record destination");
            return None;
        }
        Some(copy.stored_name)
    }
}

/// The record of a file of `container_id` of `size` bytes stored at `place`, made now,
/// that `user_id` uploaded (empty for nobody known).
pub(super) fn file_record(
    container_id: &str,
    place: FilePlace,
    size: u64,
    user_id: &str,
) -> FileRecord {
    FileRecord {
        id: new_id(),
        container_id: container_id.to_owned(),
        stored_name: place.stored_name,
        path: place.path,
        size: size_to_i64(size),
        name: place.name,
        extension: place.extension,
        created_at: now_millis(),
        version: 1,
        created_by: user_id.to_owned(),
    }
}

/// Points the images and file blocks of a copied body at the copies of their files. One
/// whose file was not copied becomes an empty paragraph: it would show a file of
/// another board. `None` if that leaves no body.
fn rename_body_files(body: Body, new_file_names: &HashMap<String, String>) -> Option<Body> {
    body.with_files_renamed(|old| new_file_names.get(old).cloned())
}

/// The last path segment of a client-supplied name, so that a name with directory parts
/// cannot escape its directory.
pub(crate) fn base_name(name: &str) -> &str {
    name.rsplit('/').next().unwrap_or(name)
}

/// The lower-cased extension of a name, with its leading dot, as kept in the record.
/// `.jpeg` is stored as `.jpg`.
pub(crate) fn stored_extension(name: &str) -> String {
    let extension = match name.rfind('.') {
        Some(index) => name[index..].to_lowercase(),
        None => String::new(),
    };
    // What ends up in a stored name and a path is plain: an extension with anything
    // else in it is dropped from the name. The record keeps the client's name.
    let plain = extension.len() <= MAX_EXTENSION_LENGTH
        && extension
            .bytes()
            .skip(1)
            .all(|byte| byte.is_ascii_alphanumeric());
    match extension.as_str() {
        _ if !plain => String::new(),
        ".jpeg" => ".jpg".to_owned(),
        _ => extension,
    }
}

/// The longest extension, dot included, that a stored name takes over from an upload.
const MAX_EXTENSION_LENGTH: usize = 17;

fn size_to_i64(size: u64) -> i64 {
    i64::try_from(size).unwrap_or(i64::MAX)
}

/// Returns a storage error as a not-found when it is one, so callers can tell a missing
/// file from a real failure.
impl From<StorageError> for AppError {
    fn from(error: StorageError) -> Self {
        match error {
            StorageError::NotFound(_) => {
                tracing::debug!(%error, "not found in the file storage");
                Self::not_found(FILE)
            }
            other => Self::internal(other),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stored_extension_takes_the_last_dot_lower_cased() {
        assert_eq!(stored_extension("a.tar.GZ"), ".gz", "the last extension");
        assert_eq!(stored_extension("noext"), "", "no extension");
        assert_eq!(stored_extension("secret.JPEG"), ".jpg", "jpeg becomes jpg");
        assert_eq!(stored_extension("photo.png"), ".png", "others are kept");
    }

    #[test]
    fn an_extension_that_is_not_plain_stays_out_of_the_stored_name() {
        for name in [
            "x.p/ng",
            "x.png ",
            "x.a b",
            "x.%2e",
            "x.ä",
            "x.aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
        ] {
            assert_eq!(stored_extension(name), "", "name {name:?}");
        }
    }

    #[test]
    fn a_new_place_is_a_key_of_its_organization_and_board() {
        let org = "01890000-0000-7000-8000-00000000000a";
        let board = "01890000-0000-7000-8000-000000000001";
        let place = FilePlace::new(org, board, "../My Photo.JPEG");
        let key = FileKey::parse(&place.path).expect("a key of the server's making");
        assert_eq!(
            (key.org_id, key.container_id),
            (org, board),
            "{}",
            place.path
        );
        assert!(!place.path.contains("Photo"), "no name in {}", place.path);
        assert!(!place.path.contains('.'), "no extension in {}", place.path);
        assert_eq!(place.stored_name, format!("{}.jpg", key.secret));
        assert_eq!(place.name, "My Photo.JPEG");
        assert_eq!(
            FilePlace::at(&place.path, "../My Photo.JPEG"),
            place,
            "read back"
        );
        let plain = FilePlace::new(org, board, "noext");
        assert_eq!(
            plain.stored_name,
            plain.path.rsplit('/').next().unwrap_or_default()
        );
        assert_ne!(
            FilePlace::new(org, board, "noext").path,
            plain.path,
            "a new secret"
        );
    }

    #[test]
    fn base_name_keeps_only_the_last_segment() {
        assert_eq!(
            base_name("../../evil.sh"),
            "evil.sh",
            "traversal is stripped"
        );
        assert_eq!(base_name("plain.png"), "plain.png", "a plain name is kept");
        assert_eq!(
            base_name(r"c:\dir\x.png"),
            r"c:\dir\x.png",
            "backslashes stay"
        );
    }
}
