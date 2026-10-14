//! The dispatched CLI must reject missing real inputs before reading a receipt
//! or starting a test. No product URL/credential is needed for these refusals.
use std::path::Path;
use std::process::Command;
use xtask::native_library::{input_names, COHORTS};

#[test]
fn each_missing_required_input_refuses_before_a_test_or_receipt_is_opened() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).parent().unwrap();
    for (cohort, _) in COHORTS {
        for missing in input_names(cohort) {
            let mut command = Command::new(env!("CARGO_BIN_EXE_xtask"));
            command.env_clear().current_dir(root).args([
                "native-library",
                "run",
                "--cohort",
                cohort,
                "--directory",
                "absent-receipt",
                "--sqlite-identity",
                &"b".repeat(64),
            ]);
            for key in input_names(cohort) {
                if key != missing {
                    command.env(key, "fixture-must-never-be-opened");
                }
            }
            let output = command.output().unwrap();
            assert_eq!(output.status.code(), Some(1));
            assert!(
                String::from_utf8_lossy(&output.stderr)
                    .contains(&format!("missing required input: {missing}")),
                "{cohort} {missing}: {:?}",
                output.stderr
            );
            assert!(output.stdout.is_empty());
        }
    }
}

#[test]
fn unknown_cohort_and_unregistered_arguments_are_refused() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).parent().unwrap();
    for argv in [
        vec![
            "native-library",
            "run",
            "--cohort",
            "unknown",
            "--directory",
            "absent",
            "--sqlite-identity",
            "b",
        ],
        vec!["native-library", "run", "--test", "unknown"],
        vec![
            "native-library",
            "prepare-sqld",
            "--directory",
            "absent",
            "--cohort",
            "sqld",
        ],
    ] {
        let output = Command::new(env!("CARGO_BIN_EXE_xtask"))
            .env_clear()
            .current_dir(root)
            .args(argv)
            .output()
            .unwrap();
        assert!(!output.status.success());
        assert!(output.stdout.is_empty());
    }
}
