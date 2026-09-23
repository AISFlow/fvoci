use rhwp::parse_document;
use rhwp::parser::{detect_format, FileFormat, ParseError};

use crate::classify::{
    hint_from_name, inspect_zip_limits, reject_drm, reject_empty, reject_oversize, HintedKind,
};
use crate::limits::Limits;
use crate::outcome::{DocFormat, ExtractReport, ExtractStatus, LimitKind, UnsupportedReason};
use crate::walk::walk_body;

pub fn extract_bytes(bytes: &[u8], name: &str, limits: &Limits) -> ExtractReport {
    if let Some(report) = reject_empty(bytes) {
        return report;
    }
    if let Some(report) = reject_oversize(bytes, limits) {
        return report;
    }
    if let Some(report) = reject_drm(bytes) {
        return report;
    }
    if let Some(report) = inspect_zip_limits(bytes, limits) {
        return report;
    }

    let detected = detect_format(bytes);
    let hint = hint_from_name(name);
    if let Some(report) = mismatch(hint, detected, name) {
        return report;
    }

    match detected {
        FileFormat::Hwp => parse_and_walk(bytes, DocFormat::Hwp5, limits),
        FileFormat::Hwpx => parse_and_walk(bytes, DocFormat::Hwpx, limits),
        FileFormat::Hwp3 => ExtractReport::new(ExtractStatus::Unsupported {
            reason: UnsupportedReason::Hwp3,
            detail: "HWP 3.0 is not in this extract slice".to_string(),
        }),
        FileFormat::Hml => ExtractReport::new(ExtractStatus::Unsupported {
            reason: UnsupportedReason::Hml,
            detail: "HWPML is not in this extract slice".to_string(),
        }),
        FileFormat::DrmProtected => ExtractReport::new(ExtractStatus::Unsupported {
            reason: UnsupportedReason::Drm,
            detail: "DRM/security container".to_string(),
        }),
        FileFormat::Empty => ExtractReport::new(ExtractStatus::Unsupported {
            reason: UnsupportedReason::EmptyFile,
            detail: "empty file".to_string(),
        }),
        FileFormat::Unknown => ExtractReport::new(ExtractStatus::Unsupported {
            reason: UnsupportedReason::UnknownFormat,
            detail: format!("magic does not match HWP5/HWPX (name={name})"),
        }),
    }
}

fn mismatch(hint: HintedKind, detected: FileFormat, name: &str) -> Option<ExtractReport> {
    let ok = match hint {
        HintedKind::Other => return None,
        HintedKind::Hwp => matches!(detected, FileFormat::Hwp),
        HintedKind::Hwpx => matches!(detected, FileFormat::Hwpx),
    };
    if ok {
        return None;
    }
    Some(ExtractReport::new(ExtractStatus::Unsupported {
        reason: UnsupportedReason::ExtensionMagicMismatch,
        detail: format!("name {name:?} does not match detected {detected:?}"),
    }))
}

fn parse_and_walk(bytes: &[u8], format: DocFormat, limits: &Limits) -> ExtractReport {
    let doc = match parse_document(bytes) {
        Ok(doc) => doc,
        Err(err) => return map_parse_error(err, format),
    };

    let mut warnings = Vec::new();
    if doc.header.encrypted {
        return ExtractReport::new(ExtractStatus::Unsupported {
            reason: UnsupportedReason::Encrypted,
            detail: "parsed document still marked encrypted".to_string(),
        });
    }
    if doc.header.distribution {
        return ExtractReport::new(ExtractStatus::Unsupported {
            reason: UnsupportedReason::Distribution,
            detail: "distribution/ViewText documents are not extracted in this slice".to_string(),
        });
    }

    let walked = walk_body(&doc, limits.max_output_chars);
    warnings.extend(walked.warnings);

    if walked.truncated || walked.omitted_supported || walked.omitted_shape {
        return ExtractReport::new(ExtractStatus::Partial {
            text: walked.text,
            format,
            warnings,
        });
    }
    if walked.text.is_empty() {
        return ExtractReport::new(ExtractStatus::Empty { format, warnings });
    }
    ExtractReport::new(ExtractStatus::Ok {
        text: walked.text,
        format,
        warnings,
    })
}

fn map_parse_error(err: ParseError, format: DocFormat) -> ExtractReport {
    match err {
        ParseError::EncryptedDocument => ExtractReport::new(ExtractStatus::Unsupported {
            reason: UnsupportedReason::Encrypted,
            detail: err.to_string(),
        }),
        ParseError::UnsupportedFormat {
            code,
            format: fmt,
            hint,
        } => {
            let reason = match code {
                "DRM_PROTECTED" => UnsupportedReason::Drm,
                "EMPTY_FILE" => UnsupportedReason::EmptyFile,
                "UNSUPPORTED_HWP3" => UnsupportedReason::Hwp3,
                _ => UnsupportedReason::UnknownFormat,
            };
            ExtractReport::new(ExtractStatus::Unsupported {
                reason,
                detail: format!("{fmt}: {hint}"),
            })
        }
        ParseError::CryptoError(e) => {
            let msg = e.to_string();
            if msg.to_ascii_lowercase().contains("password")
                || msg.contains("암호")
                || msg.contains("Encrypted")
            {
                ExtractReport::new(ExtractStatus::Unsupported {
                    reason: UnsupportedReason::Encrypted,
                    detail: msg,
                })
            } else {
                ExtractReport::new(ExtractStatus::Corrupt { detail: msg })
            }
        }
        ParseError::CfbError(e) => map_limit_or_corrupt(e.to_string(), format),
        ParseError::HwpxError(e) => {
            let msg = e.to_string();
            if msg.contains("암호") || msg.to_ascii_lowercase().contains("encrypt") {
                ExtractReport::new(ExtractStatus::Unsupported {
                    reason: UnsupportedReason::Encrypted,
                    detail: msg,
                })
            } else {
                map_limit_or_corrupt(msg, format)
            }
        }
        ParseError::HeaderError(e) => ExtractReport::new(ExtractStatus::Corrupt {
            detail: e.to_string(),
        }),
        ParseError::DocInfoError(e) => ExtractReport::new(ExtractStatus::Corrupt {
            detail: e.to_string(),
        }),
        ParseError::BodyTextError(e) => ExtractReport::new(ExtractStatus::Corrupt {
            detail: e.to_string(),
        }),
        ParseError::Hwp3Error(e) => ExtractReport::new(ExtractStatus::Unsupported {
            reason: UnsupportedReason::Hwp3,
            detail: e.to_string(),
        }),
        ParseError::HmlError(e) => ExtractReport::new(ExtractStatus::Unsupported {
            reason: UnsupportedReason::Hml,
            detail: e.to_string(),
        }),
    }
}

fn map_limit_or_corrupt(msg: String, _format: DocFormat) -> ExtractReport {
    let lower = msg.to_ascii_lowercase();
    if lower.contains("limit")
        || lower.contains("bomb")
        || lower.contains("exceed")
        || msg.contains("상한")
    {
        ExtractReport::new(ExtractStatus::ResourceLimit {
            kind: LimitKind::Decompress,
            detail: msg,
        })
    } else if lower.contains("viewtext") || msg.contains("배포") {
        ExtractReport::new(ExtractStatus::Unsupported {
            reason: UnsupportedReason::Distribution,
            detail: msg,
        })
    } else {
        ExtractReport::new(ExtractStatus::Corrupt { detail: msg })
    }
}
