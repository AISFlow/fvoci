//! `xtask prepare-rustup-ci-metadata` as a child process outside an
//! allocated GitHub job: it refuses with a fixed reason before touching any
//! path, and never echoes the output path.

use std::process::{Command, Stdio};

fn xtask(args: &[&str]) -> std::process::Output {
    Command::new(env!("CARGO_BIN_EXE_xtask"))
        .arg("prepare-rustup-ci-metadata")
        .args(args)
        .env("CI", "false")
        .env("GITHUB_ACTIONS", "false")
        .stdin(Stdio::null())
        .output()
        .unwrap()
}

#[test]
fn local_environment_refuses_without_echoing_paths() {
    let output = xtask(&["--output", "/unused-owned-fixture-output"]);
    assert_eq!(output.status.code(), Some(1));
    assert!(output.stdout.is_empty());
    assert_eq!(
        String::from_utf8(output.stderr).unwrap(),
        "Rustup CI metadata preparation refused: not-github-ci\n"
    );
    assert!(!std::path::Path::new("/unused-owned-fixture-output").exists());
}

#[test]
fn usage_errors_exit_two_and_help_exits_zero() {
    for args in [
        &[][..],
        &["--output"],
        &["--out", "x"],
        &["--output", "x", "y"],
    ] {
        let output = xtask(args);
        assert_eq!(output.status.code(), Some(2), "{args:?}");
        assert!(output.stdout.is_empty());
        assert!(String::from_utf8(output.stderr)
            .unwrap()
            .starts_with("usage: cargo xtask prepare-rustup-ci-metadata"));
    }
    let output = xtask(&["--help"]);
    assert_eq!(output.status.code(), Some(0));
    assert!(String::from_utf8(output.stdout)
        .unwrap()
        .contains("--output OUTPUT"));
}
