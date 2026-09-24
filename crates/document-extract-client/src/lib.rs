//! Killable extract process client and canonical wire types.
//!
//! This crate does not depend on `rhwp`, zip, CFB, or renderers. Parser-side
//! `extract_in_process` stays in `document-extract`. Attachment HTTP is out of
//! scope.

/// Authoritative native parser revision carried on every [`ExtractReport`].
pub const RHWP_REV: &str = "e8800c8def63449808a4092798442652ed460552";
pub const RHWP_REPO: &str = "https://github.com/edwardkim/rhwp";
pub const RHWP_LICENSE: &str = "MIT";

pub mod limits;
pub mod outcome;
pub mod process;

pub use limits::Limits;
pub use outcome::{
    DocFormat, ExtractReport, ExtractStatus, LimitKind, UnsupportedReason, WorkerFailureReason,
};
pub use process::{apply_rlimits_now, extract_killable, ExtractRequest};
#[cfg(feature = "test-hang")]
pub use process::{take_last_spawn, SpawnTrace};

#[cfg(test)]
mod tests {
    use super::{
        extract_killable, ExtractReport, ExtractRequest, ExtractStatus, Limits,
        WorkerFailureReason, RHWP_LICENSE, RHWP_REPO, RHWP_REV,
    };
    use std::path::PathBuf;

    #[test]
    fn rhwp_revision_constants_are_the_accepted_pin() {
        assert_eq!(RHWP_REV, "e8800c8def63449808a4092798442652ed460552");
        assert_eq!(RHWP_REPO, "https://github.com/edwardkim/rhwp");
        assert_eq!(RHWP_LICENSE, "MIT");
        let report = ExtractReport::new(ExtractStatus::Corrupt { detail: "x".into() });
        assert_eq!(report.rhwp_rev, RHWP_REV);
        let json = serde_json::to_string(&report).expect("json");
        assert!(json.contains(RHWP_REV), "{json}");
        assert!(json.contains("\"status\":\"corrupt\""), "{json}");
        assert_eq!(
            serde_json::from_str::<ExtractReport>(&json).expect("roundtrip"),
            report
        );
    }

    #[test]
    fn missing_extractor_path_is_worker_failure() {
        let report = extract_killable(ExtractRequest {
            bytes: b"not-a-document".to_vec(),
            name: "x.hwp".into(),
            limits: Limits::for_tests(),
            extractor_bin: PathBuf::from("/no/such/document-extract"),
            test_hang_ms: None,
        });
        assert!(
            matches!(
                report.outcome,
                ExtractStatus::WorkerFailure {
                    reason: WorkerFailureReason::MissingExecutable,
                    ..
                }
            ),
            "{:?}",
            report.outcome
        );
        assert!(report.child_pid.is_none());
    }

    #[test]
    fn zero_timeout_is_invalid_limits() {
        let mut limits = Limits::for_tests();
        limits.timeout_ms = 0;
        let report = extract_killable(ExtractRequest {
            bytes: Vec::new(),
            name: "x.hwp".into(),
            limits,
            extractor_bin: PathBuf::from("/no/such/document-extract"),
            test_hang_ms: None,
        });
        assert!(
            matches!(
                report.outcome,
                ExtractStatus::WorkerFailure {
                    reason: WorkerFailureReason::InvalidLimits,
                    ..
                }
            ),
            "{:?}",
            report.outcome
        );
    }
}
