//! Real child runs of `xtask selected-library` against a stand-in `cargo` on
//! PATH and a stand-in library test executable: one build, every registered
//! filter run alone with `--exact` from the package directory, stdout and
//! stderr checked as one stream, and fail-closed exits for zero-match,
//! ignored, failed, signalled and unbuilt controls.

use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};
use xtask::selected_library::{filters, FILTERS_FILE};

struct Temp(PathBuf);

impl Drop for Temp {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn executable(path: &Path, body: &str) {
    fs::write(path, body).unwrap();
    fs::set_permissions(path, fs::Permissions::from_mode(0o755)).unwrap();
}

/// A checkout holding a copy of the registered filter list. `bin/cargo` logs
/// `argv|CARGO_PKG_NAME|CARGO_MANIFEST_DIR` and prints the records
/// `CARGO_RECORDS` names; `test-exe` logs
/// `filter|flag|cwd|CARGO_MANIFEST_DIR|CARGO_PKG_NAME` and answers like
/// libtest: a registered name passes, any other name matches no test, and
/// `FAKE_AT` gets the `FAKE_MODE` answer.
fn fixture() -> Temp {
    let root = xtask::host::mkdtemp("selected-library-", &std::env::temp_dir()).unwrap();
    let root = root.canonicalize().unwrap();
    fs::create_dir_all(root.join("bin")).unwrap();
    fs::create_dir_all(root.join("pkg")).unwrap();
    fs::create_dir_all(root.join("xtask")).unwrap();
    fs::write(root.join(FILTERS_FILE), registered_text()).unwrap();
    fs::write(root.join("known"), registered_text()).unwrap();
    let lib = format!(
        r#"{{"reason":"compiler-artifact","manifest_path":"{0}/pkg/Cargo.toml","target":{{"kind":["lib"],"name":"fvoci_server"}},"profile":{{"test":true}},"executable":"{0}/test-exe"}}"#,
        root.display()
    );
    let bin = r#"{"reason":"compiler-artifact","manifest_path":"/x/Cargo.toml","target":{"kind":["bin"],"name":"b"},"profile":{"test":true},"executable":"/x/b"}"#;
    let finished = r#"{"reason":"build-finished","success":true}"#;
    for (name, lines) in [
        ("ok", vec![bin, &lib, finished]),
        ("two", vec![&lib, &lib, finished]),
        ("none", vec![bin, finished]),
        ("unfinished", vec![&lib]),
    ] {
        fs::write(root.join(name), lines.join("\n") + "\n").unwrap();
    }
    executable(
        &root.join("bin/cargo"),
        r#"#!/bin/sh
printf '%s|%s|%s\n' "$*" "${CARGO_PKG_NAME-unset}" "${CARGO_MANIFEST_DIR-unset}" >> "$FAKE_DIR/cargo.log"
echo 'Compiling fvoci-server (stand-in)' >&2
cat "$FAKE_DIR/$CARGO_RECORDS"
exit "${CARGO_EXIT:-0}"
"#,
    );
    executable(
        &root.join("test-exe"),
        r#"#!/bin/sh
printf '%s|%s|%s|%s|%s\n' "$1" "$2" "$(pwd)" "$CARGO_MANIFEST_DIR" "${CARGO_PKG_NAME-unset}" >> "$FAKE_DIR/runs.log"
mode=zero
grep -qxF -- "$1" "$FAKE_DIR/known" && mode=pass
[ "$1" = "$FAKE_AT" ] && mode="$FAKE_MODE"
case "$mode" in
  zero) printf 'running 0 tests\n\ntest result: ok. 0 passed; 0 failed; 0 ignored; 0 measured; 9 filtered out; finished in 0.00s\n'; exit 0 ;;
  ignored) printf 'running 1 test\ntest %s ... ignored\n\ntest result: ok. 0 passed; 0 failed; 1 ignored; 0 measured; 8 filtered out; finished in 0.00s\n' "$1"; exit 0 ;;
  failed) printf 'running 1 test\ntest %s ... FAILED\n\ntest result: FAILED. 0 passed; 1 failed; 0 ignored; 0 measured; 8 filtered out; finished in 0.00s\n' "$1"; exit 101 ;;
  signal) printf 'running 1 test\n'; kill -9 $$ ;;
  binary) printf 'running 1 test\n\377\n'; exit 0 ;;
esac
# libtest's count line on stderr: the check reads one merged stream.
printf 'running 1 test\n' >&2
printf 'test %s ... ok\n\ntest result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 8 filtered out; finished in 0.01s\n' "$1"
"#,
    );
    Temp(root)
}

fn xtask(fixture: &Temp, records: &str, envs: &[(&str, &str)], args: &[&str]) -> Output {
    let path = format!(
        "{}:{}",
        fixture.0.join("bin").display(),
        std::env::var("PATH").unwrap()
    );
    let mut command = Command::new(env!("CARGO_BIN_EXE_xtask"));
    command
        .arg("selected-library")
        .args(args)
        .env("PATH", path)
        .env("FAKE_DIR", &fixture.0)
        .env("CARGO_RECORDS", records)
        .env_remove("FAKE_AT")
        .current_dir(&fixture.0)
        .stdin(Stdio::null());
    for (key, value) in envs {
        command.env(key, value);
    }
    command.output().unwrap()
}

fn log(fixture: &Temp, name: &str) -> Vec<String> {
    fs::read_to_string(fixture.0.join(name))
        .unwrap_or_default()
        .lines()
        .map(str::to_owned)
        .collect()
}

fn registered_text() -> String {
    let repo = Path::new(env!("CARGO_MANIFEST_DIR")).parent().unwrap();
    fs::read_to_string(repo.join(FILTERS_FILE)).unwrap()
}

fn registered() -> Vec<String> {
    filters(&registered_text())
        .unwrap()
        .into_iter()
        .map(str::to_owned)
        .collect()
}

#[test]
fn one_build_then_every_filter_exactly_once_from_the_package_directory() {
    let fixture = fixture();
    // As under `cargo xtask`: the enclosing `cargo run` set xtask's own package variables.
    let xtask_dir = "/repo/xtask";
    let out = xtask(
        &fixture,
        "ok",
        &[
            ("CARGO_MANIFEST_DIR", xtask_dir),
            ("CARGO_PKG_NAME", "xtask"),
        ],
        &["--target-dir", "target/db-lib"],
    );
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert_eq!(out.status.code(), Some(0), "{stderr}");
    assert_eq!(
        log(&fixture, "cargo.log"),
        ["test --locked --offline --features db-tests --lib --no-run --message-format=json-render-diagnostics --target-dir target/db-lib|unset|unset"]
    );
    let package = fixture.0.join("pkg");
    let expected: Vec<String> = registered()
        .iter()
        .map(|f| format!("{f}|--exact|{0}|{0}|unset", package.display()))
        .collect();
    assert_eq!(log(&fixture, "runs.log"), expected);
    // Each control's output is copied to stdout; Cargo's own stderr stays on stderr.
    let stdout = String::from_utf8(out.stdout).unwrap();
    for filter in registered() {
        assert!(
            stdout.contains(&format!("test {filter} ... ok\n")),
            "{filter}"
        );
    }
    assert_eq!(
        stdout.matches("running 1 test\n").count(),
        registered().len()
    );
    assert!(stderr.contains("Compiling fvoci-server"), "{stderr}");
}

#[test]
fn without_target_dir_cargo_keeps_its_own_target_selection() {
    let fixture = fixture();
    let out = xtask(&fixture, "ok", &[], &[]);
    assert_eq!(out.status.code(), Some(0));
    assert_eq!(
        log(&fixture, "cargo.log"),
        ["test --locked --offline --features db-tests --lib --no-run --message-format=json-render-diagnostics|unset|unset"]
    );
}

#[test]
fn a_control_that_is_not_an_exact_pass_fails_and_stops_the_run() {
    let list = registered();
    let at = list[9].as_str();
    for (mode, message) in [
        (
            "zero",
            "rust: selected library command must run exactly one test",
        ),
        (
            "ignored",
            "rust: selected library result must be the exact requested test and ok",
        ),
        ("failed", "rust: selected library test command failed"),
        ("signal", "rust: selected library test command failed"),
        ("binary", "rust: selected library output is not UTF-8"),
    ] {
        let fixture = fixture();
        let out = xtask(&fixture, "ok", &[("FAKE_AT", at), ("FAKE_MODE", mode)], &[]);
        let stderr = String::from_utf8_lossy(&out.stderr);
        assert_eq!(out.status.code(), Some(1), "{mode}: {stderr}");
        assert!(
            stderr.contains(&format!("{message}: {at}\n")),
            "{mode}: {stderr}"
        );
        let runs = log(&fixture, "runs.log");
        assert_eq!(runs.len(), 10, "{mode}: {runs:?}");
        assert!(runs[9].starts_with(&format!("{at}|--exact|")), "{mode}");
    }
}

#[test]
fn a_registered_filter_that_matches_no_test_fails_the_run() {
    // The list is the checkout's file: a renamed entry still runs, and the
    // libtest stand-in finds no such test.
    let fixture = fixture();
    let mut list = registered();
    list[20] = format!("{}_renamed", list[20]);
    fs::write(fixture.0.join(FILTERS_FILE), list.join("\n") + "\n").unwrap();
    let out = xtask(&fixture, "ok", &[], &[]);
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert_eq!(out.status.code(), Some(1), "{stderr}");
    assert!(
        stderr.contains(&format!(
            "rust: selected library command must run exactly one test: {}\n",
            list[20]
        )),
        "{stderr}"
    );
    assert_eq!(log(&fixture, "runs.log").len(), 21);
}

#[test]
fn a_missing_or_malformed_filter_list_refuses_before_the_build() {
    let fixture = fixture();
    let path = fixture.0.join(FILTERS_FILE);
    let good = registered_text();
    for (text, message) in [
        (None, "xtask/selected-library-filters.txt: No such file"),
        (Some(String::new()), "must end with a newline"),
        (Some(good.replace('\n', "\r\n")), "is not a test path"),
        (Some(format!("{good}{good}")), "is repeated"),
        (Some(format!("\u{feff}{good}")), "is not a test path"),
    ] {
        match &text {
            Some(text) => fs::write(&path, text).unwrap(),
            None => fs::remove_file(&path).unwrap(),
        }
        let out = xtask(&fixture, "ok", &[], &[]);
        let stderr = String::from_utf8_lossy(&out.stderr);
        assert_eq!(out.status.code(), Some(1), "{stderr}");
        assert!(stderr.contains(message), "{message}: {stderr}");
    }
    fs::write(&path, b"a::b\n\xff\n").unwrap();
    let out = xtask(&fixture, "ok", &[], &[]);
    assert!(String::from_utf8_lossy(&out.stderr).contains("is not UTF-8"));
    assert!(log(&fixture, "cargo.log").is_empty());
    assert!(log(&fixture, "runs.log").is_empty());
}

#[test]
fn build_failures_and_ambiguous_records_run_no_control() {
    for (records, envs, message) in [
        (
            "ok",
            &[("CARGO_EXIT", "101")][..],
            "rust: selected library build failed with status 101",
        ),
        (
            "two",
            &[][..],
            "exactly one library test executable, found 2",
        ),
        (
            "none",
            &[][..],
            "exactly one library test executable, found 0",
        ),
        ("unfinished", &[][..], "did not finish successfully"),
    ] {
        let fixture = fixture();
        let out = xtask(&fixture, records, envs, &[]);
        let stderr = String::from_utf8_lossy(&out.stderr);
        assert_eq!(out.status.code(), Some(1), "{records}: {stderr}");
        assert!(stderr.contains(message), "{records}: {stderr}");
        assert!(log(&fixture, "runs.log").is_empty(), "{records}");
    }
}

#[test]
fn a_missing_test_executable_is_a_spawn_failure() {
    let fixture = fixture();
    fs::remove_file(fixture.0.join("test-exe")).unwrap();
    let out = xtask(&fixture, "ok", &[], &[]);
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert_eq!(out.status.code(), Some(1), "{stderr}");
    assert!(stderr.contains("test-exe: No such file"), "{stderr}");
}

#[test]
fn usage_errors_exit_2_and_help_exits_0() {
    let fixture = fixture();
    for args in [
        &["extra"][..],
        &["--target-dir"][..],
        &["--target-dir", "a", "--unknown"][..],
        &["--target", "a"][..],
    ] {
        let out = xtask(&fixture, "ok", &[], args);
        assert_eq!(out.status.code(), Some(2), "{args:?}");
        assert!(String::from_utf8_lossy(&out.stderr).starts_with("usage: xtask selected-library"));
    }
    let out = xtask(&fixture, "ok", &[], &["--help"]);
    assert_eq!(out.status.code(), Some(0));
    assert!(String::from_utf8_lossy(&out.stdout).starts_with("usage: xtask selected-library"));
    assert!(log(&fixture, "cargo.log").is_empty());
}
