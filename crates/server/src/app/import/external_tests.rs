//! Unit tests for external import security defenses, Markdown conversion, and Notion CSV inference.

use std::io::{Cursor, Write as _};
use std::path::{Path, PathBuf};

use fvoci_model::{BODY_MAX_NODES, Block, BlockType, new_id};
use serde_json::Value;
use zip::write::SimpleFileOptions;

use super::bundle::{
    PathCheck, SafeBundle, check_json_safety, resolve_relative_bundle_path, validate_bundle_path,
};
use super::markdown::{
    AssetResolver, BodyContext, ExternalOnlyResolver, ResolvedTarget, blocks_to_body,
    markdown_to_blocks,
};
use super::notion_csv::{InferredKind, parse_notion_csv};
use super::summary::{BundleLimits, ImportFormat, ImportWarningCode};
use crate::error::ErrorKind;

struct TestDir(PathBuf);

impl TestDir {
    fn new() -> Self {
        let path = std::env::temp_dir().join(format!("fvoci-import-unit-{}", new_id()));
        std::fs::create_dir_all(&path).expect("create unit test temp dir");
        Self(path)
    }

    fn path(&self) -> &Path {
        &self.0
    }
}

impl Drop for TestDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn write_zip_entries(path: &Path, entries: &[(&str, &[u8], SimpleFileOptions)]) {
    let mut writer = zip::ZipWriter::new(Cursor::new(Vec::new()));
    for (name, bytes, opts) in entries {
        writer.start_file(*name, *opts).expect("start zip entry");
        writer.write_all(bytes).expect("write zip entry");
    }
    let data = writer.finish().expect("finish zip").into_inner();
    std::fs::write(path, data).expect("write zip to disk");
}

#[test]
fn import_format_round_trips_strings() {
    for (wire, expected) in [
        ("fvoci", ImportFormat::Fvoci),
        ("trello", ImportFormat::Trello),
        ("notion", ImportFormat::Notion),
    ] {
        let parsed: ImportFormat = wire.parse().expect("known format parses");
        assert_eq!(parsed, expected, "parsed format matches {wire}");
        assert_eq!(parsed.as_str(), wire, "as_str matches {wire}");
        assert_eq!(parsed.to_string(), wire, "Display matches {wire}");
    }
    assert!(
        "jira".parse::<ImportFormat>().is_err(),
        "unknown format is rejected"
    );
}

#[test]
fn bundle_path_validation_rejects_traversal_and_special_paths() {
    let limits = BundleLimits {
        max_path_depth: 4,
        max_path_bytes: 64,
        max_entry_name_chars: 16,
        ..BundleLimits::default()
    };

    let rejected = [
        "",
        "../etc/passwd",
        "a/../../b",
        "/abs/path.md",
        "\\windows\\path.md",
        "dir\\file.md",
        "C:evil.md",
        "C:/evil.md",
        "dir/D:evil.md",
        "bad\0name.md",
        "bad\nname.md",
        "dir//file.md",
        "a/b/c/d/e.md",
        "this-segment-is-way-too-long.md",
        "aaaaaaaaaaaa/bbbbbbbbbbbb/cccccccccccc/dddddddddddd/eeeeeeeeeeee",
    ];
    for raw in rejected {
        let err = validate_bundle_path(raw, &limits).expect_err("path must be rejected");
        assert_eq!(
            err.kind(),
            ErrorKind::BadRequest,
            "path {raw:?} rejected as 400"
        );
    }

    assert_eq!(
        validate_bundle_path("docs/sub/page.md", &limits).expect("valid nested path"),
        PathCheck::File("docs/sub/page.md".to_owned()),
        "normal relative path accepted"
    );
    assert_eq!(
        validate_bundle_path("./docs/./page.md", &limits).expect("dot segments stripped"),
        PathCheck::File("docs/page.md".to_owned()),
        "dot segments normalized"
    );
    assert_eq!(
        validate_bundle_path("docs/", &limits).expect("directory accepted"),
        PathCheck::Directory,
        "directory entry classified"
    );
    assert_eq!(
        validate_bundle_path("__MACOSX/._page.md", &limits).expect("macos metadata"),
        PathCheck::MetadataIgnored,
        "__MACOSX ignored"
    );
    assert_eq!(
        validate_bundle_path("docs/.DS_Store", &limits).expect("ds_store metadata"),
        PathCheck::MetadataIgnored,
        ".DS_Store ignored"
    );
}

#[test]
fn relative_bundle_path_blocks_escapes_and_schemes() {
    assert_eq!(
        resolve_relative_bundle_path("", "../outside.png"),
        None,
        "parent escape from bundle root must not resolve"
    );
    for bad in [
        "",
        "../../outside.png",
        "a/../../../outside.png",
        "/etc/passwd",
        "https://example.com/pic.png",
        "file:///etc/passwd",
        "javascript:alert(1)",
        "C:/secret.txt",
        "sub%2F..%2F..%2F..%2Fsecret.txt",
        "evil%00.png",
        "broken%ZZ.png",
        "a\\b.png",
    ] {
        assert_eq!(
            resolve_relative_bundle_path("docs", bad),
            None,
            "target {bad:?} must not resolve"
        );
    }
    assert_eq!(
        resolve_relative_bundle_path("docs/sub", "../img/pic%201.png#top?v=2"),
        Some("docs/img/pic 1.png".to_owned()),
        "valid relative path with percent-encoding and fragment resolves"
    );
}

#[test]
fn json_safety_rejects_nul_escaped_nul_and_deep_nesting() {
    let nul_err = check_json_safety(b"{\"a\":\"\0\"}", 64).expect_err("NUL rejected");
    assert_eq!(nul_err.kind(), ErrorKind::BadRequest, "NUL is 400");

    let esc_nul = check_json_safety(br#"{"a":"hello\u0000world"}"#, 64)
        .expect_err("JSON \\u0000 escape rejected");
    assert_eq!(
        esc_nul.kind(),
        ErrorKind::BadRequest,
        "escaped \\u0000 is 400"
    );

    let literal_backslash = check_json_safety(br#"{"a":"literal \\u0000 text"}"#, 64);
    assert!(
        literal_backslash.is_ok(),
        "escaped backslash followed by u0000 is ordinary text"
    );

    let deep = format!("{}{}", "[".repeat(65), "]".repeat(65));
    let depth_err = check_json_safety(deep.as_bytes(), 64).expect_err("depth 65 rejected");
    assert_eq!(depth_err.kind(), ErrorKind::BadRequest, "deep JSON is 400");

    let inside_string = format!("{{\"text\":\"{}\"}}", "{[[[[[[[[[[".repeat(20));
    assert!(
        check_json_safety(inside_string.as_bytes(), 8).is_ok(),
        "braces inside quoted JSON strings do not count toward nesting"
    );
}

/// Builds a single-entry ZIP whose central-directory Unix mode is patched to `unix_mode`.
fn zip_with_unix_mode(name: &str, bytes: &[u8], unix_mode: u32) -> Vec<u8> {
    let opts = SimpleFileOptions::default().unix_permissions(0o644);
    let mut writer = zip::ZipWriter::new(Cursor::new(Vec::new()));
    writer.start_file(name, opts).expect("start entry");
    writer.write_all(bytes).expect("write entry");
    let mut raw = writer.finish().expect("finish zip").into_inner();
    // Locate the central directory file header (`PK\x01\x02`); external attributes sit at +38..+42.
    let pos = raw
        .windows(4)
        .position(|w| w == b"PK\x01\x02")
        .expect("central directory header present");
    let ext_attrs = (unix_mode << 16).to_le_bytes();
    raw[pos + 38..pos + 42].copy_from_slice(&ext_attrs);
    raw
}

#[tokio::test]
async fn safe_bundle_rejects_symlinks_and_special_unix_modes() {
    let dir = TestDir::new();
    let limits = BundleLimits::default();

    // Real ZIP symlink created with `ZipWriter::add_symlink`.
    let symlink_path = dir.path().join("symlink.zip");
    {
        let mut writer = zip::ZipWriter::new(Cursor::new(Vec::new()));
        writer
            .add_symlink("note.md", "/etc/passwd", SimpleFileOptions::default())
            .expect("add symlink");
        std::fs::write(&symlink_path, writer.finish().expect("finish").into_inner())
            .expect("write symlink zip");
    }
    let err = SafeBundle::open(&symlink_path, dir.path(), limits)
        .await
        .expect_err("symlink entry must fail");
    assert_eq!(err.kind(), ErrorKind::BadRequest, "symlink rejected as 400");

    for (label, mode) in [("fifo", 0o010_644), ("char_device", 0o020_600)] {
        let zip_path = dir.path().join(format!("{label}.zip"));
        std::fs::write(&zip_path, zip_with_unix_mode("note.md", b"hello", mode))
            .expect("write patched zip");
        let err = SafeBundle::open(&zip_path, dir.path(), limits)
            .await
            .expect_err("special mode must fail");
        assert_eq!(
            err.kind(),
            ErrorKind::BadRequest,
            "{label} rejected with 400"
        );
    }
}

#[tokio::test]
async fn safe_bundle_rejects_zip_bombs_and_size_overflow() {
    let dir = TestDir::new();
    let deflated =
        SimpleFileOptions::default().compression_method(zip::CompressionMethod::Deflated);
    let stored = SimpleFileOptions::default().compression_method(zip::CompressionMethod::Stored);

    // 1. High compression ratio bomb: 2 MiB of zeros compresses to ~2 KiB (> 100:1).
    let bomb_path = dir.path().join("bomb.zip");
    let zeros = vec![0_u8; 2 * 1024 * 1024];
    write_zip_entries(&bomb_path, &[("bomb.md", &zeros, deflated)]);
    let err = SafeBundle::open(&bomb_path, dir.path(), BundleLimits::default())
        .await
        .expect_err("zip bomb must be rejected");
    assert_eq!(
        err.kind(),
        ErrorKind::PayloadTooLarge,
        "compression ratio violation is 413"
    );

    // 2. Total unpacked bytes ceiling across multiple stored entries.
    let total_path = dir.path().join("total.zip");
    let chunk = vec![b'a'; 600];
    write_zip_entries(
        &total_path,
        &[("a.md", &chunk, stored), ("b.md", &chunk, stored)],
    );
    let tight_total = BundleLimits {
        max_total_unpacked_bytes: 1_000,
        ..BundleLimits::default()
    };
    let err = SafeBundle::open(&total_path, dir.path(), tight_total)
        .await
        .expect_err("total bytes overflow must fail");
    assert_eq!(
        err.kind(),
        ErrorKind::PayloadTooLarge,
        "total unpacked overflow is 413"
    );

    // 3. Entry count ceiling.
    let count_path = dir.path().join("count.zip");
    write_zip_entries(
        &count_path,
        &[
            ("1.md", b"a", stored),
            ("2.md", b"b", stored),
            ("3.md", b"c", stored),
        ],
    );
    let tight_count = BundleLimits {
        max_entries: 2,
        ..BundleLimits::default()
    };
    let err = SafeBundle::open(&count_path, dir.path(), tight_count)
        .await
        .expect_err("entry count overflow must fail");
    assert_eq!(
        err.kind(),
        ErrorKind::PayloadTooLarge,
        "entry count overflow is 413"
    );
}

#[tokio::test]
async fn safe_bundle_enforces_nested_zip_depth_and_rejects_nul_text() {
    let dir = TestDir::new();
    let stored = SimpleFileOptions::default().compression_method(zip::CompressionMethod::Stored);

    let make_nested_zip = |layers: usize| -> Vec<u8> {
        let mut inner = Cursor::new(Vec::new());
        {
            let mut w = zip::ZipWriter::new(&mut inner);
            w.start_file("page.md", stored).expect("start inner");
            w.write_all(b"# Hello").expect("write inner");
            w.finish().expect("finish inner");
        }
        let mut current = inner.into_inner();
        for layer in 0..layers {
            let mut outer = Cursor::new(Vec::new());
            {
                let mut w = zip::ZipWriter::new(&mut outer);
                w.start_file(format!("layer-{layer}.zip"), stored)
                    .expect("start outer");
                w.write_all(&current).expect("write outer");
                w.finish().expect("finish outer");
            }
            current = outer.into_inner();
        }
        current
    };

    let depth2 = dir.path().join("depth2.zip");
    std::fs::write(&depth2, make_nested_zip(2)).expect("write depth2");
    let ok = SafeBundle::open(&depth2, dir.path(), BundleLimits::default())
        .await
        .expect("nested depth 2 allowed");
    assert_eq!(ok.entries().len(), 1, "inner page discovered at depth 2");
    assert_eq!(
        ok.read_text(&ok.entries()[0], 1024)
            .await
            .expect("read text"),
        "# Hello",
        "inner text read cleanly"
    );

    let depth3 = dir.path().join("depth3.zip");
    std::fs::write(&depth3, make_nested_zip(3)).expect("write depth3");
    let err = SafeBundle::open(&depth3, dir.path(), BundleLimits::default())
        .await
        .expect_err("nested depth 3 must fail");
    assert_eq!(
        err.kind(),
        ErrorKind::BadRequest,
        "excessive nested ZIP depth is 400"
    );

    let nul_zip = dir.path().join("nul.zip");
    write_zip_entries(&nul_zip, &[("bad.md", b"hello\0world", stored)]);
    let bundle = SafeBundle::open(&nul_zip, dir.path(), BundleLimits::default())
        .await
        .expect("open zip with nul text");
    let err = bundle
        .read_text(&bundle.entries()[0], 1024)
        .await
        .expect_err("NUL text rejected");
    assert_eq!(err.kind(), ErrorKind::BadRequest, "NUL text is 400");
}

struct MockResolver {
    ref_id: String,
}

impl AssetResolver for MockResolver {
    fn resolve_image(&mut self, dest: &str) -> ResolvedTarget {
        if dest == "local.png" {
            ResolvedTarget::LocalImage {
                file_id: "f0".to_owned(),
            }
        } else if dest.starts_with("http") {
            ResolvedTarget::ExternalUrl
        } else {
            ResolvedTarget::MissingLocal
        }
    }

    fn resolve_link(&mut self, dest: &str) -> ResolvedTarget {
        match dest {
            "other.md" => ResolvedTarget::InternalReference {
                block_id: self.ref_id.clone(),
            },
            "doc.pdf" => ResolvedTarget::LocalFile {
                file_id: "f1".to_owned(),
            },
            other if other.contains(':') => ResolvedTarget::ExternalUrl,
            _ => ResolvedTarget::MissingLocal,
        }
    }
}

#[test]
fn markdown_converter_produces_valid_b1_tables_tasks_marks_and_respects_context() {
    let ref_id = new_id();
    let mut resolver = MockResolver {
        ref_id: ref_id.clone(),
    };
    let md = "\
# Heading 1

#### Heading 4 clamped

**bold** _italic_ ~~strike~~ `code` [ok](https://example.com) [bad](javascript:alert(1)) [page](other.md)

- [x] Done
- [ ] Todo

| Name | Score |
| :--- | ---: |
| Alpha | 10 |
| Beta | 20 |

> | Nested | Table |
> | --- | --- |
> | A | B |

![Local](local.png)

![Remote](https://example.com/remote.png)

[Download](doc.pdf)
";
    let converted = markdown_to_blocks(md, BodyContext::Document, 50, &mut resolver)
        .expect("markdown converts");
    let body = blocks_to_body(converted.nodes)
        .expect("body validates against allowlist")
        .expect("non-empty body");
    let value = body.document();
    let content = value["content"].as_array().expect("doc content array");

    assert!(
        content.iter().any(|node| node["type"] == "table"),
        "top-level GFM table emitted as B1 table node"
    );
    assert!(
        content.iter().any(|node| node["type"] == "taskList"),
        "task list emitted as taskList"
    );
    assert!(
        content
            .iter()
            .any(|node| node["type"] == "image" && node["attrs"]["fileId"] == "f0"),
        "local image emitted as image block"
    );
    assert!(
        content
            .iter()
            .any(|node| node["type"] == "file" && node["attrs"]["fileId"] == "f1"),
        "local file emitted as file block in document context"
    );
    let serialized = serde_json::to_string(value).expect("serialize body");
    assert!(
        !serialized.contains("javascript:"),
        "disallowed javascript scheme dropped"
    );
    assert!(
        serialized.contains(&ref_id),
        "internal page link emitted as reference node"
    );
    assert!(
        converted
            .notices
            .iter()
            .any(|notice| notice.code == ImportWarningCode::ExternalImageLinked),
        "external image produced ExternalImageLinked notice"
    );

    // Comment context must not emit `file` blocks so Block::validate succeeds.
    let comment_md = "See [spec](doc.pdf) and ![remote](https://example.com/img.png)";
    let comment_blocks = markdown_to_blocks(comment_md, BodyContext::Comment, 50, &mut resolver)
        .expect("comment markdown converts");
    let comment_body = blocks_to_body(comment_blocks.nodes)
        .expect("comment body validates")
        .expect("non-empty comment body");
    let comment = Block {
        id: new_id(),
        container_id: new_id(),
        parent_id: new_id(),
        block_type: BlockType::Comment,
        body: Some(comment_body),
        ..Block::default()
    };
    comment
        .validate()
        .expect("comment block with local file link passes Block::validate");
}

#[test]
fn markdown_converter_enforces_depth_node_and_nul_limits() {
    let mut resolver = ExternalOnlyResolver;

    let nul = markdown_to_blocks("a\0b", BodyContext::Document, 50, &mut resolver)
        .expect_err("NUL rejected");
    assert_eq!(nul.kind(), ErrorKind::BadRequest, "NUL markdown is 400");

    let deep = format!("{}deep", "> ".repeat(55));
    let depth_err = markdown_to_blocks(&deep, BodyContext::Document, 50, &mut resolver)
        .expect_err("deep markdown rejected");
    assert_eq!(
        depth_err.kind(),
        ErrorKind::BadRequest,
        "deep markdown is 400"
    );

    let many_lines = "- item\n".repeat(BODY_MAX_NODES / 2 + 500);
    let nodes_err = markdown_to_blocks(&many_lines, BodyContext::Document, 50, &mut resolver)
        .expect_err("excessive node count rejected");
    assert_eq!(
        nodes_err.kind(),
        ErrorKind::BadRequest,
        "excessive node count is 400"
    );

    // Wide table (> 20 cols) gracefully degrades to paragraphs instead of failing allowlist.
    let wide_header = (0..22)
        .map(|i| format!("C{i}"))
        .collect::<Vec<_>>()
        .join(" | ");
    let wide_sep = (0..22).map(|_| "---").collect::<Vec<_>>().join(" | ");
    let wide_row = (0..22)
        .map(|i| format!("V{i}"))
        .collect::<Vec<_>>()
        .join(" | ");
    let wide_md = format!("| {wide_header} |\n| {wide_sep} |\n| {wide_row} |\n");
    let wide = markdown_to_blocks(&wide_md, BodyContext::Document, 50, &mut resolver)
        .expect("wide table degrades");
    let body = blocks_to_body(wide.nodes)
        .expect("degraded wide table validates")
        .expect("non-empty body");
    assert!(
        body.document()["content"]
            .as_array()
            .expect("content array")
            .iter()
            .all(|node| node["type"] != "table"),
        "table wider than 20 columns degrades to paragraphs"
    );
}

#[test]
fn notion_csv_infers_all_seven_property_types_and_enforces_limits() {
    let csv = "\u{FEFF}Name,Done,Estimate,Due,Website,Tags,Status,Notes\n\
Task A,Yes,\"$1,250\",2026-05-01 → 2026-05-10,https://example.com/a,\"Alpha, Beta\",In Progress,First detailed sentence.\n\
Task B,No,85%,\"May 14, 2026\",https://example.com/b,Beta,Done,Second detailed sentence.\n";
    let parsed = parse_notion_csv(csv, &BundleLimits::default()).expect("CSV parses");
    assert_eq!(parsed.card_properties.len(), 7, "7 non-title columns");
    let types: Vec<&str> = parsed
        .card_properties
        .iter()
        .map(|prop| prop.property_type.as_str())
        .collect();
    assert_eq!(
        types,
        vec![
            InferredKind::Checkbox.property_type().as_str(),
            InferredKind::Number.property_type().as_str(),
            InferredKind::Date.property_type().as_str(),
            InferredKind::Url.property_type().as_str(),
            InferredKind::MultiSelect.property_type().as_str(),
            InferredKind::Select.property_type().as_str(),
            InferredKind::Text.property_type().as_str(),
        ],
        "all 7 property types inferred in order"
    );
    assert_eq!(parsed.rows.len(), 2, "two rows parsed");
    let checkbox_id = parsed.card_properties[0].id.as_str();
    let number_id = parsed.card_properties[1].id.as_str();
    let date_id = parsed.card_properties[2].id.as_str();
    assert_eq!(
        parsed.rows[0].properties.get(checkbox_id),
        Some(&Value::String("true".to_owned())),
        "checked checkbox stored as string \"true\""
    );
    assert_eq!(
        parsed.rows[1].properties.get(checkbox_id),
        None,
        "unchecked checkbox omitted"
    );
    assert_eq!(
        parsed.rows[0].properties.get(number_id),
        Some(&Value::String("1250".to_owned())),
        "number stored as normalized string"
    );
    assert_eq!(
        parsed.rows[0].properties[date_id]["from"], "2026-05-01",
        "range start parsed"
    );
    assert_eq!(
        parsed.rows[0].properties[date_id]["to"], "2026-05-10",
        "range end parsed"
    );
    assert_eq!(
        parsed.rows[1].properties[date_id]["from"], "2026-05-14",
        "English month date parsed"
    );

    // Row and column limits.
    let tight_rows = BundleLimits {
        max_csv_rows: 1,
        ..BundleLimits::default()
    };
    let row_err = parse_notion_csv(csv, &tight_rows).expect_err("row limit enforced");
    assert_eq!(
        row_err.kind(),
        ErrorKind::PayloadTooLarge,
        "row limit is 413"
    );

    let tight_cols = BundleLimits {
        max_csv_columns: 3,
        ..BundleLimits::default()
    };
    let col_err = parse_notion_csv(csv, &tight_cols).expect_err("col limit enforced");
    assert_eq!(
        col_err.kind(),
        ErrorKind::PayloadTooLarge,
        "col limit is 413"
    );

    let nul_err = parse_notion_csv("Name,Val\nA,bad\0x\n", &BundleLimits::default())
        .expect_err("NUL CSV rejected");
    assert_eq!(nul_err.kind(), ErrorKind::BadRequest, "NUL CSV is 400");

    // Non-thousands comma values (`1,5`) must NOT be silently mangled into `15` as Number.
    let comma_csv = "Name,Ratio\nA,\"1,5\"\nB,\"2,5\"\n";
    let comma_parsed = parse_notion_csv(comma_csv, &BundleLimits::default()).expect("CSV parses");
    assert_ne!(
        comma_parsed.card_properties[0].property_type,
        fvoci_model::PropertyType::Number,
        "non-thousands comma values like 1,5 are not inferred as number"
    );
}

#[tokio::test]
async fn security_review_nested_zip_fd_bomb_and_multi_csv_row_bomb() {
    let dir = TestDir::new();
    let stored = SimpleFileOptions::default().compression_method(zip::CompressionMethod::Stored);

    // 1. Nested ZIP FD exhaustion defense: > max_nested_zips rejected with 413.
    let empty_inner = {
        let mut cur = Cursor::new(Vec::new());
        let w = zip::ZipWriter::new(&mut cur);
        w.finish().expect("finish empty inner");
        cur.into_inner()
    };
    let fd_bomb_path = dir.path().join("fd-bomb.zip");
    {
        let mut w = zip::ZipWriter::new(Cursor::new(Vec::new()));
        for i in 0..20 {
            w.start_file(format!("empty-{i}.zip"), stored)
                .expect("start nested");
            w.write_all(&empty_inner).expect("write nested");
        }
        std::fs::write(&fd_bomb_path, w.finish().expect("finish").into_inner())
            .expect("write fd bomb");
    }
    let err = SafeBundle::open(&fd_bomb_path, dir.path(), BundleLimits::default())
        .await
        .expect_err("too many nested ZIPs must be rejected");
    assert_eq!(
        err.kind(),
        ErrorKind::PayloadTooLarge,
        "nested ZIP count limit is 413"
    );

    // 2. Multi-CSV cumulative row bomb rejected early against max_entries.
    let multi_csv = dir.path().join("multi-csv.zip");
    let csv_body = b"Name,Val\nR1,1\nR2,2\nR3,3\nR4,4\n";
    write_zip_entries(
        &multi_csv,
        &[
            ("db1.csv", csv_body, stored),
            ("db2.csv", csv_body, stored),
            ("db3.csv", csv_body, stored),
        ],
    );
    let tight = BundleLimits {
        max_entries: 10,
        ..BundleLimits::default()
    };
    let bundle = SafeBundle::open(&multi_csv, dir.path(), tight)
        .await
        .expect("open multi-csv bundle");
    let multi_err =
        super::notion::convert_notion_bundle(&bundle, &tight, fvoci_model::PageLimits::default())
            .await
            .expect_err("multi-CSV cumulative rows past max_entries must fail");
    assert_eq!(
        multi_err.kind(),
        ErrorKind::PayloadTooLarge,
        "multi-CSV cumulative row overflow is 413"
    );
}

#[test]
fn security_review_trello_amplification_and_data_uri_degradation() {
    // 1. Trello checklist reference amplification blocked (each checklist attaches once) &
    //    inline card labels enforced against max_trello_labels.
    let shared_checklist = serde_json::json!({
        "name": "Checklist Test",
        "lists": [{"id": "l1", "name": "Todo"}],
        "cards": [
            {"id": "c1", "name": "Card 1", "idList": "l1", "idChecklists": ["chk1"]},
            {"id": "c2", "name": "Card 2", "idList": "l1", "idChecklists": ["chk1"]}
        ],
        "checklists": [
            {"id": "chk1", "name": "Shared", "idCard": "", "checkItems": [{"id": "i1", "name": "Task"}]}
        ]
    });
    let staged = super::trello::convert_trello_bytes(
        shared_checklist.to_string().as_bytes(),
        &BundleLimits::default(),
    )
    .expect("shared checklist converts safely");
    let cards_with_body = staged.containers[0]
        .blocks
        .iter()
        .filter(|b| b.block_type == BlockType::Card && b.body.is_some())
        .count();
    assert_eq!(
        cards_with_body, 1,
        "checklist attaches to at most one card (blocks Billion Laughs amplification)"
    );

    let many_inline_labels = serde_json::json!({
        "name": "Label Bomb",
        "lists": [{"id": "l1", "name": "Todo"}],
        "cards": [{
            "id": "c1",
            "name": "Card 1",
            "idList": "l1",
            "labels": [
                {"id": "a", "name": "A"},
                {"id": "b", "name": "B"},
                {"id": "c", "name": "C"}
            ]
        }]
    });
    let label_err = super::trello::convert_trello_bytes(
        many_inline_labels.to_string().as_bytes(),
        &BundleLimits {
            max_trello_labels: 2,
            ..BundleLimits::default()
        },
    )
    .expect_err("inline card labels past limit must fail");
    assert_eq!(
        label_err.kind(),
        ErrorKind::PayloadTooLarge,
        "inline label overflow is 413"
    );

    // 2. Disallowed `data:` image URI degrades to `[image]` with MarkdownDegraded (not huge URI text).
    let mut resolver = ExternalOnlyResolver;
    let huge_data = format!("![](data:image/png;base64,{})", "A".repeat(4096));
    let converted = markdown_to_blocks(&huge_data, BodyContext::Document, 50, &mut resolver)
        .expect("data URI image degrades cleanly");
    let body = blocks_to_body(converted.nodes)
        .expect("degraded data URI validates")
        .expect("non-empty body");
    let serialized = serde_json::to_string(body.document()).expect("serialize");
    assert!(
        serialized.contains("[image]") && !serialized.contains("data:image"),
        "disallowed data: URI replaced by [image] placeholder"
    );
    assert!(
        converted
            .notices
            .iter()
            .any(|n| n.code == ImportWarningCode::MarkdownDegraded)
            && converted
                .notices
                .iter()
                .all(|n| n.code != ImportWarningCode::ExternalImageLinked),
        "disallowed image scheme emits MarkdownDegraded, not ExternalImageLinked"
    );
}

#[test]
fn trello_importer_emits_native_view_field_keys() {
    let trello_json = serde_json::json!({
        "name": "Sprint Board",
        "lists": [{"id": "l1", "name": "Todo"}, {"id": "l2", "name": "Done"}],
        "cards": [
            {"id": "c1", "name": "Task 1", "idList": "l1", "due": "2026-06-01T00:00:00Z"}
        ]
    });
    let trello_staged = super::trello::convert_trello_bytes(
        trello_json.to_string().as_bytes(),
        &BundleLimits::default(),
    )
    .expect("trello export converts");
    let trello_views: Vec<&Block> = trello_staged.containers[0]
        .blocks
        .iter()
        .filter(|block| block.block_type == BlockType::View)
        .collect();
    assert_eq!(trello_views.len(), 1, "one board view emitted for Trello");
    assert_eq!(
        trello_views[0].fields.len(),
        13,
        "exact native Trello view key set"
    );
    for key in [
        "groupBy",
        "visibleGroups",
        "hiddenGroups",
        "collapsedGroups",
        "shownProperties",
        "sort",
        "groupCalculations",
    ] {
        assert!(
            trello_views[0].fields.contains_key(key),
            "Trello view fields must contain native key {key}"
        );
    }
}

#[tokio::test]
async fn notion_importer_emits_native_view_field_keys() {
    let dir = TestDir::new();
    let stored = SimpleFileOptions::default().compression_method(zip::CompressionMethod::Stored);
    let notion_zip = dir.path().join("notion.zip");
    write_zip_entries(
        &notion_zip,
        &[
            (
                "Sprint.csv",
                b"Name,Status\nA,Todo\nB,Todo\nC,Done\n",
                stored,
            ),
            (
                "Notes.csv",
                b"Name,Summary\nR1,First long sentence.\nR2,Second long sentence.\n",
                stored,
            ),
        ],
    );
    let bundle = SafeBundle::open(&notion_zip, dir.path(), BundleLimits::default())
        .await
        .expect("open notion zip");
    let notion_staged = super::notion::convert_notion_bundle(
        &bundle,
        &BundleLimits::default(),
        fvoci_model::PageLimits::default(),
    )
    .await
    .expect("notion bundle converts");
    let notion_views: Vec<&Block> = notion_staged
        .containers
        .iter()
        .flat_map(|c| c.blocks.iter())
        .filter(|block| block.block_type == BlockType::View)
        .collect();
    assert_eq!(
        notion_views.len(),
        2,
        "board + table views emitted across Notion CSVs"
    );
    assert!(
        notion_views
            .iter()
            .any(|v| v.fields.contains_key("groupBy")),
        "select CSV emits board view with groupBy"
    );
    for view in notion_views {
        let expected_len = if view.title == "Board" { 13 } else { 12 };
        assert_eq!(
            view.fields.len(),
            expected_len,
            "exact native Notion {} view key set",
            view.title
        );
        for key in [
            "visibleGroups",
            "hiddenGroups",
            "collapsedGroups",
            "shownProperties",
            "sort",
            "groupCalculations",
        ] {
            assert!(
                view.fields.contains_key(key),
                "Notion view {} must contain native key {key}",
                view.title
            );
        }
    }
}

#[test]
fn trello_streaming_bounds_and_body_budget() {
    let trello_actions = serde_json::json!({
        "name": "Bounded", "lists": [{"id": "l1", "name": "Todo"}],
        "cards": [{"id": "c1", "name": "One", "idList": "l1"}],
        "actions": [
            {"type": "updateCard", "data": {"card": {"id": "c1"}, "text": "ignored"}},
            {"type": "commentCard", "idMemberCreator": "m1", "data": {"card": {"id": "c1"}, "text": "kept"}}
        ]
    });
    let limits = BundleLimits {
        max_trello_comments: 1,
        ..BundleLimits::default()
    };
    let staged =
        super::trello::convert_trello_bytes(trello_actions.to_string().as_bytes(), &limits)
            .expect("non-comment actions skipped");
    assert_eq!(staged.containers[0].blocks.len(), 3, "3 blocks");

    let overflow = serde_json::json!({
        "name": "Overflow", "lists": [{"id": "l1", "name": "Todo"}],
        "cards": [{"id": "c1", "name": "A", "idList": "l1"}, {"id": "c2", "name": "B", "idList": "l1"}]
    });
    let card_limits = BundleLimits {
        max_trello_cards: 1,
        ..BundleLimits::default()
    };
    let err = super::trello::convert_trello_bytes(overflow.to_string().as_bytes(), &card_limits)
        .expect_err("N+1st Trello card rejected");
    assert_eq!(err.kind(), ErrorKind::PayloadTooLarge, "card cap 413");

    let mut resolver = ExternalOnlyResolver;
    let nodes = markdown_to_blocks("a\n\nb\n\nc", BodyContext::Document, 50, &mut resolver)
        .expect("markdown")
        .nodes;
    let body = blocks_to_body(nodes).expect("valid").expect("body");
    let mut budget = super::markdown_nodes::BodyBudget::new(2);
    let cap_err = budget.charge(Some(&body)).expect_err("cap");
    assert_eq!(cap_err.kind(), ErrorKind::PayloadTooLarge, "budget 413");
}

#[tokio::test]
async fn notion_synthetic_ancestors_and_page_limits() {
    let dir = TestDir::new();
    let stored = SimpleFileOptions::default().compression_method(zip::CompressionMethod::Stored);
    let deep_zip = dir.path().join("deep.zip");
    write_zip_entries(
        &deep_zip,
        &[(
            "a/b/c/Leaf 11111111111111111111111111111111.md",
            b"# Leaf\n\nhello",
            stored,
        )],
    );
    let tight = BundleLimits {
        max_entries: 3,
        ..BundleLimits::default()
    };
    let bundle = SafeBundle::open(&deep_zip, dir.path(), tight)
        .await
        .expect("open");
    let err =
        super::notion::convert_notion_bundle(&bundle, &tight, fvoci_model::PageLimits::default())
            .await
            .expect_err("ancestors past max_entries");
    assert_eq!(err.kind(), ErrorKind::PayloadTooLarge, "ancestors 413");

    let wide = SafeBundle::open(&deep_zip, dir.path(), BundleLimits::default())
        .await
        .expect("open");
    let lim = fvoci_model::PageLimits {
        pages: 2,
        depth: 10,
    };
    let page_err = super::notion::convert_notion_bundle(&wide, &BundleLimits::default(), lim)
        .await
        .expect_err("page count checked before markdown");
    assert_eq!(page_err.kind(), ErrorKind::BadRequest, "page limit 400");
}
