//! Staged in-memory representation of a converted archive and its atomic commit path.
//!
//! External converters (Trello JSON, Notion ZIP) build a [`StagedArchive`]. Before
//! anything touches the database or file storage, [`App::commit_staged_archive`] verifies
//! caller permissions, runs the exact validation rules of the native import pipeline
//! (`check_storable`, `places::check`, value cleanup, orphan pruning, identifier renewal,
//! `block.validate`), writes any local bundle files to marked storage locations, and
//! commits all containers, files, blocks, and creator admin memberships in a single
//! database transaction. Any failure cleans up every written storage object and rolls
//! back the database transaction so nothing is left behind.

use std::collections::HashMap;
use std::path::Path;

use fvoci_model::{
    Block, BlockType, Container, ContainerKind, ContainersAndBlocks, ContainersAndBlocksError,
    FileRecord, WorkspaceRole, renew_containers_and_blocks_ids,
};
use tokio::fs;

use super::bundle::{BundleEntry, SafeBundle};
use super::summary::{BundleLimits, ImportCounts, ImportSummary};
use super::{ImportOptions, clear_values_of_another_kind, drop_orphans, places};
use crate::app::App;
use crate::app::container_copies::counted_as;
use crate::app::doomed::Doomed;
use crate::app::file_name::check_file_name;
use crate::app::files::{FilePlace, file_record, stored_extension};
use crate::app::loose_objects::WrittenPlace;
use crate::auth::now_millis;
use crate::auth::secret::new_secret;
use crate::collab::AccessScope;
use crate::error::{AppError, Reason};
use crate::metrics::ContainerWork;
use fvoci_store::StoreError;

/// A staged file belonging to a [`StagedContainer`], referencing a validated bundle entry.
#[derive(Debug, Clone)]
pub(crate) struct StagedFile {
    /// Synthetic single-segment placeholder used as `fileId` in staged block bodies.
    pub(crate) placeholder: String,
    /// Human-readable file name (verified with `check_file_name`).
    pub(crate) original_name: String,
    /// Source entry inside the validated [`SafeBundle`].
    pub(crate) entry: BundleEntry,
}

/// One converted board or wiki with its blocks and local files, ready to validate and store.
#[derive(Debug)]
pub(crate) struct StagedContainer {
    pub(crate) container: Container,
    pub(crate) blocks: Vec<Block>,
    pub(crate) files: Vec<StagedFile>,
}

/// Complete converted output of an external import, before validation and persistence.
#[derive(Debug)]
pub(crate) struct StagedArchive {
    pub(crate) containers: Vec<StagedContainer>,
    pub(crate) summary: ImportSummary,
}

/// File prepared for writing to `FileBackend` before the single DB commit transaction.
struct PreparedFile {
    record: FileRecord,
    entry: BundleEntry,
}

impl App {
    /// Verifies that `user_id` is a non-guest member of `workspace_id` within this handle's
    /// organization and returns that organization's identifier.
    ///
    /// # Errors
    ///
    /// Returns 404 if the workspace is outside this handle's reach, does not exist, or
    /// `user_id` is not a non-guest member of it.
    pub(super) async fn require_import_workspace(
        &self,
        workspace_id: &str,
        user_id: &str,
    ) -> Result<String, AppError> {
        if !self.reaches_workspace(workspace_id) {
            return Err(AppError::not_found("workspace"));
        }
        let Some(org_id) = self.store().tenant().org_id() else {
            return Err(AppError::not_found("workspace"));
        };
        self.store()
            .get_workspace(workspace_id)
            .await
            .map_err(|error| {
                if error.is_not_found() {
                    AppError::not_found("workspace")
                } else {
                    error.into()
                }
            })?;
        let actor = self.actor(user_id, workspace_id).await?;
        match actor.workspace {
            Some(role) if role != WorkspaceRole::Guest => Ok(org_id),
            _ => Err(AppError::not_found("workspace")),
        }
    }

    /// Validates and atomically commits a [`StagedArchive`] into the workspace.
    ///
    /// If `bundle` is `Some`, staged files are streamed from it one at a time via a
    /// temporary file and written to `FileBackend` under `loose_objects` marks before the
    /// single database transaction commits. If any validation, file write, or database
    /// statement fails, all written objects are cleaned up and nothing is stored.
    ///
    /// # Errors
    ///
    /// Returns 404 if the workspace or permissions check fails, 400 or 413 if any staged
    /// container, block, or file fails validation or exceeds limits, or 500 on storage or
    /// database failure.
    pub(super) async fn commit_staged_archive(
        &self,
        staged: StagedArchive,
        bundle: Option<&SafeBundle>,
        options: &ImportOptions,
        limits: &BundleLimits,
    ) -> Result<ImportSummary, AppError> {
        let org_id = self
            .require_import_workspace(&options.workspace_id, &options.modified_by)
            .await?;

        let (mut combined, prepared_files, mut summary) =
            prepare_staged_off_thread(staged, options, *limits, org_id).await?;

        for container in &combined.containers {
            self.require_makes(container)?;
        }

        let total_new_bytes = prepared_files.iter().fold(0_u64, |acc, file| {
            acc.saturating_add(u64::try_from(file.record.size).unwrap_or(0))
        });
        self.check_org_storage_room(total_new_bytes).await?;

        let written_places = self
            .write_staged_files(bundle, &prepared_files, limits.max_file_bytes)
            .await?;

        let records: Vec<FileRecord> = prepared_files
            .iter()
            .map(|file| file.record.clone())
            .collect();
        let commit_result = self
            .store()
            .create_imported_containers_blocks_and_files(
                &mut combined,
                &records,
                &options.modified_by,
            )
            .await;

        let members = match commit_result {
            Ok(members) => {
                for place in written_places {
                    place.settled_in_transaction();
                }
                members
            }
            Err(error) => {
                for place in written_places {
                    place.give_up().await;
                }
                return Err(AppError::from(error).context("error committing staged import"));
            }
        };

        if !combined.containers.is_empty() {
            let made = combined.containers.iter().map(|container| &container.id);
            self.collab()
                .recheck_access(self, AccessScope::containers(made))
                .await;
        }
        self.announce_created(&combined, &members).await;

        for container in &combined.containers {
            if !container.is_template
                && let Err(error) = self
                    .add_containers_to_default_category(
                        &options.modified_by,
                        &container.workspace_id,
                        std::slice::from_ref(&container.id),
                    )
                    .await
            {
                tracing::warn!(
                    container_id = %container.id,
                    %error,
                    "failed to add imported container to default sidebar category after commit"
                );
            }
            self.metrics()
                .container_copied(ContainerWork::Import, counted_as(container.kind));
        }

        summary.imported = count_imported(&combined, records.len());
        summary.container_ids = combined
            .containers
            .into_iter()
            .map(|container| container.id)
            .collect();
        Ok(summary)
    }

    /// Streams every prepared file from `bundle` to `FileBackend` under `writing` marks.
    /// Cleans up every marked object on the first failure.
    async fn write_staged_files(
        &self,
        bundle: Option<&SafeBundle>,
        files: &[PreparedFile],
        max_file_bytes: u64,
    ) -> Result<Vec<WrittenPlace>, AppError> {
        if files.is_empty() {
            return Ok(Vec::new());
        }
        let Some(bundle) = bundle else {
            return Err(AppError::internal(
                "staged import declared files without a source bundle",
            ));
        };
        let work_dir = self
            .config()
            .tmp_dir()
            .join(format!("import-stage-{}", new_secret()));
        fs::create_dir_all(&work_dir).await.map_err(|error| {
            AppError::internal(format_args!("cannot create staging directory: {error}"))
        })?;
        let doomed_dir = Doomed::directory(work_dir);
        let temp_file = doomed_dir.path().join("entry");

        let mut written_places: Vec<WrittenPlace> = Vec::with_capacity(files.len());
        for prepared in files {
            match self
                .write_one_staged_file(bundle, prepared, max_file_bytes, &temp_file)
                .await
            {
                Ok(written) => written_places.push(written),
                Err(error) => {
                    for place in written_places {
                        place.give_up().await;
                    }
                    doomed_dir.remove().await;
                    return Err(error);
                }
            }
        }
        doomed_dir.remove().await;
        Ok(written_places)
    }

    async fn write_one_staged_file(
        &self,
        bundle: &SafeBundle,
        prepared: &PreparedFile,
        max_file_bytes: u64,
        temp_file: &Path,
    ) -> Result<WrittenPlace, AppError> {
        let written_bytes = bundle
            .unpack_to_file(&prepared.entry, max_file_bytes, temp_file)
            .await?;
        if written_bytes > max_file_bytes {
            return Err(AppError::too_large(
                format_args!("staged file exceeds {max_file_bytes} bytes"),
                max_file_bytes,
            )
            .with_reason(Reason::ArchiveEntryTooLarge));
        }
        let marked = self.mark_place(&prepared.record.path).await?;
        let mut reader = match fs::File::open(temp_file).await {
            Ok(file) => file,
            Err(error) => {
                marked.give_up_unused().await;
                return Err(AppError::internal(format_args!(
                    "cannot open staged file: {error}"
                )));
            }
        };
        let written = marked.write(&mut reader).await;
        let _ = fs::remove_file(temp_file).await;
        written.map_err(|error| {
            AppError::internal(format_args!("unable to store imported file: {error}"))
        })
    }
}

/// Runs [`prepare_staged_for_commit`] off the async runtime so validating and renewing IDs
/// for large imports does not block Tokio worker threads.
async fn prepare_staged_off_thread(
    staged: StagedArchive,
    options: &ImportOptions,
    limits: BundleLimits,
    org_id: String,
) -> Result<(ContainersAndBlocks, Vec<PreparedFile>, ImportSummary), AppError> {
    let blocking_opts = ImportOptions {
        workspace_id: options.workspace_id.clone(),
        modified_by: options.modified_by.clone(),
        page_limits: options.page_limits,
    };
    tokio::task::spawn_blocking(move || {
        prepare_staged_for_commit(staged, &blocking_opts, &limits, &org_id)
    })
    .await
    .map_err(|error| {
        AppError::internal(format_args!(
            "staged import validation task failed: {error}"
        ))
    })?
}

/// Validates every staged container, block, and file in memory (zero side effects),
/// renews all identifiers, assigns storage places, and points block bodies at stored names.
fn prepare_staged_for_commit(
    staged: StagedArchive,
    options: &ImportOptions,
    limits: &BundleLimits,
    org_id: &str,
) -> Result<(ContainersAndBlocks, Vec<PreparedFile>, ImportSummary), AppError> {
    let StagedArchive {
        containers: staged_containers,
        summary,
    } = staged;
    let (combined, staged_files_by_index) =
        validate_and_merge_staged(staged_containers, options, limits)?;
    // Renew all IDs together so cross-container references (e.g. Wiki -> Board card) remap cleanly.
    let mut renewed = renew_containers_and_blocks_ids(combined);
    let prepared_files =
        bind_staged_files(&mut renewed, staged_files_by_index, options, limits, org_id)?;
    Ok((renewed, prepared_files, summary))
}

fn validate_and_merge_staged(
    staged_containers: Vec<StagedContainer>,
    options: &ImportOptions,
    limits: &BundleLimits,
) -> Result<(ContainersAndBlocks, Vec<Vec<StagedFile>>), AppError> {
    if staged_containers.is_empty() {
        return Err(AppError::bad_request(
            "the import contains no boards or wikis to create",
        ));
    }
    let total_blocks: usize = staged_containers.iter().map(|item| item.blocks.len()).sum();
    let total_files: usize = staged_containers.iter().map(|item| item.files.len()).sum();
    let total_items = u64::try_from(staged_containers.len())
        .unwrap_or(u64::MAX)
        .saturating_add(u64::try_from(total_blocks).unwrap_or(u64::MAX))
        .saturating_add(u64::try_from(total_files).unwrap_or(u64::MAX));
    if total_items > limits.max_entries {
        return Err(AppError::too_large(
            format_args!("import produces more than {} items", limits.max_entries),
            limits.max_entries,
        )
        .with_reason(Reason::ArchiveTooLarge));
    }

    let now = now_millis();
    let mut combined = ContainersAndBlocks::default();
    let mut files_by_index = Vec::with_capacity(staged_containers.len());
    for mut item in staged_containers {
        stamp_container_and_blocks(&mut item.container, &mut item.blocks, options, now);
        let mut single = ContainersAndBlocks {
            containers: vec![item.container],
            blocks: item.blocks,
        };
        for container in &single.containers {
            container
                .check_storable()
                .map_err(|error| AppError::from(StoreError::InvalidContainer(error)))?;
        }
        places::check(&single, options.page_limits)?;
        clear_values_of_another_kind(&mut single);
        drop_orphans(&mut single);
        match single.validate() {
            Ok(()) | Err(ContainersAndBlocksError::NoBlocks) => {}
            Err(error) => {
                return Err(AppError::bad_request(format_args!(
                    "imported boards and blocks do not fit together: {error}"
                )));
            }
        }
        combined.containers.append(&mut single.containers);
        combined.blocks.append(&mut single.blocks);
        files_by_index.push(item.files);
    }
    Ok((combined, files_by_index))
}

fn stamp_container_and_blocks(
    container: &mut Container,
    blocks: &mut [Block],
    options: &ImportOptions,
    now: i64,
) {
    container.workspace_id.clone_from(&options.workspace_id);
    container.created_by.clone_from(&options.modified_by);
    container.modified_by.clone_from(&options.modified_by);
    container.updated_at = now;
    container.deleted_at = None;

    for block in blocks {
        block.container_id.clone_from(&container.id);
        block.created_by.clone_from(&options.modified_by);
        block.modified_by.clone_from(&options.modified_by);
        block.updated_at = now;
        block.deleted_at = None;
    }
}

fn bind_staged_files(
    renewed: &mut ContainersAndBlocks,
    staged_files_by_index: Vec<Vec<StagedFile>>,
    options: &ImportOptions,
    limits: &BundleLimits,
    org_id: &str,
) -> Result<Vec<PreparedFile>, AppError> {
    let mut prepared_files = Vec::new();
    let mut file_maps: HashMap<String, HashMap<String, String>> = HashMap::new();
    for (container, files) in renewed.containers.iter().zip(staged_files_by_index) {
        let mut rename_map = HashMap::with_capacity(files.len());
        for staged_file in files {
            if staged_file.entry.unpacked_bytes > limits.max_file_bytes {
                return Err(AppError::too_large(
                    format_args!(
                        "imported file {} exceeds {} bytes",
                        staged_file.original_name, limits.max_file_bytes
                    ),
                    limits.max_file_bytes,
                )
                .with_reason(Reason::ArchiveEntryTooLarge));
            }
            let safe_name = sanitize_upload_name(&staged_file.original_name);
            let place = FilePlace::new(org_id, &container.id, &safe_name);
            let record = file_record(
                &container.id,
                place,
                staged_file.entry.unpacked_bytes,
                &options.modified_by,
            );
            rename_map.insert(staged_file.placeholder, record.stored_name.clone());
            prepared_files.push(PreparedFile {
                record,
                entry: staged_file.entry,
            });
        }
        file_maps.insert(container.id.clone(), rename_map);
    }

    for block in &mut renewed.blocks {
        if let Some(body) = block.body.take() {
            let map = file_maps.get(&block.container_id);
            block.body = if body.file_ids().is_empty() {
                Some(body)
            } else {
                body.with_files_renamed(|old| map.and_then(|names| names.get(old).cloned()))
            };
        }
        block.validate().map_err(|source| {
            AppError::from(StoreError::InvalidBlock {
                id: block.id.clone(),
                source,
            })
        })?;
    }
    Ok(prepared_files)
}

/// Ensures a file name from a bundle satisfies [`check_file_name`], preserving its plain
/// extension when falling back.
fn sanitize_upload_name(name: &str) -> String {
    if check_file_name(name).is_ok() {
        return name.to_owned();
    }
    let ext = stored_extension(name);
    format!("attachment{ext}")
}

fn count_imported(combined: &ContainersAndBlocks, files: usize) -> ImportCounts {
    let mut counts = ImportCounts {
        files,
        ..ImportCounts::default()
    };
    for container in &combined.containers {
        match container.kind {
            ContainerKind::Board => counts.boards += 1,
            ContainerKind::Wiki => counts.wikis += 1,
            ContainerKind::Course => counts.courses += 1,
            ContainerKind::Diary => {}
        }
    }
    for block in &combined.blocks {
        match block.block_type {
            BlockType::Card => counts.cards += 1,
            BlockType::Page => counts.pages += 1,
            BlockType::Unit => counts.units += 1,
            BlockType::Comment => counts.comments += 1,
            _ => {}
        }
    }
    counts
}
