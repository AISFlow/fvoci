//! Atomic, bounded-memory staging and single-transaction commit for native fvoci v1
//! archives.
//!
//! After Pass 1 (`native_stream::scan_native_board`) and Pass 1.5 (`build_write_plan` +
//! `recheck_planned_fields`) have validated every container, block, and structural rule,
//! this module resolves kept memberships, stages all archive files into `FileBackend`
//! under `loose_objects` marks using a temporary `Doomed::directory`, and then streams
//! all planned boards' blocks (with body file names, attachments, and cross-board
//! references pointed in-flight) together with all containers, file records, and
//! memberships into one single database transaction.
//!
//! If any file write, validation, limit check (such as `DIARIES_PER_SPACE` or duplicate
//! diary days), or database statement fails, the transaction rolls back completely and
//! every temporary staging file and staged object in `FileBackend` is cleaned up.

use std::collections::{HashMap, HashSet};
use std::path::Path;
use std::sync::Arc;

use fvoci_model::{Container, ContainerKind, ContainerMember, FileRecord, ValidatedCourseSchedule};
use fvoci_store::{StreamBatch, StreamCreateError};
use tokio::fs;

use super::archive::{Archive, entry_name, unpack_entry};
use super::native_stream::{BoardWritePlan, StreamPlan, too_many_archive_items};
use super::native_stream_write::{
    ArchiveStreamShared, BlockIdCollection, IMPORT_STREAM_TIMEOUT, StreamedBoardHandle,
    StreamedBoardStats, spawn_archive_streamer,
};
use super::{
    ANNOUNCE_CHUNK_BLOCKS, ArchiveFiles, ImportCounts, ImportOptions, Imported, name_in,
    no_more_than_holdable, tally_container_counts,
};
use crate::app::App;
use crate::app::container_copies::counted_as;
use crate::app::doomed::Doomed;
use crate::app::files::{FilePlace, file_record};
use crate::app::loose_objects::WrittenPlace;
use crate::auth::secret::new_secret;
use crate::collab::AccessScope;
use crate::error::AppError;
use crate::metrics::{ContainerWork, CourseSource};

/// Total expanded `container_members` ceiling across an entire `.fvoci` import.
pub(super) const MAX_IMPORTED_MEMBERSHIPS: usize = 50_000;

/// Prepared metadata across every planned board in a native archive.
struct PreparedStreamedArchive {
    containers: Vec<Container>,
    extra_members: Vec<ContainerMember>,
    schedules: Vec<(usize, ValidatedCourseSchedule)>,
    by_dir: HashMap<String, String>,
    files: HashMap<String, ArchiveFiles>,
    created_order: Vec<(ContainerKind, String)>,
    stream_plans: Vec<(usize, StreamPlan)>,
    members_left_out: usize,
}

/// Files staged and written to `FileBackend` prior to the single database transaction.
struct StagedNativeFiles {
    records: Vec<FileRecord>,
    written_places: Vec<WrittenPlace>,
    /// Container ID -> original archive filename -> stored filename.
    file_map: Arc<HashMap<String, HashMap<String, String>>>,
}

impl App {
    /// Stages archive files and commits all planned boards, blocks, file records, and
    /// memberships atomically in a single streamed database transaction.
    ///
    /// # Errors
    ///
    /// Returns 400/413 on archive validation or limit errors, 404 if the workspace is
    /// unreachable, or 500 on storage or database failure. On any error, zero
    /// containers, blocks, file records, stored objects, or temporary staging files stay
    /// behind.
    pub(super) async fn commit_streamed_native_archive(
        &self,
        zip: &Archive,
        plans: Vec<BoardWritePlan>,
        all_copies: Arc<HashMap<String, String>>,
        options: &ImportOptions,
        largest_file: u64,
    ) -> Result<(Imported, ImportCounts, Vec<String>), AppError> {
        if plans.is_empty() {
            return Ok((
                Imported {
                    members_left_out: 0,
                },
                ImportCounts::default(),
                Vec::new(),
            ));
        }
        let Some(org_id) = self.store().tenant().org_id() else {
            return Err(AppError::not_found("workspace"));
        };
        let mut prepared = self.prepare_streamed_archive(plans, options).await?;
        let staged = self
            .stage_native_files(
                zip,
                largest_file,
                &org_id,
                &prepared.by_dir,
                &prepared.files,
            )
            .await?;
        let staged_file_count = staged.records.len();

        let collect_ids = if self.ws().has_workspace_listeners(&options.workspace_id)
            || !self.config().block_change_webhooks.is_empty()
        {
            BlockIdCollection::All
        } else {
            BlockIdCollection::None
        };
        let StreamedBoardHandle {
            rx,
            stats: streamer_handle,
        } = spawn_archive_streamer(
            zip.clone(),
            std::mem::take(&mut prepared.stream_plans),
            ArchiveStreamShared {
                file_map: Arc::clone(&staged.file_map),
                all_copies,
                modified_by: options.modified_by.clone(),
                collect_ids,
            },
        );

        let (members, mut stats) = self
            .run_timed_archive_stream(
                &mut prepared,
                staged,
                &options.modified_by,
                rx,
                streamer_handle,
            )
            .await?;

        self.finish_streamed_native_commit(
            &prepared.containers,
            &members,
            &stats.inserted_block_ids,
        )
        .await;
        tracing::debug!(
            containers_imported = prepared.containers.len(),
            "import archive - done"
        );
        stats.counts.files = staged_file_count;
        let container_ids = tally_container_counts(&prepared.created_order, &mut stats.counts);
        Ok((
            Imported {
                members_left_out: prepared.members_left_out,
            },
            stats.counts,
            container_ids,
        ))
    }

    async fn run_timed_archive_stream(
        &self,
        prepared: &mut PreparedStreamedArchive,
        staged: StagedNativeFiles,
        modified_by: &str,
        rx: tokio::sync::mpsc::Receiver<StreamBatch<AppError>>,
        streamer_handle: tokio::task::JoinHandle<StreamedBoardStats>,
    ) -> Result<(Vec<ContainerMember>, StreamedBoardStats), AppError> {
        let StagedNativeFiles {
            records,
            written_places,
            file_map: _,
        } = staged;
        let write_future = self
            .store()
            .create_imported_containers_blocks_files_members_and_schedules_streamed(
                &mut prepared.containers,
                &records,
                &prepared.extra_members,
                &prepared.schedules,
                modified_by,
                rx,
            );
        let Ok(write_result) = tokio::time::timeout(IMPORT_STREAM_TIMEOUT, write_future).await
        else {
            streamer_handle.abort();
            clean_written_places(written_places).await;
            return Err(AppError::bad_request(
                "archive import exceeded maximum transaction time limit",
            ));
        };
        let streamer_outcome = streamer_handle.await;
        let members = match write_result {
            Ok(members) => {
                for place in written_places {
                    place.settled_in_transaction();
                }
                members
            }
            Err(error) => {
                clean_written_places(written_places).await;
                return Err(map_stream_write_error(error));
            }
        };
        let stats = streamer_outcome.map_err(|error| {
            AppError::internal(format_args!("archive stream task failed: {error}"))
        })?;
        Ok((members, stats))
    }

    async fn prepare_streamed_archive(
        &self,
        plans: Vec<BoardWritePlan>,
        options: &ImportOptions,
    ) -> Result<PreparedStreamedArchive, AppError> {
        let mut containers = Vec::with_capacity(plans.len());
        let mut extra_members = Vec::new();
        let mut schedules = Vec::new();
        let mut by_dir = HashMap::with_capacity(plans.len());
        let mut files = HashMap::with_capacity(plans.len());
        let mut created_order = Vec::with_capacity(plans.len());
        let mut stream_plans = Vec::with_capacity(plans.len());
        let mut left_out: HashSet<String> = HashSet::new();
        let mut actor_cache: HashMap<String, Option<fvoci_model::WorkspaceRole>> = HashMap::new();

        for plan in plans {
            let Some(primary) = plan.containers.first() else {
                return Err(AppError::bad_request("missing board in archive"));
            };
            if by_dir
                .insert(plan.dir.clone(), primary.id.clone())
                .is_some()
            {
                return Err(AppError::bad_request(format_args!(
                    "duplicate board directory in archive: {}",
                    plan.dir
                )));
            }
            files.insert(plan.dir, plan.files);
            let mut kept_members = Vec::new();
            let mut seen_in_plan: HashSet<String> = HashSet::new();
            for member in plan.members {
                if !seen_in_plan.insert(member.user_id.clone()) {
                    continue;
                }
                if !fvoci_model::is_id(&member.user_id) {
                    left_out.insert(member.user_id);
                    continue;
                }
                let workspace_role = if let Some(cached) = actor_cache.get(&member.user_id) {
                    *cached
                } else {
                    let resolved = self
                        .actor(&member.user_id, &options.workspace_id)
                        .await?
                        .workspace;
                    actor_cache.insert(member.user_id.clone(), resolved);
                    resolved
                };
                match workspace_role {
                    Some(workspace_role) => {
                        kept_members.push(no_more_than_holdable(member, workspace_role));
                    }
                    None => {
                        left_out.insert(member.user_id);
                    }
                }
            }
            let added_memberships = kept_members.len().saturating_mul(plan.containers.len());
            if extra_members.len().saturating_add(added_memberships) > MAX_IMPORTED_MEMBERSHIPS {
                return Err(too_many_archive_items(
                    "memberships",
                    MAX_IMPORTED_MEMBERSHIPS,
                ));
            }
            for container in &plan.containers {
                created_order.push((container.kind, container.id.clone()));
                for member in &kept_members {
                    extra_members.push(ContainerMember {
                        container_id: container.id.clone(),
                        user_id: member.user_id.clone(),
                        role: member.role,
                        synthetic: false,
                        inherited_role: None,
                    });
                }
            }
            let container_offset = containers.len();
            schedules.extend(
                plan.schedules
                    .into_iter()
                    .map(|(local_idx, schedule)| (container_offset + local_idx, schedule)),
            );
            containers.extend(plan.containers);
            stream_plans.push((plan.zip_index, plan.stream_plan));
        }

        Ok(PreparedStreamedArchive {
            containers,
            extra_members,
            schedules,
            by_dir,
            files,
            created_order,
            stream_plans,
            members_left_out: left_out.len(),
        })
    }

    async fn stage_native_files(
        &self,
        zip: &Archive,
        largest_file: u64,
        org_id: &str,
        by_dir: &HashMap<String, String>,
        files: &HashMap<String, ArchiveFiles>,
    ) -> Result<StagedNativeFiles, AppError> {
        let mut seen_files: HashSet<(String, String)> = HashSet::new();
        let mut entries = Vec::new();
        for index in 0..zip.len() {
            let (dir, filename) = entry_name(zip, index)?;
            if filename.is_empty() || filename == "version.json" || filename == "container.jsonl" {
                continue;
            }
            let Some(container_id) = by_dir.get(&dir) else {
                tracing::warn!(dir, filename, "skipping orphan image in archive");
                continue;
            };
            if files
                .get(&dir)
                .is_some_and(|said| said.missing.contains(&filename))
            {
                tracing::warn!(
                    dir,
                    filename,
                    "skipping an entry of the archive that its board says is missing"
                );
                continue;
            }
            if !seen_files.insert((dir.clone(), filename.clone())) {
                return Err(AppError::bad_request(format_args!(
                    "duplicate archive entry: {dir}/{filename}"
                )));
            }
            entries.push((index, dir, filename, container_id.clone()));
        }
        if entries.is_empty() {
            return Ok(StagedNativeFiles {
                records: Vec::new(),
                written_places: Vec::new(),
                file_map: Arc::new(HashMap::new()),
            });
        }

        let work_dir = self
            .config()
            .tmp_dir()
            .join(format!("import-stage-{}", new_secret()));
        fs::create_dir_all(&work_dir).await.map_err(|error| {
            AppError::internal(format_args!("cannot create staging directory: {error}"))
        })?;
        let doomed_dir = Doomed::directory(work_dir);
        let unpacked = doomed_dir.path().join("entry");

        let mut records = Vec::with_capacity(entries.len());
        let mut written_places = Vec::with_capacity(entries.len());
        let mut file_map: HashMap<String, HashMap<String, String>> = HashMap::new();
        let mut total_staged_bytes: u64 = 0;

        for (index, dir, filename, container_id) in entries {
            let upload_name = name_in(files, &dir, &filename);
            let place = FilePlace::new(org_id, &container_id, upload_name);
            let written = self
                .write_one_native_file(zip, index, largest_file, &unpacked, &place.path)
                .await;
            let written = match written {
                Ok(written) => written,
                Err(error) => {
                    clean_written_places(written_places).await;
                    doomed_dir.remove().await;
                    return Err(error.context(format_args!(
                        "cannot import file {filename} for board {dir}"
                    )));
                }
            };
            let size = written.size();
            written_places.push(written);
            total_staged_bytes = total_staged_bytes.saturating_add(size);
            if let Err(error) = self.check_org_storage_room(total_staged_bytes).await {
                clean_written_places(written_places).await;
                doomed_dir.remove().await;
                return Err(error);
            }
            let record = file_record(&container_id, place, size, "");
            file_map
                .entry(container_id)
                .or_default()
                .insert(filename, record.stored_name.clone());
            records.push(record);
        }

        doomed_dir.remove().await;
        Ok(StagedNativeFiles {
            records,
            written_places,
            file_map: Arc::new(file_map),
        })
    }

    async fn write_one_native_file(
        &self,
        zip: &Archive,
        index: usize,
        largest_file: u64,
        unpacked: &Path,
        storage_path: &str,
    ) -> Result<WrittenPlace, AppError> {
        unpack_entry(zip, index, largest_file, unpacked).await?;
        let marked = match self.mark_place(storage_path).await {
            Ok(marked) => marked,
            Err(error) => {
                remove_unpacked(unpacked).await;
                return Err(error);
            }
        };
        let mut file = match fs::File::open(unpacked).await {
            Ok(file) => file,
            Err(error) => {
                remove_unpacked(unpacked).await;
                marked.give_up_unused().await;
                return Err(AppError::internal(format_args!(
                    "cannot open an unpacked archive entry: {error}"
                )));
            }
        };
        let outcome = marked.write(&mut file).await;
        remove_unpacked(unpacked).await;
        outcome
    }

    async fn finish_streamed_native_commit(
        &self,
        containers: &[Container],
        members: &[ContainerMember],
        inserted_block_ids: &[String],
    ) {
        if !containers.is_empty() {
            let made = containers.iter().map(|container| &container.id);
            self.collab()
                .recheck_access(self, AccessScope::containers(made))
                .await;
        }
        let workspace_id = containers
            .first()
            .map_or("", |container| container.workspace_id.as_str());
        for container in containers {
            self.ws()
                .broadcast_container_change(workspace_id, container)
                .await;
        }
        for chunk in inserted_block_ids.chunks(ANNOUNCE_CHUNK_BLOCKS) {
            match self.store().get_blocks_by_ids(chunk).await {
                Ok(blocks) => {
                    for block in &blocks {
                        self.ws().broadcast_block_change(workspace_id, block).await;
                        self.tell_webhooks(block);
                    }
                }
                Err(error) => {
                    tracing::error!(
                        %error,
                        workspace_id,
                        "cannot load imported blocks for broadcast"
                    );
                }
            }
        }
        let by_id: HashMap<&str, (&str, bool)> = containers
            .iter()
            .map(|c| (c.id.as_str(), (c.workspace_id.as_str(), c.is_template)))
            .collect();
        for member in members {
            if let Some(&(ws_id, false)) = by_id.get(member.container_id.as_str())
                && let Err(error) = self
                    .add_containers_to_default_category(
                        &member.user_id,
                        ws_id,
                        std::slice::from_ref(&member.container_id),
                    )
                    .await
            {
                tracing::warn!(
                    container_id = %member.container_id,
                    user_id = %member.user_id,
                    %error,
                    "failed to add imported container to default sidebar category after commit"
                );
            }
        }
        for container in containers {
            self.metrics()
                .container_copied(ContainerWork::Import, counted_as(container.kind));
            if container.kind == ContainerKind::Course && !container.is_template {
                self.metrics().course_created(CourseSource::Import);
            }
        }
    }
}

fn map_stream_write_error(error: StreamCreateError<AppError>) -> AppError {
    match error {
        StreamCreateError::Producer(error) => error,
        StreamCreateError::Incomplete => {
            AppError::internal("archive stream ended before completion")
        }
        StreamCreateError::Store(error) => {
            AppError::from(error).context("error inserting archive blocks")
        }
    }
}

async fn clean_written_places(places: Vec<WrittenPlace>) {
    for place in places {
        place.give_up().await;
    }
}

async fn remove_unpacked(unpacked: &Path) {
    if let Err(error) = fs::remove_file(unpacked).await
        && error.kind() != std::io::ErrorKind::NotFound
    {
        tracing::warn!(%error, path = %unpacked.display(), "could not remove staged archive entry");
    }
}
