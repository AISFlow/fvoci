//! Isolated PostgreSQL, MinIO, and Meilisearch for local and CI tests.
//!
//! Each service publishes on `127.0.0.1` with an ephemeral host port, exports
//! the connection variables its tests already read, runs one command, and
//! removes the container when that command returns or this process is
//! interrupted.

use std::fs::{self, OpenOptions};
use std::io::{self, Write};
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
use std::os::unix::process::ExitStatusExt;
use std::path::{Path, PathBuf};
use std::process::{Command, ExitCode, ExitStatus, Stdio};
use std::sync::atomic::{AtomicI32, Ordering};
use std::thread;
use std::time::{Duration, Instant};

const READY_BUDGET: Duration = Duration::from_secs(30);
const SIGINT: i32 = 2;
const SIGTERM: i32 = 15;

const PG16_IMAGE: &str =
    "postgres:16.15@sha256:1a6ab3f5345eb6dbe04a1349529caabdb0ab09293a09590fad07b2246bfa4b54";
const PG17_IMAGE: &str =
    "postgres:17.11@sha256:d74eeac9a635390a49bc21bd49fccd973de707e2a53a76ac49b552b8712ec46f";
const PG18_IMAGE: &str =
    "postgres:18.3@sha256:7e32e9833a6fb1c92c32552794cb6ed569d51b445a54907d35fc112ef39684db";
const MINIO_IMAGE: &str = "pgsty/silo:RELEASE.2026-08-06T00-00-00Z@sha256:29a498b24669cae1fed11c1a2fb2b3d73c68829a0a9c0b14e71b386671d38fac";
const MEILI_IMAGE: &str = "getmeili/meilisearch:v1.53.2@sha256:c94e58ca09662dd6e65e8f1b0fd145767be3da7d5422a863a27b8d2b68e090c9";

static SIGNAL: AtomicI32 = AtomicI32::new(0);

extern "C" fn on_signal(sig: i32) {
    SIGNAL.store(sig, Ordering::SeqCst);
}

unsafe extern "C" {
    fn signal(signum: i32, handler: extern "C" fn(i32)) -> usize;
    fn kill(pid: i32, sig: i32) -> i32;
    fn waitpid(pid: i32, status: *mut i32, options: i32) -> i32;
}

const WNOHANG: i32 = 1;
const EINTR: i32 = 4;
const ECHILD: i32 = 10;

struct Outcome {
    code: u8,
    stderr: String,
}

impl Outcome {
    fn code(code: u8) -> Self {
        Self {
            code,
            stderr: String::new(),
        }
    }

    fn message(code: u8, stderr: impl Into<String>) -> Self {
        Self {
            code,
            stderr: stderr.into(),
        }
    }
}

struct Cleanup {
    container: String,
    env_file: Option<PathBuf>,
}

impl Drop for Cleanup {
    fn drop(&mut self) {
        let _ = Command::new("docker")
            .args(["rm", "-f", "-v", &self.container])
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status();
        if let Some(path) = self.env_file.take() {
            let _ = fs::remove_file(path);
        }
    }
}

pub fn run(args: &[String]) -> ExitCode {
    let outcome = launch(args);
    if !outcome.stderr.is_empty() {
        let mut err = io::stderr();
        let _ = err.write_all(outcome.stderr.as_bytes());
        let _ = err.flush();
    }
    ExitCode::from(outcome.code)
}

fn launch(args: &[String]) -> Outcome {
    launch_budget(args, READY_BUDGET)
}

fn launch_budget(args: &[String], budget: Duration) -> Outcome {
    if args.is_empty() {
        return Outcome::message(
            2,
            "usage: cargo xtask start-test <postgres|minio|meili> [command...]\n",
        );
    }
    match args[0].as_str() {
        "postgres" => launch_postgres(&args[1..], budget),
        "minio" => launch_minio(&args[1..], budget),
        "meili" => launch_meili(&args[1..], budget),
        other => Outcome::message(
            2,
            format!(
                "unknown service: {other}\nusage: cargo xtask start-test <postgres|minio|meili> [command...]\n"
            ),
        ),
    }
}

fn launch_postgres(cmd: &[String], budget: Duration) -> Outcome {
    if let Some(outcome) = require_tools(&["docker", "openssl"], "PostgreSQL") {
        return outcome;
    }
    let command = if cmd.is_empty() {
        vec![default_postgres_script()]
    } else {
        cmd.to_vec()
    };
    let major = match std::env::var("FVOCI_TEST_PG_MAJOR") {
        Ok(value) => value,
        Err(_) => "18".to_string(),
    };
    let image = match major.as_str() {
        "16" => PG16_IMAGE,
        "17" => PG17_IMAGE,
        "18" => PG18_IMAGE,
        _ => {
            return Outcome::message(
                2,
                format!("FVOCI_TEST_PG_MAJOR must be 16, 17 or 18 (got '{major}')\n"),
            );
        }
    };
    let max_connections = match std::env::var("FVOCI_TEST_PG_MAX_CONNECTIONS") {
        Ok(value) if !value.is_empty() => value,
        _ => "150".to_string(),
    };
    let run_id = match openssl_hex(16) {
        Ok(value) => value,
        Err(outcome) => return outcome,
    };
    let password = match openssl_hex(24) {
        Ok(value) => value,
        Err(outcome) => return outcome,
    };
    let container = format!("fvoci-rust-test-pg-{run_id}");
    let env_file = match write_private(
        &format!("fvoci-pg-env.{run_id}"),
        &format!("POSTGRES_PASSWORD={password}\n"),
    ) {
        Ok(path) => path,
        Err(outcome) => return outcome,
    };
    let _cleanup = Cleanup {
        container: container.clone(),
        env_file: Some(env_file.clone()),
    };
    install_signal_handlers();
    if let Some(outcome) = signal_outcome() {
        return outcome;
    }

    let cid = match docker_out(&[
        "run",
        "-d",
        "--rm",
        "--name",
        &container,
        "--label",
        &format!("fvoci.test-run={run_id}"),
        "--env-file",
        &env_file.display().to_string(),
        "-p",
        "127.0.0.1:0:5432",
        image,
        "postgres",
        "-c",
        &format!("max_connections={max_connections}"),
    ]) {
        Ok(cid) => cid,
        Err(outcome) => return outcome,
    };
    if let Some(outcome) = signal_outcome() {
        return outcome;
    }

    let ready = wait_ready(budget, || {
        docker_status(&[
            "exec",
            &cid,
            "pg_isready",
            "-h",
            "127.0.0.1",
            "-U",
            "postgres",
        ])
        .is_ok_and(|status| status.success())
    });
    match ready {
        Wait::Ready => {}
        Wait::Timeout => {
            return Outcome::message(1, "postgres did not become ready within 30s\n");
        }
        Wait::Signal(sig) => return Outcome::code(signal_exit(sig)),
    }

    let version = match docker_out(&[
        "exec",
        &cid,
        "psql",
        "-U",
        "postgres",
        "-tAc",
        "SHOW server_version_num",
    ]) {
        Ok(value) => value,
        Err(outcome) => return outcome,
    };
    if !pg_version_matches(&major, &version) {
        return Outcome::message(
            1,
            format!("expected PostgreSQL {major}, got server_version_num {version}\n"),
        );
    }
    let port = match published_port(&cid, "5432") {
        Ok(port) => port,
        Err(outcome) => return outcome,
    };
    let url = format!("postgres://postgres:{password}@127.0.0.1:{port}/postgres");
    let child_env = [
        ("TEST_DATABASE_URL", url),
        ("FVOCI_TEST_PG_CONTAINER", container),
    ];
    run_command(&command, &child_env)
}

fn launch_minio(cmd: &[String], budget: Duration) -> Outcome {
    if let Some(outcome) = require_tools(&["docker", "openssl", "curl"], "MinIO") {
        return outcome;
    }
    if cmd.is_empty() {
        return Outcome::message(
            1,
            "usage: cargo xtask start-test minio <command> [args...]\n",
        );
    }
    let run_id = match openssl_hex(16) {
        Ok(value) => value,
        Err(outcome) => return outcome,
    };
    let access_key = match openssl_hex(8) {
        Ok(value) => format!("fvoci{value}"),
        Err(outcome) => return outcome,
    };
    let secret_key = match openssl_hex(24) {
        Ok(value) => value,
        Err(outcome) => return outcome,
    };
    let container = format!("fvoci-rust-test-minio-{run_id}");
    let mut body = format!("MINIO_ROOT_USER={access_key}\nMINIO_ROOT_PASSWORD={secret_key}\n");
    if let Ok(origin) = std::env::var("FVOCI_TEST_MINIO_CORS_ALLOW_ORIGIN") {
        if !origin.is_empty() {
            body.push_str(&format!("MINIO_API_CORS_ALLOW_ORIGIN={origin}\n"));
        }
    }
    let env_file = match write_private(&format!("fvoci-minio-env.{run_id}"), &body) {
        Ok(path) => path,
        Err(outcome) => return outcome,
    };
    let _cleanup = Cleanup {
        container: container.clone(),
        env_file: Some(env_file.clone()),
    };
    install_signal_handlers();
    if let Some(outcome) = signal_outcome() {
        return outcome;
    }

    let cid = match docker_out(&[
        "run",
        "-d",
        "--rm",
        "--name",
        &container,
        "--label",
        &format!("fvoci.test-run={run_id}"),
        "--env-file",
        &env_file.display().to_string(),
        "-p",
        "127.0.0.1:0:9000",
        MINIO_IMAGE,
        "server",
        "/data",
    ]) {
        Ok(cid) => cid,
        Err(outcome) => return outcome,
    };
    let port = match published_port(&cid, "9000") {
        Ok(port) => port,
        Err(outcome) => return outcome,
    };
    let endpoint = format!("http://127.0.0.1:{port}");
    let health = format!("{endpoint}/minio/health/ready");
    let ready = wait_ready(budget, || {
        Command::new("curl")
            .args(["-fsS", "-o", "/dev/null", &health])
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::inherit())
            .status()
            .is_ok_and(|status| status.success())
    });
    match ready {
        Wait::Ready => {}
        Wait::Timeout => {
            let logs = docker_logs(&cid, Some(20));
            return Outcome::message(
                1,
                format!("minio/silo did not become ready within 30s\n{logs}"),
            );
        }
        Wait::Signal(sig) => return Outcome::code(signal_exit(sig)),
    }

    let region = match std::env::var("S3_REGION") {
        Ok(value) if !value.is_empty() => value,
        _ => "us-east-1".to_string(),
    };
    let path_style = match std::env::var("S3_FORCE_PATH_STYLE") {
        Ok(value) if !value.is_empty() => value,
        _ => "1".to_string(),
    };
    let bucket = format!("fvoci-test-{}", &run_id[..run_id.len().min(12)]);
    let child_env = [
        ("S3_ENDPOINT", endpoint),
        ("S3_REGION", region),
        ("S3_BUCKET", bucket),
        ("S3_ACCESS_KEY_ID", access_key),
        ("S3_SECRET_ACCESS_KEY", secret_key),
        ("S3_FORCE_PATH_STYLE", path_style),
        ("FVOCI_TEST_MINIO_CONTAINER", container),
    ];
    run_command(cmd, &child_env)
}

fn launch_meili(cmd: &[String], budget: Duration) -> Outcome {
    if let Some(outcome) = require_tools(&["docker", "openssl"], "Meilisearch") {
        return outcome;
    }
    if cmd.is_empty() {
        return Outcome::message(
            1,
            "usage: cargo xtask start-test meili <command> [args...]\n",
        );
    }
    let run_id = match openssl_hex(16) {
        Ok(value) => value,
        Err(outcome) => return outcome,
    };
    let master_key = match openssl_hex(16) {
        Ok(value) => value,
        Err(outcome) => return outcome,
    };
    let container = format!("fvoci-rust-test-meili-{run_id}");
    let _cleanup = Cleanup {
        container: container.clone(),
        env_file: None,
    };
    install_signal_handlers();
    if let Some(outcome) = signal_outcome() {
        return outcome;
    }

    let key_arg = format!("MEILI_MASTER_KEY={master_key}");
    let cid = match docker_out(&[
        "run",
        "-d",
        "--rm",
        "--name",
        &container,
        "--label",
        &format!("fvoci.test-run={run_id}"),
        "-e",
        &key_arg,
        "-e",
        "MEILI_NO_ANALYTICS=true",
        "-e",
        "MEILI_ENV=production",
        "-p",
        "127.0.0.1:0:7700",
        MEILI_IMAGE,
    ]) {
        Ok(cid) => cid,
        Err(outcome) => return outcome,
    };
    let ready = wait_ready(budget, || {
        docker_status(&[
            "exec",
            &cid,
            "wget",
            "-q",
            "-O",
            "/dev/null",
            "http://127.0.0.1:7700/health",
        ])
        .is_ok_and(|status| status.success())
    });
    match ready {
        Wait::Ready => {}
        Wait::Timeout => {
            let logs = docker_logs(&cid, None);
            return Outcome::message(
                1,
                format!("meilisearch did not become ready within 30s\n{logs}"),
            );
        }
        Wait::Signal(sig) => return Outcome::code(signal_exit(sig)),
    }
    let port = match published_port(&cid, "7700") {
        Ok(port) => port,
        Err(outcome) => return outcome,
    };
    let url = format!("http://127.0.0.1:{port}");
    let child_env = [
        ("FVOCI_MEILI_URL", url),
        ("FVOCI_MEILI_KEY", master_key.clone()),
        ("MEILI_MASTER_KEY", master_key),
        ("FVOCI_TEST_MEILI_CONTAINER", container),
    ];
    run_command(cmd, &child_env)
}

fn require_tools(tools: &[&str], label: &str) -> Option<Outcome> {
    for tool in tools {
        if tool_on_path(tool) {
            continue;
        }
        return Some(Outcome::message(
            1,
            format!("{tool} is required for local test {label}\n"),
        ));
    }
    None
}

fn tool_on_path(name: &str) -> bool {
    let Ok(path) = std::env::var("PATH") else {
        return false;
    };
    for dir in path.split(':') {
        if dir.is_empty() {
            continue;
        }
        let candidate = Path::new(dir).join(name);
        let Ok(meta) = fs::metadata(&candidate) else {
            continue;
        };
        if meta.is_file() && meta.permissions().mode() & 0o111 != 0 {
            return true;
        }
    }
    false
}

fn default_postgres_script() -> String {
    repo_root()
        .join("scripts/run-db-tests.sh")
        .to_string_lossy()
        .into_owned()
}

fn repo_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("xtask lives in the repository")
        .to_path_buf()
}

fn openssl_hex(nbytes: u32) -> Result<String, Outcome> {
    let output = Command::new("openssl")
        .args(["rand", "-hex", &nbytes.to_string()])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::inherit())
        .output();
    let output = match output {
        Ok(output) => output,
        Err(err) if err.kind() == io::ErrorKind::NotFound => {
            return Err(Outcome::message(127, "openssl: command not found\n"));
        }
        Err(err) => return Err(Outcome::message(1, format!("openssl: {err}\n"))),
    };
    if !output.status.success() {
        return Err(Outcome::code(code_u8(status_code(output.status))));
    }
    Ok(strip_trailing_newlines(&String::from_utf8_lossy(&output.stdout)).to_string())
}

fn write_private(name: &str, body: &str) -> Result<PathBuf, Outcome> {
    let path = std::env::temp_dir().join(name);
    let file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(&path);
    let mut file = match file {
        Ok(file) => file,
        Err(err) => {
            return Err(Outcome::message(
                1,
                format!("could not create {}: {err}\n", path.display()),
            ));
        }
    };
    if let Err(err) = file.write_all(body.as_bytes()) {
        let _ = fs::remove_file(&path);
        return Err(Outcome::message(
            1,
            format!("could not write {}: {err}\n", path.display()),
        ));
    }
    drop(file);
    if let Err(err) = fs::set_permissions(&path, fs::Permissions::from_mode(0o600)) {
        let _ = fs::remove_file(&path);
        return Err(Outcome::message(
            1,
            format!("could not restrict {}: {err}\n", path.display()),
        ));
    }
    Ok(path)
}

fn docker_out(args: &[&str]) -> Result<String, Outcome> {
    let output = docker_command(args).output();
    let output = match output {
        Ok(output) => output,
        Err(err) if err.kind() == io::ErrorKind::NotFound => {
            return Err(Outcome::message(127, "docker: command not found\n"));
        }
        Err(err) => return Err(Outcome::message(1, format!("docker: {err}\n"))),
    };
    let _ = io::stderr().write_all(&output.stderr);
    if !output.status.success() {
        return Err(Outcome::code(code_u8(status_code(output.status))));
    }
    Ok(strip_trailing_newlines(&String::from_utf8_lossy(&output.stdout)).to_string())
}

fn docker_status(args: &[&str]) -> io::Result<ExitStatus> {
    docker_command(args)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
}

fn docker_command(args: &[&str]) -> Command {
    let mut command = Command::new("docker");
    command
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    command
}

fn docker_logs(cid: &str, tail: Option<usize>) -> String {
    let output = docker_command(&["logs", cid]).output();
    let Ok(output) = output else {
        return String::new();
    };
    let mut text = String::from_utf8_lossy(&output.stdout).into_owned();
    text.push_str(&String::from_utf8_lossy(&output.stderr));
    if let Some(limit) = tail {
        text = last_lines(&text, limit);
    }
    if !text.is_empty() && !text.ends_with('\n') {
        text.push('\n');
    }
    text
}

fn last_lines(text: &str, limit: usize) -> String {
    let lines: Vec<&str> = text.lines().collect();
    let start = lines.len().saturating_sub(limit);
    let mut out = lines[start..].join("\n");
    if !out.is_empty() {
        out.push('\n');
    }
    out
}

fn published_port(cid: &str, container_port: &str) -> Result<String, Outcome> {
    let text = docker_out(&["port", cid, container_port])?;
    Ok(parse_published_port(&text))
}

fn parse_published_port(text: &str) -> String {
    text.lines()
        .next()
        .unwrap_or("")
        .rsplit(':')
        .next()
        .unwrap_or("")
        .to_string()
}

fn pg_version_matches(major: &str, server_version_num: &str) -> bool {
    let Some(rest) = server_version_num.strip_prefix(major) else {
        return false;
    };
    rest.len() == 4 && rest.chars().all(|c| c.is_ascii_digit())
}

enum Wait {
    Ready,
    Timeout,
    Signal(i32),
}

fn wait_ready(budget: Duration, mut probe: impl FnMut() -> bool) -> Wait {
    let deadline = Instant::now() + budget;
    loop {
        if probe() {
            return Wait::Ready;
        }
        if let Some(sig) = take_signal() {
            return Wait::Signal(sig);
        }
        if Instant::now() >= deadline {
            return Wait::Timeout;
        }
        let slice = Duration::from_secs(1).min(deadline.saturating_duration_since(Instant::now()));
        if slice.is_zero() {
            return Wait::Timeout;
        }
        thread::sleep(slice);
    }
}

fn run_command(command: &[String], child_env: &[(&str, String)]) -> Outcome {
    if command.is_empty() {
        return Outcome::message(
            1,
            "usage: cargo xtask start-test <service> <command> [args...]\n",
        );
    }
    if let Some(outcome) = signal_outcome() {
        return outcome;
    }
    let mut child = Command::new(&command[0]);
    child
        .args(&command[1..])
        .stdin(Stdio::inherit())
        .stdout(Stdio::inherit())
        .stderr(Stdio::inherit());
    for (key, value) in child_env {
        child.env(key, value);
    }
    match child.spawn() {
        Ok(child) => Outcome::code(code_u8(supervise(child, &SIGNAL))),
        Err(err) if err.kind() == io::ErrorKind::NotFound => {
            let reason = if command[0].contains('/') {
                "No such file or directory"
            } else {
                "command not found"
            };
            Outcome::message(127, format!("{}: {reason}\n", command[0]))
        }
        Err(err) if err.kind() == io::ErrorKind::PermissionDenied => {
            Outcome::message(126, format!("{}: permission denied\n", command[0]))
        }
        Err(err) => Outcome::message(1, format!("{}: {err}\n", command[0])),
    }
}

/// Wait for `child` on this thread. A raised signal kills that process and
/// reaps it before returning, so an inherited stdout handle cannot outlive the
/// supervisor. The command stays in this process's group; a new group would
/// escape the parent's cleanup.
fn supervise(child: std::process::Child, signals: &AtomicI32) -> i32 {
    let pid = i32::try_from(child.id()).unwrap_or(-1);
    let code = wait_child(pid, signals);
    // The raw wait already reaped `pid`. Dropping `Child` would wait again.
    std::mem::forget(child);
    code
}

fn wait_child(pid: i32, signals: &AtomicI32) -> i32 {
    if pid <= 0 {
        return 1;
    }
    loop {
        if let Some(sig) = take(signals) {
            return stop_child(pid, sig);
        }
        let mut status = 0;
        let waited = unsafe { waitpid(pid, &mut status, WNOHANG) };
        if waited == pid {
            if let Some(sig) = take(signals) {
                return signal_exit_i32(sig);
            }
            return exit_from_wait(status);
        }
        if waited < 0 {
            let err = std::io::Error::last_os_error().raw_os_error();
            if err == Some(EINTR) {
                continue;
            }
            if err == Some(ECHILD) {
                return 1;
            }
        }
        let mut status = 0;
        let waited = unsafe { waitpid(pid, &mut status, 0) };
        if waited == pid {
            if let Some(sig) = take(signals) {
                return signal_exit_i32(sig);
            }
            return exit_from_wait(status);
        }
        if waited < 0 {
            let err = std::io::Error::last_os_error().raw_os_error();
            if err == Some(EINTR) {
                continue;
            }
            if err == Some(ECHILD) {
                return 1;
            }
        }
    }
}

fn stop_child(pid: i32, sig: i32) -> i32 {
    unsafe { kill(pid, sig) };
    loop {
        let mut status = 0;
        let waited = unsafe { waitpid(pid, &mut status, 0) };
        if waited == pid || waited < 0 {
            let err = std::io::Error::last_os_error().raw_os_error();
            if waited < 0 && err == Some(EINTR) {
                continue;
            }
            break;
        }
    }
    signal_exit_i32(sig)
}

fn take(signals: &AtomicI32) -> Option<i32> {
    let sig = signals.swap(0, Ordering::SeqCst);
    if sig == 0 {
        None
    } else {
        Some(sig)
    }
}

fn exit_from_wait(status: i32) -> i32 {
    // wait status: exit code is bits 8..16 when the low 7 bits are clear.
    if status & 0x7f == 0 {
        return (status >> 8) & 0xff;
    }
    128 + (status & 0x7f)
}

fn install_signal_handlers() {
    SIGNAL.store(0, Ordering::SeqCst);
    unsafe {
        signal(SIGINT, on_signal);
        signal(SIGTERM, on_signal);
    }
}

fn take_signal() -> Option<i32> {
    let sig = SIGNAL.swap(0, Ordering::SeqCst);
    if sig == 0 {
        None
    } else {
        Some(sig)
    }
}

fn signal_outcome() -> Option<Outcome> {
    take_signal().map(|sig| Outcome::code(signal_exit(sig)))
}

fn signal_exit(sig: i32) -> u8 {
    signal_exit_i32(sig) as u8
}

fn signal_exit_i32(sig: i32) -> i32 {
    128 + sig
}

fn status_code(status: ExitStatus) -> i32 {
    if let Some(code) = status.code() {
        return code;
    }
    if let Some(sig) = status.signal() {
        return 128 + sig;
    }
    1
}

fn code_u8(code: i32) -> u8 {
    if (0..=255).contains(&code) {
        code as u8
    } else {
        1
    }
}

fn strip_trailing_newlines(text: &str) -> &str {
    text.trim_end_matches(['\n', '\r'])
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::OpenOptionsExt;
    use std::sync::{Mutex, OnceLock};

    fn env_lock() -> &'static Mutex<()> {
        static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
        LOCK.get_or_init(|| Mutex::new(()))
    }

    struct EnvSet {
        key: &'static str,
        prev: Option<String>,
    }

    impl EnvSet {
        fn set(key: &'static str, value: &str) -> Self {
            let prev = std::env::var(key).ok();
            std::env::set_var(key, value);
            Self { key, prev }
        }
    }

    impl Drop for EnvSet {
        fn drop(&mut self) {
            match &self.prev {
                Some(value) => std::env::set_var(self.key, value),
                None => std::env::remove_var(self.key),
            }
        }
    }

    struct Sandbox {
        dir: PathBuf,
        log: PathBuf,
        env_copy: PathBuf,
        env_mode: PathBuf,
    }

    impl Sandbox {
        fn new(docker_body: &str, curl: Option<bool>) -> Self {
            let dir = std::env::temp_dir().join(format!(
                "fvoci-xtask-start-{}-{}",
                std::process::id(),
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .expect("clock")
                    .as_nanos()
            ));
            fs::create_dir_all(&dir).unwrap();
            let log = dir.join("docker.log");
            let env_copy = dir.join("env.copy");
            let env_mode = dir.join("env.mode");
            let script = docker_script(&log, &env_copy, &env_mode, docker_body);
            write_exe(&dir.join("docker"), &script);
            if let Some(curl_fails) = curl {
                write_exe(
                    &dir.join("curl"),
                    &format!(
                        "#!/usr/bin/env bash\nexit {}\n",
                        if curl_fails { 1 } else { 0 }
                    ),
                );
            }
            Self {
                dir,
                log,
                env_copy,
                env_mode,
            }
        }

        fn prepend_path(&self) -> EnvSet {
            let old = std::env::var("PATH").unwrap_or_default();
            EnvSet::set("PATH", &format!("{}:{}", self.dir.display(), old))
        }

        fn only_path(&self) -> EnvSet {
            EnvSet::set("PATH", &self.dir.to_string_lossy())
        }

        fn invocations(&self) -> Vec<Vec<String>> {
            let bytes = fs::read(&self.log).unwrap_or_default();
            parse_invocations(&bytes)
        }
    }

    impl Drop for Sandbox {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.dir);
        }
    }

    fn write_exe(path: &Path, body: &str) {
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o755)
            .open(path)
            .unwrap();
        file.write_all(body.as_bytes()).unwrap();
    }

    fn docker_script(log: &Path, env_copy: &Path, env_mode: &Path, body: &str) -> String {
        format!(
            r#"#!/usr/bin/env bash
set -u
log="{log}"
env_copy="{env_copy}"
env_mode="{env_mode}"
{{
  printf '%s' "$#"
  printf '\0'
  printf '%s\0' "$@"
  printf '\0'
}} >> "$log"
if [[ "$1" == run ]]; then
  for ((i = 1; i <= $#; i++)); do
    if [[ "${{!i}}" == --env-file ]]; then
      j=$((i + 1))
      stat -c %a "${{!j}}" > "$env_mode"
      cp "${{!j}}" "$env_copy"
    fi
  done
fi
{body}
"#,
            log = log.display(),
            env_copy = env_copy.display(),
            env_mode = env_mode.display(),
        )
    }

    fn parse_invocations(bytes: &[u8]) -> Vec<Vec<String>> {
        let mut out = Vec::new();
        let mut i = 0;
        while i < bytes.len() {
            let start = i;
            while i < bytes.len() && bytes[i] != 0 {
                i += 1;
            }
            if i >= bytes.len() {
                break;
            }
            let argc: usize = std::str::from_utf8(&bytes[start..i])
                .unwrap()
                .parse()
                .unwrap();
            i += 1;
            let mut args = Vec::with_capacity(argc);
            for _ in 0..argc {
                let arg_start = i;
                while i < bytes.len() && bytes[i] != 0 {
                    i += 1;
                }
                args.push(String::from_utf8(bytes[arg_start..i].to_vec()).unwrap());
                i += 1;
            }
            if i < bytes.len() && bytes[i] == 0 {
                i += 1;
            }
            out.push(args);
        }
        out
    }

    fn success_docker(pg_version: &str) -> String {
        format!(
            r#"case "$1" in
  run) printf '%s\n' fixturecid ;;
  port) printf '%s\n' '127.0.0.1:43210' ;;
  exec)
    joined="$*"
    case "$joined" in
      *psql*) printf '%s\n' {pg_version} ;;
    esac
    ;;
  logs) printf '%s\n' log-a log-b ;;
  rm) ;;
  *) printf 'unexpected docker %s\n' "$*" >&2; exit 2 ;;
esac
"#
        )
    }

    fn has_pair(args: &[String], left: &str, right: &str) -> bool {
        args.windows(2)
            .any(|pair| pair[0] == left && pair[1] == right)
    }

    fn find_run(calls: &[Vec<String>]) -> &[String] {
        calls
            .iter()
            .find(|args| args.first().is_some_and(|arg| arg == "run"))
            .map(Vec::as_slice)
            .unwrap_or(&[])
    }

    fn assert_removed(calls: &[Vec<String>], container: &str) {
        assert!(
            calls.iter().any(|args| {
                args.first().is_some_and(|arg| arg == "rm")
                    && args.get(1).is_some_and(|arg| arg == "-f")
                    && args.get(2).is_some_and(|arg| arg == "-v")
                    && args.get(3).is_some_and(|arg| arg == container)
            }),
            "missing docker rm -f -v {container}: {calls:?}"
        );
    }

    #[test]
    fn published_port_uses_the_last_colon_field_of_the_first_line() {
        assert_eq!(parse_published_port("127.0.0.1:43210\n"), "43210");
        assert_eq!(parse_published_port("[::1]:7700\n0.0.0.0:1\n"), "7700");
        assert_eq!(parse_published_port("54321\n"), "54321");
        assert_eq!(parse_published_port(""), "");
    }

    #[test]
    fn postgres_version_must_be_the_major_plus_four_digits() {
        assert!(pg_version_matches("18", "180003"));
        assert!(pg_version_matches("16", "160015"));
        assert!(!pg_version_matches("18", "18003"));
        assert!(!pg_version_matches("18", "170003"));
        assert!(!pg_version_matches("18", "180003 "));
        assert!(!pg_version_matches("18", ""));
    }

    #[test]
    fn signal_status_uses_the_conventional_offset() {
        assert_eq!(signal_exit(SIGINT), 130);
        assert_eq!(signal_exit(SIGTERM), 143);
    }

    #[test]
    fn missing_service_and_unknown_service_exit_2() {
        let missing = launch(&[]);
        assert_eq!(missing.code, 2);
        assert!(missing.stderr.contains("postgres|minio|meili"));
        let unknown = launch(&["redis".to_string()]);
        assert_eq!(unknown.code, 2);
        assert!(unknown.stderr.contains("unknown service: redis"));
    }

    #[test]
    fn missing_tools_name_the_service_and_exit_1() {
        let _guard = env_lock().lock().unwrap();
        let sandbox = Sandbox::new(&success_docker("180003"), None);
        let empty = sandbox.dir.join("empty");
        fs::create_dir(&empty).unwrap();
        let _path = EnvSet::set("PATH", &empty.to_string_lossy());
        let postgres = launch(&["postgres".to_string(), "true".to_string()]);
        assert_eq!(postgres.code, 1);
        assert_eq!(
            postgres.stderr,
            "docker is required for local test PostgreSQL\n"
        );
        let minio = launch(&["minio".to_string(), "true".to_string()]);
        assert_eq!(minio.stderr, "docker is required for local test MinIO\n");
        let meili = launch(&["meili".to_string(), "true".to_string()]);
        assert_eq!(
            meili.stderr,
            "docker is required for local test Meilisearch\n"
        );
    }

    #[test]
    fn minio_requires_curl_after_docker_and_openssl() {
        let _guard = env_lock().lock().unwrap();
        let sandbox = Sandbox::new(&success_docker("180003"), None);
        let openssl = which("openssl").expect("openssl");
        fs::copy(openssl, sandbox.dir.join("openssl")).unwrap();
        let _path = sandbox.only_path();
        let outcome = launch(&["minio".to_string(), "true".to_string()]);
        assert_eq!(outcome.code, 1);
        assert_eq!(outcome.stderr, "curl is required for local test MinIO\n");
    }

    #[test]
    fn minio_and_meili_require_a_command() {
        let _guard = env_lock().lock().unwrap();
        let sandbox = Sandbox::new(&success_docker("180003"), Some(false));
        let _path = sandbox.prepend_path();
        let minio = launch(&["minio".to_string()]);
        assert_eq!(minio.code, 1);
        assert_eq!(
            minio.stderr,
            "usage: cargo xtask start-test minio <command> [args...]\n"
        );
        let meili = launch(&["meili".to_string()]);
        assert_eq!(meili.code, 1);
        assert_eq!(
            meili.stderr,
            "usage: cargo xtask start-test meili <command> [args...]\n"
        );
        assert!(sandbox.invocations().is_empty());
    }

    #[test]
    fn postgres_rejects_an_unknown_or_empty_major() {
        let _guard = env_lock().lock().unwrap();
        let sandbox = Sandbox::new(&success_docker("180003"), Some(false));
        let _path = sandbox.prepend_path();
        let _major = EnvSet::set("FVOCI_TEST_PG_MAJOR", "19");
        let outcome = launch(&["postgres".to_string(), "true".to_string()]);
        assert_eq!(outcome.code, 2);
        assert_eq!(
            outcome.stderr,
            "FVOCI_TEST_PG_MAJOR must be 16, 17 or 18 (got '19')\n"
        );
        drop(_major);
        let _empty = EnvSet::set("FVOCI_TEST_PG_MAJOR", "");
        let outcome = launch(&["postgres".to_string(), "true".to_string()]);
        assert_eq!(outcome.code, 2);
        assert!(outcome.stderr.contains("got ''"));
        assert!(sandbox.invocations().is_empty());
    }

    #[test]
    fn postgres_default_command_is_the_repo_db_test_script() {
        let script = default_postgres_script();
        assert!(script.ends_with("scripts/run-db-tests.sh"));
        assert!(Path::new(&script).is_file());
    }

    #[test]
    fn postgres_exports_the_url_and_keeps_the_password_out_of_argv() {
        let _guard = env_lock().lock().unwrap();
        let sandbox = Sandbox::new(&success_docker("160015"), Some(false));
        let _path = sandbox.prepend_path();
        let _major = EnvSet::set("FVOCI_TEST_PG_MAJOR", "16");
        let _max = EnvSet::set("FVOCI_TEST_PG_MAX_CONNECTIONS", "40");
        let dump = sandbox.dir.join("child.env");
        let outcome = launch(&[
            "postgres".to_string(),
            "bash".to_string(),
            "-c".to_string(),
            "umask 077; env > \"$1\"".to_string(),
            "bash".to_string(),
            dump.display().to_string(),
        ]);
        assert_eq!(outcome.code, 0, "{}", outcome.stderr);
        let calls = sandbox.invocations();
        let run = find_run(&calls);
        assert!(run.iter().any(|arg| arg == PG16_IMAGE));
        assert!(has_pair(run, "-c", "max_connections=40"));
        assert!(has_pair(run, "-p", "127.0.0.1:0:5432"));
        let mode = fs::read_to_string(&sandbox.env_mode).unwrap();
        assert_eq!(mode.trim(), "600");
        let env_file = fs::read_to_string(&sandbox.env_copy).unwrap();
        let password = env_file
            .strip_prefix("POSTGRES_PASSWORD=")
            .and_then(|rest| rest.strip_suffix('\n'))
            .unwrap();
        assert!(!run.iter().any(|arg| arg.contains(password)));
        let child = fs::read_to_string(&dump).unwrap();
        let url = env_value(&child, "TEST_DATABASE_URL");
        assert_eq!(
            url,
            format!("postgres://postgres:{password}@127.0.0.1:43210/postgres")
        );
        let container = env_value(&child, "FVOCI_TEST_PG_CONTAINER");
        assert!(container.starts_with("fvoci-rust-test-pg-"));
        assert_eq!(container.len(), "fvoci-rust-test-pg-".len() + 32);
        assert!(has_pair(run, "--name", &container));
        assert_removed(&calls, &container);
        let env_name = run.windows(2).find(|pair| pair[0] == "--env-file").unwrap()[1].clone();
        assert!(!Path::new(&env_name).exists());
    }

    #[test]
    fn postgres_timeout_and_version_mismatch_skip_the_command() {
        let _guard = env_lock().lock().unwrap();
        let timeout = Sandbox::new(
            r#"case "$1" in
  run) printf '%s\n' fixturecid ;;
  exec) exit 1 ;;
  rm) ;;
  *) exit 2 ;;
esac
"#,
            Some(false),
        );
        let _path = timeout.prepend_path();
        let marker = timeout.dir.join("ran");
        let outcome = launch_budget(
            &[
                "postgres".to_string(),
                "bash".to_string(),
                "-c".to_string(),
                format!("touch {}", marker.display()),
            ],
            Duration::ZERO,
        );
        assert_eq!(outcome.code, 1);
        assert_eq!(outcome.stderr, "postgres did not become ready within 30s\n");
        assert!(!marker.exists());
        drop(_path);
        drop(timeout);

        let mismatch = Sandbox::new(
            r#"case "$1" in
  run) printf '%s\n' fixturecid ;;
  port) printf '%s\n' '127.0.0.1:1' ;;
  exec)
    case "$*" in
      *psql*) printf '%s\n' 170003 ;;
    esac
    ;;
  rm) ;;
  *) exit 2 ;;
esac
"#,
            Some(false),
        );
        let _path = mismatch.prepend_path();
        let outcome = launch(&["postgres".to_string(), "true".to_string()]);
        assert_eq!(outcome.code, 1);
        assert_eq!(
            outcome.stderr,
            "expected PostgreSQL 18, got server_version_num 170003\n"
        );
        assert!(mismatch.invocations().iter().all(|args| args[0] != "port"));
    }

    #[test]
    fn postgres_returns_the_command_status_and_still_removes_the_container() {
        let _guard = env_lock().lock().unwrap();
        let sandbox = Sandbox::new(&success_docker("180003"), Some(false));
        let _path = sandbox.prepend_path();
        let outcome = launch(&[
            "postgres".to_string(),
            "bash".to_string(),
            "-c".to_string(),
            "exit 7".to_string(),
        ]);
        assert_eq!(outcome.code, 7);
        let calls = sandbox.invocations();
        let container = find_run(&calls)
            .windows(2)
            .find(|pair| pair[0] == "--name")
            .unwrap()[1]
            .clone();
        assert_removed(&calls, &container);
    }

    #[test]
    fn minio_exports_s3_settings_and_keeps_credentials_in_the_env_file() {
        let _guard = env_lock().lock().unwrap();
        let sandbox = Sandbox::new(&success_docker("180003"), Some(false));
        let _path = sandbox.prepend_path();
        let _region = EnvSet::set("S3_REGION", "eu-central-1");
        let _style = EnvSet::set("S3_FORCE_PATH_STYLE", "0");
        let _cors = EnvSet::set("FVOCI_TEST_MINIO_CORS_ALLOW_ORIGIN", "https://app.example");
        let dump = sandbox.dir.join("child.env");
        let outcome = launch(&[
            "minio".to_string(),
            "bash".to_string(),
            "-c".to_string(),
            "umask 077; env > \"$1\"".to_string(),
            "bash".to_string(),
            dump.display().to_string(),
        ]);
        assert_eq!(outcome.code, 0, "{}", outcome.stderr);
        assert_eq!(fs::read_to_string(&sandbox.env_mode).unwrap().trim(), "600");
        let stored = fs::read_to_string(&sandbox.env_copy).unwrap();
        assert!(stored.contains("MINIO_API_CORS_ALLOW_ORIGIN=https://app.example\n"));
        let access = stored
            .lines()
            .find_map(|line| line.strip_prefix("MINIO_ROOT_USER="))
            .unwrap();
        let secret = stored
            .lines()
            .find_map(|line| line.strip_prefix("MINIO_ROOT_PASSWORD="))
            .unwrap();
        assert!(access.starts_with("fvoci"));
        let calls = sandbox.invocations();
        let run = find_run(&calls);
        assert!(run.iter().any(|arg| arg == MINIO_IMAGE));
        assert!(has_pair(run, "server", "/data"));
        assert!(!run
            .iter()
            .any(|arg| arg.contains(secret) || arg.contains(access)));
        let child = fs::read_to_string(&dump).unwrap();
        assert_eq!(env_value(&child, "S3_ENDPOINT"), "http://127.0.0.1:43210");
        assert_eq!(env_value(&child, "S3_REGION"), "eu-central-1");
        assert_eq!(env_value(&child, "S3_FORCE_PATH_STYLE"), "0");
        assert_eq!(env_value(&child, "S3_ACCESS_KEY_ID"), access);
        assert_eq!(env_value(&child, "S3_SECRET_ACCESS_KEY"), secret);
        let container = env_value(&child, "FVOCI_TEST_MINIO_CONTAINER");
        let run_id = container.strip_prefix("fvoci-rust-test-minio-").unwrap();
        assert_eq!(
            env_value(&child, "S3_BUCKET"),
            format!("fvoci-test-{}", &run_id[..12])
        );
        assert_removed(&calls, &container);
        let port_at = calls
            .iter()
            .position(|args| args.first().is_some_and(|arg| arg == "port"))
            .unwrap();
        let run_at = calls
            .iter()
            .position(|args| args.first().is_some_and(|arg| arg == "run"))
            .unwrap();
        assert!(run_at < port_at);
        assert!(calls
            .iter()
            .all(|args| args.first().is_some_and(|arg| arg != "exec")));
    }

    #[test]
    fn minio_timeout_keeps_the_message_and_the_last_twenty_log_lines() {
        let _guard = env_lock().lock().unwrap();
        let mut lines = String::new();
        for n in 1..=25 {
            lines.push_str(&format!("printf '%s\\n' 'line-{n}'\n"));
        }
        let body = format!(
            r#"case "$1" in
  run) printf '%s\n' fixturecid ;;
  port) printf '%s\n' '127.0.0.1:9' ;;
  logs)
    {lines}    ;;
  rm) ;;
  *) exit 2 ;;
esac
"#
        );
        let sandbox = Sandbox::new(&body, Some(true));
        let _path = sandbox.prepend_path();
        let outcome = launch_budget(&["minio".to_string(), "true".to_string()], Duration::ZERO);
        assert_eq!(outcome.code, 1);
        assert!(outcome
            .stderr
            .starts_with("minio/silo did not become ready within 30s\n"));
        assert!(!outcome.stderr.contains("line-5\n"));
        assert!(outcome.stderr.contains("line-6\n"));
        assert!(outcome.stderr.contains("line-25\n"));
    }

    #[test]
    fn meili_exports_the_key_and_passes_it_to_the_container_environment() {
        let _guard = env_lock().lock().unwrap();
        let sandbox = Sandbox::new(&success_docker("180003"), Some(false));
        let _path = sandbox.prepend_path();
        let dump = sandbox.dir.join("child.env");
        let outcome = launch(&[
            "meili".to_string(),
            "bash".to_string(),
            "-c".to_string(),
            "umask 077; env > \"$1\"".to_string(),
            "bash".to_string(),
            dump.display().to_string(),
        ]);
        assert_eq!(outcome.code, 0, "{}", outcome.stderr);
        let child = fs::read_to_string(&dump).unwrap();
        let key = env_value(&child, "FVOCI_MEILI_KEY");
        assert_eq!(env_value(&child, "MEILI_MASTER_KEY"), key);
        assert_eq!(
            env_value(&child, "FVOCI_MEILI_URL"),
            "http://127.0.0.1:43210"
        );
        let container = env_value(&child, "FVOCI_TEST_MEILI_CONTAINER");
        assert!(container.starts_with("fvoci-rust-test-meili-"));
        let calls = sandbox.invocations();
        let run = find_run(&calls);
        assert!(run.iter().any(|arg| arg == MEILI_IMAGE));
        assert!(has_pair(run, "-e", &format!("MEILI_MASTER_KEY={key}")));
        assert!(has_pair(run, "-e", "MEILI_NO_ANALYTICS=true"));
        assert!(has_pair(run, "-e", "MEILI_ENV=production"));
        assert!(has_pair(run, "-p", "127.0.0.1:0:7700"));
        let exec = calls
            .iter()
            .find(|args| args.first().is_some_and(|arg| arg == "exec"))
            .unwrap();
        assert_eq!(
            exec[2..].iter().map(String::as_str).collect::<Vec<_>>(),
            [
                "wget",
                "-q",
                "-O",
                "/dev/null",
                "http://127.0.0.1:7700/health"
            ]
        );
        let port_at = calls
            .iter()
            .position(|args| args.first().is_some_and(|arg| arg == "port"))
            .unwrap();
        let exec_at = calls
            .iter()
            .position(|args| args.first().is_some_and(|arg| arg == "exec"))
            .unwrap();
        assert!(exec_at < port_at);
        assert_removed(&calls, &container);
    }

    #[test]
    fn a_missing_command_exits_127_after_the_container_is_removed() {
        let _guard = env_lock().lock().unwrap();
        let sandbox = Sandbox::new(&success_docker("180003"), Some(false));
        let _path = sandbox.prepend_path();
        let missing = sandbox.dir.join("not-a-command");
        let outcome = launch(&["meili".to_string(), missing.display().to_string()]);
        assert_eq!(outcome.code, 127);
        assert_eq!(
            outcome.stderr,
            format!("{}: No such file or directory\n", missing.display())
        );
        assert!(sandbox
            .invocations()
            .iter()
            .any(|args| args.first().is_some_and(|arg| arg == "rm")));
        let outcome = launch(&["meili".to_string(), "not-a-real-command".to_string()]);
        assert_eq!(outcome.code, 127);
        assert_eq!(outcome.stderr, "not-a-real-command: command not found\n");
    }

    #[test]
    fn sigterm_reaps_the_command_before_returning() {
        let flag = AtomicI32::new(SIGTERM);
        let child = Command::new("sleep")
            .arg("30")
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .expect("sleep");
        let pid = i32::try_from(child.id()).unwrap();
        let code = supervise(child, &flag);
        assert_eq!(code, 143);
        assert_eq!(
            unsafe { kill(pid, 0) },
            -1,
            "sleep must not outlive the supervisor"
        );
    }

    #[test]
    fn image_pins_match_the_workflow_and_product_constants() {
        let root = repo_root();
        let workflow = fs::read_to_string(root.join(".github/workflows/rust.yml")).unwrap();
        for image in [PG16_IMAGE, PG17_IMAGE, PG18_IMAGE, MEILI_IMAGE] {
            assert!(workflow.contains(image), "{image}");
        }
        let meili = fs::read_to_string(root.join("src/search/meili.rs")).unwrap();
        assert!(meili.contains(MEILI_IMAGE));
        let s3 = fs::read_to_string(root.join("src/attachments/s3.rs")).unwrap();
        assert!(s3.contains(MINIO_IMAGE));
    }

    fn env_value(dump: &str, key: &str) -> String {
        dump.lines()
            .find_map(|line| line.strip_prefix(&format!("{key}=")))
            .unwrap_or_else(|| panic!("{key} missing in {dump}"))
            .to_string()
    }

    fn which(name: &str) -> Option<PathBuf> {
        std::env::var_os("PATH")?
            .to_string_lossy()
            .split(':')
            .find_map(|dir| {
                let path = Path::new(dir).join(name);
                path.is_file().then_some(path)
            })
    }
}
