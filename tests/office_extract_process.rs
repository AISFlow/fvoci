//! The office child (`fvoci-server --internal-office-extract`) as the server
//! runs it: real process, real rlimits, hostile inputs.

#[path = "support/office_fixtures.rs"]
mod office_fixtures;

use std::path::PathBuf;
use std::time::{Duration, Instant};

use fvoci_server::documents::office::{
    run_office_helper, OfficeCancelled, OfficeKind, OfficeLimits, OfficeMode, OfficeOutcome,
};
use tokio_util::sync::CancellationToken;

fn helper() -> PathBuf {
    PathBuf::from(env!("CARGO_BIN_EXE_fvoci-server"))
}

async fn run(
    bytes: Vec<u8>,
    kind: OfficeKind,
    mode: OfficeMode,
    limits: OfficeLimits,
) -> OfficeOutcome {
    run_office_helper(
        &helper(),
        bytes,
        kind,
        mode,
        &limits,
        &CancellationToken::new(),
    )
    .await
    .expect("not cancelled")
}

#[tokio::test]
async fn every_format_extracts_through_the_child() {
    let cases: Vec<(OfficeKind, Vec<u8>, &str)> = vec![
        (
            OfficeKind::Docx,
            office_fixtures::docx("제목", &["docx 본문"]),
            "docx 본문",
        ),
        (
            OfficeKind::Pptx,
            office_fixtures::pptx(&[("슬라이드", "pptx 본문")]),
            "pptx 본문",
        ),
        (
            OfficeKind::Xlsx,
            office_fixtures::xlsx("시트", &[&["xlsx 셀"]]),
            "xlsx 셀",
        ),
        (
            OfficeKind::Odt,
            office_fixtures::odt("제목", &["odt 본문"]),
            "odt 본문",
        ),
        (
            OfficeKind::Odp,
            office_fixtures::odp(&[("표지", "odp 본문")]),
            "odp 본문",
        ),
        (
            OfficeKind::Ods,
            office_fixtures::ods("시트", &[&["ods 셀"]]),
            "ods 셀",
        ),
        (
            OfficeKind::Pdf,
            office_fixtures::pdf(&["pdf body text"]),
            "pdf body text",
        ),
    ];
    for (kind, bytes, expected) in cases {
        for mode in [OfficeMode::Markdown, OfficeMode::Text] {
            let limits = match mode {
                OfficeMode::Markdown => OfficeLimits::import(),
                OfficeMode::Text => OfficeLimits::attachment(),
            };
            match run(bytes.clone(), kind, mode, limits).await {
                OfficeOutcome::Ok { text, truncated } => {
                    assert!(!truncated);
                    assert!(text.contains(expected), "{kind:?} {mode:?}: {text}");
                }
                other => panic!("{kind:?} {mode:?}: {other:?}"),
            }
        }
    }
}

#[tokio::test]
async fn zip_bomb_is_a_resource_limit_in_the_child() {
    let out = run(
        office_fixtures::docx_bomb(),
        OfficeKind::Docx,
        OfficeMode::Markdown,
        OfficeLimits::import(),
    )
    .await;
    assert!(
        matches!(out, OfficeOutcome::ResourceLimit { .. }),
        "{out:?}"
    );
}

#[tokio::test]
async fn address_space_ceiling_kills_the_child_not_the_server() {
    // 32 MiB of address space cannot even hold the input buffer growth plus
    // the binary's mappings: the allocation aborts the child.
    let limits = OfficeLimits {
        address_space: 32 * 1024 * 1024,
        ..OfficeLimits::import()
    };
    let bytes = office_fixtures::docx("t", &[&"x".repeat(1 << 20)]);
    let out = run(bytes, OfficeKind::Docx, OfficeMode::Text, limits).await;
    match out {
        OfficeOutcome::ResourceLimit { detail } => {
            assert!(
                detail.contains("SIGABRT") || detail.contains("signal"),
                "{detail}"
            )
        }
        other => panic!("{other:?}"),
    }
}

#[tokio::test]
async fn oversize_input_is_refused_before_spawning() {
    let limits = OfficeLimits {
        max_input_bytes: 10,
        ..OfficeLimits::attachment()
    };
    let started = Instant::now();
    let out = run(vec![b'x'; 11], OfficeKind::Pdf, OfficeMode::Text, limits).await;
    assert!(
        matches!(out, OfficeOutcome::ResourceLimit { .. }),
        "{out:?}"
    );
    assert!(started.elapsed() < Duration::from_secs(1));
}

#[tokio::test]
async fn output_budget_truncates_text_and_rejects_markdown() {
    let bytes = office_fixtures::docx("t", &[&"가".repeat(5000)]);
    let text_limits = OfficeLimits {
        max_output: 100,
        ..OfficeLimits::attachment()
    };
    match run(
        bytes.clone(),
        OfficeKind::Docx,
        OfficeMode::Text,
        text_limits,
    )
    .await
    {
        OfficeOutcome::Ok { text, truncated } => {
            assert!(truncated);
            assert_eq!(text.chars().count(), 100);
        }
        other => panic!("{other:?}"),
    }
    let md_limits = OfficeLimits {
        max_output: 100,
        ..OfficeLimits::import()
    };
    let out = run(bytes, OfficeKind::Docx, OfficeMode::Markdown, md_limits).await;
    assert!(
        matches!(out, OfficeOutcome::ResourceLimit { .. }),
        "{out:?}"
    );
}

#[tokio::test]
async fn corrupt_and_mismatched_documents() {
    let mut pdf = office_fixtures::pdf(&["x"]);
    pdf.truncate(60);
    let out = run(
        pdf,
        OfficeKind::Pdf,
        OfficeMode::Text,
        OfficeLimits::attachment(),
    )
    .await;
    assert!(matches!(out, OfficeOutcome::Corrupt { .. }), "{out:?}");
    let out = run(
        office_fixtures::pdf(&["x"]),
        OfficeKind::Docx,
        OfficeMode::Text,
        OfficeLimits::attachment(),
    )
    .await;
    assert!(matches!(out, OfficeOutcome::Unsupported { .. }), "{out:?}");
    // Random bytes behind a PDF header must not crash the server process;
    // the child answers corrupt (parser error or panic).
    let mut junk = b"%PDF-1.7\n".to_vec();
    junk.extend((0..4096u32).map(|i| (i.wrapping_mul(2_654_435_761) >> 13) as u8));
    let out = run(
        junk,
        OfficeKind::Pdf,
        OfficeMode::Text,
        OfficeLimits::attachment(),
    )
    .await;
    assert!(
        matches!(out, OfficeOutcome::Corrupt { .. } | OfficeOutcome::Empty),
        "{out:?}"
    );
}

#[tokio::test]
async fn missing_helper_is_a_worker_failure() {
    let out = run_office_helper(
        std::path::Path::new("/nonexistent/fvoci-server"),
        office_fixtures::pdf(&["x"]),
        OfficeKind::Pdf,
        OfficeMode::Text,
        &OfficeLimits::attachment(),
        &CancellationToken::new(),
    )
    .await
    .unwrap();
    assert!(
        matches!(out, OfficeOutcome::WorkerFailure { .. }),
        "{out:?}"
    );
}

#[tokio::test]
async fn cancel_returns_cancelled_instead_of_a_result() {
    let cancel = CancellationToken::new();
    cancel.cancel();
    let out = run_office_helper(
        &helper(),
        office_fixtures::docx_bomb(),
        OfficeKind::Docx,
        OfficeMode::Text,
        &OfficeLimits::attachment(),
        &cancel,
    )
    .await;
    assert_eq!(out, Err(OfficeCancelled));
}
