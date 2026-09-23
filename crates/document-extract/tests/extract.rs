use document_extract::gen::{
    corrupt_cfb, expected_hwp5_body, expected_hwpx_body, expected_table_caption_bottom_body,
    expected_table_caption_top_body, hwp5_distribution_flag, hwp5_empty_body, hwp5_encrypted_flag,
    hwp5_known_body, hwp5_only_section_truncated_records, hwp5_section1_truncated_records,
    hwp5_table_caption_top, hwp5_uncompressed_body, hwpx_empty_body, hwpx_encrypted_manifest,
    hwpx_equation_and_body, hwpx_form_and_body, hwpx_known_body, hwpx_many_shapes,
    hwpx_nested_tables, hwpx_out_of_range_cell, hwpx_picture_caption_and_body,
    hwpx_section1_malformed, hwpx_shape_and_body, hwpx_table_caption_bottom,
    hwpx_table_caption_top, hwpx_two_short_lines, hwpx_zero_paragraph_section, pdf_named_hwp,
    truncated, zip_with_entry_count, zip_with_forged_uncompressed, zip_with_path_escape,
    EQUATION_SCRIPT, FORM_CAPTION, HWPX_SEC0_P0, HWP_PRVTEXT_DECOY, HWP_SEC0_P0, PICTURE_CAPTION,
};
use document_extract::limits::{Limits, MAX_CHILD_STDOUT_BYTES, MAX_INPUT_BYTES};
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
fn hwp5_section1_record_truncation_is_partial() {
    let report = extract_bytes(
        &hwp5_section1_truncated_records(),
        "sec1-cut.hwp",
        &limits(),
    );
    match report.outcome {
        ExtractStatus::Partial {
            ref text,
            ref warnings,
            ..
        } => {
            assert!(text.contains(HWP_SEC0_P0), "{text:?}");
            assert!(
                warnings.iter().any(|w| w.contains("section 1 dropped")),
                "{warnings:?}"
            );
        }
        other => panic!("Section1 record truncation must be Partial, got {other:?}"),
    }
}

#[test]
fn hwp5_only_truncated_section_is_corrupt() {
    let report = extract_bytes(
        &hwp5_only_section_truncated_records(),
        "all-cut.hwp",
        &limits(),
    );
    assert!(
        matches!(report.outcome, ExtractStatus::Corrupt { .. }),
        "no recovered section must be Corrupt, got {:?}",
        report.outcome
    );
}

#[test]
fn hwpx_malformed_section1_is_partial() {
    let report = extract_bytes(&hwpx_section1_malformed(), "sec1-bad.hwpx", &limits());
    match report.outcome {
        ExtractStatus::Partial {
            ref text,
            ref warnings,
            ..
        } => {
            assert!(text.contains(HWPX_SEC0_P0), "{text:?}");
            assert!(
                warnings.iter().any(|w| w.contains("section 1 dropped")),
                "{warnings:?}"
            );
        }
        other => panic!("malformed HWPX section1 must be Partial, got {other:?}"),
    }
}

#[test]
fn hwpx_zero_paragraph_section_is_genuine_empty() {
    let report = extract_bytes(&hwpx_zero_paragraph_section(), "bare.hwpx", &limits());
    assert!(
        matches!(report.outcome, ExtractStatus::Empty { .. }),
        "successful public section parse distinguishes genuine empty HWPX; got {:?}",
        report.outcome
    );
}

#[test]
fn hwp5_table_caption_top_before_cells() {
    let report = extract_bytes(&hwp5_table_caption_top(), "caption.hwp", &limits());
    match report.outcome {
        ExtractStatus::Ok { ref text, .. } => {
            assert_eq!(text, &expected_table_caption_top_body());
        }
        other => panic!("HWP5 top caption must extract, got {other:?}"),
    }
}

#[test]
fn hwpx_table_caption_top_before_cells() {
    let report = extract_bytes(&hwpx_table_caption_top(), "caption.hwpx", &limits());
    match report.outcome {
        ExtractStatus::Ok { ref text, .. } => {
            assert_eq!(text, &expected_table_caption_top_body());
        }
        other => panic!("HWPX top caption must extract, got {other:?}"),
    }
}

#[test]
fn hwpx_table_caption_bottom_after_cells() {
    let report = extract_bytes(&hwpx_table_caption_bottom(), "caption-b.hwpx", &limits());
    match report.outcome {
        ExtractStatus::Ok { ref text, .. } => {
            assert_eq!(text, &expected_table_caption_bottom_body());
        }
        other => panic!("HWPX bottom caption must follow cells, got {other:?}"),
    }
}

#[test]
fn hwpx_equation_script_is_extracted() {
    let report = extract_bytes(&hwpx_equation_and_body(), "eq.hwpx", &limits());
    match report.outcome {
        ExtractStatus::Ok { ref text, .. } => {
            assert!(text.contains("보이는문단"), "{text:?}");
            assert!(text.contains(EQUATION_SCRIPT), "{text:?}");
        }
        other => panic!("equation script must extract, got {other:?}"),
    }
}

#[test]
fn hwpx_form_caption_is_extracted() {
    let report = extract_bytes(&hwpx_form_and_body(), "form.hwpx", &limits());
    match report.outcome {
        ExtractStatus::Ok { ref text, .. } => {
            assert!(text.contains("보이는문단"), "{text:?}");
            assert!(text.contains(FORM_CAPTION), "{text:?}");
        }
        other => panic!("form caption must extract, got {other:?}"),
    }
}

#[test]
fn hwpx_picture_caption_is_extracted() {
    let report = extract_bytes(&hwpx_picture_caption_and_body(), "pic.hwpx", &limits());
    match report.outcome {
        ExtractStatus::Ok { ref text, .. } => {
            assert!(text.contains("보이는문단"), "{text:?}");
            assert!(text.contains(PICTURE_CAPTION), "{text:?}");
        }
        other => panic!("picture caption must extract, got {other:?}"),
    }
}

#[test]
fn many_shapes_dedupe_warnings_and_stay_partial() {
    let report = extract_bytes(&hwpx_many_shapes(40), "shapes.hwpx", &limits());
    match report.outcome {
        ExtractStatus::Partial {
            ref text,
            ref warnings,
            ..
        } => {
            assert!(text.contains("보이는문단"), "{text:?}");
            assert_eq!(warnings.iter().filter(|w| w.contains("shape")).count(), 1);
            assert!(warnings.iter().any(|w| w.contains("×40")), "{warnings:?}");
        }
        other => {
            panic!("many omitted shapes must be Partial with one counted warning, got {other:?}")
        }
    }
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
    const { assert!(MAX_CHILD_STDOUT_BYTES >= 500_000 * 6) };
}

#[test]
fn zip_entry_count_guard() {
    let mut limits = limits();
    limits.max_zip_entries = 2;
    let report = extract_bytes(&zip_with_entry_count(4), "many.hwpx", &limits);
    assert!(
        matches!(
            report.outcome,
            ExtractStatus::ResourceLimit {
                kind: LimitKind::ZipEntries,
                ..
            }
        ),
        "{:?}",
        report.outcome
    );
}

#[test]
fn newline_separator_counts_in_output_limit() {
    let mut limits = limits();
    limits.max_output_chars = 3;
    let report = extract_bytes(&hwpx_two_short_lines(), "ab-c.hwpx", &limits);
    match report.outcome {
        ExtractStatus::Partial { ref text, .. } => {
            assert!(text.chars().count() <= 3, "{text:?}");
            assert!(!text.contains('\n') || text.chars().count() < 4);
        }
        other => panic!("separator overflow must be Partial, got {other:?}"),
    }
}

#[test]
fn out_of_range_table_cell_is_partial() {
    let report = extract_bytes(&hwpx_out_of_range_cell(), "grid.hwpx", &limits());
    match report.outcome {
        ExtractStatus::Partial { ref warnings, .. } => {
            assert!(
                warnings.iter().any(|w| w.contains("outside")),
                "{warnings:?}"
            );
        }
        other => panic!("dropped out-of-range cell must be Partial, got {other:?}"),
    }
}

#[test]
fn omitted_shape_is_partial_not_ok() {
    let report = extract_bytes(&hwpx_shape_and_body(), "shape.hwpx", &limits());
    match report.outcome {
        ExtractStatus::Partial {
            ref text,
            ref warnings,
            ..
        } => {
            assert!(text.contains("보이는문단"), "{text:?}");
            assert!(!text.contains("도형안텍스트"), "{text:?}");
            assert!(warnings.iter().any(|w| w.contains("shape")), "{warnings:?}");
        }
        other => panic!("omitted shape must be Partial, got {other:?}"),
    }
}

#[test]
fn nested_supported_table_drop_is_partial() {
    let report = extract_bytes(&hwpx_nested_tables(10), "deep.hwpx", &limits());
    assert!(
        matches!(report.outcome, ExtractStatus::Partial { .. }),
        "omitted nested table body must be Partial, got {:?}",
        report.outcome
    );
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

#[test]
fn hwpx_failed_sole_section_remains_corrupt() {
    let bytes = document_extract::gen::hwpx_only_section_malformed();
    assert!(matches!(
        extract_bytes(&bytes, "failed.hwpx", &limits()).outcome,
        ExtractStatus::Corrupt { .. }
    ));
}

#[test]
fn hwpx_empty_section_does_not_make_valid_body_partial() {
    let bytes = document_extract::gen::hwpx_empty_then_body();
    match extract_bytes(&bytes, "mixed.hwpx", &limits()).outcome {
        ExtractStatus::Ok { text, .. } => assert_eq!(text, HWPX_SEC0_P0),
        other => panic!("valid empty section is not a parse failure: {other:?}"),
    }
}
