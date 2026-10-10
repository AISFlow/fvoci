//! `xtask selected-install` as a real non-root process: every artifact
//! selection and file refusal happens before any ownership change, and a
//! healthy selection still refuses without root. The root staging and the
//! actual product test run are exercised under sudo outside `cargo test`.

use serde_json::{json, Value};
use std::fs;
use std::os::unix::fs::{symlink, MetadataExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

struct Temp(PathBuf);

impl Drop for Temp {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn temp() -> (Temp, PathBuf) {
    let dir = xtask::host::mkdtemp("selected-install-", &std::env::temp_dir()).unwrap();
    let root = dir.canonicalize().unwrap();
    (Temp(root.clone()), root)
}

fn executable(path: &Path) {
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, "#!/bin/sh\nexit 0\n").unwrap();
    fs::set_permissions(path, fs::Permissions::from_mode(0o644)).unwrap();
}

fn record(name: &str, kind: &str, test: bool, path: &Path) -> Value {
    json!({"reason": "compiler-artifact", "target": {"name": name, "kind": [kind]},
           "profile": {"test": test}, "features": ["db-tests"], "executable": path.to_str().unwrap()})
}

/// Three executables and the helper on disk with their Cargo records.
fn healthy(root: &Path) -> (Vec<Value>, PathBuf) {
    let bin = root.join("target/db-tests/debug");
    let files = [
        ("fvoci-server", "bin", false, bin.join("fvoci-server")),
        ("fvoci-migrate", "bin", false, bin.join("fvoci-migrate")),
        (
            "selected_install_lifetime",
            "test",
            true,
            bin.join("deps/selected_install_lifetime"),
        ),
    ];
    let records = files
        .iter()
        .map(|(name, kind, test, path)| {
            executable(path);
            record(name, kind, *test, path)
        })
        .collect();
    let engine = root.join("crates/collab-engine/target/debug/collab-engine");
    executable(&engine);
    (records, engine)
}

fn run(root: &Path, records: &[Value], engine: &Path) -> Output {
    let lines: Vec<String> = records.iter().map(Value::to_string).collect();
    let artifacts = root.join("selected-install-build.jsonl");
    fs::write(&artifacts, lines.join("\n")).unwrap();
    Command::new(env!("CARGO_BIN_EXE_xtask"))
        .arg("selected-install")
        .current_dir(root)
        .arg(&artifacts)
        .arg(engine)
        .output()
        .unwrap()
}

/// Every fixture file keeps its owner and 0644 mode.
fn untouched(root: &Path) {
    let mut pending = vec![root.to_path_buf()];
    while let Some(dir) = pending.pop() {
        for entry in fs::read_dir(dir).unwrap() {
            let path = entry.unwrap().path();
            let meta = fs::symlink_metadata(&path).unwrap();
            if meta.is_dir() {
                pending.push(path);
            } else if meta.is_file() && path.extension().is_none() {
                assert_eq!(meta.mode() & 0o7777, 0o644, "{}", path.display());
                assert_eq!(meta.uid(), xtask::host::getuid());
            }
        }
    }
}

fn stderr(output: &Output) -> String {
    String::from_utf8(output.stderr.clone()).unwrap()
}

fn not_root() {
    // SAFETY: geteuid has no preconditions.
    assert_ne!(
        unsafe { libc::geteuid() },
        0,
        "run these controls as a non-root user"
    );
}

#[test]
fn artifact_selection_refusals_change_nothing() {
    not_root();
    type Mutation = fn(&mut Vec<Value>, &Path);
    let cases: [(Mutation, &str); 8] = [
        (
            |r, _| drop(r.remove(0)),
            "fvoci-server: 0 matching bin artifacts",
        ),
        (
            |r, _| r.push(r[1].clone()),
            "fvoci-migrate: 2 matching bin artifacts",
        ),
        (
            |r, _| drop(r.remove(2)),
            "selected_install_lifetime: 0 matching test artifacts",
        ),
        (
            |r, _| r[2]["features"] = json!(["default"]),
            "not built with db-tests",
        ),
        (
            |r, root| {
                let other = root.join("other-bin/fvoci-migrate");
                executable(&other);
                r[1]["executable"] = json!(other.to_str().unwrap());
            },
            "siblings",
        ),
        (
            |r, _| r[0]["executable"] = json!("target/db-tests/debug/fvoci-server"),
            "siblings",
        ),
        (
            |r, _| {
                r[0]["executable"] = json!("fvoci-server");
                r[1]["executable"] = json!("fvoci-migrate");
            },
            "is not absolute",
        ),
        (
            |r, _| r.push(json!({"reason": "compiler-artifact", "executable": 5})),
            "executable is not a string",
        ),
    ];
    for (mutate, message) in cases {
        let (_temp, root) = temp();
        let (mut records, engine) = healthy(&root);
        mutate(&mut records, &root);
        let output = run(&root, &records, &engine);
        assert_eq!(output.status.code(), Some(1), "{message}");
        assert!(
            stderr(&output).contains(message),
            "{message}: {}",
            stderr(&output)
        );
        untouched(&root);
    }
}

#[test]
fn symlinked_missing_or_nonregular_files_refuse_before_ownership_changes() {
    not_root();
    type Setup = fn(&Path, &Path) -> PathBuf;
    let cases: [(Setup, &str); 5] = [
        (
            |root, _| {
                let fifo = root.join("engine-fifo");
                let name = std::ffi::CString::new(fifo.to_str().unwrap()).unwrap();
                // SAFETY: name is a valid NUL-terminated path.
                assert_eq!(unsafe { libc::mkfifo(name.as_ptr(), 0o644) }, 0);
                fifo
            },
            "engine-fifo is not a regular file",
        ),
        (
            |root, _| {
                let test = root.join("target/db-tests/debug/deps/selected_install_lifetime");
                let real = root.join("real-test");
                fs::rename(&test, &real).unwrap();
                symlink(&real, &test).unwrap();
                root.join("crates/collab-engine/target/debug/collab-engine")
            },
            "selected_install_lifetime",
        ),
        (
            |root, engine| {
                let real = root.join("real-engine");
                fs::rename(engine, &real).unwrap();
                symlink(&real, engine).unwrap();
                engine.to_path_buf()
            },
            "collab-engine",
        ),
        (|root, _| root.join("missing-engine"), "missing-engine"),
        (|root, _| root.join("target"), "is not a regular file"),
    ];
    for (setup, message) in cases {
        let (_temp, root) = temp();
        let (records, engine) = healthy(&root);
        let engine = setup(&root, &engine);
        let output = run(&root, &records, &engine);
        assert_eq!(output.status.code(), Some(1), "{message}");
        assert!(
            stderr(&output).contains(message),
            "{message}: {}",
            stderr(&output)
        );
        untouched(&root);
    }
}

#[test]
fn healthy_selection_still_requires_root_and_changes_nothing() {
    not_root();
    let (_temp, root) = temp();
    let (records, engine) = healthy(&root);
    let output = run(&root, &records, &engine);
    assert_eq!(output.status.code(), Some(1));
    assert!(
        stderr(&output).contains("must run as root"),
        "{}",
        stderr(&output)
    );
    untouched(&root);
}

#[test]
fn malformed_or_missing_artifact_lists_refuse() {
    not_root();
    let (_temp, root) = temp();
    let (records, engine) = healthy(&root);
    let mut lines: Vec<String> = records.iter().map(Value::to_string).collect();
    lines.insert(1, String::new());
    let artifacts = root.join("blank-line.jsonl");
    fs::write(&artifacts, lines.join("\n")).unwrap();
    for path in [artifacts, root.join("absent.jsonl")] {
        let output = Command::new(env!("CARGO_BIN_EXE_xtask"))
            .arg("selected-install")
            .arg(&path)
            .current_dir(&root)
            .arg(&engine)
            .output()
            .unwrap();
        assert_eq!(output.status.code(), Some(1), "{}", path.display());
    }
    untouched(&root);
}

#[test]
fn usage_errors_exit_2() {
    for argv in [
        &[][..],
        &["only.jsonl"][..],
        &["a.jsonl", "engine", "extra"][..],
        &["--", "engine"][..],
        &["a.jsonl", ""][..],
    ] {
        let output = Command::new(env!("CARGO_BIN_EXE_xtask"))
            .arg("selected-install")
            .args(argv)
            .output()
            .unwrap();
        assert_eq!(output.status.code(), Some(2), "{argv:?}");
    }
    let output = Command::new(env!("CARGO_BIN_EXE_xtask"))
        .args(["selected-install", "--help"])
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(0));
}
