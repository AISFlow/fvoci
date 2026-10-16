//! `xtask sqlite-build`: authenticated native SQLite static build for SQLx
//! 0.9.0 `sqlite-unbundled` / libsqlite3-sys 0.37.0. No downloads, system
//! installation, Cargo source edits, CLI or extension artifacts.
//! Entry point: `bash scripts/prepare-sqlite-build.sh --archive FILE
//! --prefix NEW_PATH --target x86_64-unknown-linux-gnu [--cc cc] [--ar ar]`.
//! Source PREFIX/env.sh only after success, with the matching Rust target.

use crate::args::{self, Outcome};
use crate::host::{self, sha256_hex};
use crate::process::{self, Timing};
use crate::shell;
use crate::sqlite::{
    helper_sha256, ARCHIVE_HASH, C_HASH, HEADER_HASH, SIZE_LIMIT, SOURCE_ID, VERSION,
};
use crate::sqlite_zip;
use serde_json::{json, Map, Value};
use sha3::{Digest, Sha3_256};
use std::collections::BTreeSet;
use std::ffi::OsString;
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::os::unix::fs::OpenOptionsExt;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{Duration, Instant};

const USAGE: &str = "usage: prepare-sqlite-build.sh [-h] --archive ARCHIVE --prefix PREFIX --target TARGET [--cc CC] [--ar AR] [--identity-only]";
const HELP: &str = "\
Authenticated native SQLite static build; no fallback

options:
  -h, --help       show this help message and exit
  --archive ARCHIVE
  --prefix PREFIX
  --target TARGET
  --cc CC          single native compiler executable
  --ar AR          single archiver executable
  --identity-only  authenticate native inputs without compiling SQLite or exporting a usable prefix
";

// Required SQLx C APIs plus conservative libsqlite3-sys bundled defaults.
// JSON is built in upstream. Optional FTS/RTREE/preupdate features are not enabled.
// SQLx 0.8.6 always referenced load_extension; 0.9.0 does only with its
// `sqlite-load-extension` feature, which FVOCI leaves off. The API stays built,
// disabled by default at connection open, so the build inputs do not change.
// No extension is built, loaded, or installed here.
pub const FLAGS: [&str; 13] = [
    "-O2",
    "-fPIC",
    "-std=c11",
    "-pthread",
    "-DSQLITE_CORE",
    "-DSQLITE_THREADSAFE=1",
    "-DSQLITE_DEFAULT_FOREIGN_KEYS=1",
    "-DSQLITE_ENABLE_API_ARMOR",
    "-DSQLITE_ENABLE_COLUMN_METADATA",
    "-DSQLITE_ENABLE_UNLOCK_NOTIFY",
    "-DSQLITE_USE_URI",
    "-DHAVE_USLEEP=1",
    "-DHAVE_ISNAN=1",
];
pub const LINK_FLAGS: [&str; 3] = ["-pthread", "-ldl", "-lm"];
/// The only environment native tools receive.
pub const ENV: [(&str, &str); 3] = [("PATH", "/usr/bin:/bin"), ("LC_ALL", "C"), ("LANG", "C")];
/// Required consumer inputs cannot be removed by editing the cache manifest.
pub const REQUIRED_OUTPUTS: [&str; 8] = [
    "env.sh",
    "include/sqlite3.h",
    "lib/libsqlite3.a",
    "proof.txt",
    "smoke",
    "smoke.c",
    "sqlite3.c",
    "sqlite3.o",
];
pub const COMPILE_TIMEOUT: Duration = Duration::from_secs(180);
// Hosted Rust run 37740220181 postgres-c ar exceeded 30s; use the C compile's bounded 180s class.
pub const ARCHIVE_TIMEOUT: Duration = COMPILE_TIMEOUT;
const INSPECTION_TIMEOUT: Duration = Duration::from_secs(30);

// Every function API the SQLx SQLite driver calls with any of its optional
// APIs, including metadata, notify, hooks, serialize/deserialize, progress and
// extension entry points (0.9.0 calls serialize/deserialize and
// load_extension only behind features FVOCI leaves off).
// Type names and optional preupdate/regexp implementation names are excluded.
const SYMBOLS: &str = "bind_blob64 bind_double bind_int bind_int64 bind_null
bind_parameter_count bind_parameter_name bind_text64 busy_timeout changes
clear_bindings close column_blob column_bytes column_count column_database_name
column_decltype column_double column_int column_int64 column_name
column_origin_name column_table_name column_type column_value commit_hook
create_collation_v2 create_function_v2 db_config db_handle deserialize errmsg
errstr exec extended_errcode extended_result_codes finalize free get_autocommit
get_auxdata last_insert_rowid load_extension malloc malloc64 open open_v2
prepare_v2 prepare_v3 progress_handler reset result_error_code result_int
rollback_hook serialize set_auxdata sql step stmt_readonly table_column_metadata
unlock_notify update_hook value_blob value_bytes value_double value_dup
value_free value_int value_int64 value_text value_type";

const SMOKE_TEMPLATE: &str = r#"
#include "sqlite3.h"
#include <stdio.h>
#include <string.h>
static void (*volatile required[])(void) = { SYMBOL_ADDRESSES };
static int scalar(sqlite3 *db, const char *sql, int expected) {
    sqlite3_stmt *s = 0;
    int ok = sqlite3_prepare_v2(db, sql, -1, &s, 0) == SQLITE_OK &&
        sqlite3_step(s) == SQLITE_ROW && sqlite3_column_int(s, 0) == expected;
    sqlite3_finalize(s);
    return ok;
}
static int check(sqlite3 *db) {
    sqlite3_stmt *s = 0;
    int enabled = -1;
    if (sqlite3_db_config(db, SQLITE_DBCONFIG_ENABLE_LOAD_EXTENSION, -1,
                         &enabled) != SQLITE_OK || enabled != 0) return 10;
    if (!scalar(db, "PRAGMA foreign_keys", 1)) return 11;
    if (sqlite3_exec(db, "PRAGMA journal_mode=WAL;PRAGMA synchronous=FULL;",
                     0, 0, 0) != SQLITE_OK ||
        !scalar(db, "PRAGMA synchronous", 2)) return 19;
    if (sqlite3_prepare_v2(db, "PRAGMA journal_mode", -1, &s, 0) != SQLITE_OK) return 20;
    int wal = sqlite3_step(s) == SQLITE_ROW &&
        strcmp((const char *)sqlite3_column_text(s, 0), "wal") == 0;
    sqlite3_finalize(s);
    s = 0;
    if (!wal) return 21;
    if (sqlite3_exec(db, "CREATE TABLE p(id INTEGER PRIMARY KEY);"
        "CREATE TABLE c(id INTEGER REFERENCES p(id));BEGIN IMMEDIATE;"
        "INSERT INTO p VALUES(1);ROLLBACK;", 0, 0, 0) != SQLITE_OK) return 12;
    if (!sqlite3_get_autocommit(db) || !scalar(db, "SELECT count(*) FROM p", 0)) return 13;
    if (sqlite3_exec(db, "INSERT INTO c VALUES(999)", 0, 0, 0) != SQLITE_CONSTRAINT) return 14;
    if (sqlite3_exec(db, "BEGIN IMMEDIATE;INSERT INTO p VALUES(1);"
        "INSERT INTO c VALUES(1);COMMIT;", 0, 0, 0) != SQLITE_OK) return 15;
    if (!scalar(db, "SELECT count(*) FROM c", 1) ||
        !scalar(db, "SELECT json_valid('{\"ok\":true}')", 1)) return 16;
    if (sqlite3_prepare_v3(db, "SELECT id FROM p", -1, SQLITE_PREPARE_PERSISTENT,
                         &s, 0) != SQLITE_OK) return 17;
    const char *origin = sqlite3_column_origin_name(s, 0);
    int ok = origin && strcmp(origin, "id") == 0;
    sqlite3_finalize(s);
    if (!ok || sqlite3_unlock_notify(db, 0, 0) != SQLITE_OK) return 18;
    return 0;
}
int main(int argc, char **argv) {
    if (argc != 2) return 7;
    for (unsigned i = 0; i < sizeof(required)/sizeof(required[0]); ++i)
        if (!required[i]) return 1;
    if (strcmp(sqlite3_libversion(), "3.53.4") ||
        strcmp(sqlite3_sourceid(), "EXPECTED_SOURCE_ID") ||
        sqlite3_libversion_number() != 3053004 || sqlite3_threadsafe() != 1) return 2;
    const char *needed[] = {"ENABLE_COLUMN_METADATA", "ENABLE_UNLOCK_NOTIFY",
        "ENABLE_API_ARMOR", "DEFAULT_FOREIGN_KEYS", "THREADSAFE=1", "USE_URI"};
    for (unsigned i = 0; i < sizeof(needed)/sizeof(needed[0]); ++i)
        if (!sqlite3_compileoption_used(needed[i])) return 3;
    const char *omitted[] = {"OMIT_FOREIGN_KEY", "OMIT_TRIGGER", "OMIT_WAL",
        "OMIT_LOAD_EXTENSION", "OMIT_PROGRESS_CALLBACK", "OMIT_SHARED_CACHE",
        "ENABLE_LOAD_EXTENSION"};
    for (unsigned i = 0; i < sizeof(omitted)/sizeof(omitted[0]); ++i)
        if (sqlite3_compileoption_used(omitted[i])) return 4;
    sqlite3 *db = 0;
    if (sqlite3_open_v2(argv[1], &db, SQLITE_OPEN_READWRITE |
        SQLITE_OPEN_CREATE | SQLITE_OPEN_NOMUTEX, 0) != SQLITE_OK) return 5;
    int result = check(db);
    if (sqlite3_close(db) != SQLITE_OK) return 6;
    if (result) return result;
    if (sqlite3_open_v2(argv[1], &db, SQLITE_OPEN_READWRITE |
        SQLITE_OPEN_NOMUTEX, 0) != SQLITE_OK) return 22;
    if (!scalar(db, "SELECT count(*) FROM c", 1) ||
        !scalar(db, "PRAGMA foreign_keys", 1)) return 23;
    if (sqlite3_close(db) != SQLITE_OK) return 24;
    printf("version=%s\nsource_id=%s\n", sqlite3_libversion(), sqlite3_sourceid());
    for (int i = 0; sqlite3_compileoption_get(i); ++i)
        printf("compile_option=%s\n", sqlite3_compileoption_get(i));
    puts("sqlx_symbols/metadata/unlock_notify/threadsafe/fk/rollback/commit/json/wal/full/fresh_connection/extensions_disabled=PASS");
    return 0;
}
"#;

/// The smoke program, byte-identical to the former helper's `SMOKE`.
pub fn smoke_source() -> String {
    let addresses = SYMBOLS
        .split_whitespace()
        .map(|symbol| format!("(void (*)(void))sqlite3_{symbol}"))
        .collect::<Vec<_>>()
        .join(", ");
    SMOKE_TEMPLATE
        .replace("EXPECTED_SOURCE_ID", SOURCE_ID)
        .replace("SYMBOL_ADDRESSES", &addresses)
}

type Failure = String;

struct Runner {
    timings: Map<String, Value>,
}

impl Runner {
    fn run(&mut self, argv: &[String], timeout: Duration, step: &str) -> Result<String, Failure> {
        let (stdout, stderr) = self.output(argv, timeout, step)?;
        if !stderr.is_empty() {
            eprint!("{stderr}");
        }
        Ok(stdout)
    }

    fn output(
        &mut self,
        argv: &[String],
        timeout: Duration,
        step: &str,
    ) -> Result<(String, String), Failure> {
        eprintln!("+ {}", shell::join(argv));
        let started = Instant::now();
        let mut command = Command::new(&argv[0]);
        command.args(&argv[1..]).env_clear().envs(ENV);
        let result = process::run(&mut command, timeout, true);
        let status = match &result {
            Ok(done) => process::returncode(done.status),
            Err(_) => -1,
        };
        let timing = Timing::new(started.elapsed(), timeout, status);
        eprintln!("sqlite-prepare step={step} {}", timing.log_line());
        self.timings.insert(step.to_owned(), timing.to_json());
        let done = result.map_err(|error| match error {
            process::RunError::Timeout(_) => format!(
                "Command '{}' timed out after {} seconds",
                shell::join(argv),
                timeout.as_secs()
            ),
            other => other.to_string(),
        })?;
        let stdout = String::from_utf8(done.stdout).map_err(|e| e.to_string())?;
        let stderr = String::from_utf8(done.stderr).map_err(|e| e.to_string())?;
        if status != 0 {
            return Err(format!("command exit {status}: {stdout}{stderr}"));
        }
        Ok((stdout, stderr))
    }

    fn tool(&mut self, name: &str) -> Result<(PathBuf, Value), Failure> {
        let path_var = std::env::var_os("PATH");
        let found = host::which(name, path_var.as_deref())
            .ok_or_else(|| format!("tool unavailable: {name}"))?;
        let path = fs::canonicalize(found).map_err(|e| e.to_string())?;
        let display = path.to_string_lossy().into_owned();
        let bytes = fs::read(&path).map_err(|e| e.to_string())?;
        let version = self.run(
            &[display.clone(), "--version".to_owned()],
            INSPECTION_TIMEOUT,
            &format!("{name}_version"),
        )?;
        Ok((
            path,
            json!({"path": display, "sha256": sha256_hex(&bytes), "version": version}),
        ))
    }

    fn compiler_file(&mut self, cc: &str, name: &str) -> Result<PathBuf, Failure> {
        let found = self.run(
            &[cc.to_owned(), format!("-print-file-name={name}")],
            INSPECTION_TIMEOUT,
            &format!("compiler_file_{name}"),
        )?;
        let path = PathBuf::from(found.trim());
        if path == Path::new(name) || !path.is_absolute() || !path.exists() {
            return Err(format!(
                "compiler did not resolve required native input: {name}"
            ));
        }
        Ok(path)
    }

    /// Preprocessing and the driver's dry-run describe the actual headers,
    /// subprograms and CRT/link inputs without compiling a usable prefix.
    fn native_inputs(
        &mut self,
        cc: &str,
        ar: &str,
        parent: &Path,
        source: &[u8],
        header: &[u8],
    ) -> Result<Value, Failure> {
        let stage = host::mkdtemp("sqlite-inputs-", parent).map_err(io)?;
        let result = (|| {
            fs::write(stage.join("sqlite3.c"), source).map_err(io)?;
            fs::write(stage.join("sqlite3.h"), header).map_err(io)?;
            fs::write(stage.join("smoke.c"), smoke_source()).map_err(io)?;
            let mut paths = BTreeSet::from([PathBuf::from(cc), PathBuf::from(ar)]);
            let mut libraries = BTreeSet::new();
            let mut search = BTreeSet::new();
            let include = self.compiler_file(cc, "include")?;
            paths.insert(
                include
                    .parent()
                    .ok_or("compiler resource directory missing")?
                    .to_path_buf(),
            );
            for (name, flags) in [
                ("sqlite3.c", FLAGS.to_vec()),
                ("smoke.c", vec!["-std=c11", "-pthread"]),
            ] {
                let file = text(&stage.join(name));
                let mut argv = vec![cc.to_owned()];
                argv.extend(flags.iter().map(|s| (*s).to_owned()));
                argv.extend([
                    "-M".to_owned(),
                    "-MT".to_owned(),
                    "sqlite-native-inputs".to_owned(),
                    file.clone(),
                ]);
                let deps = self.run(&argv, INSPECTION_TIMEOUT, &format!("headers_{name}"))?;
                paths.extend(header_paths(&deps, &stage)?);
                let mut argv = vec![cc.to_owned(), "-###".to_owned()];
                argv.extend(flags.iter().map(|s| (*s).to_owned()));
                if name == "sqlite3.c" {
                    argv.push("-c".to_owned());
                }
                argv.extend([
                    file,
                    "-o".to_owned(),
                    text(&stage.join("inspection-output")),
                ]);
                if name == "smoke.c" {
                    argv.extend(LINK_FLAGS.iter().map(|s| (*s).to_owned()));
                }
                let (stdout, stderr) =
                    self.output(&argv, INSPECTION_TIMEOUT, &format!("driver_{name}"))?;
                let mut commands = 0;
                for raw_line in stdout.lines().chain(stderr.lines()) {
                    let line = raw_line.trim();
                    if let Some(spec) = line
                        .strip_prefix("Reading specs from ")
                        .or_else(|| line.strip_prefix("Configuration file: "))
                    {
                        paths.insert(PathBuf::from(spec));
                    }
                    if !line.starts_with('/')
                        && !line.starts_with('"')
                        && !raw_line.starts_with(' ')
                    {
                        continue;
                    }
                    let words =
                        shell::split_quoted(line).ok_or("malformed compiler dry-run command")?;
                    let program = words.first().ok_or("empty compiler dry-run command")?;
                    let found =
                        host::which(program, Some(std::ffi::OsStr::new("/usr/bin:/bin")))
                            .ok_or_else(|| format!("compiler subprogram unavailable: {program}"))?;
                    paths.insert(found);
                    commands += 1;
                    for word in &words[1..] {
                        if word.starts_with("-L") {
                            search.insert(word.clone());
                        }
                        let path = Path::new(word);
                        if path.is_absolute() && path.is_file() && !path.starts_with(&stage) {
                            paths.insert(path.to_path_buf());
                        }
                        if let Some(library) = word.strip_prefix("-l") {
                            if !library.is_empty() {
                                let shared = format!("lib{library}.so");
                                let found = self.run(
                                    &[cc.to_owned(), format!("-print-file-name={shared}")],
                                    INSPECTION_TIMEOUT,
                                    &format!("link_library_{library}"),
                                )?;
                                let resolved = PathBuf::from(found.trim());
                                let resolved = if resolved.is_absolute() && resolved.is_file() {
                                    resolved
                                } else {
                                    self.compiler_file(cc, &format!("lib{library}.a"))?
                                };
                                libraries.insert(resolved);
                            }
                        }
                    }
                }
                if commands == 0 {
                    return Err("compiler dry-run did not identify native subprograms".to_owned());
                }
            }
            if libraries.is_empty() {
                return Err("compiler did not report native link libraries".to_owned());
            }
            let linker = self.run(
                &[cc.to_owned(), "-print-prog-name=ld".to_owned()],
                INSPECTION_TIMEOUT,
                "native_linker",
            )?;
            let linker = host::which(linker.trim(), Some(std::ffi::OsStr::new("/usr/bin:/bin")))
                .ok_or("native linker unavailable")?;
            paths.insert(linker.clone());
            // Let the system linker interpret its own scripts and report every
            // input. This links only the reported libraries, without compiling
            // SQLite or exporting a usable SQLite prefix.
            let mut argv = vec![
                text(&linker),
                "--trace".to_owned(),
                "--shared".to_owned(),
                "--no-as-needed".to_owned(),
                "-o".to_owned(),
                text(&stage.join("link-inspection.so")),
            ];
            argv.extend(search);
            argv.extend(libraries.iter().map(|p| text(p)));
            let trace = self.run(&argv, INSPECTION_TIMEOUT, "native_link_inputs")?;
            paths.extend(link_paths(&trace)?);
            crate::sqlite::native_files(&paths.iter().map(PathBuf::as_path).collect::<Vec<_>>())
        })();
        let cleanup = fs::remove_dir_all(&stage).map_err(io);
        match (result, cleanup) {
            (Err(cause), Err(cleanup)) => {
                Err(format!("{cause}; native input scratch cleanup: {cleanup}"))
            }
            (Err(cause), _) => Err(cause),
            (Ok(_), Err(cleanup)) => Err(cleanup),
            (Ok(inputs), Ok(())) => Ok(inputs),
        }
    }

    fn smoke(&mut self, prefix: &Path) -> Result<String, Failure> {
        self.smoke_at(prefix, prefix)
    }

    fn smoke_at(&mut self, prefix: &Path, scratch: &Path) -> Result<String, Failure> {
        let directory = host::mkdtemp("smoke-run-", scratch).map_err(|e| e.to_string())?;
        let result = self.run(
            &[
                text(&prefix.join("smoke")),
                text(&directory.join("smoke.db")),
            ],
            INSPECTION_TIMEOUT,
            "smoke_run",
        );
        let cleanup = fs::remove_dir_all(&directory);
        let proof = result?;
        cleanup.map_err(|e| e.to_string())?;
        Ok(proof)
    }
}

fn header_paths(output: &str, stage: &Path) -> Result<BTreeSet<PathBuf>, Failure> {
    let flattened = output.replace("\\\n", " ");
    let dependencies = flattened
        .strip_prefix("sqlite-native-inputs:")
        .ok_or("compiler header dependency target missing")?;
    let words = shell::split_quoted(dependencies.trim())
        .ok_or("malformed compiler header dependency output")?;
    let mut paths = BTreeSet::new();
    for word in words {
        let path = PathBuf::from(word);
        if !path.is_absolute() || !path.is_file() {
            return Err("compiler header dependency is unresolved".to_owned());
        }
        if !path.starts_with(stage) {
            paths.insert(path);
        }
    }
    if paths.is_empty() {
        return Err("compiler did not identify system headers".to_owned());
    }
    Ok(paths)
}

// GNU ld --trace prints one input path per line; interpretation of linker
// scripts stays entirely with ld. Unknown/unresolved output is refused.
fn link_paths(output: &str) -> Result<BTreeSet<PathBuf>, Failure> {
    let paths = output
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .map(PathBuf::from)
        .collect::<BTreeSet<_>>();
    if paths.is_empty()
        || paths
            .iter()
            .any(|path| !path.is_absolute() || !path.is_file())
    {
        return Err("native linker did not report concrete input paths".to_owned());
    }
    Ok(paths)
}

fn text(path: &Path) -> String {
    path.to_string_lossy().into_owned()
}

fn no_symlinks(path: &Path) -> Result<(), Failure> {
    match host::first_symlink(path) {
        Some(part) => Err(format!("symlink path refused: {}", part.display())),
        None => Ok(()),
    }
}

fn io(error: std::io::Error) -> Failure {
    error.to_string()
}

fn sha3_hex(data: &[u8]) -> String {
    host::hex(&Sha3_256::digest(data))
}

/// Files below `root` (not directories), relative and sorted, like `rglob`.
fn files_below(root: &Path) -> Result<BTreeSet<String>, Failure> {
    let mut out = BTreeSet::new();
    let mut pending = vec![root.to_path_buf()];
    while let Some(dir) = pending.pop() {
        for entry in fs::read_dir(&dir).map_err(io)? {
            let path = entry.map_err(io)?.path();
            if path.is_dir() {
                if !path.is_symlink() {
                    pending.push(path);
                }
            } else {
                let relative = path.strip_prefix(root).map_err(|e| e.to_string())?;
                out.insert(text(relative));
            }
        }
    }
    Ok(out)
}

fn write_new(path: &Path, data: &[u8]) -> Result<(), Failure> {
    fs::write(path, data).map_err(io)
}

fn pretty(value: &Value) -> String {
    serde_json::to_string_pretty(value).unwrap_or_default() + "\n"
}

pub fn main(argv: Vec<OsString>) -> i32 {
    let parsed = match args::parse(
        argv,
        &[
            args::required("archive"),
            args::required("prefix"),
            args::required("target"),
            args::value("cc"),
            args::value("ar"),
            args::flag("identity-only"),
        ],
        false,
    ) {
        Outcome::Parsed(parsed) => parsed,
        Outcome::Help => {
            print!("{USAGE}\n\n{HELP}");
            return 0;
        }
        Outcome::Usage(message) => {
            eprintln!("{USAGE}\nprepare-sqlite-build.sh: error: {message}");
            return 2;
        }
    };
    let mut runner = Runner {
        timings: Map::new(),
    };
    match build(&parsed, &mut runner) {
        Ok(()) => 0,
        Err(error) => {
            eprintln!("prepare-sqlite-build: {error}");
            1
        }
    }
}

fn build(parsed: &args::Parsed, runner: &mut Runner) -> Result<(), Failure> {
    let value = |name: &str| parsed.get(name).unwrap_or_default().to_owned();
    let archive = host::abspath(Path::new(&value("archive"))).map_err(io)?;
    let prefix = host::abspath(Path::new(&value("prefix"))).map_err(io)?;
    let target_arg = value("target");
    no_symlinks(&archive)?;
    no_symlinks(&prefix)?;
    let archive_ok = fs::metadata(&archive)
        .map(|m| m.is_file() && m.len() <= SIZE_LIMIT)
        .unwrap_or(false);
    if !archive_ok {
        return Err("archive must be a regular file no larger than 16 MiB".to_owned());
    }
    let data = fs::read(&archive).map_err(io)?;
    if sha256_hex(&data) != ARCHIVE_HASH {
        return Err("archive SHA-256 mismatch; no source extraction/build/fallback".to_owned());
    }
    let sources = sqlite_zip::read_sources(&data)?;
    let (c_source, header) = (sources.c_source, sources.header);
    if sha3_hex(&c_source) != C_HASH {
        return Err("sqlite3.c official SHA3-256 mismatch".to_owned());
    }
    if sha256_hex(&header) != HEADER_HASH {
        return Err("pinned sqlite3.h SHA-256 mismatch".to_owned());
    }
    let contains = |haystack: &[u8], needle: String| {
        haystack
            .windows(needle.len())
            .any(|window| window == needle.as_bytes())
    };
    for content in [&c_source, &header] {
        if !contains(content, format!("\"{SOURCE_ID}\""))
            || !contains(content, format!("\"{VERSION}\""))
        {
            return Err("source/header version or source ID mismatch".to_owned());
        }
    }
    let (system, machine) = host::uname().map_err(io)?;
    let host_target = match machine.as_str() {
        "x86_64" => Some(("x86_64-unknown-linux-gnu", "x86_64-linux-gnu")),
        "aarch64" => Some(("aarch64-unknown-linux-gnu", "aarch64-linux-gnu")),
        _ => None,
    };
    let Some((rust_target, gnu_target)) =
        host_target.filter(|(rust, _)| system == "Linux" && target_arg == *rust)
    else {
        return Err(
            "only explicit matching native Linux GNU x86_64/aarch64 targets supported; no cross build"
                .to_owned(),
        );
    };
    let cc_name = parsed.get("cc").unwrap_or("cc").to_owned();
    let ar_name = parsed.get("ar").unwrap_or("ar").to_owned();
    let (cc, cc_id) = runner.tool(&cc_name)?;
    let (ar, ar_id) = runner.tool(&ar_name)?;
    let cc = text(&cc);
    let ar = text(&ar);
    let compiler_target = runner
        .run(
            &[cc.clone(), "-dumpmachine".to_owned()],
            INSPECTION_TIMEOUT,
            "compiler_target",
        )?
        .trim()
        .to_owned();
    if compiler_target != gnu_target {
        return Err(format!(
            "compiler target mismatch: {compiler_target}, expected {gnu_target}"
        ));
    }
    let parent = prefix.parent().unwrap_or(Path::new("/")).to_path_buf();
    if !parent.is_dir() || host::owner(&parent).map_err(io)? != host::getuid() {
        return Err(
            "prefix parent must be an existing directory owned by the current user".to_owned(),
        );
    }
    let inputs = json!({
        "schema": 2,
        "archive_sha256": ARCHIVE_HASH,
        "sqlite3_c_sha3_256": C_HASH,
        "header_sha256": sha256_hex(&header),
        "source_id": SOURCE_ID,
        "version": VERSION,
        "helper_sha256": helper_sha256(),
        "compiler": cc_id,
        "archiver": ar_id,
        "compiler_target": compiler_target,
        "native_files": runner.native_inputs(&cc, &ar, &parent, &c_source, &header)?,
        "target": rust_target,
        "host_arch": machine,
        "flags": FLAGS,
        "link_flags": LINK_FLAGS,
        "environment": ENV.iter().map(|(k, v)| ((*k).to_owned(), json!(v))).collect::<Map<_, _>>(),
    });
    let env_text: String = [
        ("SQLITE3_LIB_DIR", text(&prefix.join("lib"))),
        ("SQLITE3_INCLUDE_DIR", text(&prefix.join("include"))),
        ("SQLITE3_STATIC", "1".to_owned()),
        ("SQLITE3_NO_PKG_CONFIG", "1".to_owned()),
    ]
    .iter()
    .map(|(key, value)| format!("export {key}={}\n", shell::quote(value)))
    .collect();
    if parsed.has("identity-only") {
        println!(
            "{}",
            json!({"inputs": inputs, "exports": env_text, "timings": runner.timings})
        );
        return Ok(());
    }

    let name = prefix
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    let lock = parent.join(format!("{name}.lock"));
    // O_EXCL refuses another writer or a stale lock without touching its files.
    OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(&lock)
        .map_err(|e| format!("{e}: '{}'", lock.display()))?;
    let mut stage: Option<PathBuf> = None;
    let result = build_or_verify(
        runner, &prefix, &parent, &name, &inputs, &env_text, &c_source, &header, &cc, &ar,
        &mut stage,
    );
    if let Some(stage) = &stage {
        let _ = fs::write(
            stage.join("failure-timings.json"),
            pretty(&Value::Object(runner.timings.clone())),
        );
        eprintln!(
            "Incomplete build preserved for diagnosis: {}",
            stage.display()
        );
    }
    let unlock = fs::remove_file(&lock).map_err(io);
    result?;
    unlock?;
    print!("{env_text}");
    std::io::stdout().flush().map_err(io)
}

fn verified_outputs(prefix: &Path, inputs: &Value, env_text: &str) -> Result<Value, Failure> {
    let required: BTreeSet<String> = REQUIRED_OUTPUTS.iter().map(|s| (*s).to_owned()).collect();
    let manifest: Value =
        serde_json::from_str(&fs::read_to_string(prefix.join("manifest.json")).map_err(io)?)
            .map_err(|e| e.to_string())?;
    if manifest.get("inputs") != Some(inputs) {
        return Err("existing build input identity differs; use a new owned prefix".to_owned());
    }
    let outputs = manifest
        .get("outputs")
        .and_then(Value::as_object)
        .filter(|o| o.keys().cloned().collect::<BTreeSet<_>>() == required)
        .ok_or("existing prefix required output inventory differs")?;
    let mut expected_files = required.clone();
    expected_files.insert("manifest.json".to_owned());
    if files_below(prefix)? != expected_files {
        return Err("existing prefix output inventory differs".to_owned());
    }
    for output in &required {
        let path = prefix.join(output);
        no_symlinks(&path)?;
        let matches = path.is_file()
            && Some(sha256_hex(&fs::read(&path).map_err(io)?).as_str()) == outputs[output].as_str();
        if !matches {
            return Err(format!(
                "existing build output hash/type mismatch: {output}"
            ));
        }
    }
    if fs::read_to_string(prefix.join("env.sh")).map_err(io)? != env_text {
        return Err("existing build environment differs".to_owned());
    }
    Ok(manifest)
}

/// Image adoption reuses the existing complete output inventory and real smoke,
/// but places the temporary database outside the immutable SDK.
pub fn verify_image_outputs(prefix: &Path, scratch: &Path, inputs: &Value) -> Result<(), String> {
    let env_text = image_exports(prefix);
    let manifest = verified_outputs(prefix, inputs, &env_text)?;
    if sha3_hex(&fs::read(prefix.join("sqlite3.c")).map_err(io)?) != C_HASH
        || sha256_hex(&fs::read(prefix.join("include/sqlite3.h")).map_err(io)?) != HEADER_HASH
        || fs::read_to_string(prefix.join("smoke.c")).map_err(io)? != smoke_source()
    {
        return Err("image SDK pinned source/header/smoke differs".into());
    }
    let mut runner = Runner {
        timings: Map::new(),
    };
    let proof = runner.smoke_at(prefix, scratch)?;
    if manifest.get("proof").and_then(Value::as_str) != Some(proof.as_str()) {
        return Err("image SDK actual smoke proof differs".into());
    }
    Ok(())
}

pub fn image_exports(prefix: &Path) -> String {
    [
        ("SQLITE3_LIB_DIR", text(&prefix.join("lib"))),
        ("SQLITE3_INCLUDE_DIR", text(&prefix.join("include"))),
        ("SQLITE3_STATIC", "1".to_owned()),
        ("SQLITE3_NO_PKG_CONFIG", "1".to_owned()),
    ]
    .iter()
    .map(|(key, value)| format!("export {key}={}\n", shell::quote(value)))
    .collect()
}

#[allow(clippy::too_many_arguments)]
fn build_or_verify(
    runner: &mut Runner,
    prefix: &Path,
    parent: &Path,
    name: &str,
    inputs: &Value,
    env_text: &str,
    c_source: &[u8],
    header: &[u8],
    cc: &str,
    ar: &str,
    stage_slot: &mut Option<PathBuf>,
) -> Result<(), Failure> {
    if prefix.exists() {
        if !prefix.is_dir() || host::owner(prefix).map_err(io)? != host::getuid() {
            return Err("existing prefix is not an owned build directory".to_owned());
        }
        let manifest = verified_outputs(prefix, inputs, env_text)?;
        let proof = runner.smoke(prefix)?;
        if manifest.get("proof").and_then(Value::as_str) != Some(proof.as_str()) {
            return Err("cached actual smoke proof differs".to_owned());
        }
        eprintln!("Verified same-input SQLite build reuse");
    } else {
        let stage = host::mkdtemp(&format!("{name}.build-"), parent).map_err(io)?;
        *stage_slot = Some(stage.clone());
        fs::create_dir(stage.join("lib")).map_err(io)?;
        fs::create_dir(stage.join("include")).map_err(io)?;
        write_new(&stage.join("sqlite3.c"), c_source)?;
        write_new(&stage.join("include/sqlite3.h"), header)?;
        write_new(&stage.join("smoke.c"), smoke_source().as_bytes())?;
        let s = |relative: &str| text(&stage.join(relative));
        let mut compile = vec![cc.to_owned()];
        compile.extend(FLAGS.iter().map(|f| (*f).to_owned()));
        compile.extend([
            "-c".to_owned(),
            s("sqlite3.c"),
            "-o".to_owned(),
            s("sqlite3.o"),
        ]);
        runner.run(&compile, COMPILE_TIMEOUT, "compile")?;
        runner.run(
            &[
                ar.to_owned(),
                "rcs".to_owned(),
                s("lib/libsqlite3.a"),
                s("sqlite3.o"),
            ],
            ARCHIVE_TIMEOUT,
            "archive",
        )?;
        // Exact archive filename: never -lsqlite3/-L or a system lookup.
        let mut link = vec![
            cc.to_owned(),
            "-std=c11".to_owned(),
            "-I".to_owned(),
            s("include"),
            s("smoke.c"),
            s("lib/libsqlite3.a"),
        ];
        link.extend(LINK_FLAGS.iter().map(|f| (*f).to_owned()));
        link.extend(["-o".to_owned(), s("smoke")]);
        runner.run(&link, INSPECTION_TIMEOUT, "smoke_link")?;
        let proof = runner.smoke(&stage)?;
        write_new(&stage.join("proof.txt"), proof.as_bytes())?;
        write_new(&stage.join("env.sh"), env_text.as_bytes())?;
        let mut outputs = Map::new();
        for file in files_below(&stage)? {
            let digest = sha256_hex(&fs::read(stage.join(&file)).map_err(io)?);
            outputs.insert(file, json!(digest));
        }
        let manifest = json!({"inputs": inputs, "outputs": outputs, "proof": proof});
        write_new(&stage.join("manifest.json"), pretty(&manifest).as_bytes())?;
        if prefix.exists() {
            return Err("prefix appeared during build; refusing overwrite".to_owned());
        }
        fs::rename(&stage, prefix).map_err(io)?;
        *stage_slot = None;
        eprintln!("Built and linked pinned SQLite {VERSION}");
    }
    // Keep elapsed provenance outside the prefix: Web hashes every prefix file.
    fs::write(
        parent.join(format!("{name}.prepare-timings.json")),
        pretty(&Value::Object(runner.timings.clone())),
    )
    .map_err(io)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn header_interrogation_preserves_escaped_paths_and_refuses_missing_inputs() {
        let root = host::mkdtemp("sqlite-header-inputs-", &std::env::temp_dir()).unwrap();
        let header = root.join("a header.h");
        fs::write(&header, "header").unwrap();
        let output = format!(
            "sqlite-native-inputs: \\\n {}\n",
            text(&header).replace(' ', "\\ ")
        );
        assert_eq!(
            header_paths(&output, &root.join("stage")).unwrap(),
            BTreeSet::from([header.clone()])
        );
        assert!(header_paths("foreign-target: /missing.h", &root).is_err());
        fs::remove_file(&header).unwrap();
        assert!(header_paths(&output, &root.join("stage")).is_err());
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn standard_linker_trace_requires_resolved_file_paths() {
        let root = host::mkdtemp("sqlite-link-inputs-", &std::env::temp_dir()).unwrap();
        let script = root.join("libc.so");
        let library = root.join("libc.so.6");
        fs::write(&script, "opaque script bytes interpreted by ld").unwrap();
        fs::write(&library, "fixture library bytes").unwrap();
        assert_eq!(
            link_paths(&format!("{}\n{}\n", text(&script), text(&library))).unwrap(),
            BTreeSet::from([script, library.clone()])
        );
        assert!(link_paths("GROUP ( /some/file )").is_err());
        assert!(link_paths("relative-file").is_err());
        fs::remove_file(library).unwrap();
        assert!(link_paths(&format!("{}/libc.so.6\n", text(&root))).is_err());
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn sha3_256_matches_fips_202_vectors() {
        assert_eq!(
            sha3_hex(b"abc"),
            "3a985da74fe225b2045c172d6bd390bd855f086e3e9d525b46bfe24511431532"
        );
        // Longer than the 136-byte rate, so it absorbs more than one block.
        assert_eq!(
            sha3_hex(&[0xa3; 200]),
            "79f38adec5c20307a98ef76e8324afbfd46cfd81b22e3973c65fa1bd9de31787"
        );
    }
}
