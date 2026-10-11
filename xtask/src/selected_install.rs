//! `xtask selected-install <compiler-artifacts.jsonl> <collab-engine>`: the
//! privileged SQLite install lifetime controls of rust.yml's PG18 B shards.
//!
//! It selects the db-tests `fvoci-server`, `fvoci-migrate` and
//! `selected_install_lifetime` executables from the restored Cargo records,
//! makes them and the production collaboration helper root-owned 0755 in
//! place, allocates a root-owned run root under `/run` (root:1000, 0710) with a
//! service-owned storage directory and a private 0600 environment file, and
//! runs the test executable unfiltered with a replaced environment. The test
//! output and its receipts are printed before the status and the exact
//! 4-passed count are checked; the run root is always removed.
//!
//! It must run as root, from the workspace (every file it changes lies under
//! the current directory): the caller runs the already built binary under
//! sudo and never runs Cargo as root.
//!
//! Exit status: 0, 1 (refused or failed), 2 (usage).

use crate::ci_fixture::{self, capture, Streams};
use crate::host::{hex, mkdtemp};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::ffi::OsString;
use std::fs::{self, DirBuilder, File, OpenOptions, Permissions};
use std::io::{Read, Write};
use std::os::unix::fs::{fchown, DirBuilderExt, OpenOptionsExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::process::Command;

const USAGE: &str = "usage: xtask selected-install [-h] ARTIFACTS ENGINE";
const HELP: &str = "\
Selected SQLite install lifetime controls (rust.yml PG18 B shards; run as root)

positional arguments:
  ARTIFACTS   selected-install-build.jsonl written by rust-binaries unpack
  ENGINE      production collab-engine executable

options:
  -h, --help  show this help message and exit
";
const RUN_PARENT: &str = "/run";
const SERVICE_GID: u32 = 1000;
const SERVICE_UID: u32 = 1000;
const PATH: &str = "/usr/local/sbin:/usr/local/bin:/usr/sbin:/usr/bin:/sbin:/bin";
const COUNT: &str = "test result: ok. 4 passed; 0 failed; 0 ignored;";
const DB_TESTS: &str = "db-tests";

/// The three executables taken from the Cargo records.
#[derive(Debug, PartialEq, Eq)]
pub struct Selection {
    pub server: PathBuf,
    pub migrate: PathBuf,
    pub test: PathBuf,
}

/// `compiler-artifact` records with a non-empty executable, each with the
/// fields selection reads.
fn artifacts(text: &str) -> Result<Vec<Value>, String> {
    let mut artifacts = Vec::new();
    for (index, line) in text.lines().enumerate() {
        let line_no = index + 1;
        let record: Value = serde_json::from_str(line)
            .map_err(|e| format!("compiler artifact line {line_no} is not JSON: {e}"))?;
        if !record.is_object() {
            return Err(format!("compiler artifact line {line_no} is not an object"));
        }
        if record["reason"] != "compiler-artifact" {
            continue;
        }
        match &record["executable"] {
            Value::Null => continue,
            Value::String(path) if path.is_empty() => continue,
            Value::String(_) => {}
            _ => {
                return Err(format!(
                    "compiler artifact line {line_no}: executable is not a string"
                ))
            }
        }
        let kinds_ok = record["target"]["kind"]
            .as_array()
            .is_some_and(|kinds| kinds.iter().all(Value::is_string));
        if !record["target"]["name"].is_string()
            || !kinds_ok
            || !record["profile"]["test"].is_boolean()
        {
            return Err(format!(
                "compiler artifact line {line_no}: target name/kind or profile.test malformed"
            ));
        }
        artifacts.push(record);
    }
    Ok(artifacts)
}

fn executable(artifacts: &[Value], name: &str, kind: &str, test: bool) -> Result<PathBuf, String> {
    let matches: Vec<&Value> = artifacts
        .iter()
        .filter(|r| {
            r["target"]["name"] == name
                && r["target"]["kind"]
                    .as_array()
                    .is_some_and(|kinds| kinds.iter().any(|k| k == kind))
                && r["profile"]["test"] == test
        })
        .collect();
    let [record] = matches.as_slice() else {
        return Err(format!(
            "{name}: {} matching {kind} artifacts, expected exactly one",
            matches.len()
        ));
    };
    let db_tests = record["features"]
        .as_array()
        .is_some_and(|features| features.iter().any(|f| f == DB_TESTS));
    if !db_tests {
        return Err(format!("{name}: artifact was not built with {DB_TESTS}"));
    }
    Ok(PathBuf::from(
        record["executable"].as_str().unwrap_or_default(),
    ))
}

/// Exactly one db-tests artifact per executable; the server and the migrator
/// that execs it are siblings.
pub fn select(text: &str) -> Result<Selection, String> {
    let artifacts = artifacts(text)?;
    let selection = Selection {
        server: executable(&artifacts, "fvoci-server", "bin", false)?,
        migrate: executable(&artifacts, "fvoci-migrate", "bin", false)?,
        test: executable(&artifacts, "selected_install_lifetime", "test", true)?,
    };
    if selection.server.parent() != selection.migrate.parent() {
        return Err("fvoci-server and fvoci-migrate must be siblings".into());
    }
    Ok(selection)
}

/// The private environment file the test hands to the installed service.
pub fn environment(storage: &str, engine: &str, pepper: &str, encryption: &str) -> Value {
    json!({
        "PATH": PATH,
        "PASSWORD_PEPPER_KEYS": json!({"fixture": pepper}).to_string(),
        "PASSWORD_PEPPER_ACTIVE_KEY_ID": "fixture",
        "ENCRYPTION_KEYS": json!({"fixture": encryption}).to_string(),
        "ENCRYPTION_ACTIVE_KEY_ID": "fixture",
        "FVOCI_PUBLIC_ORIGIN": "http://127.0.0.1:8080",
        "FVOCI_COOKIE_SECURE": "0",
        "STORAGE_DRIVER": "local",
        "FVOCI_STORAGE_DIR": storage,
        "FVOCI_COLLAB_ENGINE": engine,
        "FVOCI_COLLAB_FAMILY_LEASE_MS": "30000",
        "FVOCI_COLLAB_FAMILY_RENEW_MS": "5000",
    })
}

/// The test ran exactly four controls.
pub fn check_count(output: &str) -> Result<(), String> {
    if !output.contains(COUNT) {
        return Err("selected install must report exactly 4 passed controls".into());
    }
    Ok(())
}

// ---- filesystem, ownership and process I/O ----

fn failed(path: &Path) -> impl Fn(std::io::Error) -> String + '_ {
    move |e| format!("{}: {e}", path.display())
}

/// Open an absolute path under the workspace (the current directory) whose
/// final component is itself a regular file: not a symlink, and a FIFO is
/// refused without blocking on its open.
fn open_regular(path: &Path, workspace: &Path) -> Result<File, String> {
    if !path.is_absolute() {
        return Err(format!("{} is not absolute", path.display()));
    }
    let parent = path
        .parent()
        .ok_or_else(|| format!("{} has no parent", path.display()))?;
    let parent = parent.canonicalize().map_err(failed(parent))?;
    if !parent.starts_with(workspace) {
        return Err(format!(
            "{} is outside the workspace {}",
            path.display(),
            workspace.display()
        ));
    }
    let file = OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
        .open(path)
        .map_err(failed(path))?;
    if !file.metadata().map_err(failed(path))?.is_file() {
        return Err(format!("{} is not a regular file", path.display()));
    }
    Ok(file)
}

/// root:root 0755 through the open descriptor.
fn make_root_executable(path: &Path, file: &File) -> Result<(), String> {
    fchown(file, Some(0), Some(0)).map_err(failed(path))?;
    file.set_permissions(Permissions::from_mode(0o755))
        .map_err(failed(path))
}

/// Streamed SHA-256 of the open descriptor's content.
fn digest(path: &Path, mut file: &File) -> Result<String, String> {
    let mut hasher = Sha256::new();
    let mut buffer = vec![0u8; 1 << 16];
    loop {
        let read = file.read(&mut buffer).map_err(failed(path))?;
        if read == 0 {
            return Ok(hex(&hasher.finalize()));
        }
        hasher.update(&buffer[..read]);
    }
}

fn set_owner(path: &Path, uid: u32, gid: u32, mode: u32) -> Result<(), String> {
    std::os::unix::fs::chown(path, Some(uid), Some(gid)).map_err(failed(path))?;
    fs::set_permissions(path, Permissions::from_mode(mode)).map_err(failed(path))
}

/// `run_root/receipts-*/*.json`, sorted as Python sorts paths (by component).
fn receipts(run_root: &Path) -> Result<Vec<PathBuf>, String> {
    let names = |dir: &Path| -> Result<Vec<OsString>, String> {
        fs::read_dir(dir)
            .map_err(failed(dir))?
            .map(|entry| entry.map(|e| e.file_name()).map_err(failed(dir)))
            .collect()
    };
    let mut found = Vec::new();
    for directory in names(run_root)? {
        let path = run_root.join(&directory);
        if !directory.as_encoded_bytes().starts_with(b"receipts-") || !path.is_dir() {
            continue;
        }
        for name in names(&path)? {
            if name.as_encoded_bytes().ends_with(b".json") {
                found.push((directory.clone(), name));
            }
        }
    }
    found.sort();
    Ok(found
        .into_iter()
        .map(|(directory, name)| run_root.join(directory).join(name))
        .collect())
}

fn controls(
    selection: &Selection,
    engine: &Path,
    hashes: &Hashes,
    run_root: &Path,
) -> Result<(), String> {
    set_owner(run_root, 0, SERVICE_GID, 0o710)?;
    let storage = run_root.join("storage");
    DirBuilder::new()
        .mode(0o700)
        .create(&storage)
        .map_err(failed(&storage))?;
    set_owner(&storage, SERVICE_UID, SERVICE_GID, 0o700)?;
    let utf8 = |path: &Path| {
        path.to_str()
            .map(str::to_owned)
            .ok_or_else(|| format!("{} is not UTF-8", path.display()))
    };
    let environment = environment(
        &utf8(&storage)?,
        &utf8(engine)?,
        &ci_fixture::random_hex(32)?,
        &ci_fixture::random_hex(32)?,
    );
    let private = run_root.join("environment.json");
    ci_fixture::write_private(&private, environment.to_string().as_bytes())?;
    let mut command = Command::new(&selection.test);
    command
        .args(["--test-threads=1", "--nocapture"])
        .env_clear()
        .env("PATH", PATH)
        .env("FVOCI_SELECTED_INSTALL_RUN_ROOT", run_root)
        .env("FVOCI_SELECTED_INSTALL_ENV_FILE", &private)
        .env("FVOCI_SELECTED_INSTALL_MIGRATE_SHA256", &hashes.migrate)
        .env("FVOCI_SELECTED_INSTALL_SERVER_SHA256", &hashes.server);
    let done = capture(&mut command, None, Streams::Merged, true)
        .map_err(|e| format!("{}: {e}", selection.test.display()))?;
    let mut stdout = std::io::stdout().lock();
    stdout
        .write_all(&done.stdout)
        .map_err(|e| format!("stdout: {e}"))?;
    stdout.flush().map_err(|e| format!("stdout: {e}"))?;
    for receipt in receipts(run_root)? {
        let text = fs::read_to_string(&receipt).map_err(failed(&receipt))?;
        let name = receipt.file_name().unwrap_or_default().to_string_lossy();
        writeln!(stdout, "selected-install receipt {name} {text}")
            .map_err(|e| format!("stdout: {e}"))?;
        stdout.flush().map_err(|e| format!("stdout: {e}"))?;
    }
    if !done.status.success() {
        return Err(format!(
            "selected install test exit={}",
            crate::process::returncode(done.status)
        ));
    }
    let output =
        String::from_utf8(done.stdout).map_err(|_| "selected install output is not UTF-8")?;
    check_count(&output)?;
    if ci_fixture::interrupted() {
        return Err("interrupted".into());
    }
    Ok(())
}

struct Hashes {
    migrate: String,
    server: String,
}

fn execute(artifacts: &Path, engine: &Path) -> Result<(), String> {
    let text = fs::read_to_string(artifacts).map_err(failed(artifacts))?;
    let selection = select(&text)?;
    let cwd = Path::new(".");
    let workspace = cwd.canonicalize().map_err(failed(cwd))?;
    let open = |path: &Path| open_regular(path, &workspace);
    let server = open(&selection.server)?;
    let migrate = open(&selection.migrate)?;
    let test = open(&selection.test)?;
    let helper = open(engine)?;
    // SAFETY: geteuid has no preconditions and cannot fail.
    if unsafe { libc::geteuid() } != 0 {
        return Err("must run as root (sudo of the prebuilt xtask binary)".into());
    }
    for (path, file) in [
        (selection.server.as_path(), &server),
        (selection.migrate.as_path(), &migrate),
        (selection.test.as_path(), &test),
        (engine, &helper),
    ] {
        make_root_executable(path, file)?;
    }
    let hashes = Hashes {
        server: digest(&selection.server, &server)?,
        migrate: digest(&selection.migrate, &migrate)?,
    };
    ci_fixture::catch_interrupt()?;
    let run_root = mkdtemp("fvoci-selected-install-", Path::new(RUN_PARENT))
        .map_err(|e| format!("{RUN_PARENT}: {e}"))?;
    let outcome = controls(&selection, engine, &hashes, &run_root);
    let removed = fs::remove_dir_all(&run_root).map_err(failed(&run_root));
    // A cancellation during cleanup still fails the step.
    let interrupted = if ci_fixture::interrupted() {
        Err("interrupted".to_owned())
    } else {
        Ok(())
    };
    outcome.and(removed).and(interrupted)
}

/// Process entry: real environment, stdout and stderr.
pub fn main(argv: Vec<OsString>) -> i32 {
    if argv.len() == 1 && matches!(argv[0].to_str(), Some("-h" | "--help")) {
        print!("{USAGE}\n\n{HELP}");
        return 0;
    }
    let [artifacts, engine] = argv.as_slice() else {
        eprintln!("{USAGE}\nxtask selected-install: error: expected exactly ARTIFACTS and ENGINE");
        return 2;
    };
    if [artifacts, engine]
        .iter()
        .any(|a| a.is_empty() || a.as_encoded_bytes().starts_with(b"-"))
    {
        eprintln!("{USAGE}\nxtask selected-install: error: ARTIFACTS and ENGINE must be paths");
        return 2;
    }
    match execute(Path::new(artifacts), Path::new(engine)) {
        Ok(()) => 0,
        Err(message) => {
            eprintln!("selected install refused or failed: {message}");
            1
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn record(name: &str, kind: &str, test: bool, path: &str) -> Value {
        json!({"reason": "compiler-artifact", "target": {"name": name, "kind": [kind]},
               "profile": {"test": test}, "features": ["db-tests"], "executable": path})
    }

    fn healthy() -> Vec<Value> {
        vec![
            record("fvoci-server", "bin", false, "/fixture/bin/fvoci-server"),
            record("fvoci-migrate", "bin", false, "/fixture/bin/fvoci-migrate"),
            record(
                "selected_install_lifetime",
                "test",
                true,
                "/fixture/bin/deps/selected_install_lifetime",
            ),
        ]
    }

    fn lines(records: &[Value]) -> String {
        records
            .iter()
            .map(Value::to_string)
            .collect::<Vec<_>>()
            .join("\n")
    }

    #[test]
    fn selects_the_three_db_tests_executables() {
        let mut records = healthy();
        records.push(json!({"reason": "build-script-executed"}));
        records.push(json!({"reason": "compiler-artifact", "executable": null}));
        records.push(record("fvoci-server", "bin", true, ""));
        records.push(record(
            "fvoci-server",
            "lib",
            false,
            "/fixture/libfvoci.rlib",
        ));
        records.push(record(
            "fvoci-server",
            "bin",
            true,
            "/fixture/bin/deps/fvoci_server-test",
        ));
        assert_eq!(
            select(&lines(&records)).unwrap(),
            Selection {
                server: "/fixture/bin/fvoci-server".into(),
                migrate: "/fixture/bin/fvoci-migrate".into(),
                test: "/fixture/bin/deps/selected_install_lifetime".into(),
            }
        );
        assert!(select(&format!("{}\n", lines(&records))).is_ok());
    }

    #[test]
    fn refuses_missing_ambiguous_featureless_and_split_artifacts() {
        type Mutation = fn(&mut Vec<Value>);
        let cases: [(Mutation, &str); 9] = [
            (|r| drop(r.remove(0)), "fvoci-server: 0 matching"),
            (|r| r.push(r[1].clone()), "fvoci-migrate: 2 matching"),
            (
                |r| drop(r.remove(2)),
                "selected_install_lifetime: 0 matching",
            ),
            (
                |r| r[2]["profile"]["test"] = json!(false),
                "selected_install_lifetime: 0 matching",
            ),
            (|r| r[0]["features"] = json!([]), "not built with db-tests"),
            (
                |r| r[1]["features"] = json!(["default"]),
                "not built with db-tests",
            ),
            (
                |r| r[2]["features"] = json!("db-tests"),
                "not built with db-tests",
            ),
            (
                |r| r[1]["executable"] = json!("/fixture/other-bin/fvoci-migrate"),
                "siblings",
            ),
            (|r| r[0]["target"]["kind"] = json!("bin"), "malformed"),
        ];
        for (mutate, message) in cases {
            let mut records = healthy();
            mutate(&mut records);
            let error = select(&lines(&records)).unwrap_err();
            assert!(error.contains(message), "{message}: {error}");
        }
    }

    #[test]
    fn refuses_malformed_jsonl() {
        let healthy = lines(&healthy());
        for text in [
            String::new(),
            format!("{healthy}\n\n{healthy}"),
            format!("{healthy}\nnot json"),
            format!("{healthy}\n[]"),
            format!("{healthy}\n{{\"reason\":\"compiler-artifact\",\"executable\":5}}"),
            format!("{healthy}\n{{\"reason\":\"compiler-artifact\",\"executable\":\"/x\",\"target\":{{\"name\":\"x\",\"kind\":[\"bin\"]}},\"profile\":{{\"test\":1}}}}"),
            format!("{healthy}\n{{\"reason\":\"compiler-artifact\",\"executable\":\"/x\"}}"),
        ] {
            assert!(select(&text).is_err(), "{text:?}");
        }
    }

    #[test]
    fn environment_names_storage_engine_and_fixture_keys() {
        let value = environment("/run/r/storage", "/e/collab-engine", "aa", "bb");
        assert_eq!(value["FVOCI_STORAGE_DIR"], "/run/r/storage");
        assert_eq!(value["FVOCI_COLLAB_ENGINE"], "/e/collab-engine");
        assert_eq!(value["PASSWORD_PEPPER_KEYS"], "{\"fixture\":\"aa\"}");
        assert_eq!(value["ENCRYPTION_KEYS"], "{\"fixture\":\"bb\"}");
        assert_eq!(value.as_object().unwrap().len(), 12);
    }

    #[test]
    fn count_must_be_exactly_four() {
        assert!(check_count(&format!("running\n{COUNT} finished\n")).is_ok());
        assert!(check_count(&COUNT.replace("4 passed", "0 passed")).is_err());
        assert!(check_count(&COUNT.replace("4 passed", "40 passed")).is_err());
    }
}
