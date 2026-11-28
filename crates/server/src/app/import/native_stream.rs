//! Pass-1 streaming scanner, write-plan builder, and optional post-rewrite field recheck
//! for native fvoci v1 `container.jsonl` archive entries (T12).
//!
//! Pass 1 ([`scan_native_board`]) streams each `container.jsonl` line by line via `BufRead`
//! with a strict per-line byte ceiling ([`MAX_LINE_BYTES`]), validates every container,
//! block, and structural rule (`places::check_places`), drops unreachable orphans, and
//! keeps only lightweight metadata in memory (no card/page/comment bodies are retained).
//!
//! If any kept block could change validity after identifier/relation rewriting (e.g., stale
//! relations dropped or short IDs expanding to 36-char UUIDs), [`recheck_planned_fields`]
//! validates only those flagged line numbers before any database transaction begins.

use std::collections::{HashMap, HashSet};
use std::io::{self, BufRead, BufReader};

use fvoci_model::{
    BODY_MAX_BYTES, Block, BlockError, BlockType, Body, CardPropertyDefinition, Container,
    ContainerKind, ContainerMember, ContainersAndBlocksError, CourseScheduleUpdate, DiaryDate,
    JsonMap, PropertyType, RelationProperties, ValidatedCourseSchedule, collect_block_references,
    new_id,
};
use serde::Deserialize;
use serde_json::Value;
use serde_json::value::RawValue;

use super::archive::{Archive, entry_name};
use super::places::{self, BlockPlace};
use super::{ArchiveFiles, ImportOptions, MAX_JSONL_BYTES, file_names_of, missing_files_of};
use crate::auth::now_millis;
use crate::error::{AppError, Reason};
use fvoci_store::StoreError;

/// Maximum raw byte length accepted for a single `container.jsonl` line (8 MiB body ceiling
/// plus 2 MiB headroom for fields, title, and JSON framing) to bound per-line memory use.
pub(super) const MAX_LINE_BYTES: usize = BODY_MAX_BYTES + 2 * 1024 * 1024;

/// `BufReader` buffer size for streaming `container.jsonl` entries.
pub(super) const STREAM_READER_BUF_BYTES: usize = 64 * 1024;

/// Shrinks the reusable line buffer back to 64 KiB after reading an unusually large line.
pub(super) const LINE_BUF_SHRINK_THRESHOLD: usize = 256 * 1024;

/// Placeholder used while running Pass-1 validation on a block whose board had an empty
/// identifier (which `build_write_plan` renews to a fresh UUID).
const PENDING_CONTAINER_ID: &str = "__pending__";

/// Maximum `container` lines accepted in a single `container.jsonl` entry.
pub(super) const MAX_CONTAINERS_PER_ENTRY: usize = 64;

/// Maximum `containerMember` lines accepted in a single `container.jsonl` entry.
pub(super) const MAX_MEMBER_LINES: usize = 10_000;

/// Keys of a view that no view has: an archive whose view holds one is refused.
const RETIRED_VIEW_KEYS: [&str; 8] = [
    "hiddenOptionIds",
    "visibleOptionIds",
    "collapsedOptionIds",
    "kanbanCalculations",
    "defaultPropertyIds",
    "groupById",
    "sortOptions",
    "datePropertyId",
];

pub(super) fn too_many_archive_items(kind: &str, limit: usize) -> AppError {
    let limit_u64 = u64::try_from(limit).unwrap_or(u64::MAX);
    AppError::too_large(
        format_args!("archive exceeds maximum of {limit} {kind}"),
        limit_u64,
    )
    .with_reason(Reason::ArchiveTooLarge)
}

/// Streaming envelope for a `container.jsonl` line whose `data` payload stays an unparsed
/// [`RawValue`] until dispatched to its typed target struct (`H-4`).
#[derive(Default, Deserialize)]
#[serde(default)]
pub(super) struct RawArchiveLine {
    #[serde(rename = "type")]
    pub(super) line_type: String,
    pub(super) data: Option<Box<RawValue>>,
}

/// Optional archive-only metadata carried alongside a `container` JSONL line.
#[derive(Default, Deserialize)]
#[serde(default, rename_all = "camelCase")]
struct ArchiveContainerExtras {
    file_names: Option<Value>,
    missing_files: Option<Value>,
}

/// Typed `block` wire payload keeping `body` as `Option<Box<RawValue>>` so large card/page
/// bodies never materialize as untyped `serde_json::Value` trees (`H-4`).
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct ArchiveBlockWire {
    #[serde(default)]
    id: String,
    #[serde(default)]
    parent_id: String,
    #[serde(default)]
    created_by: String,
    #[serde(default)]
    modified_by: String,
    #[serde(rename = "type")]
    block_type: BlockType,
    #[serde(default)]
    title: String,
    #[serde(default)]
    fields: JsonMap,
    #[serde(default)]
    body: Option<Box<RawValue>>,
    #[serde(default)]
    created_at: i64,
    #[serde(default)]
    updated_at: i64,
    #[serde(default, deserialize_with = "deserialize_non_null_i64")]
    deleted_at: Option<i64>,
    #[serde(default)]
    container_id: String,
    #[serde(default, deserialize_with = "deserialize_card_number")]
    number: Option<u32>,
    #[serde(default)]
    edited_at: i64,
}

fn deserialize_non_null_i64<'de, D>(deserializer: D) -> Result<Option<i64>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    i64::deserialize(deserializer).map(Some)
}

fn deserialize_card_number<'de, D>(deserializer: D) -> Result<Option<u32>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let sent = Value::deserialize(deserializer)?;
    Ok(sent
        .as_u64()
        .and_then(|number| u32::try_from(number).ok())
        .filter(|number| *number > 0))
}

pub(super) fn decode_archive_block(raw_data: &str, line_number: usize) -> Result<Block, AppError> {
    let wire: ArchiveBlockWire = serde_json::from_str(raw_data).map_err(|error| {
        AppError::bad_request(format_args!(
            "invalid block in archive line {line_number}: {error}"
        ))
    })?;
    if wire.block_type == BlockType::View
        && let Some(&retired) = RETIRED_VIEW_KEYS
            .iter()
            .find(|&&key| wire.fields.contains_key(key))
    {
        return Err(AppError::bad_request(format_args!(
            "view block in archive line {line_number} uses retired field {retired:?}"
        )));
    }
    let body: Option<Body> = match wire.body {
        Some(raw_body) => serde_json::from_str(raw_body.get()).map_err(|error| {
            AppError::bad_request(format_args!(
                "invalid block in archive line {line_number}: {error}"
            ))
        })?,
        None => None,
    };
    Ok(Block {
        id: wire.id,
        parent_id: wire.parent_id,
        created_by: wire.created_by,
        modified_by: wire.modified_by,
        block_type: wire.block_type,
        title: wire.title,
        fields: wire.fields,
        body,
        created_at: wire.created_at,
        updated_at: wire.updated_at,
        deleted_at: wire.deleted_at,
        container_id: wire.container_id,
        number: wire.number,
        edited_at: wire.edited_at,
    })
}

/// Lightweight per-block metadata retained across Pass 1 (no body, title, or fields).
struct BlockMeta {
    line_number: usize,
    id: String,
    container_idx: Option<usize>,
    parent_id: String,
    block_type: BlockType,
    is_template: bool,
    diary_date: Option<DiaryDate>,
    invalid: Option<BlockError>,
    needs_fields_recheck: bool,
}

/// One `courseSchedule` line decoded from `container.jsonl` during Pass 1.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct ArchiveCourseSchedule {
    #[serde(default)]
    container_id: String,
    start_date: String,
    target_date: String,
    study_days: i32,
}

/// Output of Pass 1 for a single `container.jsonl` entry, holding only structural metadata.
pub(super) struct ScannedBoard {
    pub(super) zip_index: usize,
    pub(super) dir: String,
    pub(super) containers: Vec<Container>,
    pub(super) members: Vec<ContainerMember>,
    pub(super) schedules: Vec<(usize, ValidatedCourseSchedule)>,
    pub(super) files: ArchiveFiles,
    blocks: Vec<BlockMeta>,
    keep: Vec<bool>,
    referenced: HashSet<String>,
    pub(super) external_refs: HashSet<String>,
    pub(super) diaries: u32,
}

/// Compact identifier-renewal plan for Pass 2 of one `container.jsonl` entry.
pub(super) struct BoardWritePlan {
    pub(super) zip_index: usize,
    pub(super) dir: String,
    pub(super) containers: Vec<Container>,
    pub(super) members: Vec<ContainerMember>,
    pub(super) schedules: Vec<(usize, ValidatedCourseSchedule)>,
    pub(super) files: ArchiveFiles,
    /// `line_number -> block_index` for kept blocks needing post-rewrite field validation.
    pub(super) recheck_lines: HashMap<usize, usize>,
    pub(super) stream_plan: StreamPlan,
}

/// Immutable lookup tables moved into the Pass-2 blocking streamer task.
pub(super) struct StreamPlan {
    pub(super) container_new_ids: Vec<String>,
    pub(super) block_container_idx: Vec<Option<u32>>,
    pub(super) card_properties: HashMap<String, Vec<CardPropertyDefinition>>,
    pub(super) new_block_ids: Vec<Option<String>>,
    pub(super) renamed: HashMap<String, String>,
    pub(super) card_boards: HashMap<String, String>,
    pub(super) relations: RelationProperties,
    pub(super) has_children: HashSet<String>,
}

/// Reads the next `\n`-terminated line (or final line at EOF) from `reader` into `buf`
/// without allocating beyond `max_line_bytes` or reading past `max_total_bytes`.
///
/// Returns `Ok(true)` if any bytes were read for this line, or `Ok(false)` at EOF.
pub(super) fn read_bounded_line(
    reader: &mut impl BufRead,
    buf: &mut Vec<u8>,
    max_line_bytes: usize,
    total_read: &mut u64,
    max_total_bytes: u64,
    entry_name: &str,
) -> Result<bool, AppError> {
    buf.clear();
    let mut saw_any = false;
    loop {
        let available = match reader.fill_buf() {
            Ok(bytes) => bytes,
            Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
            Err(error) => {
                return Err(AppError::bad_request(format_args!(
                    "cannot read archive entry: {error}"
                )));
            }
        };
        if available.is_empty() {
            return Ok(saw_any);
        }
        saw_any = true;
        let (consume, found_newline) = match available.iter().position(|&byte| byte == b'\n') {
            Some(pos) => (pos + 1, true),
            None => (available.len(), false),
        };
        let consume_u64 = u64::try_from(consume).unwrap_or(u64::MAX);
        *total_read = total_read.saturating_add(consume_u64);
        if *total_read > max_total_bytes {
            return Err(AppError::too_large(
                format_args!(
                    "the archive entry {entry_name} unpacks to more than {max_total_bytes} bytes"
                ),
                max_total_bytes,
            )
            .with_reason(Reason::ArchiveEntryTooLarge));
        }
        let payload_len = if found_newline { consume - 1 } else { consume };
        if buf.len().saturating_add(payload_len) > max_line_bytes {
            let limit = u64::try_from(max_line_bytes).unwrap_or(u64::MAX);
            return Err(AppError::too_large(
                format_args!("a line of {entry_name} exceeds the {max_line_bytes}-byte limit"),
                limit,
            )
            .with_reason(Reason::ArchiveEntryTooLarge));
        }
        buf.extend_from_slice(&available[..payload_len]);
        reader.consume(consume);
        if found_newline {
            return Ok(true);
        }
    }
}

/// Pass 1: streams a `container.jsonl` entry off the Tokio runtime, validates its containers,
/// block placements, and kept blocks, and returns lightweight metadata without keeping
/// block bodies in RAM.
pub(super) async fn scan_native_board(
    zip: &Archive,
    zip_index: usize,
    dir: String,
    options: &ImportOptions,
) -> Result<ScannedBoard, AppError> {
    let mut zip = zip.clone();
    let workspace_id = options.workspace_id.clone();
    let modified_by = options.modified_by.clone();
    let limits = options.page_limits;
    tokio::task::spawn_blocking(move || {
        scan_native_board_blocking(
            &mut zip,
            zip_index,
            dir,
            &workspace_id,
            &modified_by,
            limits,
        )
    })
    .await
    .map_err(|error| AppError::internal(format_args!("archive task failed: {error}")))?
}

fn scan_native_board_blocking(
    zip: &mut Archive,
    zip_index: usize,
    dir: String,
    workspace_id: &str,
    modified_by: &str,
    limits: fvoci_model::PageLimits,
) -> Result<ScannedBoard, AppError> {
    let entry = zip
        .by_index(zip_index)
        .map_err(|error| AppError::bad_request(format_args!("{error}")))?;
    let entry_label = entry.name().map_or_default(std::borrow::Cow::into_owned);
    let mut reader = BufReader::with_capacity(STREAM_READER_BUF_BYTES, entry);
    let mut state = ScanAccum::new(now_millis());
    let mut line_buf = Vec::with_capacity(4096);
    let mut total_read: u64 = 0;
    let mut line_number: usize = 0;

    while read_bounded_line(
        &mut reader,
        &mut line_buf,
        MAX_LINE_BYTES,
        &mut total_read,
        MAX_JSONL_BYTES,
        &entry_label,
    )? {
        line_number += 1;
        let line = line_buf.trim_ascii();
        if !line.is_empty() {
            state.take_line(line, line_number, workspace_id, modified_by)?;
        }
        if line_buf.capacity() > LINE_BUF_SHRINK_THRESHOLD {
            line_buf.clear();
            line_buf.shrink_to(STREAM_READER_BUF_BYTES);
        }
    }

    state.finish(zip_index, dir, limits)
}

/// Accumulator for one `container.jsonl` during Pass 1.
struct ScanAccum {
    now: i64,
    containers: Vec<Container>,
    container_has_relations: Vec<bool>,
    container_idx_by_id: HashMap<String, usize>,
    relations: RelationProperties,
    members: Vec<ContainerMember>,
    raw_schedules: Vec<(String, ValidatedCourseSchedule, usize)>,
    files: ArchiveFiles,
    blocks: Vec<BlockMeta>,
    referenced: HashSet<String>,
    body_refs: HashSet<String>,
    current_container_idx: Option<usize>,
}

impl ScanAccum {
    fn new(now: i64) -> Self {
        Self {
            now,
            containers: Vec::new(),
            container_has_relations: Vec::new(),
            container_idx_by_id: HashMap::new(),
            relations: RelationProperties::new(),
            members: Vec::new(),
            raw_schedules: Vec::new(),
            files: ArchiveFiles::default(),
            blocks: Vec::new(),
            referenced: HashSet::new(),
            body_refs: HashSet::new(),
            current_container_idx: None,
        }
    }

    fn take_line(
        &mut self,
        line: &[u8],
        line_number: usize,
        workspace_id: &str,
        modified_by: &str,
    ) -> Result<(), AppError> {
        let archive_line: RawArchiveLine = serde_json::from_slice(line).map_err(|error| {
            AppError::bad_request(format_args!(
                "error parsing archive line {line_number}: {error}"
            ))
        })?;
        let raw_data = archive_line.data.as_deref().map_or("null", RawValue::get);
        match archive_line.line_type.as_str() {
            "container" => self.take_container(raw_data, line_number, workspace_id, modified_by),
            "block" => self.take_block(raw_data, line_number, modified_by),
            "containerMember" => {
                if self.members.len() >= MAX_MEMBER_LINES {
                    return Err(too_many_archive_items("members", MAX_MEMBER_LINES));
                }
                let member: ContainerMember = serde_json::from_str(raw_data).map_err(|error| {
                    AppError::bad_request(format_args!(
                        "invalid containerMember in archive line {line_number}: {error}"
                    ))
                })?;
                self.members.push(member);
                Ok(())
            }
            "courseSchedule" => self.take_course_schedule(raw_data, line_number),
            other => Err(AppError::bad_request(format_args!(
                "unsupported archive line type; got {other}, line {line_number}"
            ))),
        }
    }

    fn take_course_schedule(&mut self, raw_data: &str, line_number: usize) -> Result<(), AppError> {
        let parsed: ArchiveCourseSchedule = serde_json::from_str(raw_data).map_err(|error| {
            AppError::bad_request(format_args!(
                "invalid courseSchedule in archive line {line_number}: {error}"
            ))
        })?;
        let update = CourseScheduleUpdate {
            start_date: parsed.start_date,
            target_date: parsed.target_date,
            study_days: parsed.study_days,
            auto_assign: false,
        };
        let validated = update.check_request().map_err(|error| {
            AppError::bad_request(format_args!(
                "invalid courseSchedule in archive line {line_number}: {error}"
            ))
        })?;
        self.raw_schedules
            .push((parsed.container_id, validated, line_number));
        Ok(())
    }

    fn take_container(
        &mut self,
        raw_data: &str,
        line_number: usize,
        workspace_id: &str,
        modified_by: &str,
    ) -> Result<(), AppError> {
        if self.containers.len() >= MAX_CONTAINERS_PER_ENTRY {
            return Err(too_many_archive_items("boards", MAX_CONTAINERS_PER_ENTRY));
        }
        let mut container: Container = serde_json::from_str(raw_data).map_err(|error| {
            AppError::bad_request(format_args!(
                "invalid container in archive line {line_number}: {error}"
            ))
        })?;
        let extras: ArchiveContainerExtras = serde_json::from_str(raw_data).unwrap_or_default();
        if let Some(file_names) = extras.file_names {
            let wrapper = serde_json::json!({ "fileNames": file_names });
            self.files.names.extend(file_names_of(&wrapper));
        }
        if let Some(missing_files) = extras.missing_files {
            let wrapper = serde_json::json!({ "missingFiles": missing_files });
            self.files.missing.extend(missing_files_of(&wrapper));
        }
        modified_by.clone_into(&mut container.modified_by);
        container.updated_at = self.now;
        container.deleted_at = None;
        workspace_id.clone_into(&mut container.workspace_id);
        container
            .check_storable()
            .map_err(|error| AppError::from(StoreError::InvalidContainer(error)))?;
        let next_idx = self.containers.len();
        let canonical_idx = *self
            .container_idx_by_id
            .entry(container.id.clone())
            .or_insert(next_idx);
        self.current_container_idx = Some(canonical_idx);
        if canonical_idx == next_idx {
            self.relations
                .add_board(&container.id, &container.card_properties);
        }
        let has_rel = container
            .card_properties
            .iter()
            .any(|prop| prop.property_type == PropertyType::Relation);
        self.container_has_relations.push(has_rel);
        self.containers.push(container);
        Ok(())
    }

    fn take_block(
        &mut self,
        raw_data: &str,
        line_number: usize,
        modified_by: &str,
    ) -> Result<(), AppError> {
        let mut block = decode_archive_block(raw_data, line_number)?;
        modified_by.clone_into(&mut block.modified_by);
        block.updated_at = self.now;
        block.deleted_at = None;
        let container_idx = self.current_container_idx;
        let container_state = if let Some(idx) = container_idx {
            block.container_id.clone_from(&self.containers[idx].id);
            clear_block_values_of_another_kind(&mut block, &self.containers[idx].card_properties);
            if self.container_has_relations[idx] {
                Pass1Container::WithRelations
            } else {
                Pass1Container::WithoutRelations
            }
        } else {
            block.container_id.clear();
            Pass1Container::Missing
        };
        collect_block_references(&block, &self.relations, &mut self.referenced);
        if let Some(body) = &block.body {
            for id in body.linked_ids() {
                self.body_refs.insert(id.to_owned());
            }
        }
        let diary_date = (block.block_type == BlockType::Entry)
            .then(|| fvoci_model::entry_date(&block))
            .flatten();
        let is_template = block.is_template();
        let (invalid, needs_fields_recheck) = validate_pass1_block(&mut block, container_state);
        self.blocks.push(BlockMeta {
            line_number,
            id: block.id,
            container_idx,
            parent_id: block.parent_id,
            block_type: block.block_type,
            is_template,
            diary_date,
            invalid,
            needs_fields_recheck,
        });
        // `block` (including its body and fields) is dropped here.
        Ok(())
    }

    fn finish(
        self,
        zip_index: usize,
        dir: String,
        limits: fvoci_model::PageLimits,
    ) -> Result<ScannedBoard, AppError> {
        let places: Vec<BlockPlace<'_>> = self
            .blocks
            .iter()
            .map(|meta| BlockPlace {
                id: &meta.id,
                container_id: meta
                    .container_idx
                    .map_or("", |idx| self.containers[idx].id.as_str()),
                parent_id: &meta.parent_id,
                block_type: &meta.block_type,
                is_template: meta.is_template,
                diary_date: meta.diary_date,
            })
            .collect();
        places::check_places(&self.containers, &places, limits)?;
        drop(places);

        let (keep, orphans) = compute_reachable(&self.blocks);
        if orphans > 0 {
            tracing::warn!(
                orphans,
                "skipped blocks of the archive whose parent is not in the archive"
            );
        }
        validate_kept_structure(&self.containers, &self.blocks, &keep)?;
        let schedules = resolve_course_schedules(
            &self.containers,
            &self.container_idx_by_id,
            self.raw_schedules,
        )?;

        let own_kept: HashSet<&str> = self
            .blocks
            .iter()
            .zip(&keep)
            .filter_map(|(meta, &kept)| kept.then_some(meta.id.as_str()))
            .collect();
        let external_refs: HashSet<String> = self
            .body_refs
            .into_iter()
            .filter(|id| !own_kept.contains(id.as_str()))
            .collect();
        let diaries = u32::try_from(
            self.containers
                .iter()
                .filter(|one| one.kind == ContainerKind::Diary)
                .count(),
        )
        .unwrap_or(u32::MAX);

        Ok(ScannedBoard {
            zip_index,
            dir,
            containers: self.containers,
            members: self.members,
            schedules,
            files: self.files,
            blocks: self.blocks,
            keep,
            referenced: self.referenced,
            external_refs,
            diaries,
        })
    }
}

/// Resolves validated `courseSchedule` lines to their course container index, rejecting
/// missing/non-course targets and duplicate schedules for the same course.
fn resolve_course_schedules(
    containers: &[Container],
    container_idx_by_id: &HashMap<String, usize>,
    raw_schedules: Vec<(String, ValidatedCourseSchedule, usize)>,
) -> Result<Vec<(usize, ValidatedCourseSchedule)>, AppError> {
    let mut seen: HashSet<usize> = HashSet::with_capacity(raw_schedules.len());
    let mut resolved = Vec::with_capacity(raw_schedules.len());
    for (container_id, schedule, line_number) in raw_schedules {
        let idx = if container_id.is_empty() && containers.len() == 1 {
            0
        } else {
            let Some(&idx) = container_idx_by_id.get(&container_id) else {
                return Err(AppError::bad_request(format_args!(
                    "courseSchedule in archive line {line_number} names no course of the archive"
                )));
            };
            idx
        };
        if containers[idx].kind != ContainerKind::Course {
            return Err(AppError::bad_request(format_args!(
                "courseSchedule in archive line {line_number} applies only to a course"
            )));
        }
        if !seen.insert(idx) {
            return Err(AppError::bad_request(format_args!(
                "duplicate courseSchedule in archive line {line_number}"
            )));
        }
        resolved.push((idx, schedule));
    }
    Ok(resolved)
}

/// Container context available during Pass-1 block validation.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Pass1Container {
    Missing,
    WithoutRelations,
    WithRelations,
}

/// Validates a block during Pass 1 while accounting for identifier/relation rewrites that
/// occur in Pass 2: empty board IDs are renewed before storage, stale relations may shrink
/// `fields`, and short IDs in order/relation fields may expand to 36-char UUIDs.
fn validate_pass1_block(
    block: &mut Block,
    container: Pass1Container,
) -> (Option<BlockError>, bool) {
    let had_empty_cid = container != Pass1Container::Missing && block.container_id.is_empty();
    if had_empty_cid {
        PENDING_CONTAINER_ID.clone_into(&mut block.container_id);
    }
    let mut invalid = block.validate().err();
    if had_empty_cid {
        block.container_id.clear();
    }
    let has_card_relations =
        block.block_type == BlockType::Card && container == Pass1Container::WithRelations;
    if invalid == Some(BlockError::FieldsSizeLimitExceeded) && has_card_relations {
        // Stale/unmatched relation IDs are dropped and truncated to `RELATION_LIMIT` on
        // rewrite; defer final size validation until Pass 1.5 with the full rewrite map.
        invalid = None;
        return (invalid, true);
    }
    let needs_recheck = invalid.is_none() && may_expand_fields_on_rewrite(block, container);
    (invalid, needs_recheck)
}

/// Returns `true` when rewriting short IDs in `fields` to 36-char UUIDs could plausibly
/// push serialized `fields` past `block_type.fields_max_chars()`.
fn may_expand_fields_on_rewrite(block: &Block, container: Pass1Container) -> bool {
    let has_card_relations =
        block.block_type == BlockType::Card && container == Pass1Container::WithRelations;
    let has_attachment_file = block.block_type == BlockType::Attachment
        && (block.fields.contains_key("fileId") || block.fields.contains_key("attachmentId"));
    let has_rewritten_field = block.fields.contains_key("cardOrder")
        || block.fields.contains_key("contentOrder")
        || block.fields.contains_key("defaultTemplateId")
        || has_attachment_file
        || (has_card_relations && block.fields.contains_key("properties"));
    if !has_rewritten_field {
        return false;
    }
    // A 1-char JSON string element (`"1",`, 4 chars) expands at most ~10x to a 36-char UUID;
    // a 1-char attachment `fileId` (`"1"`, 3 chars) expands at most ~21x to a 60-char stored name.
    let multiplier = if has_attachment_file { 21 } else { 10 };
    serde_json::to_string(&block.fields).map_or(true, |json| {
        json.chars().count().saturating_mul(multiplier) > block.block_type.fields_max_chars()
    })
}

/// Same O(N) reachability walk as [`super::drop_orphans`], operating on [`BlockMeta`].
fn compute_reachable(blocks: &[BlockMeta]) -> (Vec<bool>, usize) {
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
    let mut kept_count = 0usize;
    while let Some(index) = reached.pop() {
        if !std::mem::replace(&mut keep[index], true) {
            kept_count += 1;
            reached.extend(below.get(blocks[index].id.as_str()).into_iter().flatten());
        }
    }
    (keep, blocks.len() - kept_count)
}

/// Validates container/block membership and per-block validation results for kept blocks.
fn validate_kept_structure(
    containers: &[Container],
    blocks: &[BlockMeta],
    keep: &[bool],
) -> Result<(), AppError> {
    if containers.is_empty() {
        return Err(AppError::bad_request(format_args!(
            "the archive's boards and blocks do not fit together: {}",
            ContainersAndBlocksError::NoContainers
        )));
    }
    for (meta, &kept) in blocks.iter().zip(keep) {
        if !kept {
            continue;
        }
        if meta.container_idx.is_none() {
            let error = ContainersAndBlocksError::BlockWithoutContainer {
                block_id: meta.id.clone(),
            };
            return Err(AppError::bad_request(format_args!(
                "the archive's boards and blocks do not fit together: {error}"
            )));
        }
        if let Some(source) = &meta.invalid {
            return Err(AppError::from(StoreError::InvalidBlock {
                id: meta.id.clone(),
                source: source.clone(),
            }));
        }
    }
    Ok(())
}

/// Clears card property values that do not match their definition and tidies copied
/// relation values on a single block.
pub(super) fn clear_block_values_of_another_kind(
    block: &mut Block,
    definitions: &[CardPropertyDefinition],
) {
    let id = block.id.clone();
    if let Some(Value::Object(values)) = block.fields.get_mut("properties") {
        fvoci_model::clear_values_of_another_kind(values, definitions);
        fvoci_model::tidy_copied_relations(values, &id, definitions);
    }
}

/// Converts a Pass-1 [`ScannedBoard`] into a compact Pass-2 [`BoardWritePlan`], generating
/// fresh container and block IDs in deterministic container-grouped order and returning
/// any cross-board copies (`old_id -> new_id`) requested by `elsewhere`.
pub(super) fn build_write_plan(
    scanned: ScannedBoard,
    elsewhere: &HashSet<String>,
) -> (BoardWritePlan, HashMap<String, String>) {
    let ScannedBoard {
        zip_index,
        dir,
        mut containers,
        members,
        schedules,
        files,
        blocks,
        keep,
        referenced,
        external_refs: _,
        diaries: _,
    } = scanned;

    let mut old_to_new_container: HashMap<String, String> =
        HashMap::with_capacity(containers.len());
    let mut container_new_ids: Vec<String> = Vec::with_capacity(containers.len());
    let mut card_properties: HashMap<String, Vec<CardPropertyDefinition>> =
        HashMap::with_capacity(containers.len());
    let mut relations = RelationProperties::new();
    for container in &mut containers {
        let old_id = std::mem::take(&mut container.id);
        let new_cid = new_id();
        let mapped = old_to_new_container
            .entry(old_id)
            .or_insert_with(|| new_cid.clone())
            .clone();
        card_properties.insert(new_cid.clone(), container.card_properties.clone());
        relations.add_board(&new_cid, &container.card_properties);
        container.id = new_cid;
        container_new_ids.push(mapped);
    }

    let (block_container_idx, new_block_ids, renamed, card_boards, has_children, recheck_lines) =
        assign_block_ids(&container_new_ids, &blocks, &keep, &referenced, elsewhere);

    let copies: HashMap<String, String> = renamed
        .iter()
        .filter(|(old, _)| elsewhere.contains(old.as_str()))
        .map(|(old, new)| (old.clone(), new.clone()))
        .collect();

    let plan = BoardWritePlan {
        zip_index,
        dir,
        containers,
        members,
        schedules,
        files,
        recheck_lines,
        stream_plan: StreamPlan {
            container_new_ids,
            block_container_idx,
            card_properties,
            new_block_ids,
            renamed,
            card_boards,
            relations,
            has_children,
        },
    };
    (plan, copies)
}

type AssignedBlockIds = (
    Vec<Option<u32>>,
    Vec<Option<String>>,
    HashMap<String, String>,
    HashMap<String, String>,
    HashSet<String>,
    HashMap<usize, usize>,
);

/// Assigns deterministic fresh IDs to kept blocks grouped by container and computes the
/// parent-children set and post-rewrite field recheck set.
fn assign_block_ids(
    container_new_ids: &[String],
    blocks: &[BlockMeta],
    keep: &[bool],
    referenced: &HashSet<String>,
    elsewhere: &HashSet<String>,
) -> AssignedBlockIds {
    let mut indices_by_cidx: Vec<Vec<usize>> = vec![Vec::new(); container_new_ids.len()];
    let mut block_container_idx: Vec<Option<u32>> = vec![None; blocks.len()];
    let mut recheck_lines: HashMap<usize, usize> = HashMap::new();
    for (idx, (meta, &kept)) in blocks.iter().zip(keep).enumerate() {
        if kept && let Some(cidx) = meta.container_idx {
            indices_by_cidx[cidx].push(idx);
            block_container_idx[idx] = u32::try_from(cidx).ok();
            if meta.needs_fields_recheck {
                recheck_lines.insert(meta.line_number, idx);
            }
        }
    }

    let mut new_block_ids: Vec<Option<String>> = vec![None; blocks.len()];
    let mut renamed: HashMap<String, String> = HashMap::new();
    let mut card_boards: HashMap<String, String> = HashMap::new();
    for (cidx, new_cid) in container_new_ids.iter().enumerate() {
        for &idx in &indices_by_cidx[cidx] {
            let meta = &blocks[idx];
            if meta.block_type == BlockType::Card {
                card_boards.insert(meta.id.clone(), new_cid.clone());
            }
            let assigned = if referenced.contains(&meta.id) || elsewhere.contains(&meta.id) {
                renamed
                    .entry(meta.id.clone())
                    .or_insert_with(new_id)
                    .clone()
            } else {
                new_id()
            };
            new_block_ids[idx] = Some(assigned);
        }
    }

    let mut has_children: HashSet<String> = HashSet::new();
    for (meta, assigned) in blocks.iter().zip(&new_block_ids) {
        if assigned.is_some() && !meta.parent_id.is_empty() {
            let parent_new = renamed
                .get(&meta.parent_id)
                .cloned()
                .unwrap_or_else(|| meta.parent_id.clone());
            has_children.insert(parent_new);
        }
    }
    (
        block_container_idx,
        new_block_ids,
        renamed,
        card_boards,
        has_children,
        recheck_lines,
    )
}

mod recheck;
pub(super) use recheck::recheck_planned_fields;
