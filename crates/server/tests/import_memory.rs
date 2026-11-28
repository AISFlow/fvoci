//! Peak allocator and RSS memory tests for native `container.jsonl` archive imports (T12).
//!
//! Uses a counting [`GlobalAlloc`] so peak heap usage during `App::import_external_archive`
//! is measured directly in bytes, alongside Linux `VmHWM` when available.

#[path = "api/common.rs"]
mod common;

use std::alloc::{GlobalAlloc, Layout, System};
use std::fs::File;
use std::io::{BufWriter, Write as _};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Instant;

use fvoci_model::{Block, BlockType, Container, PublicContainers, new_id};
use fvoci_server::app::ImportFormat;
use fvoci_server::error::ErrorKind;
use fvoci_store::{Creator, NewBodyFiles, NewContainers, StreamBatch, StreamCreateError};
use serde_json::json;
use zip::write::SimpleFileOptions;

use common::{Setup, TEST_WORKSPACE_ID};

static ALLOCATED: AtomicUsize = AtomicUsize::new(0);
static PEAK: AtomicUsize = AtomicUsize::new(0);

struct CountingAllocator;

// SAFETY: Every call forwards directly to `std::alloc::System` with the exact
// layout and pointer passed in; the atomic counters perform no heap allocation.
unsafe impl GlobalAlloc for CountingAllocator {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        let now = ALLOCATED.fetch_add(layout.size(), Ordering::Relaxed) + layout.size();
        PEAK.fetch_max(now, Ordering::Relaxed);
        // SAFETY: caller upholds `GlobalAlloc::alloc` contract for `layout`.
        unsafe { System.alloc(layout) }
    }

    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        ALLOCATED.fetch_sub(layout.size(), Ordering::Relaxed);
        // SAFETY: caller passes a pointer previously allocated by `System::alloc` with `layout`.
        unsafe { System.dealloc(ptr, layout) }
    }
}

#[global_allocator]
static COUNTING: CountingAllocator = CountingAllocator;

fn live_bytes() -> usize {
    ALLOCATED.load(Ordering::Relaxed)
}

fn reset_peak() -> usize {
    let baseline = live_bytes();
    PEAK.store(baseline, Ordering::Relaxed);
    baseline
}

fn peak_delta(baseline: usize) -> usize {
    PEAK.load(Ordering::Relaxed).saturating_sub(baseline)
}

fn bytes_to_mib(bytes: usize) -> f64 {
    let kib = u32::try_from(bytes / 1024).unwrap_or(u32::MAX);
    f64::from(kib) / 1024.0
}

fn kib_to_mib(kib: u64) -> f64 {
    let kib = u32::try_from(kib).unwrap_or(u32::MAX);
    f64::from(kib) / 1024.0
}

fn vm_hwm_kib() -> Option<u64> {
    let status = std::fs::read_to_string("/proc/self/status").ok()?;
    for line in status.lines() {
        if let Some(rest) = line.strip_prefix("VmHWM:") {
            let value = rest.split_whitespace().next()?;
            return value.parse::<u64>().ok();
        }
    }
    None
}

struct TempDir(PathBuf);

impl TempDir {
    fn new() -> Self {
        let path = std::env::temp_dir().join(format!("fvoci-import-mem-{}", new_id()));
        std::fs::create_dir_all(&path).expect("create temp dir");
        Self(path)
    }

    fn path(&self) -> &Path {
        &self.0
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

#[derive(Clone, Copy)]
enum CardShape {
    /// Cards with large prose paragraphs (~128 KiB JSON each).
    RichParagraphs,
    /// Cards with many tiny paragraph/text nodes (~128 KiB JSON each; high AST expansion).
    SmallNodes,
}

impl CardShape {
    const fn label(self) -> &'static str {
        match self {
            Self::RichParagraphs => "rich-paragraphs",
            Self::SmallNodes => "small-nodes",
        }
    }

    fn build_body(self) -> serde_json::Value {
        match self {
            Self::RichParagraphs => {
                let text = "Lorem ipsum dolor sit amet, consectetur adipiscing elit. ".repeat(32);
                let paragraphs: Vec<_> = (0..70)
                    .map(|_| json!({"type": "paragraph", "content": [{"type": "text", "text": text}]}))
                    .collect();
                json!({"type": "doc", "content": paragraphs})
            }
            Self::SmallNodes => {
                // ~2,000 small paragraphs per card (~125 KiB JSON) to stress JSON AST expansion.
                let paragraphs: Vec<_> = (0..2_000)
                    .map(|idx| json!({"type": "paragraph", "content": [{"type": "text", "text": format!("n{idx}")}]}))
                    .collect();
                json!({"type": "doc", "content": paragraphs})
            }
        }
    }
}

/// Streams a native v1 ZIP archive directly to `dest` whose `b1/container.jsonl` is at least
/// `target_jsonl_bytes` long (never exceeding 64 MiB), returning `(actual_bytes, cards)`.
fn write_large_native_archive(
    dest: &Path,
    target_jsonl_bytes: usize,
    shape: CardShape,
) -> (usize, usize) {
    let file = File::create(dest).expect("create archive file");
    let mut writer = zip::ZipWriter::new(BufWriter::new(file));
    let opts = SimpleFileOptions::default().compression_method(zip::CompressionMethod::Deflated);

    writer
        .start_file("version.json", opts)
        .expect("start version");
    writer
        .write_all(br#"{"version":1,"date":1}"#)
        .expect("write version");

    writer
        .start_file("b1/container.jsonl", opts)
        .expect("start container.jsonl");
    let board_line = format!(
        "{}\n",
        json!({
            "type": "container",
            "data": {
                "id": "b1",
                "workspaceId": TEST_WORKSPACE_ID,
                "visibility": "private",
                "kind": "board",
                "title": "Large Native Import",
                "createdAt": 1,
                "updatedAt": 1,
            }
        })
    );
    writer
        .write_all(board_line.as_bytes())
        .expect("write board");
    let mut written = board_line.len();

    let body = shape.build_body();
    let mut cards = 0usize;
    while written < target_jsonl_bytes {
        let card_id = format!("card-{cards:06}");
        let line = format!(
            "{}\n",
            json!({
                "type": "block",
                "data": {
                    "id": card_id,
                    "containerId": "b1",
                    "parentId": "",
                    "type": "card",
                    "title": format!("Card {cards}"),
                    "fields": {},
                    "body": body,
                    "createdAt": 1,
                    "updatedAt": 1,
                }
            })
        );
        if written + line.len() > 64 * 1024 * 1024 {
            break;
        }
        writer.write_all(line.as_bytes()).expect("write card");
        written += line.len();
        cards += 1;
    }

    writer.finish().expect("finish archive");
    (written, cards)
}

#[tokio::test]
#[ignore = "memory measurement for T12, run by name in release mode"]
async fn measure_native_archive_import_memory() {
    let tmp = TempDir::new();
    println!(
        "| fixture | container.jsonl MiB | cards | peak heap delta MiB | VmHWM MiB | elapsed ms |"
    );
    println!("| --- | ---: | ---: | ---: | ---: | ---: |");

    for (label, target_mib, shape) in [
        ("32 MiB rich-paragraphs", 32, CardShape::RichParagraphs),
        ("32 MiB small-nodes", 32, CardShape::SmallNodes),
        ("62 MiB rich-paragraphs", 62, CardShape::RichParagraphs),
        ("62 MiB small-nodes", 62, CardShape::SmallNodes),
    ] {
        let archive_path = tmp
            .path()
            .join(format!("{}-{target_mib}.fvoci", shape.label()));
        let (jsonl_bytes, expected_cards) =
            write_large_native_archive(&archive_path, target_mib * 1024 * 1024, shape);

        let ts = Setup::native().start().await.init_basic().await;
        let importer_id = ts.client.me().await.id;

        let baseline = reset_peak();
        let started = Instant::now();
        let summary = ts
            .app
            .import_external_archive(
                &archive_path,
                ImportFormat::Fvoci,
                TEST_WORKSPACE_ID,
                &importer_id,
            )
            .await
            .expect("native import succeeds");
        let elapsed_ms = started.elapsed().as_millis();
        let peak_bytes = peak_delta(baseline);
        let hwm_mib = vm_hwm_kib().map_or(0.0, kib_to_mib);

        assert_eq!(summary.imported.boards, 1, "{label}: one board");
        let blocks = ts
            .app
            .blocks_for_container(&summary.container_ids[0])
            .await
            .expect("read imported blocks");
        let card_count = blocks
            .iter()
            .filter(|b| b.block_type == BlockType::Card)
            .count();
        assert_eq!(card_count, expected_cards, "{label}: all cards imported");

        println!(
            "| {label} | {:.2} | {expected_cards} | {:.2} | {:.2} | {elapsed_ms} |",
            bytes_to_mib(jsonl_bytes),
            bytes_to_mib(peak_bytes),
            hwm_mib,
        );

        ts.teardown().await;
        let _ = std::fs::remove_file(&archive_path);
    }
}

/// Fast regression guard (runs in standard test suite): imports an 8 MiB small-nodes
/// `container.jsonl` archive AND an out-of-order parent archive (comments before cards),
/// verifying all blocks/comments round-trip and peak heap stays far below the ~200+ MiB
/// non-streaming expansion.
#[tokio::test]
async fn streaming_native_import_bounds_peak_memory_and_handles_out_of_order_parents() {
    let tmp = TempDir::new();
    let archive_path = tmp.path().join("small-nodes-8mib.fvoci");
    let (_jsonl_bytes, expected_cards) =
        write_large_native_archive(&archive_path, 8 * 1024 * 1024, CardShape::SmallNodes);

    let ts = Setup::native().start().await.init_basic().await;
    let importer_id = ts.client.me().await.id;

    let baseline = reset_peak();
    let summary = ts
        .app
        .import_external_archive(
            &archive_path,
            ImportFormat::Fvoci,
            TEST_WORKSPACE_ID,
            &importer_id,
        )
        .await
        .expect("streaming 8 MiB native import succeeds");
    let peak_bytes = peak_delta(baseline);

    assert_eq!(summary.imported.boards, 1);
    assert_eq!(summary.imported.cards, expected_cards);
    // Non-streaming 8 MiB small-nodes expands to >200 MiB AST; streaming stays well under 120 MiB
    // even in unoptimized debug builds.
    let ceiling = 120 * 1024 * 1024;
    assert!(
        peak_bytes < ceiling,
        "streaming peak heap delta {:.2} MiB exceeded {ceiling} bytes ceiling",
        bytes_to_mib(peak_bytes)
    );

    // Out-of-order parents: comments appear BEFORE their parent cards in `container.jsonl`.
    let ooo_path = tmp.path().join("out-of-order.fvoci");
    {
        let file = File::create(&ooo_path).expect("create ooo archive");
        let mut writer = zip::ZipWriter::new(BufWriter::new(file));
        let opts =
            SimpleFileOptions::default().compression_method(zip::CompressionMethod::Deflated);
        writer.start_file("version.json", opts).expect("version");
        writer.write_all(br#"{"version":1,"date":1}"#).expect("w");
        writer
            .start_file("b1/container.jsonl", opts)
            .expect("board");
        let lines = [
            json!({"type": "container", "data": {
                "id": "b1", "workspaceId": TEST_WORKSPACE_ID, "visibility": "private", "kind": "board",
                "title": "Out Of Order", "createdAt": 1, "updatedAt": 1
            }}),
            json!({"type": "block", "data": {
                "id": "cm1", "containerId": "b1", "parentId": "c1", "type": "comment",
                "title": "", "fields": {}, "createdAt": 1, "updatedAt": 1,
                "body": {"type": "doc", "content": [{"type": "paragraph", "content": [{"type": "text", "text": "before parent"}]}]}
            }}),
            json!({"type": "block", "data": {
                "id": "c1", "containerId": "b1", "parentId": "", "type": "card",
                "title": "Parent Card", "fields": {}, "createdAt": 1, "updatedAt": 1
            }}),
        ];
        for line in lines {
            writer
                .write_all(format!("{line}\n").as_bytes())
                .expect("line");
        }
        writer.finish().expect("finish");
    }
    let ooo = ts
        .app
        .import_external_archive(
            &ooo_path,
            ImportFormat::Fvoci,
            TEST_WORKSPACE_ID,
            &importer_id,
        )
        .await
        .expect("out-of-order native import succeeds");
    assert_eq!(ooo.imported.boards, 1);
    assert_eq!(ooo.imported.cards, 1);
    assert_eq!(ooo.imported.comments, 1);
    let blocks = ts
        .app
        .blocks_for_container(&ooo.container_ids[0])
        .await
        .expect("read blocks");
    let card = blocks
        .iter()
        .find(|b| b.block_type == BlockType::Card)
        .expect("card");
    let comment = blocks
        .iter()
        .find(|b| b.block_type == BlockType::Comment)
        .expect("comment");
    assert_eq!(
        comment.parent_id, card.id,
        "comment attached to renamed parent card"
    );

    ts.teardown().await;
}

fn write_zip_entries(dest: &Path, entries: &[(&str, &[u8])]) {
    let file = File::create(dest).expect("create zip");
    let mut writer = zip::ZipWriter::new(BufWriter::new(file));
    let opts = SimpleFileOptions::default().compression_method(zip::CompressionMethod::Deflated);
    for (name, data) in entries {
        writer.start_file(*name, opts).expect("start entry");
        writer.write_all(data).expect("write entry");
    }
    writer.finish().expect("finish zip");
}

fn jsonl_bytes(lines: &[serde_json::Value]) -> Vec<u8> {
    let mut out = Vec::new();
    for line in lines {
        out.extend_from_slice(line.to_string().as_bytes());
        out.push(b'\n');
    }
    out
}

/// Covers Pass-2 revisit spillover (>2 MiB pending) and the bounded revisit guard
/// (>32 reverse-ordered parent depth rejected with 400 and full rollback).
#[tokio::test]
async fn streaming_native_import_handles_revisit_spillover_and_bounds_deep_reverse_chains() {
    let tmp = TempDir::new();
    let ts = Setup::native().start().await.init_basic().await;
    let importer_id = ts.client.me().await.id;

    // 1. Revisit spillover: 10 x ~260 KiB comments BEFORE their parent card (>2 MiB pending).
    let big_text = "x".repeat(260 * 1024);
    let mut revisit_lines = vec![json!({"type": "container", "data": {
        "id": "b-rev", "workspaceId": TEST_WORKSPACE_ID, "visibility": "private", "kind": "board",
        "title": "Revisit Spillover", "createdAt": 1, "updatedAt": 1
    }})];
    for i in 0..10 {
        revisit_lines.push(json!({"type": "block", "data": {
            "id": format!("cm-{i}"), "containerId": "b-rev", "parentId": "c-parent", "type": "comment",
            "title": "", "fields": {}, "createdAt": 1, "updatedAt": 1,
            "body": {"type": "doc", "content": [{"type": "paragraph", "content": [{"type": "text", "text": big_text}]}]}
        }}));
    }
    revisit_lines.push(json!({"type": "block", "data": {
        "id": "c-parent", "containerId": "b-rev", "parentId": "", "type": "card",
        "title": "Late Parent", "fields": {}, "createdAt": 1, "updatedAt": 1
    }}));
    let revisit_path = tmp.path().join("revisit.fvoci");
    write_zip_entries(
        &revisit_path,
        &[
            ("version.json", br#"{"version":1,"date":1}"#),
            ("b-rev/container.jsonl", &jsonl_bytes(&revisit_lines)),
        ],
    );
    let rev = ts
        .app
        .import_external_archive(
            &revisit_path,
            ImportFormat::Fvoci,
            TEST_WORKSPACE_ID,
            &importer_id,
        )
        .await
        .expect("revisit spillover archive succeeds");
    assert_eq!(rev.imported.cards, 1);
    assert_eq!(rev.imported.comments, 10);

    // 2. Bounded revisit guard: 45-deep reverse-ordered view chain (~260 KiB fields/node)
    //    exceeds MAX_REVISIT_PASSES (32), rejects with 400, and leaves zero containers stored.
    let before_count = ts
        .app
        .containers_for_user_and_workspace(
            &importer_id,
            TEST_WORKSPACE_ID,
            PublicContainers::Exclude,
        )
        .await
        .expect("list containers")
        .len();
    let mut deep_lines = vec![json!({"type": "container", "data": {
        "id": "b-deep", "workspaceId": TEST_WORKSPACE_ID, "visibility": "private", "kind": "board",
        "title": "Too Deep Reverse", "createdAt": 1, "updatedAt": 1
    }})];
    for i in (1..=45).rev() {
        deep_lines.push(json!({"type": "block", "data": {
            "id": format!("n-{i}"), "containerId": "b-deep", "parentId": format!("n-{}", i - 1),
            "type": "view", "title": format!("N{i}"), "fields": {"pad": big_text},
            "createdAt": 1, "updatedAt": 1
        }}));
    }
    deep_lines.push(json!({"type": "block", "data": {
        "id": "n-0", "containerId": "b-deep", "parentId": "", "type": "view",
        "title": "Root", "fields": {}, "createdAt": 1, "updatedAt": 1
    }}));
    let deep_path = tmp.path().join("deep-reverse.fvoci");
    write_zip_entries(
        &deep_path,
        &[
            ("version.json", br#"{"version":1,"date":1}"#),
            ("b-deep/container.jsonl", &jsonl_bytes(&deep_lines)),
        ],
    );
    let err = ts
        .app
        .import_external_archive(
            &deep_path,
            ImportFormat::Fvoci,
            TEST_WORKSPACE_ID,
            &importer_id,
        )
        .await
        .expect_err("deep reverse chain must be rejected");
    assert_eq!(err.kind(), ErrorKind::BadRequest);
    let after_count = ts
        .app
        .containers_for_user_and_workspace(
            &importer_id,
            TEST_WORKSPACE_ID,
            PublicContainers::Exclude,
        )
        .await
        .expect("list containers")
        .len();
    assert_eq!(after_count, before_count, "rejected board rolled back");
    ts.teardown().await;
}

/// Covers empty board IDs in Pass 1, attachment file pointing, and cross-board body
/// references across multiple streamed boards.
#[tokio::test]
async fn streaming_native_import_handles_empty_board_id_attachments_and_cross_board_refs() {
    let tmp = TempDir::new();
    let ts = Setup::native().start().await.init_basic().await;
    let importer_id = ts.client.me().await.id;

    // 1. Empty board ID + file attachment pointing + cross-board body references.
    let old_target_id = new_id();
    let b1_lines = jsonl_bytes(&[
        json!({"type": "container", "data": {
            "id": "", "workspaceId": TEST_WORKSPACE_ID, "visibility": "private", "kind": "board",
            "title": "Empty ID Target", "createdAt": 1, "updatedAt": 1
        }}),
        json!({"type": "block", "data": {
            "id": old_target_id, "containerId": "", "parentId": "", "type": "card",
            "title": "Target", "fields": {}, "createdAt": 1, "updatedAt": 1
        }}),
    ]);
    let b2_lines = jsonl_bytes(&[
        json!({"type": "container", "data": {
            "id": "b-src", "workspaceId": TEST_WORKSPACE_ID, "visibility": "private", "kind": "board",
            "title": "Attachment And CrossRef", "createdAt": 1, "updatedAt": 1
        }}),
        json!({"type": "block", "data": {
            "id": "card-a", "containerId": "b-src", "parentId": "", "type": "card",
            "title": "First", "fields": {}, "createdAt": 1, "updatedAt": 1,
            "body": {"type": "doc", "content": [{"type": "paragraph", "content": [
                {"type": "reference", "attrs": {"id": old_target_id}}
            ]}]}
        }}),
        json!({"type": "block", "data": {
            "id": "card-b", "containerId": "b-src", "parentId": "", "type": "card",
            "title": "Second", "fields": {}, "createdAt": 1, "updatedAt": 1,
            "body": {"type": "doc", "content": [{"type": "paragraph", "content": [
                {"type": "reference", "attrs": {"id": old_target_id}}
            ]}]}
        }}),
        json!({"type": "block", "data": {
            "id": "att-1", "containerId": "b-src", "parentId": "card-a", "type": "attachment",
            "title": "pic.png", "fields": {"fileId": "old.png"}, "createdAt": 1, "updatedAt": 1
        }}),
    ]);
    let png_1x1: &[u8] = &[
        0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A, 0x00, 0x00, 0x00, 0x0D, 0x49, 0x48, 0x44,
        0x52, 0x00, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00, 0x01, 0x08, 0x06, 0x00, 0x00, 0x00, 0x1F,
        0x15, 0xC4, 0x89, 0x00, 0x00, 0x00, 0x0A, 0x49, 0x44, 0x41, 0x54, 0x78, 0x9C, 0x63, 0x00,
        0x01, 0x00, 0x00, 0x05, 0x00, 0x01, 0x0D, 0x0A, 0x2D, 0xB4, 0x00, 0x00, 0x00, 0x00, 0x49,
        0x45, 0x4E, 0x44, 0xAE, 0x42, 0x60, 0x82,
    ];
    let combo_path = tmp.path().join("combo.fvoci");
    write_zip_entries(
        &combo_path,
        &[
            ("version.json", br#"{"version":1,"date":1}"#),
            ("b1/container.jsonl", &b1_lines),
            ("b2/container.jsonl", &b2_lines),
            ("b2/old.png", png_1x1),
        ],
    );
    let combo = ts
        .app
        .import_external_archive(
            &combo_path,
            ImportFormat::Fvoci,
            TEST_WORKSPACE_ID,
            &importer_id,
        )
        .await
        .expect("empty board ID + duplicate block ID + cross-board ref succeeds");
    assert_eq!(combo.imported.boards, 2);
    assert_eq!(combo.imported.files, 1);
    let target_blocks = ts
        .app
        .blocks_for_container(&combo.container_ids[0])
        .await
        .expect("target blocks");
    let new_target_id = &target_blocks[0].id;
    let src_blocks = ts
        .app
        .blocks_for_container(&combo.container_ids[1])
        .await
        .expect("src blocks");
    let att = src_blocks
        .iter()
        .find(|b| b.block_type == BlockType::Attachment)
        .expect("attachment");
    assert_ne!(
        att.fields.get("fileId").and_then(|v| v.as_str()),
        Some("old.png"),
        "attachment fileId pointed to newly stored file"
    );
    let src_card = src_blocks
        .iter()
        .find(|b| b.block_type == BlockType::Card)
        .expect("src card");
    let refs = src_card.body.as_ref().expect("card body").reference_ids();
    assert_eq!(
        refs,
        vec![new_target_id.as_str()],
        "cross-board ref pointed"
    );
    ts.teardown().await;
}

/// Verifies `Store::create_containers` of streamed blocks rolls back the entire DB
/// transaction when the producer channel closes without sending `StreamBatch::Done`.
#[tokio::test]
async fn streaming_create_rolls_back_when_done_marker_is_missing() {
    let ts = Setup::native().start().await.init_basic().await;
    let importer_id = ts.client.me().await.id;
    let (tx, rx) = tokio::sync::mpsc::channel::<StreamBatch<&'static str>>(2);
    let cid = new_id();
    tx.send(StreamBatch::Batch(vec![Block {
        id: new_id(),
        container_id: cid.clone(),
        block_type: BlockType::Card,
        title: "Partial".to_owned(),
        ..Block::default()
    }]))
    .await
    .expect("send batch");
    drop(tx); // simulate producer panic: no StreamBatch::Done sent
    let mut containers = vec![Container {
        id: cid.clone(),
        workspace_id: TEST_WORKSPACE_ID.to_owned(),
        title: "Must Rollback".to_owned(),
        ..Container::default()
    }];
    let stream_outcome = ts
        .app
        .store()
        .create_containers(
            NewContainers::streamed(&mut containers, rx)
                .body_files(NewBodyFiles::Unchecked)
                .creator(Creator::Nobody),
            &importer_id,
        )
        .await;
    assert!(
        matches!(stream_outcome, Err(StreamCreateError::Incomplete)),
        "expected Incomplete error, got {stream_outcome:?}"
    );
    assert!(
        ts.app.store().get_container(&cid).await.is_err(),
        "incomplete stream must roll back container creation"
    );

    ts.teardown().await;
}
