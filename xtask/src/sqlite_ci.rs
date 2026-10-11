//! `xtask sqlite-ci`: pinned native SQLite prerequisite and root Cargo entry.
//! Entry point: `bash scripts/prepare-sqlite-ci.sh [--parent OWNED_DIR] --
//! cargo build --locked`. Requires native GCC/ar, curl and LIBCLANG_PATH.
//! Never installs host packages or changes Cargo. `sqlite-build` owns all
//! source hashes, C flags and SQLite export policy; this wrapper runs it as a
//! bounded child and only consumes its verified exports.

use crate::args::{self, Outcome};
use crate::host::{self, sha256_hex};
use crate::process::{self, Finished, RunError, Timing};
use crate::shell;
use crate::sqlite::{archive_name, archive_url, wrapper_sha256, ENV_NAMES, SIZE_LIMIT};
use serde_json::{json, Map, Value};
use std::collections::BTreeMap;
use std::ffi::{OsStr, OsString};
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{Duration, Instant};

const USAGE: &str = "usage: prepare-sqlite-ci.sh [-h] [--parent PARENT] [--env-file ENV_FILE] [--github-env GITHUB_ENV] [--github-output GITHUB_OUTPUT] [--identity-only] [--cache-fallback] [--expected-cache-identity EXPECTED_CACHE_IDENTITY] ...";
const HELP: &str = "\
Pinned native SQLite prerequisite and root Cargo entry

positional arguments:
  command

options:
  -h, --help            show this help message and exit
  --parent PARENT       existing task/run-owned directory
  --env-file ENV_FILE   write verified shell exports after success
  --github-env GITHUB_ENV
  --github-output GITHUB_OUTPUT
  --identity-only       authenticate inputs before cache restore; export no build paths
  --cache-fallback      preserve rejected restored prefix and log a source build
  --expected-cache-identity EXPECTED_CACHE_IDENTITY
";
/// 2 heavy steps x180 + 5 inspections/link/smoke x30; finite total helper budget.
const HELPER_BUDGET: Duration = Duration::from_secs(510);
const INSPECT_BUDGET: Duration = Duration::from_secs(120);
const DOWNLOAD_BUDGET: Duration = Duration::from_secs(95);
const RUSTC_BUDGET: Duration = Duration::from_secs(30);

/// Inputs normally taken from the process; tests supply their own.
pub struct Context {
    pub env: BTreeMap<OsString, OsString>,
    /// Program and leading arguments that run the build helper.
    pub helper: Vec<OsString>,
}

impl Context {
    pub fn from_process() -> std::io::Result<Self> {
        Ok(Self {
            env: std::env::vars_os().collect(),
            helper: vec![std::env::current_exe()?.into(), "sqlite-build".into()],
        })
    }

    fn var(&self, key: &str) -> Option<&OsStr> {
        self.env.get(OsStr::new(key)).map(OsString::as_os_str)
    }

    fn var_text(&self, key: &str) -> Option<String> {
        self.var(key).map(|v| v.to_string_lossy().into_owned())
    }
}

#[derive(Debug)]
enum Failure {
    Message(String),
    /// A step's child exited non-zero; its captured stderr is printed after.
    Called {
        message: String,
        stderr: Vec<u8>,
    },
}

impl From<String> for Failure {
    fn from(message: String) -> Self {
        Self::Message(message)
    }
}

impl From<&str> for Failure {
    fn from(message: &str) -> Self {
        Self::Message(message.to_owned())
    }
}

impl From<std::io::Error> for Failure {
    fn from(error: std::io::Error) -> Self {
        Self::Message(error.to_string())
    }
}

struct Steps<'a> {
    parent: PathBuf,
    timings: Map<String, Value>,
    timed: bool,
    err: &'a mut dyn Write,
}

impl Steps<'_> {
    /// Bounded step; `capture` collects stdout/stderr, otherwise inherited.
    fn measured(
        &mut self,
        step: &str,
        argv: &[OsString],
        timeout: Duration,
        capture: bool,
        env: Option<&BTreeMap<OsString, OsString>>,
    ) -> Result<Finished, Failure> {
        let started = Instant::now();
        if self.timed {
            let _ = writeln!(
                self.err,
                "sqlite-ci-timing step={step} started at={}",
                host::utc_timestamp()
            );
        }
        let mut command = Command::new(&argv[0]);
        command.args(&argv[1..]);
        if let Some(env) = env {
            command.env_clear().envs(env);
        }
        let result = process::run(&mut command, timeout, capture);
        let status = match &result {
            Ok(done) => process::returncode(done.status),
            Err(_) => -1,
        };
        if self.timed {
            let _ = writeln!(
                self.err,
                "sqlite-ci-timing step={step} finished at={} elapsed_seconds={} exit={status}",
                host::utc_timestamp(),
                started.elapsed().as_secs_f64().round() as u64
            );
        }
        let timing = Timing::new(started.elapsed(), timeout, status);
        let _ = writeln!(self.err, "sqlite-ci step={step} {}", timing.log_line());
        self.timings.insert(step.to_owned(), timing.to_json());
        fs::write(
            self.parent.join("preparation-timings.json"),
            pretty(&Value::Object(self.timings.clone())),
        )?;
        let shown = || {
            format!(
                "{:?}",
                argv.iter().map(|a| a.to_string_lossy()).collect::<Vec<_>>()
            )
        };
        let done = result.map_err(|error| match error {
            RunError::Timeout(timeout) => Failure::Message(format!(
                "Command '{}' timed out after {} seconds",
                shown(),
                timeout.as_secs()
            )),
            other => Failure::Message(other.to_string()),
        })?;
        if status != 0 {
            let what = if status < 0 {
                format!("died with signal {}", -status)
            } else {
                format!("returned non-zero exit status {status}")
            };
            return Err(Failure::Called {
                message: format!("Command '{}' {what}.", shown()),
                stderr: done.stderr,
            });
        }
        Ok(done)
    }
}

fn pretty(value: &Value) -> String {
    serde_json::to_string_pretty(value).unwrap_or_default() + "\n"
}

fn utf8(bytes: Vec<u8>) -> Result<String, Failure> {
    String::from_utf8(bytes).map_err(|e| Failure::Message(e.to_string()))
}

fn os(path: &Path) -> OsString {
    path.as_os_str().to_owned()
}

/// Process entry: real environment, stdout and stderr.
pub fn main(argv: Vec<OsString>) -> i32 {
    let context = match Context::from_process() {
        Ok(context) => context,
        Err(error) => {
            eprintln!("prepare-sqlite-ci: {error}");
            return 1;
        }
    };
    let stdout = std::io::stdout();
    let stderr = std::io::stderr();
    run(argv, &context, &mut stdout.lock(), &mut stderr.lock())
}

/// Returns the process exit status: 0, 1 (handled failure), 2 (usage), or
/// the consumer's return code (`-signal` maps to status `256 - signal` when
/// the caller exits with it, as Python `sys.exit(-N)` did).
pub fn run(
    argv: Vec<OsString>,
    context: &Context,
    out: &mut dyn Write,
    err: &mut dyn Write,
) -> i32 {
    let parsed = match args::parse(
        argv,
        &[
            args::value("parent"),
            args::value("env-file"),
            args::value("github-env"),
            args::value("github-output"),
            args::flag("identity-only"),
            args::flag("cache-fallback"),
            args::value("expected-cache-identity"),
        ],
        true,
    ) {
        Outcome::Parsed(parsed) => parsed,
        Outcome::Help => {
            let _ = write!(out, "{USAGE}\n\n{HELP}");
            return 0;
        }
        Outcome::Usage(message) => {
            let _ = writeln!(err, "{USAGE}\nprepare-sqlite-ci.sh: error: {message}");
            return 2;
        }
    };
    match prepare(&parsed, context, out, err) {
        Ok(code) => code,
        Err(failure) => {
            let _ = match failure {
                Failure::Message(message) => writeln!(err, "prepare-sqlite-ci: {message}"),
                Failure::Called { message, stderr } => {
                    writeln!(err, "prepare-sqlite-ci: {message}")
                        .and_then(|()| err.write_all(&stderr))
                }
            };
            1
        }
    }
}

fn prepare(
    parsed: &args::Parsed,
    context: &Context,
    out: &mut dyn Write,
    err: &mut dyn Write,
) -> Result<i32, Failure> {
    let mut command = parsed.command.clone();
    if command.first().map(OsString::as_os_str) == Some(OsStr::new("--")) {
        command.remove(0);
    }
    let identity_only = parsed.has("identity-only");
    let env_file = parsed.get("env-file");
    if identity_only && (!command.is_empty() || env_file.is_some()) {
        return Err("identity-only cannot run a consumer or export build paths".into());
    }
    let job = context.var_text("GITHUB_JOB");
    let timed = matches!(
        job.as_deref(),
        Some("collaboration-build" | "collaboration-flow")
    ) || (job.as_deref() == Some("postgres-build")
        && context.var_text("RUNNER_ARCH").as_deref() == Some("X64"));

    let (system, machine) = host::uname()?;
    let target = match machine.as_str() {
        "x86_64" if system == "Linux" => "x86_64-unknown-linux-gnu",
        "aarch64" if system == "Linux" => "aarch64-unknown-linux-gnu",
        _ => return Err("only native Linux GNU x86_64/aarch64 builds supported".into()),
    };
    let mut requested = vec![context
        .var_text("CARGO_BUILD_TARGET")
        .unwrap_or_else(|| target.to_owned())];
    for (i, arg) in command.iter().enumerate() {
        let arg = arg.to_string_lossy();
        if arg == "--target" {
            requested.push(
                command
                    .get(i + 1)
                    .map(|v| v.to_string_lossy().into_owned())
                    .unwrap_or_default(),
            );
        } else if let Some(value) = arg.strip_prefix("--target=") {
            requested.push(value.to_owned());
        }
    }
    if requested.iter().any(|t| t != target) {
        return Err("Cargo target must match the native SQLite GNU target".into());
    }
    let clang_dir = PathBuf::from(
        context
            .var("LIBCLANG_PATH")
            .unwrap_or(OsStr::new("/nonexistent")),
    );
    let clang = fs::canonicalize(clang_dir.join("libclang.so"))
        .map_err(|e| format!("{e}: '{}'", clang_dir.join("libclang.so").display()))?;
    if !clang.is_file() {
        return Err("LIBCLANG_PATH must contain build-time libclang.so".into());
    }
    for tool in ["curl", "cc", "ar", "rustc"] {
        if host::which(tool, context.var("PATH")).is_none() {
            return Err(format!("build tool unavailable: {tool}").into());
        }
    }
    let inherited: BTreeMap<String, String> = ENV_NAMES
        .iter()
        .filter_map(|k| context.var_text(k).map(|v| ((*k).to_owned(), v)))
        .collect();
    let mut parent_arg = parsed.get("parent").map(PathBuf::from);
    if parent_arg.is_none() && !inherited.is_empty() {
        if inherited.len() != ENV_NAMES.len() {
            return Err("partial SQLite environment refused".into());
        }
        let lib = Path::new(&inherited["SQLITE3_LIB_DIR"]);
        let up = |p: &Path| {
            p.parent()
                .map(Path::to_path_buf)
                .unwrap_or_else(|| p.to_path_buf())
        };
        parent_arg = Some(up(&up(lib)));
    }
    let parent = match parent_arg {
        Some(path) => host::abspath(&path)?,
        None => host::mkdtemp(
            "fvoci-sqlite-",
            &host::temp_root(|k| context.var(k).map(OsStr::to_owned)),
        )?,
    };
    if let Some(link) = host::first_symlink(&parent) {
        return Err(format!("symlink build parent refused: {}", link.display()).into());
    }
    if !parent.is_dir() || host::owner(&parent)? != host::getuid() {
        return Err("build parent must exist and be owned by the current user".into());
    }
    let prefix = parent.join(target);
    if !inherited.is_empty() {
        // An explicit parent skips the partial-environment check above; a
        // missing key is the former handled KeyError (exit 1), never a panic.
        let lib = inherited
            .get("SQLITE3_LIB_DIR")
            .ok_or("'SQLITE3_LIB_DIR'")?;
        if Path::new(lib) != prefix.join("lib") {
            return Err("SQLite environment does not match the owned build prefix".into());
        }
    }
    let mut steps = Steps {
        parent: parent.clone(),
        timings: Map::new(),
        timed,
        err,
    };
    let archive = parent.join(archive_name());
    // Existing archive is checked by the helper; never overwrite/re-download a bad cache.
    if !archive.exists() {
        let download = host::mkstemp("sqlite-download-", &parent)?;
        let fetched = steps
            .measured(
                "download",
                &[
                    "curl",
                    "--fail",
                    "--silent",
                    "--show-error",
                    "--proto",
                    "=https",
                    "--tlsv1.2",
                    "--connect-timeout",
                    "15",
                    "--max-time",
                    "90",
                    "--max-filesize",
                ]
                .iter()
                .map(OsString::from)
                .chain([
                    OsString::from(SIZE_LIMIT.to_string()),
                    "--retry".into(),
                    "0".into(),
                    "--output".into(),
                    os(&download),
                    archive_url().into(),
                ])
                .collect::<Vec<_>>(),
                DOWNLOAD_BUDGET,
                false,
                Some(&context.env),
            )
            // No replacement if another writer published an archive during download.
            .and_then(|_| fs::hard_link(&download, &archive).map_err(Failure::from));
        let removed = fs::remove_file(&download);
        fetched?;
        removed?;
    }
    let mut helper_argv = context.helper.clone();
    helper_argv.extend([
        "--archive".into(),
        os(&archive),
        "--prefix".into(),
        os(&prefix),
        "--target".into(),
        target.into(),
    ]);
    let mut inspect_argv = helper_argv.clone();
    inspect_argv.push("--identity-only".into());
    let inspected = steps.measured(
        "inspect_inputs",
        &inspect_argv,
        INSPECT_BUDGET,
        true,
        Some(&context.env),
    )?;
    steps.err.write_all(&inspected.stderr)?;
    let inspected: Value =
        serde_json::from_slice(&inspected.stdout).map_err(|e| Failure::Message(e.to_string()))?;
    let exports = inspected
        .get("exports")
        .and_then(Value::as_str)
        .ok_or("'exports'")?
        .to_owned();
    let mut env: BTreeMap<String, String> = BTreeMap::new();
    for line in exports.lines() {
        let parts = shell::split_quoted(line).ok_or("unexpected helper export")?;
        let pair = match parts.as_slice() {
            [export, pair] if export == "export" => pair.split_once('='),
            _ => None,
        };
        let (key, value) = pair.ok_or("unexpected helper export")?;
        if !ENV_NAMES.contains(&key) || env.contains_key(key) || value.contains(['\n', '\r']) {
            return Err("unexpected/duplicate helper environment".into());
        }
        env.insert(key.to_owned(), value.to_owned());
    }
    if env.len() != ENV_NAMES.len() || (!inherited.is_empty() && inherited != env) {
        return Err("incomplete or mismatched SQLite environment".into());
    }
    let inputs = inspected.get("inputs").ok_or("'inputs'")?.clone();
    // Fresh/revalidated exact archive/header precede restoring Cargo output caches.
    let rustc = utf8(
        steps
            .measured(
                "rustc_inspection",
                &["rustc".into(), "-vV".into()],
                RUSTC_BUDGET,
                true,
                Some(&context.env),
            )?
            .stdout,
    )?;
    if !rustc.lines().any(|line| line == format!("host: {target}")) {
        return Err("rustc host must match the native SQLite GNU target".into());
    }
    let build_env: BTreeMap<String, String> = context
        .env
        .iter()
        .map(|(k, v)| {
            (
                k.to_string_lossy().into_owned(),
                v.to_string_lossy().into_owned(),
            )
        })
        .filter(|(k, _)| {
            [
                "LIBCLANG_PATH",
                "RUSTFLAGS",
                "CARGO_ENCODED_RUSTFLAGS",
                "RUSTC",
                "RUSTC_WRAPPER",
                "RUSTC_WORKSPACE_WRAPPER",
                "CARGO_BUILD_TARGET",
            ]
            .contains(&k.as_str())
                || [
                    "BINDGEN_EXTRA_CLANG_ARGS",
                    "CARGO_PROFILE_",
                    "CARGO_TARGET_",
                ]
                .iter()
                .any(|p| k.starts_with(p))
        })
        .collect();
    let packages_path = parent.join("build-packages.txt");
    let packages = if packages_path.is_file() {
        json!(fs::read_to_string(&packages_path)?)
    } else {
        Value::Null
    };
    let identity = json!({
        "inputs": inputs,
        "exports": env,
        "wrapper_sha256": wrapper_sha256(),
        "os_release": fs::read_to_string("/etc/os-release")?,
        "architecture": machine,
        "packages": packages,
        "libclang_path": clang.to_string_lossy(),
        "libclang_sha256": sha256_hex(&fs::read(&clang)?),
        // Fingerprint build overrides without publishing their values.
        "build_environment_sha256": sha256_hex(json!(build_env).to_string().as_bytes()),
        "rustc": rustc,
    });
    // Only immutable inputs enter the key; output hashes and timings remain verified provenance.
    let cache_identity = sha256_hex(identity.to_string().as_bytes());
    if let Some(expected) = parsed.get("expected-cache-identity") {
        if expected != cache_identity {
            return Err("preflight/cache identity changed before prefix verification".into());
        }
    }
    let with = |extra: Value| {
        let mut merged = identity.as_object().cloned().unwrap_or_default();
        merged.extend(extra.as_object().cloned().unwrap_or_default());
        Value::Object(merged)
    };
    let github_output = parsed.get("github-output");
    let output_line = format!("target={target}\ncache_identity={cache_identity}\n");
    if identity_only {
        fs::write(
            parent.join("prepare-inputs.json"),
            pretty(&with(json!({"timings": steps.timings}))),
        )?;
        if let Some(path) = github_output {
            host::append(Path::new(path), &output_line)?;
        }
        return Ok(0);
    }
    let mut existed = prefix.exists();
    let lock = parent.join(format!("{target}.lock"));
    if parsed.has("cache-fallback")
        && (prefix.is_symlink()
            || (existed && (!prefix.is_dir() || host::owner(&prefix)? != host::getuid()))
            || lock.exists())
    {
        return Err("unsafe or locked prefix cannot fall back to building".into());
    }
    if !existed {
        writeln!(
            steps.err,
            "SQLite prepared prefix cache: MISS; building pinned source"
        )?;
    }
    let result = match steps.measured(
        "verify_or_build",
        &helper_argv,
        HELPER_BUDGET,
        true,
        Some(&context.env),
    ) {
        Ok(done) => done,
        Err(Failure::Called { stderr, .. }) if parsed.has("cache-fallback") && existed => {
            // Existing-prefix helper execution only verifies; it never compiles.
            steps.err.write_all(&stderr)?;
            let rejected = host::mkdtemp(&format!("{target}.rejected-"), &parent)?;
            fs::remove_dir(&rejected)?;
            fs::rename(&prefix, &rejected)?;
            writeln!(
                steps.err,
                "SQLite prepared prefix cache: REJECTED; preserved at {}; building pinned source",
                rejected.display()
            )?;
            existed = false;
            steps.measured(
                "fallback_build",
                &helper_argv,
                HELPER_BUDGET,
                true,
                Some(&context.env),
            )?
        }
        Err(other) => return Err(other),
    };
    steps.err.write_all(&result.stderr)?;
    if result.stdout != exports.as_bytes() || exports != fs::read_to_string(prefix.join("env.sh"))?
    {
        return Err("helper export/file mismatch".into());
    }
    let manifest: Value = serde_json::from_str(&fs::read_to_string(prefix.join("manifest.json"))?)
        .map_err(|e| Failure::Message(e.to_string()))?;
    if manifest.get("inputs") != Some(&inputs) {
        return Err("helper inputs changed after preflight".into());
    }
    writeln!(
        steps.err,
        "SQLite prepared prefix cache: {}",
        if existed {
            "HIT; verified inputs, exports, inventory, hashes and smoke"
        } else {
            "MISS/BUILD; built and verified pinned source"
        }
    )?;
    let helper_timings: Value = serde_json::from_str(&fs::read_to_string(
        parent.join(format!("{target}.prepare-timings.json")),
    )?)
    .map_err(|e| Failure::Message(e.to_string()))?;
    fs::write(
        parent.join("consumer-inputs.json"),
        pretty(&with(json!({
            "manifest": manifest,
            "cache_identity": cache_identity,
            "timings": steps.timings,
            "helper_timings": helper_timings,
        }))),
    )?;
    if let Some(path) = env_file {
        fs::write(path, &exports)?;
    }
    let github_env = parsed.get("github-env");
    if let Some(path) = github_env {
        let lines: String = env.iter().map(|(k, v)| format!("{k}={v}\n")).collect();
        host::append(Path::new(path), &lines)?;
    }
    if let Some(path) = github_output {
        host::append(Path::new(path), &output_line)?;
    }
    if let Some((program, rest)) = command.split_first() {
        let mut child_env = context.env.clone();
        child_env.extend(env.iter().map(|(k, v)| (k.into(), v.into())));
        out.flush()?;
        let status = Command::new(program)
            .args(rest)
            .env_clear()
            .envs(&child_env)
            .status()
            .map_err(|e| format!("{e}: '{}'", program.to_string_lossy()))?;
        return Ok(process::returncode(status) as i32);
    }
    if env_file.is_none() && github_env.is_none() {
        out.write_all(exports.as_bytes())?;
    }
    Ok(0)
}
