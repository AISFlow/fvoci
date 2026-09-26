use super::*;

#[path = "../../../tests/support/office_fixtures.rs"]
mod fixtures;

fn md(bytes: &[u8], kind: OfficeKind) -> String {
    match extract_office(bytes, kind, OfficeMode::Markdown, 1 << 20) {
        OfficeOutcome::Ok { text, truncated } => {
            assert!(!truncated);
            text
        }
        other => panic!("{kind:?}: {other:?}"),
    }
}

#[test]
fn docx_headings_emphasis_lists_tables_and_no_fallback_duplicates() {
    let text = md(
        &fixtures::docx("회의록", &["첫 문단 *별표*", "둘째 <b>"]),
        OfficeKind::Docx,
    );
    assert_eq!(
        text,
        "# 회의록\n\n**첫 문단 \\*별표\\***\n\n둘째 \\<b\\>\n\n- list item\n\n| h1 | h2 |\n| --- | --- |\n| c1 | c2 |\n\nchoice"
    );
}

#[test]
fn pptx_follows_the_slide_id_list_and_marks_titles() {
    let text = md(
        &fixtures::pptx(&[("첫 슬라이드", "본문 A"), ("Second", "body B")]),
        OfficeKind::Pptx,
    );
    assert_eq!(
        text,
        "## 첫 슬라이드\n\n**본문 A**\n\n## Second\n\n**body B**"
    );
}

#[test]
fn xlsx_sheets_become_tables_with_shared_strings() {
    let text = md(
        &fixtures::xlsx("예산", &[&["항목", "금액"], &["서버", "1200"]]),
        OfficeKind::Xlsx,
    );
    assert_eq!(
        text,
        "## 예산\n\n| 항목 | 금액 |\n| --- | --- |\n| 서버 | 1200 |"
    );
}

#[test]
fn odt_skips_notes_and_keeps_styles() {
    let text = md(&fixtures::odt("제목", &["굵게", "보통"]), OfficeKind::Odt);
    assert_eq!(text, "# 제목\n\n**굵게**  end\n\n보통\n\n- item one");
}

#[test]
fn odp_titles_and_bodies_without_speaker_notes() {
    let text = md(&fixtures::odp(&[("표지", "내용")]), OfficeKind::Odp);
    assert_eq!(text, "## 표지\n\n내용");
}

#[test]
fn ods_repeated_empty_cells_and_rows_do_not_expand() {
    let text = md(
        &fixtures::ods("시트1", &[&["a", "b"], &["1", "2"]]),
        OfficeKind::Ods,
    );
    assert_eq!(text, "## 시트1\n\n| a | b |\n| --- | --- |\n| 1 | 2 |");
}

#[test]
fn pdf_text_blocks_become_paragraphs() {
    let text = md(
        &fixtures::pdf(&["Hello PDF import", "Second block (x)"]),
        OfficeKind::Pdf,
    );
    assert!(text.contains("Hello PDF import"), "{text}");
    assert!(text.contains("Second block (x)"), "{text}");
}

#[test]
fn text_mode_is_plain() {
    let out = extract_office(
        &fixtures::docx("Title", &["Body"]),
        OfficeKind::Docx,
        OfficeMode::Text,
        500_000,
    );
    assert_eq!(
        out,
        OfficeOutcome::Ok {
            text: "Title\nBody\nlist item\nh1\th2\nc1\tc2\nchoice".into(),
            truncated: false
        }
    );
}

#[test]
fn text_mode_truncates_and_markdown_over_budget_is_a_limit() {
    let long = "x".repeat(200);
    let bytes = fixtures::docx("t", &[&long]);
    match extract_office(&bytes, OfficeKind::Docx, OfficeMode::Text, 50) {
        OfficeOutcome::Ok { text, truncated } => {
            assert!(truncated);
            assert_eq!(text.chars().count(), 50);
        }
        other => panic!("{other:?}"),
    }
    assert!(matches!(
        extract_office(&bytes, OfficeKind::Docx, OfficeMode::Markdown, 50),
        OfficeOutcome::ResourceLimit { .. }
    ));
}

#[test]
fn mismatched_and_hostile_inputs_are_refused() {
    // Wrong container for the extension.
    assert!(matches!(
        extract_office(
            b"%PDF-1.4 not a zip",
            OfficeKind::Docx,
            OfficeMode::Text,
            100
        ),
        OfficeOutcome::Unsupported { .. }
    ));
    assert!(matches!(
        extract_office(
            &fixtures::docx("a", &[]),
            OfficeKind::Pdf,
            OfficeMode::Text,
            100
        ),
        OfficeOutcome::Unsupported { .. }
    ));
    // An xlsx renamed to .docx.
    assert!(matches!(
        extract_office(
            &fixtures::xlsx("s", &[&["a"]]),
            OfficeKind::Docx,
            OfficeMode::Text,
            100
        ),
        OfficeOutcome::Unsupported { .. }
    ));
    // ODF mimetype disagreeing with the extension.
    assert!(matches!(
        extract_office(
            &fixtures::odt("a", &[]),
            OfficeKind::Ods,
            OfficeMode::Text,
            100
        ),
        OfficeOutcome::Unsupported { .. }
    ));
    // Zip bomb: over the inflate budget before any XML is parsed.
    assert!(matches!(
        extract_office(
            &fixtures::docx_bomb(),
            OfficeKind::Docx,
            OfficeMode::Text,
            100
        ),
        OfficeOutcome::ResourceLimit { .. }
    ));
    // Traversal entry rejects the archive.
    let traversal = fixtures::zip_of(&[("../evil.xml", b"<a/>"), ("word/document.xml", b"<a/>")]);
    assert!(matches!(
        extract_office(&traversal, OfficeKind::Docx, OfficeMode::Text, 100),
        OfficeOutcome::Corrupt { .. }
    ));
    // Malformed XML and truncated PDF.
    let broken =
        fixtures::zip_of(&[("word/document.xml", b"<w:document><w:body><w:p><w:t>x</w:q")]);
    assert!(matches!(
        extract_office(&broken, OfficeKind::Docx, OfficeMode::Text, 100),
        OfficeOutcome::Corrupt { .. } | OfficeOutcome::Ok { .. }
    ));
    let mut pdf = fixtures::pdf(&["x"]);
    pdf.truncate(40);
    assert!(matches!(
        extract_office(&pdf, OfficeKind::Pdf, OfficeMode::Text, 100),
        OfficeOutcome::Corrupt { .. }
    ));
    assert_eq!(
        extract_office(b"", OfficeKind::Pdf, OfficeMode::Text, 100),
        OfficeOutcome::Unsupported {
            detail: "empty file".into()
        }
    );
}

#[test]
fn empty_documents_are_empty_not_ok() {
    assert_eq!(
        extract_office(
            &fixtures::xlsx("s", &[]),
            OfficeKind::Xlsx,
            OfficeMode::Text,
            100
        ),
        OfficeOutcome::Empty
    );
}

#[test]
fn kinds_come_from_the_extension() {
    assert_eq!(OfficeKind::from_name("보고서.DOCX"), Some(OfficeKind::Docx));
    assert_eq!(OfficeKind::from_name("a.ods"), Some(OfficeKind::Ods));
    assert_eq!(OfficeKind::from_name("a.doc"), None);
    assert_eq!(OfficeKind::from_name("pdf"), None);
}
