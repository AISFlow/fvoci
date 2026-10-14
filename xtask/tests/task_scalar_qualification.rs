use serde_json::Value;
use std::process::Command;

#[test]
fn measure_reports_the_actual_native_runtime_and_rejects_extra_arguments() {
    let executable = env!("CARGO_BIN_EXE_xtask");
    let output = Command::new(executable)
        .args(["task-scalar-qualification", "measure"])
        .output()
        .unwrap();
    assert!(output.status.success(), "{:?}", output);
    let value: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(value["arch"], std::env::consts::ARCH);
    assert_eq!(
        value["locale_archive_bytes"],
        std::fs::metadata("/usr/lib/locale/locale-archive")
            .unwrap()
            .len()
    );
    assert_eq!(value["locale_archive_sha256"].as_str().unwrap().len(), 64);
    // SAFETY: GNU returns a static NUL-terminated string.
    let version = unsafe { std::ffi::CStr::from_ptr(libc::gnu_get_libc_version()) };
    assert_eq!(value["glibc_version"], version.to_str().unwrap());
    let output = Command::new(executable)
        .args(["task-scalar-qualification", "measure", "--unqualified"])
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(2));
    assert!(output.stdout.is_empty());
}

#[test]
fn qualification_without_its_inputs_fails_closed_without_a_receipt() {
    let directory = std::env::temp_dir().join(format!(
        "fvoci-task-scalar-never-created-{}",
        std::process::id()
    ));
    let output = Command::new(env!("CARGO_BIN_EXE_xtask"))
        .args([
            "task-scalar-qualification",
            "bootstrap",
            "--image",
            "never-started",
        ])
        .arg("--directory")
        .arg(&directory)
        .args([
            "--target-dir",
            "never-built",
            "--sqlite-inputs",
            "never-read",
        ])
        .output()
        .unwrap();
    // A native host now proceeds past the architecture check, but no receipt
    // directory exists without a clean checkout and its verified install image.
    assert_eq!(output.status.code(), Some(1));
    assert!(output.stdout.is_empty());
    assert!(!directory.exists());
}
