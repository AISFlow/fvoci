//! PDF export through the `--internal-markdown` child (`tiptap-to-pdf`) as the
//! server runs it: real process and limits, the oracle corpus read back by an
//! independent parser (lopdf, via pdf-extract), the public fail-fast pool and
//! the error paths the export routes map (413 / 400 / 500 / 503).

use std::path::PathBuf;
use std::time::Duration;

use fvoci_server::documents::export::ExportRenderError;
use fvoci_server::documents::markdown_helper::{MarkdownHelper, MarkdownLimits};
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

fn text_of(bytes: &[u8]) -> String {
    pdf_extract::extract_text_from_mem(bytes)
        .unwrap()
        .chars()
        .filter(|c| !c.is_whitespace())
        .collect()
}

/// Every fixture becomes a PDF lopdf opens, with the title and Korean/emoji
/// text extractable (ToUnicode maps of the subset fonts).
#[tokio::test]
async fn corpus_through_the_child_opens_and_keeps_its_text() {
    let helper = helper();
    for (name, doc) in fixtures() {
        let bytes = helper.tiptap_to_pdf("문서 제목", &doc).await.unwrap();
        assert!(bytes.starts_with(b"%PDF-"), "{name}");
        assert!(bytes.len() < 20_000_000, "{name}");
        let pdf = pdf_extract::Document::load_mem(&bytes).unwrap_or_else(|e| panic!("{name}: {e}"));
        assert!(!pdf.get_pages().is_empty(), "{name}");
        let text = text_of(&bytes);
        assert!(text.starts_with("문서제목"), "{name}: {text}");
        match name.as_str() {
            "02-korean-emoji" => {
                for want in ["안녕하세요,세계!", "😀👍🏽", "🇰🇷", "홍길동", "𠜎"]
                {
                    assert!(text.contains(want), "{want}: {text}");
                }
            }
            "h09-korean-emoji" => assert!(text.contains("가나다라마바사"), "{text}"),
            _ => {}
        }
    }
}

/// Member and public share PDFs are the same bytes for the same body; the
/// public pool of one refuses a second request instead of queueing it.
#[tokio::test]
async fn public_pool_is_the_same_writer_and_fails_fast() {
    let doc = json!({"type": "doc", "content": [
        {"type": "paragraph", "content": [{"type": "text", "text": "공유 문서 😀"}]}]});
    let member = helper().tiptap_to_pdf("공유", &doc).await.unwrap();
    let public = helper().tiptap_to_pdf_public("공유", &doc).await.unwrap();
    assert_eq!(member, public);

    // One helper (the app state's): its public pool of one is busy.
    let slow = big_doc(1500);
    let shared = helper();
    let (a, b) = tokio::join!(shared.tiptap_to_pdf_public("t", &slow), async {
        tokio::time::sleep(Duration::from_millis(50)).await;
        shared.clone().tiptap_to_pdf_public("t", &doc).await
    });
    // Whichever call takes the one permit renders; the other is refused.
    let busy = [&a, &b]
        .iter()
        .filter(|r| matches!(r, Err(ExportRenderError::Busy)))
        .count();
    assert_eq!(busy, 1, "{a:?} {b:?}");
    assert!(a.is_ok() || b.is_ok(), "{a:?} {b:?}");
    // Members still wait for their own permits meanwhile.
    assert!(helper().tiptap_to_pdf("t", &doc).await.is_ok());
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
        let err = missing.tiptap_to_pdf("t", &body).await.unwrap_err();
        assert!(
            matches!(err, ExportRenderError::InvalidInput),
            "{body}: {err:?}"
        );
    }
    let err = missing
        .tiptap_to_pdf("t", &json!({"type": "doc"}))
        .await
        .unwrap_err();
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
    let limits = MarkdownLimits {
        max_output_bytes: 4096,
        ..MarkdownLimits::default()
    };
    let err = helper()
        .with_limits(limits)
        .tiptap_to_pdf("t", &doc)
        .await
        .unwrap_err();
    assert!(matches!(err, ExportRenderError::TooLarge), "{err:?}");
    let limits = MarkdownLimits {
        max_input_bytes: 1024,
        ..MarkdownLimits::default()
    };
    let err = helper()
        .with_limits(limits)
        .tiptap_to_pdf("t", &doc)
        .await
        .unwrap_err();
    assert!(matches!(err, ExportRenderError::TooLarge), "{err:?}");
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
        .tiptap_to_pdf("t", &big_doc(2000))
        .await
        .unwrap_err();
    assert!(matches!(err, ExportRenderError::Failed), "{err:?}");
}
