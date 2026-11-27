//! Pass 1.5 post-rewrite block field validation for streamed native `.fvoci` archives.

use fvoci_model::{UnmatchedRelation, rewrite_block_ids};

use super::{
    AppError, Archive, ArchiveFiles, BlockType, BoardWritePlan, BufReader, CardPropertyDefinition,
    HashMap, MAX_JSONL_BYTES, MAX_LINE_BYTES, RawArchiveLine, RawValue, RelationProperties,
    STREAM_READER_BUF_BYTES, StoreError, StreamPlan, Value, clear_block_values_of_another_kind,
    decode_archive_block, entry_name, read_bounded_line,
};
use crate::app::files::stored_extension;
use crate::auth::secret::SECRET_LENGTH;

/// Pass 1.5: validates the exact post-rewrite `Block` for any kept block flagged during
/// Pass 1 (`recheck_lines`), skipping all unflagged lines without JSON parsing. Runs
/// before any board is committed so multi-board archives still reject invalid blocks 400
/// before storing earlier boards.
pub(in crate::app::import) async fn recheck_planned_fields(
    zip: &Archive,
    plan: &BoardWritePlan,
    modified_by: &str,
) -> Result<(), AppError> {
    if plan.recheck_lines.is_empty() {
        return Ok(());
    }
    let staged_names = simulated_staged_file_names(zip, &plan.dir, &plan.files)?;
    let mut zip = zip.clone();
    let zip_index = plan.zip_index;
    let recheck_lines = plan.recheck_lines.clone();
    let stream_plan = StreamPlanSnapshot::from(&plan.stream_plan);
    let modified_by = modified_by.to_owned();
    tokio::task::spawn_blocking(move || {
        recheck_planned_fields_blocking(
            &mut zip,
            zip_index,
            &recheck_lines,
            &stream_plan,
            &staged_names,
            &modified_by,
        )
    })
    .await
    .map_err(|error| AppError::internal(format_args!("archive recheck task failed: {error}")))?
}

fn simulated_staged_file_names(
    zip: &Archive,
    board_dir: &str,
    files: &ArchiveFiles,
) -> Result<HashMap<String, String>, AppError> {
    let secret_placeholder = "x".repeat(SECRET_LENGTH);
    let mut names = HashMap::new();
    for index in 0..zip.len() {
        let (dir, filename) = entry_name(zip, index)?;
        if dir != board_dir
            || filename.is_empty()
            || filename == "version.json"
            || filename == "container.jsonl"
            || files.missing.contains(&filename)
        {
            continue;
        }
        let upload_name = files
            .names
            .get(&filename)
            .map_or(filename.as_str(), String::as_str);
        let ext = stored_extension(upload_name);
        names.insert(filename, format!("{secret_placeholder}{ext}"));
    }
    Ok(names)
}

struct StreamPlanSnapshot {
    container_new_ids: Vec<String>,
    block_container_idx: Vec<Option<u32>>,
    card_properties: HashMap<String, Vec<CardPropertyDefinition>>,
    new_block_ids: Vec<Option<String>>,
    renamed: HashMap<String, String>,
    card_boards: HashMap<String, String>,
    relations: RelationProperties,
}

impl From<&StreamPlan> for StreamPlanSnapshot {
    fn from(plan: &StreamPlan) -> Self {
        Self {
            container_new_ids: plan.container_new_ids.clone(),
            block_container_idx: plan.block_container_idx.clone(),
            card_properties: plan.card_properties.clone(),
            new_block_ids: plan.new_block_ids.clone(),
            renamed: plan.renamed.clone(),
            card_boards: plan.card_boards.clone(),
            relations: plan.relations.clone(),
        }
    }
}

fn recheck_planned_fields_blocking(
    zip: &mut Archive,
    zip_index: usize,
    recheck_lines: &HashMap<usize, usize>,
    plan: &StreamPlanSnapshot,
    staged_names: &HashMap<String, String>,
    modified_by: &str,
) -> Result<(), AppError> {
    let entry = zip
        .by_index(zip_index)
        .map_err(|error| AppError::bad_request(format_args!("{error}")))?;
    let entry_label = entry
        .name()
        .map(std::borrow::Cow::into_owned)
        .unwrap_or_default();
    let mut reader = BufReader::with_capacity(STREAM_READER_BUF_BYTES, entry);
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
        let Some(&idx) = recheck_lines.get(&line_number) else {
            continue;
        };
        let line = line_buf.trim_ascii();
        let archive_line: RawArchiveLine = serde_json::from_slice(line).map_err(|error| {
            AppError::bad_request(format_args!(
                "error parsing archive line {line_number}: {error}"
            ))
        })?;
        let raw_data = archive_line.data.as_deref().map_or("null", RawValue::get);
        let mut block = decode_archive_block(raw_data, line_number)?;
        let Some(new_block_id) = plan.new_block_ids.get(idx).and_then(Clone::clone) else {
            continue;
        };
        let Some(cidx) = plan
            .block_container_idx
            .get(idx)
            .and_then(|c| c.and_then(|v| usize::try_from(v).ok()))
        else {
            continue;
        };
        let new_cid = &plan.container_new_ids[cidx];
        modified_by.clone_into(&mut block.modified_by);
        if let Some(defs) = plan.card_properties.get(new_cid) {
            clear_block_values_of_another_kind(&mut block, defs);
        }
        block.container_id.clone_from(new_cid);
        rewrite_block_ids(
            &mut block,
            new_block_id,
            &plan.renamed,
            &plan.card_boards,
            &plan.relations,
            UnmatchedRelation::Dropped,
        );
        if block.block_type == BlockType::Attachment {
            for key in ["fileId", "attachmentId"] {
                if let Some(old) = block.fields.get(key).and_then(Value::as_str) {
                    let simulated = staged_names.get(old).cloned().unwrap_or_default();
                    block
                        .fields
                        .insert(key.to_owned(), Value::String(simulated));
                }
            }
        }
        block.validate().map_err(|source| {
            AppError::from(StoreError::InvalidBlock {
                id: block.id.clone(),
                source,
            })
        })?;
    }
    Ok(())
}
