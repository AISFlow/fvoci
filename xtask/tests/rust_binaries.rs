//! `rust-binaries` hand-off: pack/unpack identity and member checks, and real
//! child runs of the restored executables and of the `xtask rust-binaries`
//! process (exit codes, signals, no-fail-fast, usage errors).

use serde_json::{json, Value};
use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::os::unix::fs::{symlink, PermissionsExt};
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};
use std::sync::{Mutex, MutexGuard};
use tar::{Builder, EntryType, Header};
use xtask::rust_binaries::{product_binaries, run_tests};
use xtask::rust_binaries_archive::{pack, unpack, MANIFEST};
use xtask::rust_binaries_cohort::{entry, expectation, Entry};

// A sibling test can fork while an executable is open for writing, inheriting
// its write FD until exec and causing ETXTBSY even after the writer closes it.
static SERIAL: Mutex<()> = Mutex::new(());

fn serial() -> MutexGuard<'static, ()> {
    SERIAL.lock().unwrap_or_else(|poison| poison.into_inner())
}

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
        let fits = member.name.len().min(name.len());
        name[..fits].copy_from_slice(&member.name[..fits]);
        if fits < member.name.len() {
            // A name past the 100-byte field (an absolute path under a long
            // TMPDIR) goes in a GNU long-name record, kept verbatim.
            let mut long = Header::new_gnu();
            long.as_gnu_mut().unwrap().name[..13].copy_from_slice(b"././@LongLink");
            long.set_entry_type(EntryType::GNULongName);
            let data = [member.name.as_slice(), b"\0"].concat();
            long.set_size(data.len() as u64);
            long.set_cksum();
            builder.append(&long, data.as_slice()).unwrap();
        }
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
    let _serial = serial();
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
    let _serial = serial();
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
    let _serial = serial();
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
    let _serial = serial();
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
    let _serial = serial();
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
    let _serial = serial();
    // None is an absolute path under this case's own root, outside target/.
    for path in [
        None,
        Some("./target/db-tests/first"),
        Some("src/first"),
        Some("target//first"),
        Some("target/./first"),
    ] {
        let root = temp();
        let outside = root.join("outside/first");
        let path = path.unwrap_or_else(|| outside.to_str().unwrap());
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
        assert!(!root.join("target").exists() && !outside.exists(), "{path}");
    }
}

#[test]
fn symlinked_destination_parent_refuses() {
    let _serial = serial();
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
    let _serial = serial();
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
    let _serial = serial();
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
    let _serial = serial();
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
    let _serial = serial();
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
    let _serial = serial();
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
    let _serial = serial();
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
    let _serial = serial();
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
            "test --locked --offline --features db-tests --no-run --lib --message-format=json --test attachment_s3_integration --test collab_lifecycle --test collab_product --test collab_projection --test collab_shutdown --test document_api_integration --test document_collab_lifecycle --test document_import_export_integration --test document_import_formats_integration --test first --test revision_integration --test schema_baseline_integration --test second --test selected_install_lifetime --test task_collab_integration\n"
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
    let _serial = serial();
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

// The fixture is native Rust, so the same ELF/profile/digest admission as CI
// runs before these process controls; no product compilation or services.
const COLLAB_CHILD: &str = r#"
use std::{env, fs::{self, OpenOptions}, io::{Read, Write}, os::unix::net::UnixStream, process::{self, Command}};
fn main() {
 let name = env::current_exe().unwrap().file_name().unwrap().to_str().unwrap().to_owned();
 if name == "git" { println!("{}", "a".repeat(40)); return; }
 if name == "rustc" { println!("rustc 1.99.0 (fixture)\nhost: {}-unknown-linux-gnu", env::consts::ARCH); return; }
 if name == "cargo" { fs::write("cargo-used", "unexpected").unwrap(); process::exit(91); }
 if name == "openssl" {
   if env::var_os("FAIL_OPENSSL").is_some() { process::exit(1); }
   println!("00112233445566778899aabbccddeeff"); return;
 }
 if name == "docker" {
   let action = env::args().nth(1).unwrap();
   writeln!(OpenOptions::new().append(true).create(true).open("docker-calls").unwrap(), "{action}").unwrap();
   match action.as_str() { "run" => println!("fixturecid"), "port" => println!("127.0.0.1:43210"), "exec"|"rm"|"logs" => (), _ => process::exit(2) }
   return;
 }
 let mode = env::var("FIXTURE_MODE").unwrap_or_default();
 if mode == "descendant" { loop { std::thread::park(); } }
 if mode == "background" { Command::new(env::current_exe().unwrap()).env("FIXTURE_MODE", "descendant").spawn().unwrap(); }
 writeln!(OpenOptions::new().append(true).create(true).open("ran.log").unwrap(), "{} {} meili={}", name, env::args().skip(1).collect::<Vec<_>>().join(" "), env::var_os("FVOCI_MEILI_URL").is_some()).unwrap();
 if mode == "hang" {
   let descendant = Command::new(env::current_exe().unwrap()).env("FIXTURE_MODE", "descendant").spawn().unwrap();
   let mut ready = UnixStream::connect(env::var("READY_SOCKET").unwrap()).unwrap();
   writeln!(ready, "{} {}", process::id(), descendant.id()).unwrap();
   let mut byte = [0]; ready.read_exact(&mut byte).unwrap(); return;
 }
 let admission = name == "collab_product";
 let serial = name == "task_collab_integration";
 if admission && !["missing", "fail-first-before-admission", "fail-second-no-admission"].contains(&mode.as_str()) {
   println!("test collab_primary_huge_varint_memory_rejected_1008 ... ok");
   if mode == "duplicate" { println!("test collab_primary_huge_varint_memory_rejected_1008 ... ok"); }
   if mode == "failed" { println!("test collab_primary_huge_varint_memory_rejected_1008 ... FAILED"); }
   if mode == "ignored" { println!("test collab_primary_huge_varint_memory_rejected_1008 ... ignored"); }
 }
 if mode == "large" { std::io::stdout().write_all(&vec![b'o'; 1_000_000]).unwrap(); std::io::stderr().write_all(&vec![b'e'; 1_000_000]).unwrap(); }
 println!("fixture stdout {name}"); eprintln!("fixture stderr {name}");
 let first_fails = ["fail", "fail-first", "fail-both", "fail-first-before-admission"].contains(&mode.as_str());
 let second_fails = ["fail", "fail-second", "fail-both", "fail-second-no-admission"].contains(&mode.as_str());
 if admission && first_fails { process::exit(101); }
 if serial && second_fails { process::exit(102); }
}
"#;

fn collaboration_workspace() -> (Temp, Value) {
    use std::io::Write;
    use xtask::rust_binaries::{context, Host};
    use xtask::rust_binaries_cohort::{
        collaboration_suites, COLLABORATION_SUITES, HELPER, LIBRARY,
    };
    let root = temp();
    let binary = root.join("fixture");
    let mut rustc = Command::new("rustc")
        .args([
            "--edition=2021",
            "--crate-name",
            "collaboration_fixture",
            "-",
            "-o",
        ])
        .arg(&binary)
        .stdin(Stdio::piped())
        .spawn()
        .unwrap();
    rustc
        .stdin
        .take()
        .unwrap()
        .write_all(COLLAB_CHILD.as_bytes())
        .unwrap();
    assert!(rustc.wait().unwrap().success());
    fs::create_dir_all(root.join(".github/workflows")).unwrap();
    fs::write(root.join(".github/workflows/rust.yml"), WORKFLOW).unwrap();
    fs::create_dir(root.join("handoff")).unwrap();
    fs::create_dir(root.join("bin")).unwrap();
    for name in ["git", "rustc", "cargo", "docker", "openssl"] {
        fs::copy(&binary, root.join("bin").join(name)).unwrap();
    }
    let mut entries = BTreeMap::new();
    let products = ["fvoci-server".to_owned()];
    for name in collaboration_suites(COLLABORATION_SUITES)
        .unwrap()
        .into_iter()
        .map(|(_, name)| name)
        .chain([HELPER, LIBRARY, "fvoci-server"])
    {
        let path = root.join("target/db-tests/debug/deps").join(name);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::copy(&binary, &path).unwrap();
        let expected = expectation(name, &products);
        let mut rec = record(&expected.target, &path);
        rec["target"]["kind"] = json!([expected.kind.as_str()]);
        rec["profile"]["test"] = json!(expected.kind != xtask::rust_binaries_cohort::Kind::Bin);
        rec["features"] = json!(expected.features);
        entries.insert(
            name.to_owned(),
            entry(&[rec], name, &expected, &root).unwrap(),
        );
    }
    let context = context(
        &Host {
            head: "a".repeat(40),
            github_sha: Some("a".repeat(40)),
            runner_arch: Some(
                if std::env::consts::ARCH == "aarch64" {
                    "ARM64"
                } else {
                    "X64"
                }
                .into(),
            ),
            run_id: Some("4242".into()),
            run_attempt: Some("1".into()),
            rustc: format!(
                "rustc 1.99.0 (fixture)\nhost: {}-unknown-linux-gnu\n",
                std::env::consts::ARCH
            ),
            workspace: root.to_str().unwrap().to_owned(),
            os_release: fs::read_to_string("/etc/os-release").unwrap(),
            build_environment: [
                "CARGO_INCREMENTAL",
                "CARGO_PROFILE_DEV_DEBUG",
                "CARGO_PROFILE_TEST_DEBUG",
                "RUSTFLAGS",
                "CARGO_ENCODED_RUSTFLAGS",
                "CARGO_BUILD_TARGET",
            ]
            .into_iter()
            .map(|name| (name, std::env::var(name).ok()))
            .collect(),
        },
        Some(&"b".repeat(64)),
    )
    .unwrap();
    let manifest = manifest(&context, &entries);
    fs::write(
        root.join("handoff/helper-manifest.json"),
        manifest.to_string(),
    )
    .unwrap();
    (root, manifest)
}

fn collaboration_command(root: &Path, mode: &str, wrapper: bool) -> Command {
    let mut command = if wrapper {
        let mut command = Command::new("bash");
        command.arg(
            Path::new(env!("CARGO_MANIFEST_DIR"))
                .parent()
                .unwrap()
                .join("scripts/start-test-meili.sh"),
        );
        command.arg(env!("CARGO_BIN_EXE_xtask"));
        command
    } else {
        Command::new(env!("CARGO_BIN_EXE_xtask"))
    };
    command
        .args([
            "rust-binaries",
            "collaboration",
            "--directory",
            "handoff",
            "--sqlite-identity",
            &"b".repeat(64),
        ])
        .current_dir(root)
        .env(
            "PATH",
            format!(
                "{}:{}",
                root.join("bin").display(),
                std::env::var("PATH").unwrap()
            ),
        )
        .env("GITHUB_SHA", "a".repeat(40))
        .env("GITHUB_RUN_ID", "4242")
        .env("GITHUB_RUN_ATTEMPT", "1")
        .env(
            "RUNNER_ARCH",
            if std::env::consts::ARCH == "aarch64" {
                "ARM64"
            } else {
                "X64"
            },
        )
        .env("RUNNER_TEMP", root)
        .env("FIXTURE_MODE", mode)
        .env(
            "FVOCI_COLLAB_ENGINE",
            root.join("target/db-tests/debug/deps/collab-engine"),
        )
        .env(
            "FVOCI_IMPORT_TEST_SERVER_BIN",
            root.join("target/db-tests/debug/deps/fvoci-server"),
        )
        .env_remove("FVOCI_MEILI_URL")
        .env_remove("FVOCI_TEST_MEILI_CONTAINER")
        .stdin(Stdio::null());
    command
}

fn assert_suite_shape(root: &Path) {
    let actual = fs::read_to_string(root.join("ran.log")).unwrap();
    let lines: Vec<&str> = actual.lines().collect();
    assert_eq!(lines.len(), 10, "{actual}");
    // Independently fixed historical contract, rather than reading the TSV.
    for name in [
        "collab_product",
        "collab_projection",
        "collab_lifecycle",
        "collab_shutdown",
        "document_collab_lifecycle",
        "revision_integration",
        "document_api_integration",
        "document_import_export_integration",
        "document_import_formats_integration",
        "task_collab_integration",
    ] {
        let matches: Vec<&&str> = lines
            .iter()
            .filter(|line| line.starts_with(&format!("{name} ")))
            .collect();
        assert_eq!(matches.len(), 1, "{name}: {actual}");
        assert_eq!(
            matches[0].contains("--test-threads=1"),
            name == "task_collab_integration"
        );
        assert!(matches[0].ends_with("meili=true"));
    }
}

fn assert_meili_cleanup(root: &Path, started: bool) {
    let calls = fs::read_to_string(root.join("docker-calls")).unwrap_or_default();
    assert_eq!(
        calls.lines().filter(|line| *line == "run").count(),
        usize::from(started)
    );
    assert_eq!(
        calls.lines().filter(|line| *line == "rm").count(),
        usize::from(started)
    );
}

// A separate native libtest process exercises the real library boundary and
// signal handler without injecting a hook or flag into the production runner.
#[test]
#[ignore = "signal fixture selected by collaboration_late_interrupt_never_acknowledges_success"]
fn collaboration_late_interrupt_fixture() {
    use std::os::unix::process::ExitStatusExt;
    use xtask::process::run_owned_streaming;

    let stage = std::env::var("LATE_SIGNAL_STAGE").unwrap();
    let code = match stage.as_str() {
        "after-verdict" => xtask::rust_binaries::main(
            [
                "collaboration",
                "--directory",
                "handoff",
                "--sqlite-identity",
                &"b".repeat(64),
            ]
            .into_iter()
            .map(Into::into)
            .collect(),
        ),
        "after-reap" | "spawn-error" => {
            let path = if stage == "spawn-error" {
                "missing-executable"
            } else {
                "target/db-tests/debug/deps/task_collab_integration"
            };
            match run_owned_streaming(
                &mut Command::new(path),
                [
                    fs::File::create("late.stdout.log").unwrap(),
                    fs::File::create("late.stderr.log").unwrap(),
                ],
            ) {
                Ok(done) => done
                    .status
                    .code()
                    .unwrap_or_else(|| 128 + done.status.signal().unwrap()),
                Err(_) => 1,
            }
        }
        _ => panic!("unknown fixture stage"),
    };
    // The direct child has been reaped and all captured streams reached EOF
    // before delivery, rather than racing a child exit with a fixed delay.
    assert_eq!(
        unsafe { libc::waitpid(-1, std::ptr::null_mut(), libc::WNOHANG) },
        -1
    );
    assert_eq!(
        std::io::Error::last_os_error().raw_os_error(),
        Some(libc::ECHILD)
    );
    let signal: i32 = std::env::var("LATE_SIGNAL").unwrap().parse().unwrap();
    if signal != 0 {
        assert_eq!(unsafe { libc::raise(signal) }, 0);
    }
    std::process::exit(code);
}

#[test]
fn collaboration_late_interrupt_never_acknowledges_success() {
    let _serial = serial();
    let (root, _) = collaboration_workspace();
    for (stage, mode, normal) in [
        ("after-reap", "pass", 0),
        ("spawn-error", "pass", 1),
        ("after-verdict", "pass", 0),
        ("after-verdict", "fail-first", 101),
    ] {
        for (signal, expected) in [(0, normal), (libc::SIGINT, 130), (libc::SIGTERM, 143)] {
            let _ = fs::remove_file(root.join("ran.log"));
            let _ = fs::remove_file(root.join("docker-calls"));
            let template = collaboration_command(&root, mode, false);
            let mut command = Command::new("bash");
            command
                .arg(
                    Path::new(env!("CARGO_MANIFEST_DIR"))
                        .parent()
                        .unwrap()
                        .join("scripts/start-test-meili.sh"),
                )
                .arg(std::env::current_exe().unwrap())
                .args([
                    "--exact",
                    "collaboration_late_interrupt_fixture",
                    "--ignored",
                    "--nocapture",
                    "--test-threads=1",
                ])
                .current_dir(&root)
                .envs(
                    template
                        .get_envs()
                        .filter_map(|(key, value)| value.map(|value| (key, value))),
                )
                .env_remove("FVOCI_MEILI_URL")
                .env_remove("FVOCI_TEST_MEILI_CONTAINER")
                .env("LATE_SIGNAL_STAGE", stage)
                .env("LATE_SIGNAL", signal.to_string())
                .stdin(Stdio::null());
            let out = command.output().unwrap();
            assert_meili_cleanup(&root, true);
            if stage == "after-verdict" {
                assert_suite_shape(&root);
            }
            assert_eq!(
                out.status.code(),
                Some(expected),
                "{stage}/{mode}/{signal}: {out:?}"
            );
            assert!(!root.join("cargo-used").exists());
            eprintln!("late-signal stage={stage} mode={mode} signal={signal} exit={expected} cleanup=complete");
        }
    }
}

#[test]
fn collaboration_preserves_groups_failures_admission_and_owned_service() {
    let _serial = serial();
    let (root, _) = collaboration_workspace();
    for (mode, code) in [
        ("pass", 0),
        ("fail", 101),
        ("fail-first", 101),
        ("fail-second", 102),
        ("fail-both", 101),
        ("fail-first-before-admission", 101),
        ("fail-second-no-admission", 102),
        ("duplicate", 1),
        ("missing", 1),
        ("failed", 1),
        ("ignored", 1),
    ] {
        let _ = fs::remove_file(root.join("ran.log"));
        let _ = fs::remove_file(root.join("docker-calls"));
        let out = collaboration_command(&root, mode, true).output().unwrap();
        assert_eq!(out.status.code(), Some(code), "{mode}: {out:?}");
        assert_suite_shape(&root);
        assert_meili_cleanup(&root, true);
        for label in ["parallel-suites", "task_collab_integration"] {
            for stream in ["stdout", "stderr"] {
                let log = fs::read_to_string(
                    root.join("rust-collaboration-logs")
                        .join(format!("{label}.{stream}.log")),
                )
                .unwrap();
                assert!(
                    log.contains(&format!("fixture {stream}")),
                    "{mode}/{label}/{stream}"
                );
            }
        }
    }
    let _ = fs::remove_file(root.join("ran.log"));
    let _ = fs::remove_file(root.join("docker-calls"));
    let out = collaboration_command(&root, "pass", true)
        .env("FAIL_OPENSSL", "1")
        .env("FVOCI_TEST_MEILI_CONTAINER", "parent-owned")
        .output()
        .unwrap();
    assert!(!out.status.success());
    assert!(!root.join("ran.log").exists());
    assert_meili_cleanup(&root, false);
}

#[test]
fn collaboration_complete_separate_large_streams() {
    let _serial = serial();
    let (root, _) = collaboration_workspace();
    let out = collaboration_command(&root, "large", true)
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let stdout: Vec<u8> = ["parallel-suites", "task_collab_integration"]
        .into_iter()
        .flat_map(|label| {
            fs::read(
                root.join("rust-collaboration-logs")
                    .join(format!("{label}.stdout.log")),
            )
            .unwrap()
        })
        .collect();
    let stderr: Vec<u8> = ["parallel-suites", "task_collab_integration"]
        .into_iter()
        .flat_map(|label| {
            fs::read(
                root.join("rust-collaboration-logs")
                    .join(format!("{label}.stderr.log")),
            )
            .unwrap()
        })
        .collect();
    assert_eq!(stdout, out.stdout);
    assert_eq!(stderr, out.stderr);
    assert!(stdout.len() > 10_000_000 && stderr.len() > 10_000_000);
    assert_meili_cleanup(&root, true);
}

#[test]
fn collaboration_refuses_changed_identity_members_and_overrides_before_any_suite() {
    let _serial = serial();
    let (root, manifest) = collaboration_workspace();
    for key in [
        "sha",
        "run_id",
        "run_attempt",
        "arch",
        "workspace",
        "rustc",
        "sqlite",
        "profile",
        "build_environment",
        "os",
    ] {
        let mut foreign = manifest.clone();
        foreign["context"][key] = json!("foreign");
        fs::write(
            root.join("handoff/helper-manifest.json"),
            foreign.to_string(),
        )
        .unwrap();
        assert!(
            !collaboration_command(&root, "pass", false)
                .output()
                .unwrap()
                .status
                .success(),
            "{key}"
        );
        assert!(!root.join("ran.log").exists());
    }
    for defect in [
        "missing",
        "digest",
        "features",
        "profile",
        "path",
        "foreign-ELF",
    ] {
        let mut bad = manifest.clone();
        match defect {
            "missing" => {
                bad["entries"]
                    .as_object_mut()
                    .unwrap()
                    .remove("task_collab_integration");
            }
            "digest" => bad["entries"]["collab_product"]["sha256"] = json!("0".repeat(64)),
            "features" => bad["entries"]["collab_product"]["record"]["features"] = json!([]),
            "profile" => {
                bad["entries"]["collab_product"]["record"]["profile"]["opt_level"] = json!("3")
            }
            "path" => bad["entries"]["collab_product"]["path"] = json!("../escape"),
            "foreign-ELF" => {
                let path = root.join("target/db-tests/debug/deps/collab_product");
                let mut bytes = fs::read(&path).unwrap();
                bytes[18] = 0;
                bytes[19] = 0;
                fs::write(&path, bytes).unwrap();
                bad["entries"]["collab_product"]["sha256"] =
                    json!(xtask::rust_binaries_archive::file_sha256(&path).unwrap());
            }
            _ => unreachable!(),
        }
        fs::write(root.join("handoff/helper-manifest.json"), bad.to_string()).unwrap();
        assert!(
            !collaboration_command(&root, "pass", false)
                .output()
                .unwrap()
                .status
                .success(),
            "{defect}"
        );
        assert!(!root.join("ran.log").exists());
    }
    fs::copy(
        root.join("fixture"),
        root.join("target/db-tests/debug/deps/collab_product"),
    )
    .unwrap();
    fs::write(
        root.join("handoff/helper-manifest.json"),
        manifest.to_string(),
    )
    .unwrap();
    for args in [
        vec!["--test", "collab_product"],
        vec!["--nocapture"],
        vec!["--cohort", "helper"],
    ] {
        assert!(!collaboration_command(&root, "pass", false)
            .args(args)
            .output()
            .unwrap()
            .status
            .success());
        assert!(!root.join("ran.log").exists());
    }
    fs::remove_file(root.join("handoff/helper-manifest.json")).unwrap();
    assert!(!collaboration_command(&root, "pass", false)
        .output()
        .unwrap()
        .status
        .success());
    assert!(!root.join("ran.log").exists());
    assert!(
        !root.join("cargo-used").exists(),
        "rejected sealed inputs must never trigger Cargo fallback"
    );
}

#[test]
fn collaboration_exit_closes_descendant_pipes_and_spawn_failure_continues() {
    let _serial = serial();
    let (root, _) = collaboration_workspace();
    let out = collaboration_command(&root, "background", true)
        .output()
        .unwrap();
    assert!(out.status.success(), "{out:?}");
    assert_suite_shape(&root);
    assert_meili_cleanup(&root, true);
    fs::remove_file(root.join("ran.log")).unwrap();
    fs::remove_file(root.join("docker-calls")).unwrap();
    let path = root.join("target/db-tests/debug/deps/collab_product");
    fs::set_permissions(&path, fs::Permissions::from_mode(0o644)).unwrap();
    let out = collaboration_command(&root, "pass", true).output().unwrap();
    assert_eq!(out.status.code(), Some(1), "{out:?}");
    let actual = fs::read_to_string(root.join("ran.log")).unwrap();
    assert_eq!(actual.lines().count(), 9);
    assert!(actual.contains("task_collab_integration --test-threads=1"));
    assert_meili_cleanup(&root, true);
    assert!(!root.join("cargo-used").exists());
}

#[test]
fn collaboration_interrupt_reaps_child_drains_eof_and_cleans_meili() {
    use std::io::{BufRead, BufReader};
    use std::os::unix::{net::UnixListener, process::CommandExt};
    use std::sync::mpsc;
    use std::time::Duration;
    let _serial = serial();
    let (root, _) = collaboration_workspace();
    for (signal, expected) in [(libc::SIGINT, 130), (libc::SIGTERM, 143)] {
        let _ = fs::remove_file(root.join("ran.log"));
        let _ = fs::remove_file(root.join("docker-calls"));
        let socket = root.join("ready.socket");
        let _ = fs::remove_file(&socket);
        let listener = UnixListener::bind(&socket).unwrap();
        let (sender, receiver) = mpsc::channel();
        let ready = std::thread::spawn(move || {
            let (stream, _) = listener.accept().unwrap();
            let mut reader = BufReader::new(stream);
            let mut line = String::new();
            reader.read_line(&mut line).unwrap();
            sender.send(line).unwrap();
            reader // keep the child blocked until the signal
        });
        let child = collaboration_command(&root, "hang", true)
            .env("READY_SOCKET", &socket)
            .process_group(0)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        let line = receiver.recv_timeout(Duration::from_secs(60)).unwrap();
        let pids: Vec<i32> = line
            .split_whitespace()
            .map(|word| word.parse().unwrap())
            .collect();
        // Observe the descendant's exit through its pidfd, avoiding a
        // scheduler-dependent assertion immediately after sending SIGKILL.
        let descendant = unsafe { libc::syscall(libc::SYS_pidfd_open, pids[1], 0) } as i32;
        assert!(descendant >= 0);
        assert_eq!(unsafe { libc::killpg(child.id() as i32, signal) }, 0);
        let (sender, receiver) = mpsc::channel();
        let waiter = std::thread::spawn(move || {
            sender.send(child.wait_with_output().unwrap()).unwrap();
        });
        let out = receiver
            .recv_timeout(Duration::from_secs(60))
            .expect("interrupt must finish and close both streams");
        waiter.join().unwrap();
        drop(ready.join().unwrap());
        assert_eq!(out.status.code(), Some(expected), "{out:?}");
        assert_eq!(
            unsafe { libc::kill(pids[0], 0) },
            -1,
            "direct child must be reaped"
        );
        let mut exited = libc::pollfd {
            fd: descendant,
            events: libc::POLLIN,
            revents: 0,
        };
        assert_eq!(
            unsafe { libc::poll(&mut exited, 1, 60_000) },
            1,
            "descendant survived"
        );
        unsafe {
            libc::close(descendant);
        }
        assert_eq!(
            fs::read_to_string(root.join("ran.log"))
                .unwrap()
                .lines()
                .count(),
            1
        );
        assert_meili_cleanup(&root, true);
    }
}
