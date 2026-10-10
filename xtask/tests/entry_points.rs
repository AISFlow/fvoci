//! Real child runs of the unchanged caller entry points
//! `bash scripts/prepare-sqlite-build.sh` and `bash scripts/prepare-sqlite-ci.sh`:
//! argv, exit codes, stdout/stderr split, signal delivery through `exec`, and
//! a hostile archive failing before anything is written.

use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::os::unix::process::ExitStatusExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};
use std::time::{Duration, Instant};

fn repo() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .to_path_buf()
}

fn temp(prefix: &str) -> PathBuf {
    xtask::host::mkdtemp(prefix, &std::env::temp_dir()).unwrap()
}

fn script(name: &str) -> Command {
    let mut command = Command::new("bash");
    command
        .arg(repo().join("scripts").join(name))
        .env_remove("SQLITE3_LIB_DIR")
        .env_remove("SQLITE3_INCLUDE_DIR")
        .env_remove("SQLITE3_STATIC")
        .env_remove("SQLITE3_NO_PKG_CONFIG")
        .env_remove("GITHUB_JOB")
        .stdin(Stdio::null());
    command
}

fn run(command: &mut Command) -> Output {
    command.output().unwrap()
}

#[test]
fn build_entry_usage_and_help() {
    let out = run(&mut script("prepare-sqlite-build.sh"));
    assert_eq!(out.status.code(), Some(2));
    assert!(out.stdout.is_empty());
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("the following arguments are required: --archive, --prefix, --target"),
        "{stderr}"
    );
    let out = run(script("prepare-sqlite-build.sh").arg("--help"));
    assert_eq!(out.status.code(), Some(0));
    assert!(String::from_utf8_lossy(&out.stdout).starts_with("usage:"));
}

#[test]
fn build_entry_rejects_hostile_archive_before_writing() {
    let dir = temp("fvoci-sqlite-entry-");
    for fixture in [
        "hand/cases/03_dup6.zip",
        "hand/cases/60_symlink.zip",
        "probe/G5-duplicate-entry-count6.zip",
    ] {
        let archive = dir.join("archive.zip");
        fs::copy(
            repo().join("xtask/tests/fixtures/sqlite-zip").join(fixture),
            &archive,
        )
        .unwrap();
        let prefix = dir.join("x86_64-unknown-linux-gnu");
        let out = run(script("prepare-sqlite-build.sh").args([
            "--archive".as_ref(),
            archive.as_os_str(),
            "--prefix".as_ref(),
            prefix.as_os_str(),
            "--target".as_ref(),
            "x86_64-unknown-linux-gnu".as_ref(),
        ]));
        assert_eq!(out.status.code(), Some(1), "{fixture}");
        assert!(out.stdout.is_empty());
        assert_eq!(
            String::from_utf8_lossy(&out.stderr),
            "prepare-sqlite-build: archive SHA-256 mismatch; no source extraction/build/fallback\n"
        );
        let names: Vec<_> = fs::read_dir(&dir)
            .unwrap()
            .map(|e| e.unwrap().file_name())
            .collect();
        assert_eq!(names, ["archive.zip"], "{fixture}");
        fs::remove_file(&archive).unwrap();
    }
    fs::remove_dir_all(dir).unwrap();
}

#[test]
fn build_entry_refuses_symlinked_archive_path() {
    let dir = temp("fvoci-sqlite-entry-");
    fs::write(dir.join("real.zip"), b"x").unwrap();
    std::os::unix::fs::symlink(dir.join("real.zip"), dir.join("link.zip")).unwrap();
    let out = run(script("prepare-sqlite-build.sh").args([
        "--archive".as_ref(),
        dir.join("link.zip").as_os_str(),
        "--prefix".as_ref(),
        dir.join("p").as_os_str(),
        "--target".as_ref(),
        "x86_64-unknown-linux-gnu".as_ref(),
    ]));
    assert_eq!(out.status.code(), Some(1));
    assert!(String::from_utf8_lossy(&out.stderr).contains("symlink path refused"));
    fs::remove_dir_all(dir).unwrap();
}

fn tool(bin: &Path, name: &str, body: &str) {
    let path = bin.join(name);
    fs::write(&path, format!("#!/usr/bin/env bash\n{body}")).unwrap();
    fs::set_permissions(&path, fs::Permissions::from_mode(0o755)).unwrap();
}

/// ci entry with stub curl/rustc on PATH and the real helper: the wrong
/// downloaded bytes fail the pin, exit 1, and no GitHub file is written.
#[test]
fn ci_entry_rejects_wrong_source_and_writes_no_exports() {
    let dir = temp("fvoci-sqlite-entry-");
    let (bin, clang, parent) = (dir.join("bin"), dir.join("clang"), dir.join("owned parent"));
    for d in [&bin, &clang, &parent] {
        fs::create_dir(d).unwrap();
    }
    fs::write(clang.join("libclang.so"), b"fixture").unwrap();
    tool(
        &bin,
        "curl",
        "while [ $# -gt 0 ]; do [ \"$1\" != --output ] || printf wrong >\"$2\"; shift; done\n",
    );
    let github_env = dir.join("github-env");
    let path = format!("{}:{}", bin.display(), std::env::var("PATH").unwrap());
    let out = run(script("prepare-sqlite-ci.sh")
        .env("PATH", path)
        .env("LIBCLANG_PATH", &clang)
        .env_remove("CARGO_BUILD_TARGET")
        .args([
            "--parent".as_ref(),
            parent.as_os_str(),
            "--github-env".as_ref(),
            github_env.as_os_str(),
        ])
        .args(["--", "false"]));
    assert_eq!(out.status.code(), Some(1));
    assert!(out.stdout.is_empty());
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(stderr.contains("archive SHA-256 mismatch"), "{stderr}");
    assert!(!github_env.exists());
    assert!(!parent.join("x86_64-unknown-linux-gnu").exists());
    fs::remove_dir_all(dir).unwrap();
}

#[test]
fn ci_entry_usage_error_exits_two() {
    let out = run(script("prepare-sqlite-ci.sh").arg("--no-such-option"));
    assert_eq!(out.status.code(), Some(2));
    assert!(out.stdout.is_empty());
}

/// Consumer-directed Cargo overrides do not reach the xtask build itself.
#[test]
fn consumer_cargo_overrides_do_not_break_the_xtask_build() {
    let dir = temp("fvoci-sqlite-entry-");
    let out = run(script("prepare-sqlite-ci.sh")
        .arg("--no-such-option")
        .env("CARGO_BUILD_TARGET", "unsupported-cross-target")
        .env("CARGO_TARGET_DIR", dir.join("foreign-target"))
        .env("RUSTFLAGS", "--definitely-not-a-rustc-flag")
        .env("CARGO_ENCODED_RUSTFLAGS", "--definitely-not-a-rustc-flag"));
    assert_eq!(
        out.status.code(),
        Some(2),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(!dir.join("foreign-target").exists());
    fs::remove_dir_all(dir).unwrap();
}

/// `exec` leaves no shell between the caller and xtask: a signal sent to the
/// spawned pid reaches the task itself and is reported as a signal death.
#[test]
fn ci_entry_signal_reaches_task_directly() {
    let dir = temp("fvoci-sqlite-entry-");
    let (bin, clang, parent) = (dir.join("bin"), dir.join("clang"), dir.join("parent"));
    for d in [&bin, &clang, &parent] {
        fs::create_dir(d).unwrap();
    }
    fs::write(clang.join("libclang.so"), b"fixture").unwrap();
    let started = dir.join("started");
    tool(
        &bin,
        "curl",
        &format!("touch '{}'\nexec sleep 30\n", started.display()),
    );
    let path = format!("{}:{}", bin.display(), std::env::var("PATH").unwrap());
    let mut child = script("prepare-sqlite-ci.sh")
        .env("PATH", path)
        .env("LIBCLANG_PATH", &clang)
        .env_remove("CARGO_BUILD_TARGET")
        .args(["--parent".as_ref(), parent.as_os_str()])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
    let deadline = Instant::now() + Duration::from_secs(120);
    while !started.exists() {
        assert!(Instant::now() < deadline, "download stub never started");
        assert!(child.try_wait().unwrap().is_none(), "entry exited early");
        std::thread::sleep(Duration::from_millis(20));
    }
    let comm = fs::read_to_string(format!("/proc/{}/comm", child.id())).unwrap();
    assert_eq!(comm.trim(), "xtask");
    // SAFETY: plain kill(2) on our own unreaped child.
    assert_eq!(unsafe { libc::kill(child.id() as i32, libc::SIGTERM) }, 0);
    let status = child.wait().unwrap();
    assert_eq!(status.signal(), Some(libc::SIGTERM));
    fs::remove_dir_all(dir).unwrap();
}
