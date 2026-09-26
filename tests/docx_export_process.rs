//! DOCX export through the `--internal-markdown` child (`tiptap-to-docx`) as
//! the server runs it: real process and limits, the oracle corpus, and the
//! error paths the export routes map (413 / 400 / 500).

use std::path::PathBuf;
use std::time::Duration;

use fvoci_server::documents::markdown_helper::{MarkdownError, MarkdownHelper, MarkdownLimits};
use fvoci_server::documents::office::{extract_office, OfficeKind, OfficeMode, OfficeOutcome};
use serde_json::{json, Value};

fn helper() -> MarkdownHelper {
    MarkdownHelper::new(env!("CARGO_BIN_EXE_fvoci-server"))
}

fn fixtures() -> Vec<(String, Value)> {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("compat/fixtures");
    let mut out = Vec::new();
    for dir in ["markdown-oracle", "export-docx"] {
        for entry in std::fs::read_dir(root.join(dir)).unwrap() {
            let path = entry.unwrap().path();
            if path.extension().and_then(|e| e.to_str()) != Some("json") {
                continue;
            }
            let name = path.file_stem().unwrap().to_str().unwrap().to_string();
            let doc = serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
            out.push((name, doc));
        }
    }
    out.sort_by(|a, b| a.0.cmp(&b.0));
    assert_eq!(
        out.len(),
        65,
        "53 markdown-oracle + 12 export-docx fixtures"
    );
    out
}

fn markdown_of(bytes: &[u8]) -> String {
    match extract_office(bytes, OfficeKind::Docx, OfficeMode::Markdown, 4 << 20) {
        OfficeOutcome::Ok { text, .. } => text,
        other => panic!("office extractor: {other:?}"),
    }
}

/// Every fixture becomes a DOCX that two independent readers open: docx-rs's
/// reader and the product's own OOXML importer (a separate quick-xml parser),
/// which must see the title as a heading.
#[tokio::test]
async fn corpus_through_the_child_opens_in_two_readers() {
    let helper = helper();
    for (name, doc) in fixtures() {
        let bytes = helper.tiptap_to_docx("문서 제목", &doc).await.unwrap();
        assert!(bytes.len() < 20_000_000, "{name}");
        docx_rs::read_docx(&bytes).unwrap_or_else(|e| panic!("{name}: {e}"));
        let md = markdown_of(&bytes);
        assert!(md.starts_with("# 문서 제목"), "{name}: {md}");
    }
}

#[tokio::test]
async fn structure_survives_a_reader_round_trip() {
    let doc: Value = serde_json::from_str(
        &std::fs::read_to_string(
            PathBuf::from(env!("CARGO_MANIFEST_DIR"))
                .join("compat/fixtures/export-docx/h02-nested-lists.json"),
        )
        .unwrap(),
    )
    .unwrap();
    let bytes = helper().tiptap_to_docx("", &doc).await.unwrap();
    let md = markdown_of(&bytes);
    for want in ["하나", "1-1-a", "deep", "restart"] {
        assert!(md.contains(want), "{want}: {md}");
    }
    let table: Value = serde_json::from_str(
        &std::fs::read_to_string(
            PathBuf::from(env!("CARGO_MANIFEST_DIR"))
                .join("compat/fixtures/export-docx/h04-table.json"),
        )
        .unwrap(),
    )
    .unwrap();
    let md = markdown_of(&helper().tiptap_to_docx("", &table).await.unwrap());
    assert!(md.contains("| 이름 | 값 | 비고 |"), "{md}");
    assert!(md.contains("extra"), "{md}");
}

/// Source `invalid_input` for a body that is not a Tiptap doc: refused
/// before any child starts (the program does not even exist here).
#[tokio::test]
async fn non_doc_body_is_invalid_without_a_child() {
    let missing = MarkdownHelper::new("/nonexistent/fvoci-server");
    for body in [
        json!(null),
        json!([]),
        json!({"type": "paragraph"}),
        json!({"type": "doc", "content": {}}),
    ] {
        let err = missing.tiptap_to_docx("t", &body).await.unwrap_err();
        assert!(
            matches!(err, MarkdownError::InvalidInput(_)),
            "{body}: {err:?}"
        );
    }
    let err = missing
        .tiptap_to_docx("t", &json!({"type": "doc"}))
        .await
        .unwrap_err();
    assert!(matches!(err, MarkdownError::Failed(_)), "{err:?}");
}

fn big_doc(paragraphs: usize) -> Value {
    let text = "한글 문단과 English text 가 섞인 긴 본문입니다. ".repeat(20);
    json!({"type": "doc", "content": (0..paragraphs)
        .map(|i| json!({"type": "paragraph", "content": [
            {"type": "text", "text": format!("{i} {text}"), "marks": [{"type": "bold"}]}]}))
        .collect::<Vec<_>>()})
}

/// Output over the cap (the 20 MB `maxOutputBytes`, lowered here) is
/// `TooLarge` (routes: 413), as is input over the child's input cap.
#[tokio::test]
async fn output_and_input_caps_are_too_large() {
    let doc = big_doc(200);
    let limits = MarkdownLimits {
        max_output_bytes: 4096,
        ..MarkdownLimits::default()
    };
    let err = helper()
        .with_limits(limits)
        .tiptap_to_docx("t", &doc)
        .await
        .unwrap_err();
    assert!(matches!(err, MarkdownError::TooLarge), "{err:?}");
    let limits = MarkdownLimits {
        max_input_bytes: 1024,
        ..MarkdownLimits::default()
    };
    let err = helper()
        .with_limits(limits)
        .tiptap_to_docx("t", &doc)
        .await
        .unwrap_err();
    assert!(matches!(err, MarkdownError::TooLarge), "{err:?}");
}

/// A child killed by the watchdog is a helper failure (routes: 500, as the
/// Node export's timeout), not an input error.
#[tokio::test]
async fn watchdog_kill_is_a_failure() {
    let limits = MarkdownLimits {
        timeout: Duration::from_millis(1),
        ..MarkdownLimits::default()
    };
    let err = helper()
        .with_limits(limits)
        .tiptap_to_docx("t", &big_doc(2000))
        .await
        .unwrap_err();
    assert!(
        matches!(&err, MarkdownError::Failed(m) if m.contains("watchdog")),
        "{err:?}"
    );
}
