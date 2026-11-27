//! Importing boards and wikis from the native `.fvoci` archive format of version 1
//! (`super::export`) and external exports (Trello JSON, Notion Markdown & CSV ZIP).
//!
//! Native `fvoci` imports stream each `container.jsonl` in two passes (`native_stream`):
//! Pass 1 scans and validates line by line without holding block bodies in RAM, files
//! are staged under `loose_objects` marks, and Pass 2 streams rewritten blocks in bounded
//! batches across all boards into one single database transaction (`native_atomic`).
//! External formats convert into a [`staged::StagedArchive`] and commit atomically across
//! containers, blocks, and local files.

use std::collections::{HashMap, HashSet};
use std::path::Path;
use std::sync::Arc;

use fvoci_model::{
    ArchiveHeader, CardPropertyDefinition, ContainerKind, ContainerMember, ContainersAndBlocks,
    PageLimits, WorkspaceRole,
};
use serde_json::Value;

use super::App;
use super::export::ARCHIVE_VERSION;
use super::file_name::check_file_name;
use super::files::stored_extension;
use crate::error::{AppError, ErrorKind};
use crate::permissions;
use archive::{
    Archive, check_entry, entry_limit, entry_name, open_archive, read_entry, too_many_entries,
};
use native_stream::{build_write_plan, recheck_planned_fields, scan_native_board};

/// The most an archive's entries may decompress to together:
/// what keeps a small archive from unfolding into a disk's worth.
const MAX_UNPACKED_BYTES: u64 = 1024 * 1024 * 1024;

/// The most entries an archive has.
const MAX_ENTRIES: u64 = 10_000;

/// The largest `container.jsonl` that is read. Streamed line by
/// line across two passes (`native_stream`) so peak RAM stays bounded by a single batch
/// instead of scaling with the full file's JSON AST.
const MAX_JSONL_BYTES: u64 = 64 * 1024 * 1024;

/// The largest `version.json` that is read: it is one small object.
const MAX_VERSION_BYTES: u64 = 64 * 1024;

/// Chunk size used when loading newly imported blocks to broadcast to WS/webhooks.
const ANNOUNCE_CHUNK_BLOCKS: usize = 16;

/// How an archive is imported.
#[derive(Debug, Default)]
pub(crate) struct ImportOptions {
    /// The workspace the boards are imported into.
    pub(crate) workspace_id: String,
    /// The user who imports, recorded as the modifier and added as an admin.
    pub(crate) modified_by: String,
    /// How large the tree of a wiki of the archive may be: the organization's limits.
    pub(crate) page_limits: PageLimits,
}

impl App {
    /// Imports an archive or external export file (`fvoci`, `trello`, or `notion`) at
    /// `path` into `workspace_id` on behalf of `modified_by`, returning a structured
    /// summary of created items, skipped items, and non-fatal warnings.
    ///
    /// # Errors
    ///
    /// Returns 404 if `workspace_id` does not exist in this organization or `modified_by`
    /// is not a non-guest member; 400 or 413 if the input fails security, format, size,
    /// depth, or allowlist validation; and 500 on storage or database failure. Any error
    /// leaves zero containers, blocks, and files stored.
    pub async fn import_external_archive(
        &self,
        path: &Path,
        format: ImportFormat,
        workspace_id: &str,
        modified_by: &str,
    ) -> Result<ImportSummary, AppError> {
        let largest_file = self.limits().largest_file.get();
        let limits = BundleLimits::for_largest_file(largest_file);
        self.import_external_archive_with_limits(path, format, workspace_id, modified_by, limits)
            .await
    }

    /// Like [`App::import_external_archive`], with caller-supplied [`BundleLimits`] applied
    /// to external formats (`trello` and `notion`). Native `fvoci` archives use the
    /// server's fixed native archive bounds and organization file limit.
    ///
    /// # Errors
    ///
    /// Returns the errors of [`App::import_external_archive`].
    pub async fn import_external_archive_with_limits(
        &self,
        path: &Path,
        format: ImportFormat,
        workspace_id: &str,
        modified_by: &str,
        limits: BundleLimits,
    ) -> Result<ImportSummary, AppError> {
        self.require_import_workspace(workspace_id, modified_by)
            .await?;
        let largest_file = self.limits().largest_file.get();
        let limits = BundleLimits {
            max_file_bytes: limits.max_file_bytes.min(largest_file),
            ..limits
        };
        let options = ImportOptions {
            workspace_id: workspace_id.to_owned(),
            modified_by: modified_by.to_owned(),
            page_limits: self.limits().pages,
        };
        match format {
            ImportFormat::Fvoci => self.import_native_summary(path, &options).await,
            ImportFormat::Trello => {
                let staged = trello::convert_trello_file(path, &limits).await?;
                self.commit_staged_archive(staged, None, &options, &limits)
                    .await
            }
            ImportFormat::Notion => {
                let temp_root = self.config().tmp_dir();
                let bundle = bundle::SafeBundle::open(path, &temp_root, limits).await?;
                let staged =
                    notion::convert_notion_bundle(&bundle, &limits, options.page_limits).await?;
                self.commit_staged_archive(staged, Some(&bundle), &options, &limits)
                    .await
            }
        }
    }

    /// Runs the native fvoci v1 archive import and wraps its outcome in [`ImportSummary`].
    async fn import_native_summary(
        &self,
        path: &Path,
        options: &ImportOptions,
    ) -> Result<ImportSummary, AppError> {
        let (imported, summary_counts, container_ids) =
            self.import_archive_detailed(path, options).await?;
        Ok(ImportSummary {
            format: ImportFormat::Fvoci,
            imported: summary_counts,
            members_left_out: imported.members_left_out,
            container_ids,
            ..ImportSummary::default()
        })
    }

    /// Imports the native fvoci v1 archive in the file at `path`, of zero or more boards
    /// with their blocks and files, using two-pass streaming reads with pre-commit file
    /// staging and a single atomic transaction across all boards.
    async fn import_archive_detailed(
        &self,
        path: &Path,
        options: &ImportOptions,
    ) -> Result<(Imported, ImportCounts, Vec<String>), AppError> {
        let zip = open_archive(path).await?;
        if zip.is_empty() {
            return Err(AppError::bad_request("zip: not a valid zip file"));
        }
        if u64::try_from(zip.len()).map_or(true, |entries| entries > MAX_ENTRIES) {
            return Err(too_many_entries());
        }
        let largest_file = self.limits().largest_file.get();

        verify_native_header(&zip).await?;

        let mut seen_entries: HashSet<(String, String)> = HashSet::with_capacity(zip.len());
        let mut container_entries = Vec::new();
        for index in 0..zip.len() {
            let (dir, filename) = entry_name(&zip, index)?;
            if filename == "container.jsonl" {
                container_entries.push((index, dir.clone()));
            }
            if !filename.is_empty() && !seen_entries.insert((dir, filename)) {
                let (dir, filename) = entry_name(&zip, index)?;
                return Err(AppError::bad_request(format_args!(
                    "duplicate archive entry: {dir}/{filename}"
                )));
            }
        }

        check_native_unpacked_budget(&zip, largest_file).await?;

        // Pass 1: stream every `container.jsonl` line by line for validation & cross-board refs.
        let mut scanned_boards = Vec::with_capacity(container_entries.len());
        let mut elsewhere: HashSet<String> = HashSet::new();
        let mut diaries: u32 = 0;
        for (index, dir) in container_entries {
            let scanned = scan_native_board(&zip, index, dir, options).await?;
            for (c_idx, container) in scanned.containers.iter().enumerate() {
                self.require_makes(container)
                    .map_err(|error| error.under(&format!("/containers/{c_idx}")))?;
            }
            diaries = diaries.saturating_add(scanned.diaries);
            elsewhere.extend(scanned.external_refs.iter().cloned());
            scanned_boards.push(scanned);
        }
        if diaries > 0 {
            self.store()
                .check_room_for_diaries(&options.workspace_id, diaries)
                .await?;
        }

        // Build compact write plans and complete cross-board copy map before Pass 2 writes.
        let mut plans = Vec::with_capacity(scanned_boards.len());
        let mut all_copies: HashMap<String, String> = HashMap::new();
        for scanned in scanned_boards {
            let (plan, copies) = build_write_plan(scanned, &elsewhere);
            for (old, new) in copies {
                all_copies.entry(old).or_insert(new);
            }
            plans.push(plan);
        }
        for plan in &plans {
            recheck_planned_fields(&zip, plan, &options.modified_by).await?;
        }
        let all_copies = Arc::new(all_copies);

        self.commit_streamed_native_archive(&zip, plans, all_copies, options, largest_file)
            .await
    }
}

/// Reads and verifies `version.json` at the root of a native archive.
async fn verify_native_header(zip: &Archive) -> Result<(), AppError> {
    let version = (0..zip.len())
        .map(|index| entry_name(zip, index).map(|name| (index, name)))
        .collect::<Result<Vec<_>, _>>()?
        .into_iter()
        .find(|(_, (dir, filename))| dir == "." && filename == "version.json")
        .map(|(index, _)| index)
        .ok_or_else(|| {
            AppError::bad_request(format_args!(
                "the archive has no version.json; this server reads version {ARCHIVE_VERSION}"
            ))
        })?;
    let version = read_entry(zip, version, MAX_VERSION_BYTES)
        .await
        .map_err(|error| {
            if error.kind() == ErrorKind::PayloadTooLarge {
                AppError::bad_request("version.json is not that of an archive")
            } else {
                error
            }
        })?;
    check_version(&version)
}

/// What an import did beyond making the archive's boards.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Imported {
    /// How many of the users who were members of the archive's boards are no members of
    /// them now: they are not in the workspace (each counted once).
    pub members_left_out: usize,
}

/// The membership with its role lowered to the highest the user may hold in the
/// workspace (`permissions::highest_container_role`), and left as it is if it is not higher.
fn no_more_than_holdable(
    member: ContainerMember,
    workspace_role: WorkspaceRole,
) -> ContainerMember {
    let highest = permissions::highest_container_role(workspace_role);
    if member.role > highest {
        permissions::with_role(member, highest)
    } else {
        member
    }
}

/// The name of each file of a directory of an archive, by the name of its entry.
type FileNames = HashMap<String, String>;

/// What the board lines of a directory of an archive say of its files.
#[derive(Debug, Default)]
struct ArchiveFiles {
    /// The name of each file, by the name of its entry ([`file_names_of`]).
    names: FileNames,
    /// The entries the export did not find (`missingFiles`, [`missing_files_of`]): none
    /// is stored, and what shows them shows no file.
    missing: HashSet<String>,
}

/// The name a file of the archive is stored with: the one the board line of its
/// directory gives it ([`file_names_of`]), or failing that the name of its entry.
fn name_in<'a>(files: &'a HashMap<String, ArchiveFiles>, dir: &str, filename: &'a str) -> &'a str {
    files
        .get(dir)
        .and_then(|said| said.names.get(filename))
        .map_or(filename, String::as_str)
}

/// Removes the blocks that nothing in the archive leads to, and returns how many went:
/// those whose parent is no block of the archive, the blocks below them, and blocks that
/// are each other's parents without any of them being on a board.
pub(super) fn drop_orphans(containers_and_blocks: &mut ContainersAndBlocks) -> usize {
    let blocks = &mut containers_and_blocks.blocks;
    let mut below: HashMap<&str, Vec<usize>> = HashMap::new();
    let mut reached: Vec<usize> = Vec::new();
    for (index, block) in blocks.iter().enumerate() {
        if block.parent_id.is_empty() {
            reached.push(index);
        } else {
            below
                .entry(block.parent_id.as_str())
                .or_default()
                .push(index);
        }
    }
    let mut keep = vec![false; blocks.len()];
    while let Some(index) = reached.pop() {
        if !std::mem::replace(&mut keep[index], true) {
            reached.extend(below.get(blocks[index].id.as_str()).into_iter().flatten());
        }
    }

    let before = blocks.len();
    let mut kept = keep.into_iter();
    blocks.retain(|_| kept.next().unwrap_or(false));
    before - blocks.len()
}

/// The names a board line gives the files of its directory (`fileNames`, by the names
/// of their entries), as an upload's name may be ([`check_file_name`]), and with the
/// extension of the entry's name.
fn file_names_of(board: &Value) -> FileNames {
    let Some(names) = board.get("fileNames").and_then(Value::as_object) else {
        return FileNames::new();
    };
    names
        .iter()
        .filter_map(|(entry, name)| {
            let name = name.as_str()?;
            let fits =
                check_file_name(name).is_ok() && stored_extension(name) == stored_extension(entry);
            fits.then(|| (entry.clone(), name.to_owned()))
        })
        .collect()
}

/// The stored names a board line says the export did not find (`missingFiles`): the
/// strings of the list, anything else in it left out.
fn missing_files_of(board: &Value) -> HashSet<String> {
    board
        .get("missingFiles")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .map(str::to_owned)
        .collect()
}

/// Reads every entry of a native archive once before anything is stored so any broken
/// or oversized entry, or cumulative unpacked total past [`MAX_UNPACKED_BYTES`], refuses
/// the archive with nothing stored yet.
async fn check_native_unpacked_budget(zip: &Archive, largest_file: u64) -> Result<(), AppError> {
    let mut unpacked: u64 = 0;
    for index in 0..zip.len() {
        let (dir, filename) = entry_name(zip, index)?;
        let limit = entry_limit(&dir, &filename, largest_file);
        unpacked = unpacked.saturating_add(check_entry(zip, index, limit).await?);
        if unpacked > MAX_UNPACKED_BYTES {
            return Err(AppError::too_large(
                format_args!(
                    "the archive's entries unpack to more than {MAX_UNPACKED_BYTES} bytes"
                ),
                MAX_UNPACKED_BYTES,
            ));
        }
    }
    Ok(())
}

/// Tallies created container counts into `counts` and returns container IDs in creation order.
fn tally_container_counts(
    created_order: &[(ContainerKind, String)],
    counts: &mut ImportCounts,
) -> Vec<String> {
    let mut container_ids = Vec::with_capacity(created_order.len());
    for (kind, id) in created_order {
        match kind {
            ContainerKind::Board => counts.boards += 1,
            ContainerKind::Wiki => counts.wikis += 1,
            ContainerKind::Course => counts.courses += 1,
            ContainerKind::Diary => {}
        }
        container_ids.push(id.clone());
    }
    container_ids
}

/// Validates an archive's `version.json`.
fn check_version(bytes: &[u8]) -> Result<(), AppError> {
    let header: ArchiveHeader = serde_json::from_slice(bytes).map_err(|error| {
        AppError::bad_request(format_args!("cannot parse version.json: {error}"))
    })?;
    if header.version == ARCHIVE_VERSION {
        Ok(())
    } else {
        Err(AppError::bad_request(format_args!(
            "unsupported archive version; got {}, want {ARCHIVE_VERSION}",
            header.version
        )))
    }
}

/// Clears the card values that do not fit the kind of their property, and of relation
/// values what a write refuses, as a copy of stored cards does.
fn clear_values_of_another_kind(containers_and_blocks: &mut ContainersAndBlocks) {
    let definitions: HashMap<&str, &[CardPropertyDefinition]> = containers_and_blocks
        .containers
        .iter()
        .map(|container| (container.id.as_str(), container.card_properties.as_slice()))
        .collect();
    for block in &mut containers_and_blocks.blocks {
        let Some(definitions) = definitions.get(block.container_id.as_str()) else {
            continue;
        };
        native_stream::clear_block_values_of_another_kind(block, definitions);
    }
}

mod archive;
mod bundle;
#[cfg(test)]
mod external_tests;
pub(super) mod markdown;
mod markdown_nodes;
mod native_atomic;
mod native_stream;
mod native_stream_write;
mod notion;
mod notion_csv;
mod places;
mod pointing;
mod staged;
mod summary;
#[cfg(test)]
mod tests;
mod trello;
mod trello_wire;

pub use summary::{
    BundleLimits, ImportCounts, ImportFormat, ImportSummary, ImportWarning, ImportWarningCode,
    MAX_REPORTED_WARNINGS, ParseImportFormatError,
};
