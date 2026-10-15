//! `xtask rust-binaries {build,pack,unpack,run}`: the Rust workflow's
//! once-per-architecture executable hand-off. The producer job builds and
//! seals the finished executables with their identity context; consumer jobs
//! validate and restore them and never rebuild.
//!
//! Exit status: 0, 1 (refused), 2 (usage), or a child's status, where a child
//! killed by signal N is reported as 128+N.

use crate::args::{self, Outcome, Parsed};
use crate::host::utc_timestamp;
use crate::rust_binaries_archive::{file_sha256, pack, unpack};
use crate::rust_binaries_cohort::{
    self as cohort, entry, expectation, Entry, Kind, HELPER, LIBRARY, SCHEMA,
};
use serde_json::{json, Value};
use std::collections::{BTreeMap, BTreeSet};
use std::ffi::OsString;
use std::fs::{self, OpenOptions};
use std::os::unix::process::ExitStatusExt;
use std::path::{Path, PathBuf};
use std::process::{Command, ExitStatus, Stdio};
use std::time::Instant;

const USAGE: &str = "usage: xtask rust-binaries [-h] {build,pack,unpack,run,collaboration} --directory DIRECTORY [--sqlite-identity SQLITE_IDENTITY] [--cohort {postgres,helper}] [--test TEST] [--nocapture]";
const HELP: &str = "\
Rust workflow's PostgreSQL executable handoff (never rebuilds consumers)

actions:
  build     cargo test --no-run of every registered test target into tests.jsonl
  pack      seal postgres.tar and helper.tar from tests/schema/helper.jsonl
  unpack    validate <cohort>.tar and restore it into the workspace
  run       execute restored test executables (no test filters)
  collaboration  execute all ten sealed collaboration suites and admission gate

options:
  -h, --help                show this help message and exit
  --directory DIRECTORY     hand-off directory (required)
  --sqlite-identity ID      64-hex SQLite build input identity (pack, unpack)
  --cohort {postgres,helper}  archive to unpack (default: postgres)
  --test TEST               test executable to run (repeatable)
  --nocapture               pass --nocapture to libtest
";
const PINNED_RUSTC: &str = "rustc 1.99.0 ";
const PROFILE: &str = "dev-test-nodebug";
const BUILD_ENVIRONMENT: [&str; 6] = [
    "CARGO_INCREMENTAL",
    "CARGO_PROFILE_DEV_DEBUG",
    "CARGO_PROFILE_TEST_DEBUG",
    "RUSTFLAGS",
    "CARGO_ENCODED_RUSTFLAGS",
    "CARGO_BUILD_TARGET",
];

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Action {
    Build,
    Pack,
    Unpack,
    Run,
    Collaboration,
}

/// Shell-style exit status of a finished child: its code, or 128+N for signal
/// N. (`process::returncode` keeps Python's -N for the SQLite entry points.)
pub fn status_code(status: ExitStatus) -> i32 {
    match (status.code(), status.signal()) {
        (Some(code), _) => code,
        (None, Some(signal)) => 128 + signal,
        (None, None) => 1,
    }
}

fn is_hex(text: &str, length: usize) -> bool {
    text.len() == length && text.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f'))
}

fn is_number(text: &str) -> bool {
    !text.is_empty() && text.bytes().all(|b| b.is_ascii_digit())
}

/// Facts the identity context is built from; `None` is an unset variable.
pub struct Host {
    pub head: String,
    pub github_sha: Option<String>,
    pub runner_arch: Option<String>,
    /// `GITHUB_RUN_ID` and `GITHUB_RUN_ATTEMPT`: the artifact belongs to one run attempt.
    pub run_id: Option<String>,
    pub run_attempt: Option<String>,
    pub rustc: String,
    pub workspace: String,
    pub os_release: String,
    pub build_environment: Vec<(&'static str, Option<String>)>,
}

/// The identity a producer seals and a consumer must match exactly: source
/// SHA, workflow run and attempt, workspace path, platform, OS, pinned
/// toolchain, SQLite inputs, profile and the build-affecting environment.
pub fn context(host: &Host, sqlite_identity: Option<&str>) -> Result<Value, String> {
    if !is_hex(&host.head, 40) || host.github_sha.as_deref() != Some(host.head.as_str()) {
        return Err("Rust binary checkout SHA mismatch".into());
    }
    let (Some(run_id), Some(run_attempt)) = (
        host.run_id.as_deref().filter(|v| is_number(v)),
        host.run_attempt.as_deref().filter(|v| is_number(v)),
    ) else {
        return Err("Rust binary run/attempt identity invalid".into());
    };
    let arch = host.runner_arch.as_deref().unwrap_or_default();
    let triple = match arch {
        "X64" => "x86_64-unknown-linux-gnu",
        "ARM64" => "aarch64-unknown-linux-gnu",
        _ => return Err("Rust binary platform/SQLite identity invalid".into()),
    };
    let Some(sqlite) = sqlite_identity.filter(|id| is_hex(id, 64)) else {
        return Err("Rust binary platform/SQLite identity invalid".into());
    };
    let host_line = format!("host: {triple}");
    if !host.rustc.starts_with(PINNED_RUSTC) || !host.rustc.lines().any(|l| l == host_line) {
        return Err("Rust binary pinned toolchain mismatch".into());
    }
    let environment: serde_json::Map<String, Value> = host
        .build_environment
        .iter()
        .map(|(key, value)| ((*key).to_owned(), json!(value)))
        .collect();
    Ok(json!({
        "sha": host.head, "run_id": run_id, "run_attempt": run_attempt, "workspace": host.workspace, "arch": arch, "os": host.os_release,
        "rustc": host.rustc, "sqlite": sqlite, "profile": PROFILE,
        "build_environment": environment,
    }))
}

fn env_text(name: &str) -> Result<Option<String>, String> {
    std::env::var_os(name)
        .map(|v| v.into_string().map_err(|_| format!("{name} is not UTF-8")))
        .transpose()
}

fn output_text(program: &str, argv: &[&str]) -> Result<String, String> {
    let output = Command::new(program)
        .args(argv)
        .stdin(Stdio::null())
        .stderr(Stdio::inherit())
        .output()
        .map_err(|e| format!("{program}: {e}"))?;
    if !output.status.success() {
        return Err(format!(
            "{program} {} failed with status {}",
            argv.join(" "),
            status_code(output.status)
        ));
    }
    String::from_utf8(output.stdout).map_err(|_| format!("{program} output is not UTF-8"))
}

fn workspace() -> Result<PathBuf, String> {
    std::env::current_dir()
        .and_then(|cwd| cwd.canonicalize())
        .map_err(|e| format!("workspace: {e}"))
}

pub(crate) fn host(workspace: &Path) -> Result<Host, String> {
    Ok(Host {
        head: output_text("git", &["rev-parse", "HEAD"])?
            .trim()
            .to_owned(),
        github_sha: env_text("GITHUB_SHA")?,
        runner_arch: env_text("RUNNER_ARCH")?,
        run_id: env_text("GITHUB_RUN_ID")?,
        run_attempt: env_text("GITHUB_RUN_ATTEMPT")?,
        rustc: output_text("rustc", &["-vV"])?,
        workspace: workspace
            .to_str()
            .ok_or("workspace path is not UTF-8")?
            .to_owned(),
        os_release: fs::read_to_string("/etc/os-release")
            .map_err(|e| format!("/etc/os-release: {e}"))?,
        build_environment: BUILD_ENVIRONMENT
            .iter()
            .map(|key| env_text(key).map(|value| (*key, value)))
            .collect::<Result<_, _>>()?,
    })
}

fn read(path: &Path) -> Result<String, String> {
    fs::read_to_string(path).map_err(|e| format!("{}: {e}", path.display()))
}

fn write(path: &Path, text: &str) -> Result<(), String> {
    fs::write(path, text).map_err(|e| format!("{}: {e}", path.display()))
}

pub fn product_binaries(workspace: &Path) -> Result<Vec<String>, String> {
    let manifest = workspace.join("Cargo.toml");
    let manifest_arg = manifest.to_str().ok_or("manifest path is not UTF-8")?;
    let metadata = output_text(
        "cargo",
        &[
            "metadata",
            "--no-deps",
            "--format-version",
            "1",
            "--offline",
            "--locked",
            "--manifest-path",
            manifest_arg,
        ],
    )?;
    cohort::product_binaries(&metadata, &manifest)
}

/// `cargo test --no-run` of every test target, Cargo JSON on stdout into a new
/// `tests.jsonl`; returns Cargo's status.
fn build(directory: &Path, names: &[String]) -> Result<i32, String> {
    let mut names: BTreeSet<&str> = names.iter().map(String::as_str).collect();
    names.extend(
        cohort::collaboration_suites(cohort::COLLABORATION_SUITES)?
            .into_iter()
            .map(|(_, name)| name),
    );
    let path = directory.join("tests.jsonl");
    let output = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&path)
        .map_err(|e| format!("{}: {e}", path.display()))?;
    let mut command = Command::new("cargo");
    command
        .args([
            "test",
            "--locked",
            "--offline",
            "--features",
            "db-tests",
            "--no-run",
            "--lib",
            "--message-format=json",
        ])
        .stdout(output);
    for name in names {
        command.args(["--test", name]);
    }
    let timed = std::env::var_os("GITHUB_JOB").as_deref() == Some("postgres-build".as_ref())
        && std::env::var_os("RUNNER_ARCH").as_deref() == Some("X64".as_ref());
    let started = Instant::now();
    if timed {
        eprintln!(
            "rust-binaries stage=postgres-tests started at={}",
            utc_timestamp()
        );
    }
    let status = command.status().map(status_code);
    if timed {
        eprintln!(
            "rust-binaries stage=postgres-tests finished at={} elapsed_seconds={} exit={}",
            utc_timestamp(),
            started.elapsed().as_secs_f64().round(),
            status.as_ref().map_or(-1, |code| *code)
        );
    }
    status.map_err(|e| format!("cargo: {e}"))
}

fn entries_for(
    path: &Path,
    names: &[String],
    products: &[String],
    root: &Path,
) -> Result<BTreeMap<String, Entry>, String> {
    let records = cohort::records(&read(path)?)?;
    names
        .iter()
        .map(|name| {
            let expected = expectation(name, products);
            entry(&records, name, &expected, root).map(|entry| (name.clone(), entry))
        })
        .collect()
}

/// Select and hash every cohort member from the three Cargo JSON streams,
/// then write `helper.tar` and `postgres.tar`; a failed postgres pack also
/// removes helper.tar so a retry starts clean.
fn pack_cohorts(
    directory: &Path,
    context: &Value,
    root: &Path,
    names: &[String],
    products: &[String],
) -> Result<(), String> {
    let mut helper = entries_for(
        &directory.join("helper.jsonl"),
        &[HELPER.to_owned()],
        products,
        root,
    )?;
    let tests_and_products: Vec<String> = names
        .iter()
        .chain(products)
        .cloned()
        .chain([LIBRARY.to_owned()])
        .collect();
    let mut postgres = entries_for(
        &directory.join("tests.jsonl"),
        &tests_and_products,
        products,
        root,
    )?;
    postgres.extend(entries_for(
        &directory.join("schema.jsonl"),
        &[SCHEMA.to_owned()],
        products,
        root,
    )?);
    postgres.extend(helper.clone());
    for name in [LIBRARY, "fvoci-server"] {
        helper.insert(name.to_owned(), postgres[name].clone());
    }
    let collaboration: Vec<String> = cohort::collaboration_suites(cohort::COLLABORATION_SUITES)?
        .into_iter()
        .map(|(_, name)| name.to_owned())
        .collect();
    helper.extend(entries_for(
        &directory.join("tests.jsonl"),
        &collaboration,
        products,
        root,
    )?);
    let helper_tar = directory.join("helper.tar");
    pack(&helper_tar, context, root, &helper)?;
    pack(&directory.join("postgres.tar"), context, root, &postgres).inspect_err(|_| {
        let _ = fs::remove_file(&helper_tar);
    })
}

fn unpack_cohort(
    directory: &Path,
    cohort: &str,
    context: &Value,
    root: &Path,
    names: &[String],
    products: &[String],
) -> Result<(), String> {
    let expected: BTreeSet<String> = if cohort == "helper" {
        let mut expected: BTreeSet<String> = [
            HELPER.to_owned(),
            LIBRARY.to_owned(),
            "fvoci-server".to_owned(),
        ]
        .into();
        expected.extend(
            cohort::collaboration_suites(cohort::COLLABORATION_SUITES)?
                .into_iter()
                .map(|(_, name)| name.to_owned()),
        );
        expected
    } else {
        names
            .iter()
            .chain(products)
            .cloned()
            .chain([SCHEMA.to_owned(), HELPER.to_owned(), LIBRARY.to_owned()])
            .collect()
    };
    let manifest = unpack(
        &directory.join(format!("{cohort}.tar")),
        context,
        &expected,
        products,
        root,
    )?;
    write(
        &directory.join(format!("{cohort}-manifest.json")),
        &manifest.to_string(),
    )?;
    if cohort == "postgres" {
        // Records consumed by the selected-install step: db-tests builds only.
        let lines: Vec<String> = manifest["entries"]
            .as_object()
            .into_iter()
            .flat_map(|entries| entries.values())
            .map(|entry| &entry["record"])
            .filter(|record| record["features"] == json!([cohort::DB_TESTS]))
            .map(Value::to_string)
            .collect();
        write(
            &directory.join("selected-install-build.jsonl"),
            &lines.join("\n"),
        )?;
    }
    Ok(())
}

/// Execute each named test executable restored by `unpack`, in order, without
/// stopping at a failure (Cargo `--no-fail-fast`). Every name and executable is
/// checked before the first one starts, and each digest again right before it
/// runs. Returns the first nonzero status.
pub fn run_tests(manifest: &Value, names: &[String], libtest: &[&str]) -> Result<i32, String> {
    let unique: BTreeSet<&String> = names.iter().collect();
    if names.is_empty()
        || unique.len() != names.len()
        || libtest
            .iter()
            .any(|a| !matches!(*a, "--nocapture" | "--test-threads=1"))
    {
        return Err("Rust binary run must name unique targets without test filters".into());
    }
    let workspace = Path::new(
        manifest["context"]["workspace"]
            .as_str()
            .unwrap_or_default(),
    );
    let mut planned = Vec::new();
    for name in names {
        let entry = manifest["entries"]
            .get(name)
            .ok_or_else(|| format!("Rust binary run target not in manifest: {name}"))?;
        let (Some(path), Some(sha256)) = (entry["path"].as_str(), entry["sha256"].as_str()) else {
            return Err("Rust binary changed after validation".into());
        };
        if entry["record"]["target"]["kind"] != json!([Kind::Test.as_str()]) {
            return Err("Rust binary changed after validation".into());
        }
        planned.push((workspace.join(path), sha256));
    }
    let mut status = 0;
    for (path, sha256) in planned {
        if file_sha256(&path).ok().as_deref() != Some(sha256) {
            return Err("Rust binary changed after validation".into());
        }
        let code = Command::new(&path)
            .args(libtest)
            .status()
            .map(status_code)
            .map_err(|e| format!("{}: {e}", path.display()))?;
        if code != 0 && status == 0 {
            status = code;
        }
    }
    Ok(status)
}

fn run_restored(directory: &Path, root: &Path, parsed: &Parsed) -> Result<i32, String> {
    let manifest: Value = serde_json::from_str(&read(&directory.join("postgres-manifest.json"))?)
        .map_err(|e| format!("postgres-manifest.json: {e}"))?;
    let context = &manifest["context"];
    let root = root.to_str().map(str::to_owned);
    // Both sides must be present: an unset variable never matches a missing field.
    for (key, actual) in [
        ("sha", env_text("GITHUB_SHA")?),
        ("run_id", env_text("GITHUB_RUN_ID")?),
        ("run_attempt", env_text("GITHUB_RUN_ATTEMPT")?),
        ("arch", env_text("RUNNER_ARCH")?),
        ("workspace", root.clone()),
    ] {
        if actual.is_none() || context[key].as_str() != actual.as_deref() {
            return Err("Rust binary execution source/path mismatch".into());
        }
    }
    let libtest: &[&str] = if parsed.has("nocapture") {
        &["--nocapture"]
    } else {
        &[]
    };
    run_tests(&manifest, parsed.all("test"), libtest)
}

const ADMISSION_TEST: &str = "collab_primary_huge_varint_memory_rejected_1008";

fn verify_native_admission(stdout: &str) -> Result<(), String> {
    let prefix = format!("test {ADMISSION_TEST} ... ");
    let mut passed = 0;
    for line in stdout.split('\n') {
        if let Some(result) = line.strip_prefix(&prefix) {
            if result.starts_with("FAILED") || result.starts_with("ignored") {
                return Err(
                    "native admission test failed or was ignored in captured output".into(),
                );
            }
            passed += usize::from(result == "ok");
        }
    }
    if passed != 1 {
        return Err(format!(
            "expected exactly one passing run of {ADMISSION_TEST}, found {passed}"
        ));
    }
    Ok(())
}

/// Execute the shared inventory with default libtest scheduling for the first
/// nine targets, then the serial seed suite, retaining Cargo no-fail-fast and
/// first-nonzero semantics without invoking Cargo in the consumer.
fn run_collaboration(manifest: &Value, root: &Path, logs: &Path) -> Result<i32, String> {
    use crate::native_library::checked_entry;
    use crate::process::{run_owned_streaming, streaming_signal_status};
    let suites = cohort::collaboration_suites(cohort::COLLABORATION_SUITES)?;
    let products = ["fvoci-server".to_owned()];
    // Refuse missing/changed sealed inputs before starting any suite.
    for name in suites
        .iter()
        .map(|(_, name)| *name)
        .chain([HELPER, LIBRARY, "fvoci-server"])
    {
        checked_entry(manifest, name, root, &products)?;
    }
    fs::create_dir_all(logs).map_err(|e| format!("collaboration logs: {e}"))?;
    let mut status = 0;
    let mut admission_stdout = String::new();
    for (mode, label) in [
        ("parallel", "parallel-suites"),
        ("serial", "task_collab_integration"),
    ] {
        let stdout_path = logs.join(format!("{label}.stdout.log"));
        let stderr_path = logs.join(format!("{label}.stderr.log"));
        let stdout = fs::File::create(&stdout_path).map_err(|e| e.to_string())?;
        let stderr = fs::File::create(&stderr_path).map_err(|e| e.to_string())?;
        let mut group_status = 0;
        for (_, name) in suites.iter().filter(|(scheduling, _)| *scheduling == mode) {
            if let Some(code) = streaming_signal_status() {
                return Ok(code);
            }
            let result = (|| {
                let path = checked_entry(manifest, name, root, &products)?;
                checked_entry(manifest, HELPER, root, &products)?;
                checked_entry(manifest, "fvoci-server", root, &products)?;
                let mut command = Command::new(path);
                command.current_dir(root);
                if mode == "serial" {
                    command.arg("--test-threads=1");
                }
                run_owned_streaming(
                    &mut command,
                    [
                        stdout.try_clone().map_err(|e| e.to_string())?,
                        stderr.try_clone().map_err(|e| e.to_string())?,
                    ],
                )
                .map_err(|e| e.to_string())
            })();
            if let Some(code) = streaming_signal_status() {
                return Ok(code);
            }
            let code = match result {
                Ok(done) => {
                    admission_stdout.push_str(&String::from_utf8_lossy(&done.stdout));
                    status_code(done.status)
                }
                Err(error) => {
                    eprintln!("collaboration: {name}: {error}");
                    1
                }
            };
            if code != 0 && group_status == 0 {
                group_status = code;
            }
        }
        if group_status != 0 {
            eprintln!(
                "collaboration: {label} exited {group_status}; logs: {} {}",
                stdout_path.display(),
                stderr_path.display()
            );
            if status == 0 {
                status = group_status;
            }
        }
    }
    let admission = verify_native_admission(&admission_stdout);
    if let Some(code) = streaming_signal_status() {
        return Ok(code);
    }
    if status != 0 {
        return Ok(status);
    }
    admission?;
    Ok(0)
}

fn run_collaboration_restored(
    directory: &Path,
    root: &Path,
    parsed: &Parsed,
) -> Result<i32, String> {
    if !parsed.all("test").is_empty() || parsed.has("nocapture") || parsed.get("cohort").is_some() {
        return Err("collaboration does not allow target, filter or scheduling overrides".into());
    }
    let manifest: Value = serde_json::from_str(&read(&directory.join("helper-manifest.json"))?)
        .map_err(|e| format!("helper-manifest.json: {e}"))?;
    crate::native_library::validate_context(
        &manifest,
        &context(&host(root)?, parsed.get("sqlite-identity"))?,
    )?;
    let helper = crate::native_library::checked_entry(&manifest, HELPER, root, &[])?;
    if std::env::var_os("FVOCI_COLLAB_ENGINE").as_deref() != Some(helper.as_os_str()) {
        return Err("collaboration sealed helper input mismatch".into());
    }
    let server = crate::native_library::checked_entry(
        &manifest,
        "fvoci-server",
        root,
        &["fvoci-server".to_owned()],
    )?;
    if std::env::var_os("FVOCI_IMPORT_TEST_SERVER_BIN").as_deref() != Some(server.as_os_str()) {
        return Err("collaboration sealed server input mismatch".into());
    }
    let logs = std::env::var_os("RUNNER_TEMP")
        .map(PathBuf::from)
        .unwrap_or_else(|| directory.to_owned())
        .join("rust-collaboration-logs");
    run_collaboration(&manifest, root, &logs)
}

fn execute(action: Action, parsed: &Parsed) -> Result<i32, String> {
    let directory = Path::new(parsed.get("directory").unwrap_or_default());
    let root = workspace()?;
    let workflow = read(&root.join(cohort::WORKFLOW))?;
    let names = cohort::test_targets(&workflow)?;
    match action {
        Action::Build => return build(directory, &names),
        Action::Run => return run_restored(directory, &root, parsed),
        Action::Collaboration => return run_collaboration_restored(directory, &root, parsed),
        Action::Pack | Action::Unpack => {}
    }
    let context = context(&host(&root)?, parsed.get("sqlite-identity"))?;
    let products = product_binaries(&root)?;
    if action == Action::Pack {
        pack_cohorts(directory, &context, &root, &names, &products)?;
    } else {
        let cohort = parsed.get("cohort").unwrap_or("postgres");
        unpack_cohort(directory, cohort, &context, &root, &names, &products)?;
    }
    Ok(0)
}

fn usage(message: &str) -> i32 {
    eprintln!("{USAGE}\nxtask rust-binaries: error: {message}");
    2
}

/// Process entry: real environment, stdout and stderr.
pub fn main(argv: Vec<OsString>) -> i32 {
    let mut argv = argv.into_iter();
    let first = argv.next();
    let action = match first.as_ref().and_then(|a| a.to_str()) {
        Some("build") => Action::Build,
        Some("pack") => Action::Pack,
        Some("unpack") => Action::Unpack,
        Some("run") => Action::Run,
        Some("collaboration") => Action::Collaboration,
        Some("-h" | "--help") => {
            print!("{USAGE}\n\n{HELP}");
            return 0;
        }
        Some(other) => {
            return usage(&format!(
                "argument action: invalid choice: {other:?} (choose from build, pack, unpack, run)"
            ))
        }
        None => return usage("the following arguments are required: action"),
    };
    let specs = [
        args::required("directory"),
        args::value("sqlite-identity"),
        args::value("cohort"),
        args::repeated("test"),
        args::flag("nocapture"),
    ];
    let parsed = match args::parse(argv.collect(), &specs, false) {
        Outcome::Parsed(parsed) => parsed,
        Outcome::Help => {
            print!("{USAGE}\n\n{HELP}");
            return 0;
        }
        Outcome::Usage(message) => return usage(&message),
    };
    if let Some(cohort) = parsed
        .get("cohort")
        .filter(|c| !matches!(*c, "postgres" | "helper"))
    {
        return usage(&format!(
            "argument --cohort: invalid choice: {cohort:?} (choose from postgres, helper)"
        ));
    }
    match execute(action, &parsed) {
        Ok(code) => code,
        Err(message) => {
            eprintln!("Rust binary handoff refused: {message}");
            1
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn host() -> Host {
        Host {
            head: "a".repeat(40),
            github_sha: Some("a".repeat(40)),
            runner_arch: Some("X64".into()),
            run_id: Some("42".into()),
            run_attempt: Some("1".into()),
            rustc: "rustc 1.99.0 (b940084d7 2026-09-28)\nhost: x86_64-unknown-linux-gnu\n".into(),
            workspace: "/w".into(),
            os_release: "ID=ubuntu\n".into(),
            build_environment: vec![("CARGO_INCREMENTAL", Some("0".into())), ("RUSTFLAGS", None)],
        }
    }

    #[test]
    fn context_records_every_identity_input() {
        let value = context(&host(), Some(&"b".repeat(64))).unwrap();
        assert_eq!(
            value,
            json!({"sha": "a".repeat(40), "run_id": "42", "run_attempt": "1", "workspace": "/w", "arch": "X64", "os": "ID=ubuntu\n",
                   "rustc": host().rustc, "sqlite": "b".repeat(64), "profile": "dev-test-nodebug",
                   "build_environment": {"CARGO_INCREMENTAL": "0", "RUSTFLAGS": null}})
        );
    }

    #[test]
    fn context_refuses_foreign_source_platform_and_toolchain() {
        let sqlite = "b".repeat(64);
        type Mutation = Box<dyn Fn(&mut Host)>;
        let cases: Vec<(Mutation, Option<&str>, &str)> = vec![
            (
                Box::new(|h| h.github_sha = None),
                Some(&sqlite),
                "SHA mismatch",
            ),
            (
                Box::new(|h| h.run_attempt = None),
                Some(&sqlite),
                "run/attempt",
            ),
            (
                Box::new(|h| h.run_attempt = Some("-1".into())),
                Some(&sqlite),
                "run/attempt",
            ),
            (
                Box::new(|h| h.run_id = Some(String::new())),
                Some(&sqlite),
                "run/attempt",
            ),
            (
                Box::new(|h| h.github_sha = Some("c".repeat(40))),
                Some(&sqlite),
                "SHA mismatch",
            ),
            (
                Box::new(|h| h.head = "A".repeat(40)),
                Some(&sqlite),
                "SHA mismatch",
            ),
            (
                Box::new(|h| h.runner_arch = Some("x64".into())),
                Some(&sqlite),
                "identity invalid",
            ),
            (
                Box::new(|h| h.runner_arch = None),
                Some(&sqlite),
                "identity invalid",
            ),
            (Box::new(|_| {}), None, "identity invalid"),
            (Box::new(|_| {}), Some("B"), "identity invalid"),
            (
                Box::new(|h| h.runner_arch = Some("ARM64".into())),
                Some(&sqlite),
                "toolchain mismatch",
            ),
            (
                Box::new(|h| h.rustc = h.rustc.replace("1.99.0", "1.98.1")),
                Some(&sqlite),
                "toolchain mismatch",
            ),
        ];
        for (mutate, identity, message) in cases {
            let mut facts = host();
            mutate(&mut facts);
            let error = context(&facts, identity).unwrap_err();
            assert!(error.contains(message), "{error}");
        }
    }

    #[test]
    fn signal_status_is_128_plus_signal() {
        let status = Command::new("sh")
            .args(["-c", "kill -TERM $$"])
            .status()
            .unwrap();
        assert_eq!(status_code(status), 128 + 15);
        let status = Command::new("sh").args(["-c", "exit 7"]).status().unwrap();
        assert_eq!(status_code(status), 7);
    }
}
