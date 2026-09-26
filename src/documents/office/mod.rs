//! Office documents (source `packages/jobs/src/extract-text.ts`, officeparser
//! over PDF / DOCX / PPTX / XLSX / ODT / ODP / ODS).
//!
//! Untrusted bytes are parsed only in a child process: a hidden mode of this
//! binary, `--internal-office-extract`, like the image preview child. The
//! child sets its address-space and CPU rlimits before reading a byte, reads
//! the document from stdin (no temporary files), parses zip containers with
//! the same bounded reader as imports (entry count, inflated total, traversal,
//! zip64 and header agreement), resolves only predefined XML entities, and
//! writes one JSON outcome to stdout. The parent bounds input, output and wall
//! time, kills the child on timeout, drop or cancel, and dies-with-parent is
//! set on Linux. Imports ask for Markdown (the document body), attachment
//! extraction for plain text (search and preview).

mod odf;
mod ooxml;
mod pdf;
pub mod render;
mod xml;

use std::collections::HashMap;
use std::io::{Read, Write};
use std::path::Path;
use std::process::Stdio;
use std::time::Duration;

use serde::{Deserialize, Serialize};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio_util::sync::CancellationToken;

use crate::documents::import_zip::{
    unzip_bounded_select, ZipImportError, ZIP_MAX_UNCOMPRESSED_BYTES,
};
use render::Output;

/// Hidden argv[1] that turns this binary into the office child.
pub const OFFICE_HELPER_ARG: &str = "--internal-office-extract";

const ZIP_MAGIC: &[u8] = b"PK\x03\x04";
const MAX_STDERR_BYTES: u64 = 4096;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum OfficeKind {
    Pdf,
    Docx,
    Pptx,
    Xlsx,
    Odt,
    Odp,
    Ods,
}

impl OfficeKind {
    pub const ALL: [OfficeKind; 7] = [
        Self::Pdf,
        Self::Docx,
        Self::Pptx,
        Self::Xlsx,
        Self::Odt,
        Self::Odp,
        Self::Ods,
    ];

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Pdf => "pdf",
            Self::Docx => "docx",
            Self::Pptx => "pptx",
            Self::Xlsx => "xlsx",
            Self::Odt => "odt",
            Self::Odp => "odp",
            Self::Ods => "ods",
        }
    }

    fn parse(value: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|kind| kind.as_str() == value)
    }

    /// Source `pickExtractor`: the extension decides; bytes must then agree.
    pub fn from_name(name: &str) -> Option<Self> {
        let ext = name.rsplit_once('.')?.1.to_ascii_lowercase();
        Self::parse(&ext)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OfficeMode {
    /// Document body for imports (source `ast.to("md")`).
    Markdown,
    /// Search / preview text for attachments (source `ast.to("text")`).
    Text,
}

impl OfficeMode {
    fn as_str(self) -> &'static str {
        match self {
            Self::Markdown => "md",
            Self::Text => "text",
        }
    }
}

/// Canonical result of one office extraction.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum OfficeOutcome {
    /// Text mode may be cut at the output budget (`truncated`); Markdown
    /// over budget is a `resource_limit` instead.
    Ok { text: String, truncated: bool },
    /// Valid document without body text.
    Empty,
    /// Bytes do not match the named format, or an encrypted document.
    Unsupported { detail: String },
    /// The parser rejected or crashed on the bytes.
    Corrupt { detail: String },
    /// Input, inflate, output, time or memory ceiling.
    ResourceLimit { detail: String },
    /// Spawning or talking to the child failed.
    WorkerFailure { detail: String },
}

#[derive(Debug, Clone, Copy)]
pub struct OfficeLimits {
    pub max_input_bytes: u64,
    /// Markdown: UTF-8 bytes. Text: Unicode scalar values.
    pub max_output: usize,
    pub timeout: Duration,
    /// Child RLIMIT_AS (source `ISOLATE_CHILD_MAX_RSS_BYTES` 1.5 GiB).
    pub address_space: u64,
}

impl OfficeLimits {
    /// Attachment extraction (source `EXTRACT_MAX_BYTES`,
    /// `EXTRACT_PLAIN_MAX_CHARS`, `EXTRACT_TEXT_WATCHDOG_MS`).
    pub fn attachment() -> Self {
        Self {
            max_input_bytes: 20 * 1024 * 1024,
            max_output: 500_000,
            timeout: Duration::from_secs(120),
            address_space: 1536 * 1024 * 1024,
        }
    }

    /// Office import (source `IMPORT_PARSE_TIMEOUT_MS` 5 min; the body cap is
    /// the document body limit the result must fit anyway).
    pub fn import() -> Self {
        Self {
            max_input_bytes: crate::db::import_jobs::IMPORT_HTTP_MAX_BYTES as u64,
            max_output: crate::collab::derived_body::DOCUMENT_MAX_BODY_BYTES,
            timeout: Duration::from_secs(300),
            address_space: 1536 * 1024 * 1024,
        }
    }
}

/// Why a parser stopped early.
#[derive(Debug)]
pub(crate) enum Stop {
    Full,
    Unsupported(String),
    Corrupt(String),
    Limit(String),
}

fn wanted_part(name: &str) -> bool {
    name == "mimetype" || name.ends_with(".xml") || name.ends_with(".rels")
}

fn open_zip(bytes: &[u8]) -> Result<HashMap<String, Vec<u8>>, Stop> {
    if !bytes.starts_with(ZIP_MAGIC) {
        return Err(Stop::Unsupported("not a zip container".into()));
    }
    let entries =
        unzip_bounded_select(bytes, ZIP_MAX_UNCOMPRESSED_BYTES, wanted_part).map_err(|err| {
            match err {
                ZipImportError::TooLarge | ZipImportError::TooManyEntries => {
                    Stop::Limit(format!("zip: {err}"))
                }
                other => Stop::Corrupt(format!("zip: {other}")),
            }
        })?;
    Ok(entries.into_iter().map(|e| (e.name, e.data)).collect())
}

/// Parses `bytes` as `kind`. Runs inside the child; unit tests call it
/// directly.
pub fn extract_office(
    bytes: &[u8],
    kind: OfficeKind,
    mode: OfficeMode,
    max_output: usize,
) -> OfficeOutcome {
    if bytes.is_empty() {
        return OfficeOutcome::Unsupported {
            detail: "empty file".into(),
        };
    }
    let mut out = Output::new(mode, max_output);
    let result = match kind {
        OfficeKind::Pdf => pdf::pdf(bytes, &mut out),
        OfficeKind::Docx | OfficeKind::Pptx | OfficeKind::Xlsx => {
            open_zip(bytes).and_then(|parts| match kind {
                OfficeKind::Docx => ooxml::docx(&parts, &mut out),
                OfficeKind::Pptx => ooxml::pptx(&parts, &mut out),
                _ => ooxml::xlsx(&parts, &mut out, max_output),
            })
        }
        OfficeKind::Odt | OfficeKind::Odp | OfficeKind::Ods => {
            open_zip(bytes).and_then(|parts| odf::odf(&parts, kind, &mut out))
        }
    };
    match result {
        Ok(()) => {}
        Err(Stop::Full) if mode == OfficeMode::Text => {}
        Err(Stop::Full) => {
            return OfficeOutcome::ResourceLimit {
                detail: format!("markdown exceeds {max_output} bytes"),
            }
        }
        Err(Stop::Unsupported(detail)) => return OfficeOutcome::Unsupported { detail },
        Err(Stop::Corrupt(detail)) => return OfficeOutcome::Corrupt { detail },
        Err(Stop::Limit(detail)) => return OfficeOutcome::ResourceLimit { detail },
    }
    if out.is_empty() {
        return OfficeOutcome::Empty;
    }
    let (text, truncated) = out.finish();
    OfficeOutcome::Ok { text, truncated }
}

/// Entry point for the office child. `main` calls this first and exits with
/// the returned code when argv[1] is [`OFFICE_HELPER_ARG`].
pub fn maybe_run_helper() -> Option<i32> {
    let mut args = std::env::args().skip(1);
    if args.next().as_deref() != Some(OFFICE_HELPER_ARG) {
        return None;
    }
    let rest: Vec<String> = args.collect();
    let mut kind = None;
    let mut mode = None;
    let (mut max_input, mut max_output, mut address_space, mut cpu_secs) =
        (0u64, 0usize, 0u64, 0u64);
    for pair in rest.chunks(2) {
        let [name, value] = pair else {
            return Some(child_fail("malformed arguments"));
        };
        let number = || value.parse::<u64>().ok();
        match name.as_str() {
            "--kind" => kind = OfficeKind::parse(value),
            "--mode" => {
                mode = match value.as_str() {
                    "md" => Some(OfficeMode::Markdown),
                    "text" => Some(OfficeMode::Text),
                    _ => None,
                }
            }
            "--max-input" => max_input = number().unwrap_or(0),
            "--max-output" => max_output = number().unwrap_or(0) as usize,
            "--max-as" => address_space = number().unwrap_or(0),
            "--cpu-secs" => cpu_secs = number().unwrap_or(0),
            _ => return Some(child_fail("unknown argument")),
        }
    }
    let (Some(kind), Some(mode)) = (kind, mode) else {
        return Some(child_fail("missing --kind or --mode"));
    };
    if max_input == 0 || max_output == 0 || address_space == 0 || cpu_secs == 0 {
        return Some(child_fail("limits must be positive"));
    }
    // Before any input is read: a document that makes the parser allocate
    // past the ceiling fails the allocation instead of exhausting the host.
    if let Err(err) = document_extract_client::process::apply_rlimits_now(address_space, cpu_secs) {
        return Some(child_fail(&format!("rlimit: {err}")));
    }
    crate::alloc_guard::abort_panics_after_allocation_failure();
    let mut source = Vec::new();
    if let Err(err) = std::io::stdin()
        .take(max_input.saturating_add(1))
        .read_to_end(&mut source)
    {
        return Some(child_fail(&format!("stdin: {err}")));
    }
    let outcome = if source.len() as u64 > max_input {
        OfficeOutcome::ResourceLimit {
            detail: format!("input exceeds {max_input} bytes"),
        }
    } else {
        extract_office(&source, kind, mode, max_output)
    };
    drop(source);
    let json = match serde_json::to_vec(&outcome) {
        Ok(json) => json,
        Err(err) => return Some(child_fail(&format!("serialize: {err}"))),
    };
    let mut stdout = std::io::stdout().lock();
    match stdout.write_all(&json).and_then(|()| stdout.flush()) {
        Ok(()) => Some(0),
        Err(err) => Some(child_fail(&format!("stdout: {err}"))),
    }
}

fn child_fail(msg: &str) -> i32 {
    let _ = writeln!(std::io::stderr(), "{msg}");
    2
}

/// The run was cancelled (shutdown); the child was killed and reaped.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct OfficeCancelled;

fn worker_failure(detail: impl Into<String>) -> OfficeOutcome {
    OfficeOutcome::WorkerFailure {
        detail: detail.into(),
    }
}

/// Runs `helper --internal-office-extract` on `source`. The child is killed
/// on timeout, on cancel and when this future is dropped (`kill_on_drop`).
pub async fn run_office_helper(
    helper: &Path,
    source: Vec<u8>,
    kind: OfficeKind,
    mode: OfficeMode,
    limits: &OfficeLimits,
    cancel: &CancellationToken,
) -> Result<OfficeOutcome, OfficeCancelled> {
    if source.len() as u64 > limits.max_input_bytes {
        return Ok(OfficeOutcome::ResourceLimit {
            detail: format!("input exceeds {} bytes", limits.max_input_bytes),
        });
    }
    let mut command = tokio::process::Command::new(helper);
    #[cfg(target_os = "linux")]
    {
        let parent_pid = std::process::id() as i32;
        // SAFETY: the closure runs between fork and exec and only calls the
        // async-signal-safe prctl/getppid/raise/_exit wrapper.
        unsafe {
            command.pre_exec(move || {
                document_extract_client::process::apply_parent_death_signal(parent_pid)
            });
        }
    }
    let spawned = command
        .arg(OFFICE_HELPER_ARG)
        .arg("--kind")
        .arg(kind.as_str())
        .arg("--mode")
        .arg(mode.as_str())
        .arg("--max-input")
        .arg(limits.max_input_bytes.to_string())
        .arg("--max-output")
        .arg(limits.max_output.to_string())
        .arg("--max-as")
        .arg(limits.address_space.to_string())
        .arg("--cpu-secs")
        .arg(limits.timeout.as_secs().max(1).to_string())
        .env_clear()
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true)
        .spawn();
    let mut child = match spawned {
        Ok(child) => child,
        Err(err) => return Ok(worker_failure(format!("spawn failed: {err}"))),
    };
    let (Some(mut stdin), Some(mut stdout), Some(mut stderr)) =
        (child.stdin.take(), child.stdout.take(), child.stderr.take())
    else {
        return Ok(worker_failure("child pipes unavailable"));
    };
    // JSON may escape every output scalar as `\uXXXX`.
    let output_cap = (limits.max_output as u64).saturating_mul(6) + 4096;
    let io = async move {
        let feed = async move {
            // A child that exits early closes the pipe; that is its answer.
            let _ = stdin.write_all(&source).await;
            drop(stdin);
        };
        let mut out = Vec::new();
        let mut err = Vec::new();
        let mut out_pipe = (&mut stdout).take(output_cap + 1);
        let mut err_pipe = (&mut stderr).take(MAX_STDERR_BYTES);
        let read_out = out_pipe.read_to_end(&mut out);
        let read_err = err_pipe.read_to_end(&mut err);
        let ((), out_res, err_res) = tokio::join!(feed, read_out, read_err);
        if out.len() as u64 > output_cap {
            return Err(OfficeOutcome::ResourceLimit {
                detail: format!("child stdout exceeded {output_cap} bytes"),
            });
        }
        out_res.map_err(|e| worker_failure(format!("stdout: {e}")))?;
        err_res.map_err(|e| worker_failure(format!("stderr: {e}")))?;
        let status = child
            .wait()
            .await
            .map_err(|e| worker_failure(format!("wait: {e}")))?;
        Ok((status, out, err))
    };
    let joined = tokio::select! {
        () = cancel.cancelled() => return Err(OfficeCancelled),
        joined = tokio::time::timeout(limits.timeout, io) => joined,
    };
    // Every early return above dropped the child: kill_on_drop reaps it.
    let (status, out, err) = match joined {
        Ok(Ok(done)) => done,
        Ok(Err(outcome)) => return Ok(outcome),
        Err(_) => {
            return Ok(OfficeOutcome::ResourceLimit {
                detail: format!("office watchdog {}s", limits.timeout.as_secs()),
            })
        }
    };
    let message = String::from_utf8_lossy(&err).trim().to_string();
    if !status.success() {
        return Ok(match status.code() {
            // A parser panic: the bytes broke the parser.
            Some(101) => OfficeOutcome::Corrupt {
                detail: format!("parser crashed: {message}"),
            },
            Some(code) => worker_failure(format!("child exited {code}: {message}")),
            // Killed by a signal: RLIMIT_CPU, allocation abort under RLIMIT_AS.
            None => OfficeOutcome::ResourceLimit {
                detail: format!("child killed ({status}): {message}"),
            },
        });
    }
    Ok(serde_json::from_slice::<OfficeOutcome>(&out)
        .unwrap_or_else(|err| worker_failure(format!("invalid child json: {err}"))))
}

#[cfg(test)]
mod tests;
