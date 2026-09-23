use document_extract::gen::{
    corrupt_cfb, expected_hwp5_body, expected_hwpx_body, hwp5_distribution_flag, hwp5_empty_body,
    hwp5_encrypted_flag, hwp5_known_body, hwp5_uncompressed_body, hwpx_empty_body,
    hwpx_encrypted_manifest, hwpx_known_body, pdf_named_hwp, truncated,
    zip_with_forged_uncompressed, zip_with_path_escape, HWP_PRVTEXT_DECOY,
};
use document_extract::limits::{Limits, MAX_INPUT_BYTES};
use document_extract::outcome::{ExtractStatus, LimitKind, UnsupportedReason};
use document_extract::parse::extract_bytes;

fn limits() -> Limits {
    Limits::for_tests()
}

#[test]
fn hwp5_compressed_multi_section_korean_emoji() {
    let bytes = hwp5_known_body();
    let report = extract_bytes(&bytes, "품의서.hwp", &limits());
    match report.outcome {
        ExtractStatus::Ok { ref text, .. } => {
            assert_eq!(text, &expected_hwp5_body());
            assert!(!text.contains(HWP_PRVTEXT_DECOY));
        }
        other => panic!("expected ok, got {other:?}"),
    }
    assert!(!report.used_preview_stream);
}

#[test]
fn hwp5_uncompressed_path() {
    let bytes = hwp5_uncompressed_body();
    let report = extract_bytes(&bytes, "plain.hwp", &limits());
    assert!(
        report.outcome.text().contains("한글본문색인토큰"),
        "{:?}",
        report.outcome
    );
}

#[test]
fn hwpx_sections_table_korean_emoji() {
    let bytes = hwpx_known_body();
    let report = extract_bytes(&bytes, "품의서.hwpx", &limits());
    match report.outcome {
        ExtractStatus::Ok { ref text, .. } => {
            assert_eq!(text, &expected_hwpx_body());
        }
        other => panic!("expected ok, got {other:?}"),
    }
}

#[test]
fn empty_valid_hwp_is_success_empty() {
    let report = extract_bytes(&hwp5_empty_body(), "empty.hwp", &limits());
    assert!(
        matches!(report.outcome, ExtractStatus::Empty { .. }),
        "{:?}",
        report.outcome
    );
}

#[test]
fn empty_valid_hwpx_is_success_empty() {
    let report = extract_bytes(&hwpx_empty_body(), "empty.hwpx", &limits());
    assert!(
        matches!(report.outcome, ExtractStatus::Empty { .. }),
        "{:?}",
        report.outcome
    );
}

#[test]
fn encrypted_hwp_is_unsupported() {
    let report = extract_bytes(&hwp5_encrypted_flag(), "secret.hwp", &limits());
    assert!(
        matches!(
            report.outcome,
            ExtractStatus::Unsupported {
                reason: UnsupportedReason::Encrypted,
                ..
            }
        ),
        "{:?}",
        report.outcome
    );
}

#[test]
fn distribution_hwp_is_not_ok_body_from_prvtext() {
    let report = extract_bytes(&hwp5_distribution_flag(), "배포.hwp", &limits());
    assert!(
        matches!(
            report.outcome,
            ExtractStatus::Unsupported {
                reason: UnsupportedReason::Distribution,
                ..
            }
        ),
        "distribution must be Unsupported/Distribution, got {:?}",
        report.outcome
    );
}

#[test]
fn hwpx_odf_encryption_is_unsupported() {
    let report = extract_bytes(&hwpx_encrypted_manifest(), "secret.hwpx", &limits());
    assert!(
        matches!(
            report.outcome,
            ExtractStatus::Unsupported {
                reason: UnsupportedReason::Encrypted,
                ..
            }
        ),
        "{:?}",
        report.outcome
    );
}

#[test]
fn truncated_hwp_is_corrupt() {
    let report = extract_bytes(&truncated(&hwp5_known_body()), "cut.hwp", &limits());
    assert!(
        matches!(report.outcome, ExtractStatus::Corrupt { .. }),
        "truncated HWP must be Corrupt, got {:?}",
        report.outcome
    );
}

#[test]
fn corrupt_cfb_is_not_ok() {
    let report = extract_bytes(&corrupt_cfb(), "bad.hwp", &limits());
    assert!(!report.outcome.is_success(), "{:?}", report.outcome);
}

#[test]
fn extension_magic_mismatch() {
    let pdf = pdf_named_hwp();
    let report = extract_bytes(&pdf, "속임수.hwp", &limits());
    assert!(
        matches!(
            report.outcome,
            ExtractStatus::Unsupported {
                reason: UnsupportedReason::ExtensionMagicMismatch,
                ..
            }
        ),
        "{:?}",
        report.outcome
    );

    let report = extract_bytes(&hwp5_known_body(), "속임수.hwpx", &limits());
    assert!(
        matches!(
            report.outcome,
            ExtractStatus::Unsupported {
                reason: UnsupportedReason::ExtensionMagicMismatch,
                ..
            }
        ),
        "{:?}",
        report.outcome
    );
}

#[test]
fn empty_file_unsupported() {
    let report = extract_bytes(&[], "x.hwp", &limits());
    assert!(
        matches!(
            report.outcome,
            ExtractStatus::Unsupported {
                reason: UnsupportedReason::EmptyFile,
                ..
            }
        ),
        "{:?}",
        report.outcome
    );
}

#[test]
fn input_oversize() {
    let mut limits = limits();
    limits.max_input_bytes = 16;
    let report = extract_bytes(&hwp5_known_body(), "big.hwp", &limits);
    assert!(
        matches!(
            report.outcome,
            ExtractStatus::ResourceLimit {
                kind: LimitKind::Input,
                ..
            }
        ),
        "{:?}",
        report.outcome
    );
}

#[test]
fn declared_max_input_constant() {
    assert_eq!(MAX_INPUT_BYTES, 20 * 1024 * 1024);
}

#[test]
fn zip_uncompressed_guard() {
    let report = extract_bytes(&zip_with_forged_uncompressed(), "bomb.hwpx", &limits());
    assert!(
        matches!(
            report.outcome,
            ExtractStatus::ResourceLimit {
                kind: LimitKind::ZipUncompressed,
                ..
            }
        ),
        "{:?}",
        report.outcome
    );
}

#[test]
fn zip_path_escape_rejected() {
    let report = extract_bytes(&zip_with_path_escape(), "escape.hwpx", &limits());
    assert!(
        matches!(report.outcome, ExtractStatus::Corrupt { .. }),
        "{:?}",
        report.outcome
    );
}

#[test]
fn output_truncation_is_partial() {
    let mut limits = limits();
    limits.max_output_chars = 4;
    let report = extract_bytes(&hwpx_known_body(), "short.hwpx", &limits);
    assert!(
        matches!(report.outcome, ExtractStatus::Partial { .. }),
        "{:?}",
        report.outcome
    );
}

fn fixture(name: &str) -> Vec<u8> {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("fixtures")
        .join(name);
    std::fs::read(&path).unwrap_or_else(|err| panic!("read {}: {err}", path.display()))
}

#[test]
fn user_hancom_hwp_안녕() {
    let report = extract_bytes(
        &fixture("user-hancom-12.30-안녕.hwp"),
        "안녕.hwp",
        &limits(),
    );
    match report.outcome {
        ExtractStatus::Ok { ref text, .. } => {
            assert!(text.contains("안녕"), "got {text:?}");
            assert!(!text.contains("PrvText"));
        }
        other => panic!("expected ok body 안녕, got {other:?}"),
    }
    assert!(!report.used_preview_stream);
}

#[test]
fn user_hancom_hwpx_안녕() {
    let report = extract_bytes(
        &fixture("user-hancom-12.30-안녕.hwpx"),
        "안녕.hwpx",
        &limits(),
    );
    match report.outcome {
        ExtractStatus::Ok { ref text, .. } => {
            assert!(text.contains("안녕"), "got {text:?}");
        }
        other => panic!("expected ok body 안녕, got {other:?}"),
    }
    assert!(!report.used_preview_stream);
}
