//! Pass-2 blocking streamer for native fvoci v1 `container.jsonl` archive entries (T12).
//!
//! Streams rewritten blocks line by line into bounded batches sent over a backpressured
//! channel to `Store::create_containers`. Handles out-of-order parent
//! blocks using a bounded spillover buffer plus bounded revisit passes that skip already
//! emitted lines without JSON parsing.

use std::collections::{HashMap, HashSet};
use std::io::BufReader;
use std::sync::Arc;
use std::time::{Duration, Instant};

use fvoci_model::{Block, BlockType, UnmatchedRelation, rewrite_block_ids};
use fvoci_store::{StoreError, StreamBatch};
use serde_json::value::RawValue;
use tokio::sync::mpsc;

use super::MAX_JSONL_BYTES;
use super::archive::Archive;
use super::native_stream::{
    LINE_BUF_SHRINK_THRESHOLD, MAX_LINE_BYTES, RawArchiveLine, STREAM_READER_BUF_BYTES, StreamPlan,
    clear_block_values_of_another_kind, decode_archive_block, read_bounded_line,
};
use super::pointing::{came_with_more, pointed};
use super::summary::ImportCounts;
use crate::auth::now_millis;
use crate::error::AppError;

/// Hard wall-clock deadline for Pass-2 archive streaming so revisit sweeps cannot hold
/// the organization's visibility write counter indefinitely.
pub(super) const IMPORT_STREAM_TIMEOUT: Duration = Duration::from_secs(60);

/// Maximum number of blocks accumulated in one Pass-2 insert batch.
const BATCH_MAX_BLOCKS: usize = 64;

/// Maximum raw JSONL line bytes accumulated in one Pass-2 insert batch. Kept modest
/// because dense small-node JSON bodies expand ~25–33x as `serde_json::Value` ASTs.
const BATCH_MAX_RAW_BYTES: usize = 512 * 1024;

/// Maximum raw bytes buffered in memory for out-of-order child blocks whose parent
/// appears later in `container.jsonl`; overflow blocks are dropped and re-read on a revisit.
const MAX_PENDING_RAW_BYTES: usize = 2 * 1024 * 1024;

/// Hard cap on Pass-2 revisit sweeps for pathologically reverse-ordered archives so a
/// small DEFLATE-compressed upload cannot trigger quadratic decompression CPU work.
pub(super) const MAX_REVISIT_PASSES: usize = 32;

/// Channel depth between the blocking Pass-2 streamer and the async DB batch inserter.
const STREAM_CHANNEL_CAPACITY: usize = 1;

/// Summary metadata collected while streaming all `container.jsonl` entries in Pass 2.
#[derive(Debug, Default)]
pub(super) struct StreamedBoardStats {
    pub(super) counts: ImportCounts,
    /// Deduplicated inserted block IDs (populated only when WS/webhooks are active).
    pub(super) inserted_block_ids: Vec<String>,
}

/// Whether Pass 2 should retain every inserted block ID for post-commit broadcasts.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum BlockIdCollection {
    None,
    All,
}

/// Which sweep of `container.jsonl` Pass 2 is currently performing.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum StreamPass {
    Initial,
    Revisit,
}

/// Channel receiver and join handle returned by [`spawn_archive_streamer`].
pub(super) struct StreamedBoardHandle {
    pub(super) rx: mpsc::Receiver<StreamBatch<AppError>>,
    pub(super) stats: tokio::task::JoinHandle<StreamedBoardStats>,
}

/// Shared lookup tables read while streaming all boards of a native archive.
pub(super) struct ArchiveStreamShared {
    pub(super) file_map: Arc<HashMap<String, HashMap<String, String>>>,
    pub(super) all_copies: Arc<HashMap<String, String>>,
    pub(super) modified_by: String,
    pub(super) collect_ids: BlockIdCollection,
}

/// Spawns one Pass-2 blocking streamer across every planned `container.jsonl` entry of a
/// native archive, returning a bounded receiver of ready-to-insert block batches (with
/// body file names, attachment fields, and cross-board references already pointed) and a
/// join handle carrying combined summary stats.
pub(super) fn spawn_archive_streamer(
    zip: Archive,
    plans: Vec<(usize, StreamPlan)>,
    shared: ArchiveStreamShared,
) -> StreamedBoardHandle {
    let (tx, rx) = mpsc::channel(STREAM_CHANNEL_CAPACITY);
    let stats = tokio::task::spawn_blocking(move || {
        let mut combined = StreamedBoardStats::default();
        let now = now_millis();
        let deadline = Instant::now() + IMPORT_STREAM_TIMEOUT;
        for (zip_index, plan) in plans {
            let mut streamer = BoardStreamer::new(
                zip.clone(),
                zip_index,
                plan,
                &shared,
                now,
                deadline,
                tx.clone(),
            );
            match streamer.stream_all_passes() {
                Ok(()) => {
                    combined.counts.cards += streamer.stats.counts.cards;
                    combined.counts.pages += streamer.stats.counts.pages;
                    combined.counts.units += streamer.stats.counts.units;
                    combined.counts.comments += streamer.stats.counts.comments;
                    combined
                        .inserted_block_ids
                        .extend(streamer.stats.inserted_block_ids);
                }
                Err(error) => {
                    let _ = tx.blocking_send(StreamBatch::Err(error));
                    return StreamedBoardStats::default();
                }
            }
        }
        let _ = tx.blocking_send(StreamBatch::Done);
        combined
    });
    StreamedBoardHandle { rx, stats }
}

struct BoardStreamer<'a> {
    zip: Archive,
    zip_index: usize,
    plan: StreamPlan,
    shared: &'a ArchiveStreamShared,
    now: i64,
    deadline: Instant,
    tx: mpsc::Sender<StreamBatch<AppError>>,
    emitted_parents: HashSet<String>,
    pending: Vec<(Block, usize)>,
    pending_raw_bytes: usize,
    /// `line_number -> block_index` for overflow blocks skipped during `Initial` and
    /// selectively re-read on `Revisit` without JSON-parsing skipped lines.
    revisit_lines: HashMap<usize, usize>,
    batch: Vec<Block>,
    batch_raw_bytes: usize,
    seen_inserted: HashSet<String>,
    stats: StreamedBoardStats,
}

impl<'a> BoardStreamer<'a> {
    fn new(
        zip: Archive,
        zip_index: usize,
        plan: StreamPlan,
        shared: &'a ArchiveStreamShared,
        now: i64,
        deadline: Instant,
        tx: mpsc::Sender<StreamBatch<AppError>>,
    ) -> Self {
        Self {
            zip,
            zip_index,
            plan,
            shared,
            now,
            deadline,
            tx,
            emitted_parents: HashSet::new(),
            pending: Vec::new(),
            pending_raw_bytes: 0,
            revisit_lines: HashMap::new(),
            batch: Vec::new(),
            batch_raw_bytes: 0,
            seen_inserted: HashSet::new(),
            stats: StreamedBoardStats::default(),
        }
    }

    fn check_deadline(&self) -> Result<(), AppError> {
        if Instant::now() > self.deadline {
            return Err(AppError::bad_request(
                "archive import exceeded maximum streaming time limit",
            ));
        }
        Ok(())
    }

    fn stream_all_passes(&mut self) -> Result<(), AppError> {
        self.stream_one_pass(StreamPass::Initial)?;
        self.drain_ready_pending()?;
        self.flush_batch()?;

        let mut revisit_passes: usize = 0;
        while !self.revisit_lines.is_empty() || !self.pending.is_empty() {
            let before_revisit = self.revisit_lines.len();
            let before_pending = self.pending.len();
            if !self.revisit_lines.is_empty() {
                revisit_passes += 1;
                if revisit_passes > MAX_REVISIT_PASSES {
                    return Err(AppError::bad_request(
                        "archive blocks exceed maximum out-of-order parent depth",
                    ));
                }
                self.stream_one_pass(StreamPass::Revisit)?;
            }
            self.drain_ready_pending()?;
            self.flush_batch()?;
            if self.revisit_lines.len() == before_revisit && self.pending.len() == before_pending {
                let leftover: Vec<Block> = self.pending.drain(..).map(|(block, _)| block).collect();
                self.pending_raw_bytes = 0;
                if !leftover.is_empty() {
                    for block in leftover {
                        self.record_and_queue(block, 0)?;
                    }
                    self.flush_batch()?;
                    // Emitting leftover blocks may unblock revisit lines on the next loop.
                    continue;
                }
                if !self.revisit_lines.is_empty() {
                    return Err(AppError::bad_request(
                        "archive blocks contain an unresolvable parent cycle",
                    ));
                }
            }
        }
        Ok(())
    }

    fn stream_one_pass(&mut self, pass: StreamPass) -> Result<(), AppError> {
        let mut zip_clone = self.zip.clone();
        let entry = zip_clone
            .by_index(self.zip_index)
            .map_err(|error| AppError::bad_request(format_args!("{error}")))?;
        let entry_label = entry.name().map_or_default(std::borrow::Cow::into_owned);
        let mut reader = BufReader::with_capacity(STREAM_READER_BUF_BYTES, entry);
        let mut line_buf = Vec::with_capacity(4096);
        let mut total_read: u64 = 0;
        let mut line_number: usize = 0;
        let mut block_index: usize = 0;

        while read_bounded_line(
            &mut reader,
            &mut line_buf,
            MAX_LINE_BYTES,
            &mut total_read,
            MAX_JSONL_BYTES,
            &entry_label,
        )? {
            line_number += 1;
            if line_number.trailing_zeros() >= 8 {
                self.check_deadline()?;
            }
            if pass == StreamPass::Revisit && !self.revisit_lines.contains_key(&line_number) {
                continue;
            }
            let line = line_buf.trim_ascii();
            if line.is_empty() {
                continue;
            }
            let raw_len = line.len();
            let archive_line: RawArchiveLine = serde_json::from_slice(line).map_err(|error| {
                AppError::bad_request(format_args!(
                    "error parsing archive line {line_number}: {error}"
                ))
            })?;
            match archive_line.line_type.as_str() {
                "container" | "containerMember" | "courseSchedule" => {}
                "block" => {
                    let idx = match pass {
                        StreamPass::Initial => {
                            let current = block_index;
                            block_index += 1;
                            current
                        }
                        StreamPass::Revisit => {
                            let Some(&idx) = self.revisit_lines.get(&line_number) else {
                                continue;
                            };
                            idx
                        }
                    };
                    if self
                        .plan
                        .new_block_ids
                        .get(idx)
                        .is_some_and(Option::is_some)
                    {
                        let raw_data = archive_line.data.as_deref().map_or("null", RawValue::get);
                        self.handle_block_line(idx, raw_data, line_number, raw_len)?;
                    }
                }
                other => {
                    return Err(AppError::bad_request(format_args!(
                        "unsupported archive line type; got {other}, line {line_number}"
                    )));
                }
            }
            if line_buf.capacity() > LINE_BUF_SHRINK_THRESHOLD {
                line_buf.clear();
                line_buf.shrink_to(STREAM_READER_BUF_BYTES);
            }
        }
        Ok(())
    }

    fn handle_block_line(
        &mut self,
        idx: usize,
        raw_data: &str,
        line_number: usize,
        raw_len: usize,
    ) -> Result<(), AppError> {
        let Some(new_block_id) = self.plan.new_block_ids.get(idx).and_then(Clone::clone) else {
            return Ok(());
        };
        let Some(cidx) = self
            .plan
            .block_container_idx
            .get(idx)
            .and_then(|c| c.and_then(|v| usize::try_from(v).ok()))
        else {
            return Ok(());
        };
        let new_cid = &self.plan.container_new_ids[cidx];
        let mut block = decode_archive_block(raw_data, line_number)?;
        self.shared.modified_by.clone_into(&mut block.modified_by);
        block.updated_at = self.now;
        block.deleted_at = None;
        if let Some(defs) = self.plan.card_properties.get(new_cid) {
            clear_block_values_of_another_kind(&mut block, defs);
        }
        block.container_id.clone_from(new_cid);
        rewrite_block_ids(
            &mut block,
            new_block_id,
            &self.plan.renamed,
            &self.plan.card_boards,
            &self.plan.relations,
            UnmatchedRelation::Dropped,
        );
        point_block_files_and_copies(&mut block, &self.shared.file_map, &self.shared.all_copies);
        block.validate().map_err(|source| {
            AppError::from(StoreError::InvalidBlock {
                id: block.id.clone(),
                source,
            })
        })?;

        if self.is_ready(&block) {
            self.revisit_lines.remove(&line_number);
            self.record_and_queue(block, raw_len)?;
            self.drain_ready_pending()?;
        } else if self.pending_raw_bytes.saturating_add(raw_len) <= MAX_PENDING_RAW_BYTES {
            self.revisit_lines.remove(&line_number);
            self.pending_raw_bytes = self.pending_raw_bytes.saturating_add(raw_len);
            self.pending.push((block, raw_len));
        } else {
            // Drop body immediately and revisit after parents have been emitted.
            self.revisit_lines.insert(line_number, idx);
        }
        Ok(())
    }

    fn is_ready(&self, block: &Block) -> bool {
        block.parent_id.is_empty() || self.emitted_parents.contains(&block.parent_id)
    }

    fn drain_ready_pending(&mut self) -> Result<(), AppError> {
        loop {
            let mut progressed = false;
            let mut remaining = Vec::new();
            for (block, raw_len) in std::mem::take(&mut self.pending) {
                if self.is_ready(&block) {
                    self.pending_raw_bytes = self.pending_raw_bytes.saturating_sub(raw_len);
                    self.record_and_queue(block, raw_len)?;
                    progressed = true;
                } else {
                    remaining.push((block, raw_len));
                }
            }
            self.pending = remaining;
            if !progressed {
                return Ok(());
            }
        }
    }

    fn record_and_queue(&mut self, block: Block, raw_len: usize) -> Result<(), AppError> {
        if !self.batch.is_empty()
            && self.batch_raw_bytes.saturating_add(raw_len) > BATCH_MAX_RAW_BYTES
        {
            self.flush_batch()?;
        }
        if self.plan.has_children.contains(&block.id) {
            self.emitted_parents.insert(block.id.clone());
        }
        match block.block_type {
            BlockType::Card => self.stats.counts.cards += 1,
            BlockType::Page => self.stats.counts.pages += 1,
            BlockType::Unit => self.stats.counts.units += 1,
            BlockType::Comment => self.stats.counts.comments += 1,
            _ => {}
        }
        if self.shared.collect_ids == BlockIdCollection::All
            && self.seen_inserted.insert(block.id.clone())
        {
            self.stats.inserted_block_ids.push(block.id.clone());
        }
        self.batch.push(block);
        self.batch_raw_bytes = self.batch_raw_bytes.saturating_add(raw_len);
        if self.batch.len() >= BATCH_MAX_BLOCKS || self.batch_raw_bytes >= BATCH_MAX_RAW_BYTES {
            self.flush_batch()?;
        }
        Ok(())
    }

    fn flush_batch(&mut self) -> Result<(), AppError> {
        self.check_deadline()?;
        if self.batch.is_empty() {
            return Ok(());
        }
        let batch = std::mem::take(&mut self.batch);
        self.batch_raw_bytes = 0;
        self.tx
            .blocking_send(StreamBatch::Batch(batch))
            .map_err(|_| AppError::internal("import batch consumer dropped early"))
    }
}

/// Points a streamed block's body and attachment fields at the newly stored file names
/// and cross-container copies in-flight prior to database insertion.
fn point_block_files_and_copies(
    block: &mut Block,
    file_map: &HashMap<String, HashMap<String, String>>,
    all_copies: &HashMap<String, String>,
) {
    let container_files = file_map.get(block.container_id.as_str());
    if let Some(body) = block.body.take() {
        block.body = if came_with_more(&body, all_copies) {
            pointed(
                body,
                |old| container_files.and_then(|files| files.get(old)).cloned(),
                all_copies,
            )
        } else {
            Some(body)
        };
    }
    if block.block_type == BlockType::Attachment {
        for key in ["fileId", "attachmentId"] {
            if let Some(old) = block.fields.get(key).and_then(serde_json::Value::as_str) {
                let new_name = container_files
                    .and_then(|files| files.get(old))
                    .cloned()
                    .unwrap_or_default();
                block
                    .fields
                    .insert(key.to_owned(), serde_json::Value::String(new_name));
            }
        }
    }
}
