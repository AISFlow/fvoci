//! `rust-binaries` hand-off: pack/unpack identity and member checks, and real
//! child runs of the restored executables and of the `xtask rust-binaries`
//! process (exit codes, signals, no-fail-fast, usage errors).

use serde_json::{json, Value};
use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::os::unix::fs::{symlink, PermissionsExt};
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};
use tar::{Builder, EntryType, Header};
use xtask::rust_binaries::{product_binaries, run_tests};
use xtask::rust_binaries_archive::{pack, unpack, MANIFEST};
use xtask::rust_binaries_cohort::{entry, expectation, Entry};

const FIRST: &str = "target/db-tests/debug/deps/first";

/// Canonical temporary directory removed when the test ends.
struct Temp(PathBuf);

impl std::ops::Deref for Temp {
    type Target = Path;
    fn deref(&self) -> &Path {
        &self.0
    }
}

impl AsRef<Path> for Temp {
    fn as_ref(&self) -> &Path {
        &self.0
    }
}

impl Drop for Temp {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn temp() -> Temp {
    let dir = xtask::host::mkdtemp("rust-binaries-", &std::env::temp_dir()).unwrap();
    Temp(dir.canonicalize().unwrap())
}

fn names(list: &[&str]) -> BTreeSet<String> {
    list.iter().map(|n| (*n).to_owned()).collect()
}

fn script(path: &Path, body: &str) {
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, format!("#!/bin/sh\n{body}\n")).unwrap();
    fs::set_permissions(path, fs::Permissions::from_mode(0o755)).unwrap();
}

fn record(name: &str, path: &Path) -> Value {
    json!({"reason": "compiler-artifact", "target": {"name": name, "kind": ["test"]},
           "profile": {"test": true, "opt_level": "0", "debuginfo": 0},
           "features": ["db-tests"], "executable": path.to_str().unwrap()})
}

/// Port of the Python fixture: two db-tests executables `first` and `second`
/// that append their name to `ran.log`; `first` exits with `first_exit`.
fn fixture(root: &Path, first_body: &str) -> (Value, BTreeMap<String, Entry>) {
    let context = json!({"sha": "a".repeat(40), "arch": "X64", "workspace": root.to_str().unwrap(),
                         "sqlite": "b".repeat(64), "rustc": "pinned fixture", "profile": "dev-test-nodebug"});
    let log = root.join("ran.log");
    let mut records = Vec::new();
    for name in ["first", "second"] {
        let path = root.join("target/db-tests/debug/deps").join(name);
        let body = if name == "first" {
            first_body
        } else {
            "exit 0"
        };
        script(
            &path,
            &format!("echo {name} \"$@\" >> '{}'\n{body}", log.display()),
        );
        records.push(record(name, &path));
    }
    let entries = ["first", "second"]
        .iter()
        .map(|n| {
            let e = entry(&records, n, &expectation(n, &[]), root).unwrap();
            ((*n).to_owned(), e)
        })
        .collect();
    (context, entries)
}

fn manifest(context: &Value, entries: &BTreeMap<String, Entry>) -> Value {
    let entries: serde_json::Map<String, Value> = entries
        .iter()
        .map(|(n, e)| (n.clone(), e.to_json()))
        .collect();
    json!({"version": 1, "context": context, "entries": entries})
}

/// A raw member: the name is written into the header bytes unvalidated, as a
/// hostile producer could.
struct Raw {
    name: Vec<u8>,
    kind: EntryType,
    data: Vec<u8>,
    link: &'static str,
}

fn regular(name: &str, data: Vec<u8>) -> Raw {
    Raw {
        name: name.as_bytes().to_vec(),
        kind: EntryType::Regular,
        data,
        link: "",
    }
}

fn raw_archive(path: &Path, members: Vec<Raw>) {
    let mut builder = Builder::new(fs::File::create(path).unwrap());
    for member in members {
        let mut header = Header::new_gnu();
        let name = &mut header.as_gnu_mut().unwrap().name;
        name[..member.name.len()].copy_from_slice(&member.name);
        header.set_entry_type(member.kind);
        header.set_mode(0o755);
        header.set_size(member.data.len() as u64);
        if !member.link.is_empty() {
            header.set_link_name_literal(member.link).unwrap();
        }
        header.set_cksum();
        builder.append(&header, member.data.as_slice()).unwrap();
    }
    builder.into_inner().unwrap();
}

/// The archive a producer would seal, from a (possibly hostile) manifest.
fn hostile_pack(path: &Path, root: &Path, manifest: &Value, extra: Vec<Raw>) {
    let mut members = vec![regular(MANIFEST, manifest.to_string().into_bytes())];
    for entry in manifest["entries"].as_object().unwrap().values() {
        let relative = entry["path"].as_str().unwrap();
        members.push(regular(
            relative,
            fs::read(root.join(relative)).unwrap_or_default(),
        ));
    }
    members.extend(extra);
    raw_archive(path, members);
}

fn unpack_both(archive: &Path, context: &Value, root: &Path) -> Result<Value, String> {
    unpack(archive, context, &names(&["first", "second"]), &[], root)
}

#[test]
fn all_enabled_child_binaries_are_inventoried() {
    let repo = Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .canonicalize()
        .unwrap();
    assert_eq!(
        product_binaries(&repo).unwrap(),
        [
            "fvoci-e2e-fixture",
            "fvoci-mcp",
            "fvoci-migrate",
            "fvoci-server"
        ]
    );
}

#[test]
fn roundtrip_and_no_fail_fast_execute_actual_transferred_files() {
    let root = temp();
    let (context, entries) = fixture(&root, "exit 7");
    let archive = root.join("postgres.tar");
    pack(&archive, &context, &root, &entries).unwrap();
    fs::remove_dir_all(root.join("target")).unwrap();
    let manifest = unpack_both(&archive, &context, &root).unwrap();
    assert_eq!(manifest, self::manifest(&context, &entries));
    let mode = fs::metadata(root.join(FIRST)).unwrap().permissions().mode();
    assert_eq!(mode & 0o777, 0o755);
    let tests = ["first".to_owned(), "second".to_owned()];
    assert_eq!(run_tests(&manifest, &tests, &["--nocapture"]).unwrap(), 7);
    assert_eq!(
        fs::read_to_string(root.join("ran.log")).unwrap(),
        "first --nocapture\nsecond --nocapture\n"
    );
    // A second unpack never overwrites restored executables.
    let error = unpack_both(&archive, &context, &root).unwrap_err();
    assert!(error.contains("destination occupied"), "{error}");
}

#[test]
fn missing_archive_and_source_platform_native_context_refuse_before_writing() {
    let root = temp();
    let (context, entries) = fixture(&root, "exit 0");
    let archive = root.join("postgres.tar");
    assert!(unpack_both(&archive, &context, &root).is_err());
    pack(&archive, &context, &root, &entries).unwrap();
    assert!(
        pack(&archive, &context, &root, &entries).is_err(),
        "pack never overwrites"
    );
    fs::remove_dir_all(root.join("target")).unwrap();
    for key in ["sha", "arch", "workspace", "sqlite", "profile", "rustc"] {
        let mut foreign = context.clone();
        foreign[key] = json!("foreign");
        let error = unpack_both(&archive, &foreign, &root).unwrap_err();
        assert!(error.contains("inputs mismatch"), "{key}: {error}");
        assert!(!root.join("target").exists());
    }
}

#[test]
fn digest_feature_profile_cohort_and_path_corruption_refuse_before_writing() {
    for defect in [
        "digest",
        "feature",
        "profile",
        "missing",
        "escape",
        "duplicate-path",
        "version",
        "executable",
    ] {
        let root = temp();
        let (context, mut entries) = fixture(&root, "exit 0");
        let first = entries.get_mut("first").unwrap();
        match defect {
            "digest" => first.sha256 = "0".repeat(64),
            "feature" => first.record["features"] = json!(["extract-native-tests", "db-tests"]),
            "profile" => first.record["profile"]["debuginfo"] = json!(2),
            "missing" => {
                entries.remove("first");
            }
            "escape" => first.path = "target/../target/db-tests/debug/deps/first".into(),
            "duplicate-path" => {
                let path = first.path.clone();
                entries.get_mut("second").unwrap().path = path;
            }
            "executable" => first.record["executable"] = json!("/elsewhere/first"),
            _ => {}
        }
        let mut manifest = manifest(&context, &entries);
        if defect == "version" {
            manifest["version"] = json!(true);
        }
        let archive = root.join("postgres.tar");
        hostile_pack(&archive, &root, &manifest, Vec::new());
        fs::remove_dir_all(root.join("target")).unwrap();
        assert!(unpack_both(&archive, &context, &root).is_err(), "{defect}");
        assert!(!root.join("target").exists(), "{defect}");
    }
}

#[test]
fn nonregular_unsafe_and_extra_members_refuse_before_writing() {
    let cases: Vec<(&str, Raw)> = vec![
        (
            "symlink",
            Raw {
                name: b"target/link".to_vec(),
                kind: EntryType::Symlink,
                data: vec![],
                link: "/etc/passwd",
            },
        ),
        (
            "hardlink",
            Raw {
                name: b"target/hard".to_vec(),
                kind: EntryType::Link,
                data: vec![],
                link: FIRST,
            },
        ),
        (
            "char device",
            Raw {
                name: b"target/dev".to_vec(),
                kind: EntryType::Char,
                data: vec![],
                link: "",
            },
        ),
        (
            "block device",
            Raw {
                name: b"target/blk".to_vec(),
                kind: EntryType::Block,
                data: vec![],
                link: "",
            },
        ),
        (
            "fifo",
            Raw {
                name: b"target/fifo".to_vec(),
                kind: EntryType::Fifo,
                data: vec![],
                link: "",
            },
        ),
        (
            "directory",
            Raw {
                name: b"target/dir/".to_vec(),
                kind: EntryType::Directory,
                data: vec![],
                link: "",
            },
        ),
        (
            "contiguous",
            Raw {
                name: b"target/cont".to_vec(),
                kind: EntryType::Continuous,
                data: vec![],
                link: "",
            },
        ),
        (
            "non-utf8",
            Raw {
                name: b"target/\xff".to_vec(),
                kind: EntryType::Regular,
                data: vec![],
                link: "",
            },
        ),
        ("duplicate", regular(FIRST, b"#!/bin/sh\n".to_vec())),
        ("extra regular", regular("target/extra", b"x".to_vec())),
    ];
    for (label, extra) in cases {
        let root = temp();
        let (context, entries) = fixture(&root, "exit 0");
        let archive = root.join("postgres.tar");
        hostile_pack(&archive, &root, &manifest(&context, &entries), vec![extra]);
        fs::remove_dir_all(root.join("target")).unwrap();
        let error = unpack_both(&archive, &context, &root).unwrap_err();
        let expected = if label == "extra regular" {
            "inventory mismatch"
        } else {
            "duplicate or nonregular"
        };
        assert!(error.contains(expected), "{label}: {error}");
        assert!(!root.join("target").exists(), "{label}: {error}");
    }
}

#[test]
fn member_paths_outside_target_refuse_before_writing() {
    for path in [
        "/tmp/first",
        "./target/db-tests/first",
        "src/first",
        "target//first",
        "target/./first",
    ] {
        let root = temp();
        let (context, mut entries) = fixture(&root, "exit 0");
        let source = fs::read(root.join(FIRST)).unwrap();
        let first = entries.get_mut("first").unwrap();
        first.path = path.to_owned();
        let manifest = manifest(&context, &entries);
        let archive = root.join("postgres.tar");
        let mut members = vec![regular(MANIFEST, manifest.to_string().into_bytes())];
        members.push(regular(path, source));
        members.push(regular(
            &entries["second"].path,
            fs::read(root.join(&entries["second"].path)).unwrap(),
        ));
        raw_archive(&archive, members);
        fs::remove_dir_all(root.join("target")).unwrap();
        let error = unpack_both(&archive, &context, &root).unwrap_err();
        assert!(error.contains("path escape"), "{path}: {error}");
        assert!(
            !root.join("target").exists() && !Path::new("/tmp/first").exists(),
            "{path}"
        );
    }
}

#[test]
fn symlinked_destination_parent_refuses() {
    let root = temp();
    let (context, entries) = fixture(&root, "exit 0");
    let archive = root.join("postgres.tar");
    pack(&archive, &context, &root, &entries).unwrap();
    fs::remove_dir_all(root.join("target")).unwrap();
    let elsewhere = temp();
    symlink(&elsewhere, root.join("target")).unwrap();
    let error = unpack_both(&archive, &context, &root).unwrap_err();
    assert!(error.contains("symlinked"), "{error}");
    assert_eq!(fs::read_dir(&elsewhere).unwrap().count(), 0);
}

#[test]
fn producer_rejects_missing_ambiguous_features_and_profiles() {
    let root = temp();
    let (_, entries) = fixture(&root, "exit 0");
    let record = entries["first"].record.clone();
    let mut no_features = record.clone();
    no_features["features"] = json!([]);
    let mut optimised = record.clone();
    optimised["profile"]["opt_level"] = json!("3");
    let outside = record_for_outside(&root);
    for records in [
        vec![],
        vec![record.clone(), record.clone()],
        vec![no_features],
        vec![optimised],
        vec![outside],
    ] {
        assert!(entry(&records, "first", &expectation("first", &[]), &root).is_err());
    }
}

/// An executable reached through a symlink is not physical.
fn record_for_outside(root: &Path) -> Value {
    let link = root.join("target/db-tests/debug/deps/linked");
    symlink(root.join(FIRST), &link).unwrap();
    let mut record = record("first", &link);
    record["target"]["name"] = json!("first");
    record
}

#[test]
fn run_refuses_changed_executable_unknown_target_and_filters() {
    let root = temp();
    let (context, entries) = fixture(&root, "exit 0");
    let manifest = manifest(&context, &entries);
    let first = ["first".to_owned()];
    for (tests, libtest) in [
        (&first[..], &["nonexistent_filter"][..]),
        (&[][..], &[][..]),
        (&["first".to_owned(), "first".to_owned()][..], &[][..]),
        (&["third".to_owned()][..], &[][..]),
    ] {
        assert!(
            run_tests(&manifest, tests, libtest).is_err(),
            "{tests:?} {libtest:?}"
        );
    }
    fs::write(root.join(FIRST), b"changed").unwrap();
    assert!(run_tests(&manifest, &first, &[]).is_err());
    assert!(!root.join("ran.log").exists());
    // Each digest is checked right before its executable starts.
    let both = ["second".to_owned(), "first".to_owned()];
    assert!(run_tests(&manifest, &both, &[]).is_err());
    assert_eq!(
        fs::read_to_string(root.join("ran.log")).unwrap(),
        "second\n"
    );
}

// ---- real `xtask rust-binaries` processes ----

const WORKFLOW: &str = "jobs:\n  postgres:\n    env:\n      FVOCI_POSTGRES_MATRIX_CATALOG: |\n        [{\"tests\": \"--test first --test second\"}]\n";

/// A workspace with a minimal rust.yml, a restored `postgres-manifest.json`
/// and the two executables, plus the environment `run` checks.
fn workspace(first_body: &str) -> (Temp, Command) {
    let root = temp();
    let (mut context, entries) = fixture(&root, first_body);
    context["arch"] = json!("X64");
    context["run_id"] = json!("4242");
    context["run_attempt"] = json!("1");
    fs::create_dir_all(root.join(".github/workflows")).unwrap();
    fs::write(root.join(".github/workflows/rust.yml"), WORKFLOW).unwrap();
    fs::create_dir(root.join("handoff")).unwrap();
    fs::write(
        root.join("handoff/postgres-manifest.json"),
        manifest(&context, &entries).to_string(),
    )
    .unwrap();
    let mut command = Command::new(env!("CARGO_BIN_EXE_xtask"));
    command
        .arg("rust-binaries")
        .current_dir(&root)
        .env("GITHUB_SHA", "a".repeat(40))
        .env("RUNNER_ARCH", "X64")
        .env("GITHUB_RUN_ID", "4242")
        .env("GITHUB_RUN_ATTEMPT", "1")
        .env_remove("GITHUB_JOB")
        .stdin(Stdio::null());
    (root, command)
}

fn output(command: &mut Command, argv: &[&str]) -> Output {
    command.args(argv).output().unwrap()
}

#[test]
fn cli_run_propagates_first_failure_without_fail_fast() {
    let (root, mut command) = workspace("exit 7");
    let out = output(
        &mut command,
        &[
            "run",
            "--directory",
            "handoff",
            "--test",
            "first",
            "--test",
            "second",
        ],
    );
    assert_eq!(out.status.code(), Some(7), "{out:?}");
    assert_eq!(
        fs::read_to_string(root.join("ran.log")).unwrap(),
        "first\nsecond\n"
    );
}

#[test]
fn cli_run_reports_signal_as_128_plus_n() {
    let (root, mut command) = workspace("kill -KILL $$");
    let out = output(
        &mut command,
        &[
            "run",
            "--directory",
            "handoff",
            "--test",
            "first",
            "--test",
            "second",
            "--nocapture",
        ],
    );
    assert_eq!(out.status.code(), Some(137), "{out:?}");
    assert_eq!(
        fs::read_to_string(root.join("ran.log")).unwrap(),
        "first --nocapture\nsecond --nocapture\n"
    );
    let (_root, mut command) = workspace("exit 0");
    let out = output(
        &mut command,
        &["run", "--directory", "handoff", "--test", "first"],
    );
    assert_eq!(out.status.code(), Some(0), "{out:?}");
}

#[test]
fn cli_run_refuses_foreign_source_and_changed_files() {
    // A manifest field that is missing never matches an unset variable.
    for key in ["sha", "run_id", "run_attempt", "arch"] {
        let (root, mut command) = workspace("exit 0");
        let path = root.join("handoff/postgres-manifest.json");
        let mut manifest: Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
        manifest["context"].as_object_mut().unwrap().remove(key);
        fs::write(&path, manifest.to_string()).unwrap();
        let variable = match key {
            "sha" => "GITHUB_SHA",
            "run_id" => "GITHUB_RUN_ID",
            "run_attempt" => "GITHUB_RUN_ATTEMPT",
            _ => "RUNNER_ARCH",
        };
        let out = output(
            command.env_remove(variable),
            &["run", "--directory", "handoff", "--test", "first"],
        );
        assert_eq!(out.status.code(), Some(1), "{key}: {out:?}");
        assert!(!root.join("ran.log").exists(), "{key}");
    }
    for (variable, value) in [
        ("GITHUB_SHA", "c".repeat(40)),
        ("RUNNER_ARCH", "ARM64".to_owned()),
        ("GITHUB_RUN_ATTEMPT", "2".to_owned()),
        ("GITHUB_RUN_ID", "4243".to_owned()),
    ] {
        let (root, mut command) = workspace("exit 0");
        let out = output(
            command.env(variable, value),
            &["run", "--directory", "handoff", "--test", "first"],
        );
        assert_eq!(out.status.code(), Some(1), "{out:?}");
        assert!(String::from_utf8_lossy(&out.stderr)
            .contains("Rust binary handoff refused: Rust binary execution source/path mismatch"));
        assert!(!root.join("ran.log").exists());
    }
    let (root, mut command) = workspace("exit 0");
    fs::write(
        root.join("target/db-tests/debug/deps/second"),
        "#!/bin/sh\nexit 0\n",
    )
    .unwrap();
    let out = output(
        &mut command,
        &[
            "run",
            "--directory",
            "handoff",
            "--test",
            "first",
            "--test",
            "second",
        ],
    );
    assert_eq!(out.status.code(), Some(1), "{out:?}");
    assert!(String::from_utf8_lossy(&out.stderr).contains("changed after validation"));
}

#[test]
fn cli_usage_errors_exit_2() {
    for argv in [
        &[][..],
        &["rebuild", "--directory", "d"],
        &["run"],
        &["run", "--directory"],
        &["unpack", "--directory", "d", "--cohort", "other"],
        &["run", "--directory", "d", "--unknown"],
        &["run", "--directory", "d", "positional"],
        &["--directory", "d", "run"],
    ] {
        let (_root, mut command) = workspace("exit 0");
        let out = output(&mut command, argv);
        assert_eq!(out.status.code(), Some(2), "{argv:?}: {out:?}");
        assert!(String::from_utf8_lossy(&out.stderr).starts_with("usage: xtask rust-binaries"));
    }
    let (_root, mut command) = workspace("exit 0");
    let out = output(&mut command, &["--help"]);
    assert_eq!(out.status.code(), Some(0));
}

/// `build` runs Cargo once with every registered `--test`, Cargo JSON into a
/// new tests.jsonl, and passes Cargo's status (or 128+signal) through.
#[test]
fn cli_build_passes_cargo_status_and_never_overwrites() {
    for (body, expected) in [("exit 0", 0), ("exit 3", 3), ("kill -TERM $$", 143)] {
        let (root, mut command) = workspace("exit 0");
        let bin = root.join("bin");
        script(
            &bin.join("cargo"),
            &format!("echo \"$@\" > '{}'\necho '{{\"reason\":\"build-finished\",\"success\":true}}'\n{body}", root.join("argv").display()),
        );
        let path = format!("{}:{}", bin.display(), std::env::var("PATH").unwrap());
        let out = output(
            command.env("PATH", &path),
            &["build", "--directory", "handoff"],
        );
        assert_eq!(out.status.code(), Some(expected), "{out:?}");
        assert_eq!(
            fs::read_to_string(root.join("argv")).unwrap(),
            "test --locked --offline --features db-tests --no-run --message-format=json --test attachment_s3_integration --test first --test schema_baseline_integration --test second --test selected_install_lifetime\n"
        );
        assert_eq!(
            fs::read_to_string(root.join("handoff/tests.jsonl")).unwrap(),
            "{\"reason\":\"build-finished\",\"success\":true}\n"
        );
        let mut again = Command::new(env!("CARGO_BIN_EXE_xtask"));
        let out = output(
            again.current_dir(&root).env("PATH", &path),
            &["rust-binaries", "build", "--directory", "handoff"],
        );
        assert_eq!(out.status.code(), Some(1), "{out:?}");
    }
}

#[test]
fn cli_refuses_missing_or_malformed_workflow() {
    let (root, mut command) = workspace("exit 0");
    fs::write(root.join(".github/workflows/rust.yml"), "jobs: {}\n").unwrap();
    let out = output(
        &mut command,
        &["run", "--directory", "handoff", "--test", "first"],
    );
    assert_eq!(out.status.code(), Some(1));
    assert!(String::from_utf8_lossy(&out.stderr).contains("postgres matrix catalog missing"));
    assert!(!root.join("ran.log").exists());
}
