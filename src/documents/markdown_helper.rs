//! Markdown conversions in a child process: a hidden mode of this binary,
//! `--internal-markdown`, like the office and image preview children.
//!
//! The Markdown parser (micromark semantics, `documents::markdown`) is
//! super-linear on some inputs (deeply nested emphasis, block quotes and
//! indented lists; the JS parser it replaces shares these), so product
//! conversions never run it in the server process. The child is chosen in
//! `main` before any runtime, logger, config or credential exists; it sets
//! RLIMIT_AS/RLIMIT_CPU before reading its input from stdin, runs one
//! operation on a thread with a stack sized for the parser's recursion, and
//! writes the result to stdout. The parent bounds input, output, wall time and
//! concurrency, kills the child on timeout or drop, clears its environment and
//! sets dies-with-parent on Linux. This is an isolation boundary for CPU,
//! memory and crashes, not a filesystem or network sandbox.

use std::io::{Read, Write};
use std::path::PathBuf;
use std::process::Stdio;
use std::time::Duration;

use serde_json::Value;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::sync::Semaphore;

use crate::collab::derived_body::DOCUMENT_MAX_BODY_BYTES;
use crate::documents::markdown::{md_to_safe_html, md_to_tiptap};
use crate::share_render::{is_tiptap_doc, tiptap_doc_to_md};

/// Hidden argv[1] that turns this binary into the Markdown child.
pub const MARKDOWN_HELPER_ARG: &str = "--internal-markdown";

/// Same process-wide concurrency as the Node convert helper it replaces.
static PERMITS: Semaphore = Semaphore::const_new(2);

/// Parser worker stack: `MAX_MDAST_DEPTH` levels of walker recursion.
const WORKER_STACK_BYTES: usize = 256 * 1024 * 1024;
const MAX_STDERR_BYTES: u64 = 4096;

/// Child exit codes (0 = result on stdout).
const EXIT_FAILURE: i32 = 2;
const EXIT_INVALID_INPUT: i32 = 3;
const EXIT_OUTPUT_TOO_LARGE: i32 = 4;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MarkdownOp {
    /// Source `mdToTiptapJson`: Markdown -> Tiptap JSON.
    MdToTiptap,
    /// Source `tiptapDocToSafeHtml(mdToTiptapJson(md))` (legal documents).
    MdToSafeHtml,
    /// Source `tiptapDocToMd` (with its `selfCheckedMd` reparse).
    TiptapToMd,
}

impl MarkdownOp {
    fn as_str(self) -> &'static str {
        match self {
            Self::MdToTiptap => "md-to-tiptap",
            Self::MdToSafeHtml => "md-to-safe-html",
            Self::TiptapToMd => "tiptap-to-md",
        }
    }

    fn parse(value: &str) -> Option<Self> {
        [Self::MdToTiptap, Self::MdToSafeHtml, Self::TiptapToMd]
            .into_iter()
            .find(|op| op.as_str() == value)
    }
}

#[derive(Debug, thiserror::Error)]
pub enum MarkdownError {
    /// Nested past the storable depth, or the parser exhausted its CPU,
    /// memory or wall-time budget on this input (source: the helper answered
    /// `invalid_input` for inputs the TS parser threw on).
    #[error("invalid input: {0}")]
    InvalidInput(String),
    /// Input or output exceeds the byte caps.
    #[error("too large")]
    TooLarge,
    /// Spawn/IO/protocol failure: not caused by the input.
    #[error("markdown helper failed: {0}")]
    Failed(String),
}

#[derive(Debug, Clone, Copy)]
pub struct MarkdownLimits {
    pub max_input_bytes: usize,
    pub max_output_bytes: usize,
    pub timeout: Duration,
    /// Child RLIMIT_AS.
    pub address_space: u64,
}

impl Default for MarkdownLimits {
    /// Inputs are document bodies (1 MiB Markdown or stored Tiptap JSON) and
    /// legal Markdown (200,000 UTF-16 units). A 1 MiB body parses in about
    /// 1.2 s at most (release; 3-line tables or blank lines, ~1 GB RSS), so 30 s
    /// is a ~25x margin; the output cap is the Node helper's stdout cap.
    fn default() -> Self {
        Self {
            max_input_bytes: 4 * DOCUMENT_MAX_BODY_BYTES,
            max_output_bytes: 32 * 1024 * 1024,
            timeout: Duration::from_secs(30),
            address_space: 2 * 1024 * 1024 * 1024,
        }
    }
}

/// This binary, run as the `--internal-markdown` child.
#[derive(Debug, Clone)]
pub struct MarkdownHelper {
    program: PathBuf,
    limits: MarkdownLimits,
}

impl MarkdownHelper {
    pub fn new(program: impl Into<PathBuf>) -> Self {
        Self {
            program: program.into(),
            limits: MarkdownLimits::default(),
        }
    }

    /// The running server binary.
    pub fn current_exe() -> std::io::Result<Self> {
        std::env::current_exe().map(Self::new)
    }

    pub fn with_limits(mut self, limits: MarkdownLimits) -> Self {
        self.limits = limits;
        self
    }

    pub async fn md_to_tiptap(&self, markdown: &str) -> Result<Value, MarkdownError> {
        let out = self
            .run(MarkdownOp::MdToTiptap, markdown.as_bytes().to_vec())
            .await?;
        serde_json::from_slice(&out)
            .map_err(|e| MarkdownError::Failed(format!("invalid child json: {e}")))
    }

    pub async fn md_to_safe_html(&self, markdown: &str) -> Result<String, MarkdownError> {
        let out = self
            .run(MarkdownOp::MdToSafeHtml, markdown.as_bytes().to_vec())
            .await?;
        String::from_utf8(out).map_err(|e| MarkdownError::Failed(e.to_string()))
    }

    /// Source `documentContentMd`: a stored body that is not a Tiptap doc
    /// reads as "" (no child is started for it).
    pub async fn tiptap_to_md(&self, content_json: &Value) -> Result<String, MarkdownError> {
        if !is_tiptap_doc(content_json) {
            return Ok(String::new());
        }
        let input =
            serde_json::to_vec(content_json).map_err(|e| MarkdownError::Failed(e.to_string()))?;
        let out = self.run(MarkdownOp::TiptapToMd, input).await?;
        String::from_utf8(out).map_err(|e| MarkdownError::Failed(e.to_string()))
    }

    /// Runs one operation in a fresh child. The child is killed on timeout
    /// and when this future is dropped (`kill_on_drop`).
    pub async fn run(&self, op: MarkdownOp, input: Vec<u8>) -> Result<Vec<u8>, MarkdownError> {
        let limits = self.limits;
        if input.len() > limits.max_input_bytes {
            return Err(MarkdownError::TooLarge);
        }
        let _permit = PERMITS
            .acquire()
            .await
            .map_err(|_| MarkdownError::Failed("markdown helper closed".into()))?;
        let mut command = tokio::process::Command::new(&self.program);
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
            .arg(MARKDOWN_HELPER_ARG)
            .arg("--op")
            .arg(op.as_str())
            .arg("--max-input")
            .arg(limits.max_input_bytes.to_string())
            .arg("--max-output")
            .arg(limits.max_output_bytes.to_string())
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
        let mut child = spawned.map_err(|e| MarkdownError::Failed(format!("spawn: {e}")))?;
        let (Some(mut stdin), Some(mut stdout), Some(mut stderr)) =
            (child.stdin.take(), child.stdout.take(), child.stderr.take())
        else {
            return Err(MarkdownError::Failed("child pipes unavailable".into()));
        };
        let output_cap = limits.max_output_bytes as u64;
        let io = async move {
            let feed = async move {
                // A child that exits early closes the pipe; that is its answer.
                let _ = stdin.write_all(&input).await;
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
                return Err(MarkdownError::TooLarge);
            }
            out_res.map_err(|e| MarkdownError::Failed(format!("stdout: {e}")))?;
            err_res.map_err(|e| MarkdownError::Failed(format!("stderr: {e}")))?;
            let status = child
                .wait()
                .await
                .map_err(|e| MarkdownError::Failed(format!("wait: {e}")))?;
            Ok((status, out, err))
        };
        // Every early return drops the child: kill_on_drop reaps it.
        let (status, out, err) =
            tokio::time::timeout(limits.timeout, io)
                .await
                .map_err(|_| {
                    MarkdownError::InvalidInput(format!(
                        "markdown watchdog {}s",
                        limits.timeout.as_secs()
                    ))
                })??;
        let message = String::from_utf8_lossy(&err).trim().to_string();
        match status.code() {
            Some(0) => Ok(out),
            Some(EXIT_INVALID_INPUT) => Err(MarkdownError::InvalidInput(message)),
            Some(EXIT_OUTPUT_TOO_LARGE) => Err(MarkdownError::TooLarge),
            Some(code) => Err(MarkdownError::Failed(format!(
                "child exited {code}: {message}"
            ))),
            // Killed by a signal: RLIMIT_CPU, or an abort (allocation failure
            // under RLIMIT_AS, stack overflow) on this input.
            None => Err(MarkdownError::InvalidInput(format!(
                "child killed ({status}): {message}"
            ))),
        }
    }
}

// ---------------------------------------------------------------------------
// Child side

enum ChildResult {
    Output(Vec<u8>),
    InvalidInput(String),
}

fn convert(op: MarkdownOp, input: Vec<u8>) -> ChildResult {
    let text = |input: Vec<u8>| String::from_utf8(input).map_err(|_| "input is not UTF-8");
    let result = match op {
        MarkdownOp::MdToTiptap => text(input).and_then(|md| {
            md_to_tiptap(&md)
                .map_err(|_| "markdown nests too deeply")
                .and_then(|doc| serde_json::to_vec(&doc).map_err(|_| "serialize failed"))
        }),
        MarkdownOp::MdToSafeHtml => text(input).and_then(|md| {
            md_to_safe_html(&md)
                .map(String::into_bytes)
                .map_err(|_| "markdown nests too deeply")
        }),
        MarkdownOp::TiptapToMd => serde_json::from_slice::<Value>(&input)
            .map_err(|_| "input is not JSON")
            .map(|doc| {
                if is_tiptap_doc(&doc) {
                    tiptap_doc_to_md(&doc).into_bytes()
                } else {
                    Vec::new()
                }
            }),
    };
    match result {
        Ok(out) => ChildResult::Output(out),
        Err(detail) => ChildResult::InvalidInput(detail.to_string()),
    }
}

/// Entry point for the Markdown child. `main` calls this first and exits with
/// the returned code when argv[1] is [`MARKDOWN_HELPER_ARG`].
pub fn maybe_run_helper() -> Option<i32> {
    let mut args = std::env::args().skip(1);
    if args.next().as_deref() != Some(MARKDOWN_HELPER_ARG) {
        return None;
    }
    let rest: Vec<String> = args.collect();
    let mut op = None;
    let (mut max_input, mut max_output, mut address_space, mut cpu_secs) = (0u64, 0u64, 0u64, 0u64);
    for pair in rest.chunks(2) {
        let [name, value] = pair else {
            return Some(child_fail("malformed arguments"));
        };
        let number = || value.parse::<u64>().ok().unwrap_or(0);
        match name.as_str() {
            "--op" => op = MarkdownOp::parse(value),
            "--max-input" => max_input = number(),
            "--max-output" => max_output = number(),
            "--max-as" => address_space = number(),
            "--cpu-secs" => cpu_secs = number(),
            _ => return Some(child_fail("unknown argument")),
        }
    }
    let Some(op) = op else {
        return Some(child_fail("missing --op"));
    };
    if max_input == 0 || max_output == 0 || address_space == 0 || cpu_secs == 0 {
        return Some(child_fail("limits must be positive"));
    }
    if let Err(err) = document_extract_client::process::apply_rlimits_now(address_space, cpu_secs) {
        return Some(child_fail(&format!("rlimit: {err}")));
    }
    let mut input = Vec::new();
    if let Err(err) = std::io::stdin()
        .take(max_input.saturating_add(1))
        .read_to_end(&mut input)
    {
        return Some(child_fail(&format!("stdin: {err}")));
    }
    if input.len() as u64 > max_input {
        return Some(child_fail("input exceeds --max-input"));
    }
    let worker = std::thread::Builder::new()
        .name("markdown".into())
        .stack_size(WORKER_STACK_BYTES)
        .spawn(move || convert(op, input));
    let result = match worker.map(|w| w.join()) {
        Ok(Ok(result)) => result,
        // The input broke the parser: refuse it like any other bad input.
        Ok(Err(_)) => {
            let _ = writeln!(std::io::stderr(), "converter panicked");
            return Some(EXIT_INVALID_INPUT);
        }
        Err(err) => return Some(child_fail(&format!("worker thread: {err}"))),
    };
    let out = match result {
        ChildResult::Output(out) => out,
        ChildResult::InvalidInput(detail) => {
            let _ = writeln!(std::io::stderr(), "{detail}");
            return Some(EXIT_INVALID_INPUT);
        }
    };
    if out.len() as u64 > max_output {
        return Some(EXIT_OUTPUT_TOO_LARGE);
    }
    let mut stdout = std::io::stdout().lock();
    match stdout.write_all(&out).and_then(|()| stdout.flush()) {
        Ok(()) => Some(0),
        Err(err) => Some(child_fail(&format!("stdout: {err}"))),
    }
}

fn child_fail(msg: &str) -> i32 {
    let _ = writeln!(std::io::stderr(), "{msg}");
    EXIT_FAILURE
}
