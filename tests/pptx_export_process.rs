//! PPTX and Markdown exports through the `--internal-markdown` child
//! (`tiptap-to-pptx`, `tiptap-to-md-export`) as the server runs them: real
//! process and limits, the oracle corpus read back by the product's own
//! OOXML importer and a strict XML parse of every part, the Markdown file
//! against the TS `tiptapDocToMd` oracle, and the error paths the export
//! routes map (413 / 400 / 500).

use std::io::Read;
use std::path::PathBuf;
use std::time::Duration;

use fvoci_server::documents::export::ExportRenderError;
use fvoci_server::documents::markdown_helper::{MarkdownHelper, MarkdownLimits};
use fvoci_server::documents::office::{extract_office, OfficeKind, OfficeMode, OfficeOutcome};
use serde_json::{json, Value};

fn helper() -> MarkdownHelper {
    MarkdownHelper::new(env!("CARGO_BIN_EXE_fvoci-server"))
}

fn root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("compat/fixtures")
}

fn fixtures() -> Vec<(String, Value)> {
    let mut out = Vec::new();
    for dir in ["markdown-oracle", "export-docx"] {
        for entry in std::fs::read_dir(root().join(dir)).unwrap() {
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
    match extract_office(bytes, OfficeKind::Pptx, OfficeMode::Markdown, 4 << 20) {
        OfficeOutcome::Ok { text, .. } => text,
        other => panic!("office extractor: {other:?}"),
    }
}

/// Every `.xml`/`.rels` part parses (quick-xml, strict end tags); returns the
/// slide count from the part names.
fn check_parts(name: &str, bytes: &[u8]) -> usize {
    let mut zip = zip::ZipArchive::new(std::io::Cursor::new(bytes)).unwrap();
    let mut slides = 0;
    for i in 0..zip.len() {
        let mut part = zip.by_index(i).unwrap();
        let part_name = part.name().to_string();
        assert!(!part_name.ends_with('/'), "{name}: directory entry");
        if part_name.starts_with("ppt/slides/slide") {
            slides += 1;
        }
        let mut xml = String::new();
        part.read_to_string(&mut xml).unwrap();
        let mut reader = quick_xml::Reader::from_str(&xml);
        reader.config_mut().check_end_names = true;
        loop {
            match reader.read_event() {
                Ok(quick_xml::events::Event::Eof) => break,
                Ok(_) => {}
                Err(e) => panic!("{name} {part_name}: {e}"),
            }
        }
    }
    slides
}

/// Every fixture becomes a PPTX whose parts are well-formed and that the
/// product importer (a separate quick-xml walker over the slide id list)
/// reads back with the title first; tables survive as tables.
#[tokio::test]
async fn corpus_through_the_child_is_well_formed_and_reads_back() {
    let helper = helper();
    for (name, doc) in fixtures() {
        let bytes = helper.tiptap_to_pptx("문서 제목", &doc).await.unwrap();
        assert!(bytes.len() < 20_000_000, "{name}");
        assert!(check_parts(&name, &bytes) >= 1, "{name}");
        let md = markdown_of(&bytes);
        // The title slide's first box: the bold 28 pt title.
        assert!(md.starts_with("**문서 제목**"), "{name}: {md}");
        match name.as_str() {
            "02-korean-emoji" => {
                for want in ["안녕하세요, 세계!", "😀", "🇰🇷", "홍길동"] {
                    assert!(md.contains(want), "{want}: {md}");
                }
            }
            "h04-table" => assert!(md.contains("| 이름 | 값 | 비고 |"), "{md}"),
            _ => {}
        }
    }
}

/// A top-level horizontal rule opens a slide (source), nested ones do not.
#[tokio::test]
async fn rules_split_slides() {
    let doc = json!({"type": "doc", "content": [
        {"type": "paragraph", "content": [{"type": "text", "text": "하나"}]},
        {"type": "horizontalRule"},
        {"type": "blockquote", "content": [{"type": "horizontalRule"},
            {"type": "paragraph", "content": [{"type": "text", "text": "둘"}]}]},
        {"type": "horizontalRule"},
        {"type": "paragraph", "content": [{"type": "text", "text": "셋"}]}
    ]});
    let bytes = helper().tiptap_to_pptx("", &doc).await.unwrap();
    assert_eq!(check_parts("rules", &bytes), 3);
    let md = markdown_of(&bytes);
    let (a, b, c) = (
        md.find("하나").unwrap(),
        md.find("둘").unwrap(),
        md.find("셋").unwrap(),
    );
    assert!(a < b && b < c, "{md}");
}

/// Source `documentMarkdown`: `# ` + the escaped visible title, a blank
/// line, then `tiptapDocToMd` — byte-equal to the TS oracle body
/// (`*.roundtrip.md`) on every markdown-oracle fixture.
#[tokio::test]
async fn markdown_export_is_the_titled_oracle_markdown() {
    let helper = helper();
    let dir = root().join("markdown-oracle");
    let mut checked = 0;
    for (name, doc) in fixtures() {
        let Ok(expected) = std::fs::read_to_string(dir.join(format!("{name}.roundtrip.md"))) else {
            continue;
        };
        let got = helper
            .tiptap_to_md_export(" 문서 제목\r\n#1 [x] ", &doc)
            .await
            .unwrap();
        let got = String::from_utf8(got).unwrap();
        // git may store the CRLF fixture's oracle with LF endings.
        assert_eq!(
            got.replace("\r\n", "\n"),
            format!(
                "# 문서 제목 \\#1 \\[x\\]\n\n{}",
                expected.replace("\r\n", "\n")
            ),
            "{name}"
        );
        checked += 1;
    }
    assert_eq!(checked, 53);
    // An empty visible title: the body alone.
    let doc = json!({"type": "doc", "content": [
        {"type": "paragraph", "content": [{"type": "text", "text": "본문"}]}]});
    let got = helper.tiptap_to_md_export(" \n ", &doc).await.unwrap();
    assert_eq!(got, "본문\n".as_bytes());
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
        let err = missing.tiptap_to_pptx("t", &body).await.unwrap_err();
        assert!(
            matches!(err, ExportRenderError::InvalidInput),
            "{body}: {err:?}"
        );
        let err = missing.tiptap_to_md_export("t", &body).await.unwrap_err();
        assert!(
            matches!(err, ExportRenderError::InvalidInput),
            "{body}: {err:?}"
        );
    }
    let doc = json!({"type": "doc"});
    let err = missing.tiptap_to_pptx("t", &doc).await.unwrap_err();
    assert!(matches!(err, ExportRenderError::Failed), "{err:?}");
    let err = missing.tiptap_to_md_export("t", &doc).await.unwrap_err();
    assert!(matches!(err, ExportRenderError::Failed), "{err:?}");
}

fn big_doc(paragraphs: usize) -> Value {
    let text = "한글 문단과 English text 가 섞인 긴 본문입니다 😀. ".repeat(20);
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
    let small_output = helper().with_limits(MarkdownLimits {
        max_output_bytes: 4096,
        ..MarkdownLimits::default()
    });
    let err = small_output.tiptap_to_pptx("t", &doc).await.unwrap_err();
    assert!(matches!(err, ExportRenderError::TooLarge), "{err:?}");
    let err = small_output
        .tiptap_to_md_export("t", &doc)
        .await
        .unwrap_err();
    assert!(matches!(err, ExportRenderError::TooLarge), "{err:?}");
    let small_input = helper().with_limits(MarkdownLimits {
        max_input_bytes: 1024,
        ..MarkdownLimits::default()
    });
    let err = small_input.tiptap_to_pptx("t", &doc).await.unwrap_err();
    assert!(matches!(err, ExportRenderError::TooLarge), "{err:?}");
    let err = small_input
        .tiptap_to_md_export("t", &doc)
        .await
        .unwrap_err();
    assert!(matches!(err, ExportRenderError::TooLarge), "{err:?}");
}

/// A child killed by the watchdog is a helper failure (routes: 500, as the
/// Node export's timeout), not an input error.
#[tokio::test]
async fn watchdog_kill_is_a_failure() {
    let fast = helper().with_limits(MarkdownLimits {
        timeout: Duration::from_millis(1),
        ..MarkdownLimits::default()
    });
    let err = fast.tiptap_to_pptx("t", &big_doc(2000)).await.unwrap_err();
    assert!(matches!(err, ExportRenderError::Failed), "{err:?}");
    let err = fast
        .tiptap_to_md_export("t", &big_doc(2000))
        .await
        .unwrap_err();
    assert!(matches!(err, ExportRenderError::Failed), "{err:?}");
}
