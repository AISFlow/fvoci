//! `cargo xtask prepare-rustup-ci-metadata --output DIR`: prepare only owned
//! GitHub collaboration CI metadata before build-input capture.
//!
//! Rustup 1.29.1's schema-3 `lib/rustlib/components` list has name-based
//! semantics but an unstable row order. This task rewrites that one file to
//! the canonical order, keeps its exact installed set and every other
//! toolchain byte and identity, and records receipts. It never installs,
//! updates, or relaxes a build-handoff input check.
//!
//! Refusals print one fixed reason (or an OS error class name) and never echo
//! child output, environment values, or private filesystem paths.

mod guard;
mod json;
mod prepare;
mod scope;

use crate::args::{self, Outcome};
use std::ffi::{OsStr, OsString};
use std::fmt;
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::process::Command;

pub use guard::Owner;

pub const TOOLCHAIN: &str = "1.98.1-x86_64-unknown-linux-gnu";
/// Installed component rows in canonical order.
pub const ROWS: [&str; 4] = [
    "cargo-x86_64-unknown-linux-gnu",
    "clippy-preview-x86_64-unknown-linux-gnu",
    "rust-std-x86_64-unknown-linux-gnu",
    "rustc-x86_64-unknown-linux-gnu",
];
/// Rustup's public component list reverses the manifest's clippy ->
/// clippy-preview rename.
pub const PUBLIC_ROWS: [&str; 4] = [
    "cargo-x86_64-unknown-linux-gnu",
    "clippy-x86_64-unknown-linux-gnu",
    "rust-std-x86_64-unknown-linux-gnu",
    "rustc-x86_64-unknown-linux-gnu",
];
pub const RELATIVE: &str = "lib/rustlib/components";
pub const RECEIPT_DIRECTORY: &str = "fvoci-rustup-ci-metadata";

pub fn canonical() -> Vec<u8> {
    let mut raw = ROWS.join("\n").into_bytes();
    raw.push(b'\n');
    raw
}

#[derive(Debug)]
pub enum Refusal {
    Reason(&'static str),
    Io(io::Error),
}

impl From<io::Error> for Refusal {
    fn from(error: io::Error) -> Self {
        Self::Io(error)
    }
}

impl fmt::Display for Refusal {
    /// OS errors print only their Python `OSError` subclass name.
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        let name = match self {
            Self::Reason(reason) => reason,
            Self::Io(error) => match error.raw_os_error() {
                Some(libc::ENOENT) => "FileNotFoundError",
                Some(libc::EEXIST) => "FileExistsError",
                Some(libc::EACCES | libc::EPERM) => "PermissionError",
                Some(libc::EISDIR) => "IsADirectoryError",
                Some(libc::ENOTDIR) => "NotADirectoryError",
                Some(libc::EINTR) => "InterruptedError",
                Some(libc::EAGAIN | libc::EALREADY | libc::EINPROGRESS) => "BlockingIOError",
                Some(libc::EPIPE | libc::ESHUTDOWN) => "BrokenPipeError",
                Some(libc::ECHILD) => "ChildProcessError",
                Some(libc::ESRCH) => "ProcessLookupError",
                Some(libc::ETIMEDOUT) => "TimeoutError",
                _ => "OSError",
            },
        };
        formatter.write_str(name)
    }
}

pub fn require(condition: bool, reason: &'static str) -> Result<(), Refusal> {
    if condition {
        Ok(())
    } else {
        Err(Refusal::Reason(reason))
    }
}

/// Python `str.strip()` over ASCII text.
fn strip(text: &str) -> &str {
    text.trim_matches(|c| matches!(c, ' ' | '\t'..='\r' | '\x1c'..='\x1f'))
}

/// Run a read-only inspection command with `RUSTUP_AUTO_INSTALL=0`; return
/// its unstripped ASCII stdout. Its stderr is discarded.
fn inspection(argv: &[&OsStr], cwd: Option<&Path>) -> Result<String, Refusal> {
    let mut command = Command::new(argv[0]);
    command.args(&argv[1..]).env("RUSTUP_AUTO_INSTALL", "0");
    if let Some(cwd) = cwd {
        command.current_dir(cwd);
    }
    let output = command.output()?;
    require(output.status.success(), "inspection-command-failed")?;
    require(
        output.stdout.len() <= 1024 * 1024,
        "inspection-output-bound",
    )?;
    require(output.stdout.is_ascii(), "inspection-output-ascii")?;
    Ok(String::from_utf8(output.stdout).expect("ASCII is UTF-8"))
}

fn component_list_argv(rustup: &Path) -> [&OsStr; 6] {
    [
        rustup.as_os_str(),
        OsStr::new("component"),
        OsStr::new("list"),
        OsStr::new("--installed"),
        OsStr::new("--toolchain"),
        OsStr::new(TOOLCHAIN),
    ]
}

/// The real `rustup component list --installed` inspection.
struct Rustup<'a>(&'a Path);

impl prepare::Inspect for Rustup<'_> {
    fn component_list(&mut self) -> Result<String, Refusal> {
        inspection(&component_list_argv(self.0), None)
    }
}

/// The process host: real environment, platform, filesystem and commands.
struct System {
    owner: Owner,
}

impl scope::Host for System {
    fn env(&self, name: &str) -> Option<String> {
        std::env::var_os(name).map(|value| value.to_string_lossy().into_owned())
    }

    fn uname(&self) -> io::Result<(String, String)> {
        crate::host::uname()
    }

    fn os_release(&self) -> Result<String, Refusal> {
        String::from_utf8(std::fs::read("/etc/os-release")?)
            .map_err(|_| Refusal::Reason("unsupported-os"))
    }

    fn cwd(&self) -> io::Result<PathBuf> {
        std::env::current_dir()?.canonicalize()
    }

    fn which(&self, name: &str) -> Option<PathBuf> {
        crate::host::which(name, std::env::var_os("PATH").as_deref())
    }

    fn command(&mut self, argv: &[&OsStr], cwd: Option<&Path>) -> Result<String, Refusal> {
        inspection(argv, cwd).map(|text| strip(&text).to_owned())
    }

    fn owned_directory(&self, path: &Path) -> Result<PathBuf, Refusal> {
        guard::owned_directory(path, self.owner)
    }

    fn physical(&self, path: &Path) -> Result<PathBuf, Refusal> {
        guard::physical(path)
    }

    fn executable(&self, path: &Path) -> Result<(), Refusal> {
        guard::regular(path, Some(0o755), None).map(drop)
    }

    fn file_facts(&self, path: &Path) -> Result<(String, guard::Identity), Refusal> {
        guard::file_facts(path)
    }
}

const USAGE: &str = "usage: cargo xtask prepare-rustup-ci-metadata [-h] --output OUTPUT";
const HELP: &str = "\
Canonicalize the pinned Rustup component row order on an allocated GitHub
collaboration CI runner, keeping every other toolchain input unchanged, and
write receipts to OUTPUT ($RUNNER_TEMP/fvoci-rustup-ci-metadata).

options:
  -h, --help       show this help message and exit
  --output OUTPUT
";

fn summary_line(after: &serde_json::Map<String, serde_json::Value>) -> String {
    let fields = [
        "source_head",
        "source_tree",
        "changed",
        "canonical_sha256",
        "other_toolchain_inputs_sha256",
        "other_toolchain_entry_count",
    ]
    .map(|name| {
        let mut field = String::new();
        json::string(name, &mut field);
        field.push_str(": ");
        field.push_str(&json::compact(&after[name]));
        field
    });
    format!("{{{}}}", fields.join(", "))
}

fn prepare_from_process(output: &str, out: &mut dyn Write) -> Result<(), Refusal> {
    let owner = Owner::current();
    let found = scope::scope(output, &mut System { owner })?;
    let after = prepare::prepare(
        &found.root,
        Path::new(output),
        &found.rustup,
        &found.context,
        owner,
        &mut Rustup(&found.rustup),
        out,
    )?;
    writeln!(out, "{}", summary_line(&after))?;
    Ok(())
}

/// Returns the exit status: 0, 1 (refused) or 2 (usage).
pub fn run(argv: Vec<OsString>, out: &mut dyn Write, err: &mut dyn Write) -> i32 {
    let parsed = match args::parse(argv, &[args::required("output")], false) {
        Outcome::Parsed(parsed) => parsed,
        Outcome::Help => {
            let _ = write!(out, "{USAGE}\n\n{HELP}");
            return 0;
        }
        Outcome::Usage(message) => {
            let _ = writeln!(err, "{USAGE}\nprepare-rustup-ci-metadata: error: {message}");
            return 2;
        }
    };
    let output = parsed.get("output").expect("required option");
    match prepare_from_process(output, out) {
        Ok(()) => 0,
        Err(refusal) => {
            let _ = out.flush();
            let _ = writeln!(err, "Rustup CI metadata preparation refused: {refusal}");
            1
        }
    }
}

pub fn main(argv: Vec<OsString>) -> i32 {
    let stdout = io::stdout();
    let stderr = io::stderr();
    run(argv, &mut stdout.lock(), &mut stderr.lock())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn literal_rows_and_canonical_bytes() {
        assert!(ROWS.contains(&"clippy-preview-x86_64-unknown-linux-gnu"));
        assert!(!ROWS.contains(&"clippy-x86_64-unknown-linux-gnu"));
        assert_eq!(
            canonical(),
            b"cargo-x86_64-unknown-linux-gnu\nclippy-preview-x86_64-unknown-linux-gnu\n\
              rust-std-x86_64-unknown-linux-gnu\nrustc-x86_64-unknown-linux-gnu\n"
        );
        assert_eq!(canonical().len(), 136);
    }

    #[test]
    fn exact_component_list_invocation() {
        assert_eq!(
            component_list_argv(Path::new("/r/rustup")),
            [
                "/r/rustup",
                "component",
                "list",
                "--installed",
                "--toolchain",
                "1.98.1-x86_64-unknown-linux-gnu"
            ]
            .map(OsStr::new)
        );
    }

    #[test]
    fn inspection_keeps_unstripped_stdout_and_refuses_failures() {
        let sh = OsStr::new("sh");
        let c = OsStr::new("-c");
        let script = |text: &'static str| [sh, c, OsStr::new(text)];
        assert_eq!(
            inspection(&script("printf 'a\\n'; echo noise >&2"), None).unwrap(),
            "a\n"
        );
        assert_eq!(
            inspection(&script("echo $RUSTUP_AUTO_INSTALL"), None).unwrap(),
            "0\n"
        );
        for (text, reason) in [
            ("exit 3", "inspection-command-failed"),
            ("kill -9 $$", "inspection-command-failed"),
            ("head -c 1048577 /dev/zero", "inspection-output-bound"),
            ("printf '\\377'", "inspection-output-ascii"),
        ] {
            assert_eq!(
                inspection(&script(text), None).unwrap_err().to_string(),
                reason
            );
        }
        assert!(inspection(&script("head -c 1048576 /dev/zero"), None).is_ok());
        let missing = [OsStr::new("/nonexistent/fvoci-inspection")];
        assert_eq!(
            inspection(&missing, None).unwrap_err().to_string(),
            "FileNotFoundError"
        );
    }

    #[test]
    fn strip_matches_python_ascii_whitespace() {
        assert_eq!(strip(" \t\n\x0b\x0c\r\x1c\x1d\x1e\x1fx y\n"), "x y");
        assert_eq!(strip("\x00x"), "\x00x");
    }

    #[test]
    fn usage_help_and_local_refusal() {
        let run_with = |args: &[&str]| {
            let (mut out, mut err) = (Vec::new(), Vec::new());
            let code = run(
                args.iter().map(OsString::from).collect(),
                &mut out,
                &mut err,
            );
            (
                code,
                String::from_utf8(out).unwrap(),
                String::from_utf8(err).unwrap(),
            )
        };
        let (code, out, _) = run_with(&["--help"]);
        assert_eq!(code, 0);
        assert!(out.starts_with(USAGE));
        for args in [
            &[][..],
            &["--output"],
            &["--out", "x"],
            &["--output", "x", "extra"],
        ] {
            let (code, out, err) = run_with(args);
            assert_eq!((code, out.as_str()), (2, ""), "{args:?}");
            assert!(err.starts_with(USAGE), "{err}");
        }
    }

    #[test]
    fn summary_line_uses_python_default_separators() {
        let after = serde_json::json!({
            "source_head": "5", "source_tree": "6", "changed": false,
            "canonical_sha256": "c", "other_toolchain_inputs_sha256": "o",
            "other_toolchain_entry_count": 7, "ignored": 1,
        });
        assert_eq!(
            summary_line(after.as_object().unwrap()),
            "{\"source_head\": \"5\", \"source_tree\": \"6\", \"changed\": false, \
             \"canonical_sha256\": \"c\", \"other_toolchain_inputs_sha256\": \"o\", \
             \"other_toolchain_entry_count\": 7}"
        );
    }
}
