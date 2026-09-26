//! Markdown conversions and the DOCX export in a child process: a hidden mode
//! of this binary, `--internal-markdown`, like the office and image preview
//! children.
//!
//! The Markdown parser (micromark semantics, `documents::markdown`) is
//! super-linear on some inputs (deeply nested emphasis, block quotes and
//! indented lists; the JS parser it replaces shares these), so product
//! conversions never run it in the server process. The child is chosen in
//! `main` before any runtime, logger, config or credential exists; it sets
//! RLIMIT_AS/RLIMIT_CPU before reading its input from stdin, runs one
//! operation on a thread with a stack sized for the parser's recursion, and
//! writes the result to stdout. The document exports (Markdown, DOCX, PDF and
//! PPTX: `documents::{docx, pdf, pptx}`) run here for the same reasons:
//! CPU/memory proportional to a stored body, panics and their 20 MB output
//! cap stay out of the server. The parent bounds input, output, wall time and
//! concurrency, kills the child on timeout or drop, clears its environment and
//! sets dies-with-parent on Linux. This is an isolation boundary for CPU,
//! memory and crashes, not a filesystem or network sandbox.

use std::io::{Read, Write};
use std::path::PathBuf;
use std::process::Stdio;
use std::sync::Arc;
use std::time::Duration;

use serde_json::Value;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::sync::Semaphore;

use crate::collab::derived_body::DOCUMENT_MAX_BODY_BYTES;
use crate::documents::docx::{write_docx, DocxError, DOCX_MAX_OUTPUT_BYTES};
use crate::documents::export::ExportRenderError;
use crate::documents::export_model::{export_doc, visible_title};
use crate::documents::markdown::{md_to_safe_html, md_to_tiptap};
use crate::documents::pdf::{write_pdf, PdfError, PDF_MAX_OUTPUT_BYTES};
use crate::documents::pptx::{write_pptx, PptxError, PPTX_MAX_OUTPUT_BYTES};
use crate::share_render::{is_tiptap_doc, tiptap_doc_to_md};

/// Hidden argv[1] that turns this binary into the Markdown child.
pub const MARKDOWN_HELPER_ARG: &str = "--internal-markdown";

/// Same process-wide concurrency as the Node convert helper it replaces.
static PERMITS: Semaphore = Semaphore::const_new(2);

/// Parser worker stack: `MAX_MDAST_DEPTH` levels of walker recursion.
const WORKER_STACK_BYTES: usize = 256 * 1024 * 1024;
const MAX_STDERR_BYTES: u64 = 4096;

/// Source `convert.mjs` `MAX_OUTPUT_BYTES`: every export, Markdown included.
pub const MD_EXPORT_MAX_OUTPUT_BYTES: usize = 20_000_000;

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
    /// Source `export_docx`: `{"title", "contentJson"}` -> DOCX bytes.
    TiptapToDocx,
    /// Source `export_pdf`: `{"title", "contentJson"}` -> PDF bytes.
    TiptapToPdf,
    /// Source `export_pptx`: `{"title", "contentJson"}` -> PPTX bytes.
    TiptapToPptx,
    /// Source `export_md` (`documentMarkdown`): `{"title", "contentJson"}` ->
    /// the escaped `# title` line and `tiptapDocToMd` of the body.
    TiptapToMdExport,
}

impl MarkdownOp {
    fn as_str(self) -> &'static str {
        match self {
            Self::MdToTiptap => "md-to-tiptap",
            Self::MdToSafeHtml => "md-to-safe-html",
            Self::TiptapToMd => "tiptap-to-md",
            Self::TiptapToDocx => "tiptap-to-docx",
            Self::TiptapToPdf => "tiptap-to-pdf",
            Self::TiptapToPptx => "tiptap-to-pptx",
            Self::TiptapToMdExport => "tiptap-to-md-export",
        }
    }

    fn parse(value: &str) -> Option<Self> {
        [
            Self::MdToTiptap,
            Self::MdToSafeHtml,
            Self::TiptapToMd,
            Self::TiptapToDocx,
            Self::TiptapToPdf,
            Self::TiptapToPptx,
            Self::TiptapToMdExport,
        ]
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

/// Why one child run did not produce output.
enum RunError {
    InvalidInput(String),
    TooLarge,
    Failed(String),
    /// Watchdog, RLIMIT_CPU or an abort under RLIMIT_AS / stack overflow.
    Killed(String),
    Busy,
}

impl From<RunError> for MarkdownError {
    /// Markdown ops: a child killed on an input refuses that input.
    fn from(err: RunError) -> Self {
        match err {
            RunError::InvalidInput(m) | RunError::Killed(m) => Self::InvalidInput(m),
            RunError::TooLarge => Self::TooLarge,
            RunError::Failed(m) => Self::Failed(m),
            // Only the public PDF pool refuses; it maps its own errors.
            RunError::Busy => Self::Failed("helper busy".into()),
        }
    }
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
    /// Public share PDFs: their own pool of one that never waits (source:
    /// the Node helper's public pool), so anonymous requests cannot queue
    /// ahead of members. Shared by clones (the app state's one helper).
    public_permits: Arc<Semaphore>,
}

impl MarkdownHelper {
    pub fn new(program: impl Into<PathBuf>) -> Self {
        Self {
            program: program.into(),
            limits: MarkdownLimits::default(),
            public_permits: Arc::new(Semaphore::new(1)),
        }
    }

    /// The running server binary. Call only from `fvoci-server`: the
    /// `fvoci-migrate` doctor resolves its sibling server executable instead.
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

    /// Source `export_docx` (`markdownToDocx(documentMarkdown(title, doc))`):
    /// DOCX bytes of at most [`DOCX_MAX_OUTPUT_BYTES`]. A body that is not a
    /// Tiptap doc is `InvalidInput` (source `invalid_input`) without a child.
    /// A child killed by the watchdog or a resource limit is `Failed` (the
    /// Node export answered 500 on its timeout; the body is stored data, not
    /// a request input).
    pub async fn tiptap_to_docx(
        &self,
        title: &str,
        content_json: &Value,
    ) -> Result<Vec<u8>, MarkdownError> {
        if !is_tiptap_doc(content_json) {
            return Err(MarkdownError::InvalidInput("not a tiptap doc".into()));
        }
        let input = serde_json::to_vec(&serde_json::json!({
            "title": title,
            "contentJson": content_json,
        }))
        .map_err(|e| MarkdownError::Failed(e.to_string()))?;
        match self
            .run_child(MarkdownOp::TiptapToDocx, input, Pool::Shared)
            .await
        {
            Ok(out) => Ok(out),
            Err(RunError::Killed(m)) => Err(MarkdownError::Failed(m)),
            Err(err) => Err(err.into()),
        }
    }

    /// Source `export_pdf` (`tiptapDocToPdf(titledDocument(title, doc))`):
    /// PDF bytes of at most [`PDF_MAX_OUTPUT_BYTES`]. Waits for a shared
    /// permit (members). A body that is not a Tiptap doc is `InvalidInput`
    /// without a child; a child killed by the watchdog or a resource limit is
    /// `Failed` (logged), as for DOCX.
    pub async fn tiptap_to_pdf(
        &self,
        title: &str,
        content_json: &Value,
    ) -> Result<Vec<u8>, ExportRenderError> {
        self.pdf(title, content_json, Pool::Shared).await
    }

    /// The same PDF for an anonymous share request: its own pool of one that
    /// answers `Busy` instead of waiting (source: the Node helper's public pool).
    pub async fn tiptap_to_pdf_public(
        &self,
        title: &str,
        content_json: &Value,
    ) -> Result<Vec<u8>, ExportRenderError> {
        self.pdf(title, content_json, Pool::PublicFailFast).await
    }

    async fn pdf(
        &self,
        title: &str,
        content_json: &Value,
        pool: Pool,
    ) -> Result<Vec<u8>, ExportRenderError> {
        self.export(MarkdownOp::TiptapToPdf, title, content_json, pool)
            .await
    }

    /// Source `export_pptx` (`tiptapDocToPptx(titledDocument(title, doc))`):
    /// PPTX bytes of at most [`PPTX_MAX_OUTPUT_BYTES`], errors as for PDF.
    pub async fn tiptap_to_pptx(
        &self,
        title: &str,
        content_json: &Value,
    ) -> Result<Vec<u8>, ExportRenderError> {
        self.export(MarkdownOp::TiptapToPptx, title, content_json, Pool::Shared)
            .await
    }

    /// Source `export_md` (`documentMarkdown(title, doc)`): the UTF-8
    /// Markdown file, errors as for PDF (the Node helper's timeout was a 500).
    pub async fn tiptap_to_md_export(
        &self,
        title: &str,
        content_json: &Value,
    ) -> Result<Vec<u8>, ExportRenderError> {
        self.export(
            MarkdownOp::TiptapToMdExport,
            title,
            content_json,
            Pool::Shared,
        )
        .await
    }

    /// One export op: a body that is not a Tiptap doc is `InvalidInput`
    /// without a child; a writer failure, panic or a child killed by the
    /// watchdog or a resource limit is `Failed` (logged).
    async fn export(
        &self,
        op: MarkdownOp,
        title: &str,
        content_json: &Value,
        pool: Pool,
    ) -> Result<Vec<u8>, ExportRenderError> {
        if !is_tiptap_doc(content_json) {
            return Err(ExportRenderError::InvalidInput);
        }
        let input = serde_json::to_vec(&serde_json::json!({
            "title": title,
            "contentJson": content_json,
        }))
        .map_err(|_| ExportRenderError::InvalidInput)?;
        match self.run_child(op, input, pool).await {
            Ok(out) => Ok(out),
            Err(RunError::InvalidInput(_)) => Err(ExportRenderError::InvalidInput),
            Err(RunError::TooLarge) => Err(ExportRenderError::TooLarge),
            Err(RunError::Busy) => Err(ExportRenderError::Busy),
            Err(RunError::Failed(detail) | RunError::Killed(detail)) => {
                tracing::error!(%detail, op = op.as_str(), "export child failed");
                Err(ExportRenderError::Failed)
            }
        }
    }

    /// Runs one operation in a fresh child. The child is killed on timeout
    /// and when this future is dropped (`kill_on_drop`).
    pub async fn run(&self, op: MarkdownOp, input: Vec<u8>) -> Result<Vec<u8>, MarkdownError> {
        Ok(self.run_child(op, input, Pool::Shared).await?)
    }

    async fn run_child(
        &self,
        op: MarkdownOp,
        input: Vec<u8>,
        pool: Pool,
    ) -> Result<Vec<u8>, RunError> {
        let mut limits = self.limits;
        match op {
            MarkdownOp::TiptapToDocx => {
                limits.max_output_bytes = limits.max_output_bytes.min(DOCX_MAX_OUTPUT_BYTES);
            }
            MarkdownOp::TiptapToPdf => {
                limits.max_output_bytes = limits.max_output_bytes.min(PDF_MAX_OUTPUT_BYTES);
            }
            MarkdownOp::TiptapToPptx => {
                limits.max_output_bytes = limits.max_output_bytes.min(PPTX_MAX_OUTPUT_BYTES);
            }
            MarkdownOp::TiptapToMdExport => {
                limits.max_output_bytes = limits.max_output_bytes.min(MD_EXPORT_MAX_OUTPUT_BYTES);
            }
            _ => {}
        }
        if input.len() > limits.max_input_bytes {
            return Err(RunError::TooLarge);
        }
        let (_shared, _public) = match pool {
            Pool::Shared => (
                Some(
                    PERMITS
                        .acquire()
                        .await
                        .map_err(|_| RunError::Failed("markdown helper closed".into()))?,
                ),
                None,
            ),
            Pool::PublicFailFast => (
                None,
                Some(
                    self.public_permits
                        .clone()
                        .try_acquire_owned()
                        .map_err(|_| RunError::Busy)?,
                ),
            ),
        };
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
            .envs(
                std::env::var_os(crate::documents::pdf::FONT_DIR_ENV)
                    .map(|v| (crate::documents::pdf::FONT_DIR_ENV, v)),
            )
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true)
            .spawn();
        let mut child = spawned.map_err(|e| RunError::Failed(format!("spawn: {e}")))?;
        let (Some(mut stdin), Some(mut stdout), Some(mut stderr)) =
            (child.stdin.take(), child.stdout.take(), child.stderr.take())
        else {
            return Err(RunError::Failed("child pipes unavailable".into()));
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
                return Err(RunError::TooLarge);
            }
            out_res.map_err(|e| RunError::Failed(format!("stdout: {e}")))?;
            err_res.map_err(|e| RunError::Failed(format!("stderr: {e}")))?;
            let status = child
                .wait()
                .await
                .map_err(|e| RunError::Failed(format!("wait: {e}")))?;
            Ok((status, out, err))
        };
        // Every early return drops the child: kill_on_drop reaps it.
        let (status, out, err) =
            tokio::time::timeout(limits.timeout, io)
                .await
                .map_err(|_| {
                    RunError::Killed(format!("markdown watchdog {}s", limits.timeout.as_secs()))
                })??;
        let message = String::from_utf8_lossy(&err).trim().to_string();
        match status.code() {
            Some(0) => Ok(out),
            Some(EXIT_INVALID_INPUT) => Err(RunError::InvalidInput(message)),
            Some(EXIT_OUTPUT_TOO_LARGE) => Err(RunError::TooLarge),
            Some(code) => Err(RunError::Failed(format!("child exited {code}: {message}"))),
            // Killed by a signal: RLIMIT_CPU, or an abort (allocation failure
            // under RLIMIT_AS, stack overflow) on this input.
            None => Err(RunError::Killed(format!(
                "child killed ({status}): {message}"
            ))),
        }
    }
}

#[derive(Debug, Clone, Copy)]
enum Pool {
    Shared,
    PublicFailFast,
}

// ---------------------------------------------------------------------------
// Child side

#[derive(Debug)]
enum ChildResult {
    Output(Vec<u8>),
    InvalidInput(String),
    OutputTooLarge,
    /// The converter itself failed on valid input (exit 2, routes: 500).
    Failed(String),
}

#[derive(serde::Deserialize)]
#[serde(rename_all = "camelCase")]
struct DocxRequest {
    title: String,
    content_json: Value,
}

fn tiptap_to_docx(input: &[u8]) -> ChildResult {
    let Ok(req) = serde_json::from_slice::<DocxRequest>(input) else {
        return ChildResult::InvalidInput("input is not a docx request".into());
    };
    if !is_tiptap_doc(&req.content_json) {
        return ChildResult::InvalidInput("not a tiptap doc".into());
    }
    docx_result(write_docx(&export_doc(&req.title, &req.content_json)))
}

/// The body is a valid Tiptap doc by now: a pack (zip/IO) error is the
/// writer's fault, not the input's (the Node export answered 500).
fn docx_result(written: Result<Vec<u8>, DocxError>) -> ChildResult {
    match written {
        Ok(bytes) => ChildResult::Output(bytes),
        Err(DocxError::TooLarge) => ChildResult::OutputTooLarge,
        Err(err @ DocxError::Pack(_)) => ChildResult::Failed(err.to_string()),
    }
}

/// Exit code of a converter panic. Markdown ops refuse the input that broke
/// the parser (400); the DOCX op's input is a stored, validated body, so a
/// writer panic is a server fault (500).
fn panic_exit_code(op: MarkdownOp) -> i32 {
    match op {
        MarkdownOp::TiptapToDocx
        | MarkdownOp::TiptapToPdf
        | MarkdownOp::TiptapToPptx
        | MarkdownOp::TiptapToMdExport => EXIT_FAILURE,
        MarkdownOp::MdToTiptap | MarkdownOp::MdToSafeHtml | MarkdownOp::TiptapToMd => {
            EXIT_INVALID_INPUT
        }
    }
}

fn tiptap_to_pdf(input: &[u8]) -> ChildResult {
    let Ok(req) = serde_json::from_slice::<DocxRequest>(input) else {
        return ChildResult::InvalidInput("input is not a pdf request".into());
    };
    if !is_tiptap_doc(&req.content_json) {
        return ChildResult::InvalidInput("not a tiptap doc".into());
    }
    match write_pdf(&export_doc(&req.title, &req.content_json)) {
        Ok(bytes) => ChildResult::Output(bytes),
        Err(PdfError::TooLarge) => ChildResult::OutputTooLarge,
        // As for DOCX: the body is valid by now, a writer error is a fault.
        Err(err @ PdfError::Write(_)) => ChildResult::Failed(err.to_string()),
    }
}

fn tiptap_to_pptx(input: &[u8]) -> ChildResult {
    let Ok(req) = serde_json::from_slice::<DocxRequest>(input) else {
        return ChildResult::InvalidInput("input is not a pptx request".into());
    };
    if !is_tiptap_doc(&req.content_json) {
        return ChildResult::InvalidInput("not a tiptap doc".into());
    }
    match write_pptx(&export_doc(&req.title, &req.content_json)) {
        Ok(bytes) => ChildResult::Output(bytes),
        Err(PptxError::TooLarge) => ChildResult::OutputTooLarge,
        Err(err @ PptxError::Pack(_)) => ChildResult::Failed(err.to_string()),
    }
}

fn tiptap_to_md_export(input: &[u8]) -> ChildResult {
    let Ok(req) = serde_json::from_slice::<DocxRequest>(input) else {
        return ChildResult::InvalidInput("input is not a markdown export request".into());
    };
    if !is_tiptap_doc(&req.content_json) {
        return ChildResult::InvalidInput("not a tiptap doc".into());
    }
    let out = document_markdown(&req.title, &req.content_json).into_bytes();
    if out.len() > MD_EXPORT_MAX_OUTPUT_BYTES {
        return ChildResult::OutputTooLarge;
    }
    ChildResult::Output(out)
}

/// Source `documentMarkdown`: the visible title with every ASCII punctuation
/// character backslash-escaped as `# title`, a blank line, then
/// `tiptapDocToMd(doc)`; the body alone when the title is empty.
pub fn document_markdown(title: &str, doc: &Value) -> String {
    let body = tiptap_doc_to_md(doc);
    let title = visible_title(title);
    if title.is_empty() {
        return body;
    }
    let mut out = String::with_capacity(title.len() * 2 + body.len() + 4);
    out.push_str("# ");
    for c in title.chars() {
        if c.is_ascii_punctuation() {
            out.push('\\');
        }
        out.push(c);
    }
    out.push_str("\n\n");
    out.push_str(&body);
    out
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
        MarkdownOp::TiptapToDocx => return tiptap_to_docx(&input),
        MarkdownOp::TiptapToPdf => return tiptap_to_pdf(&input),
        MarkdownOp::TiptapToPptx => return tiptap_to_pptx(&input),
        MarkdownOp::TiptapToMdExport => return tiptap_to_md_export(&input),
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
        Ok(Err(_)) => {
            let _ = writeln!(std::io::stderr(), "converter panicked");
            return Some(panic_exit_code(op));
        }
        Err(err) => return Some(child_fail(&format!("worker thread: {err}"))),
    };
    let out = match result {
        ChildResult::Output(out) => out,
        ChildResult::InvalidInput(detail) => {
            let _ = writeln!(std::io::stderr(), "{detail}");
            return Some(EXIT_INVALID_INPUT);
        }
        ChildResult::OutputTooLarge => return Some(EXIT_OUTPUT_TOO_LARGE),
        ChildResult::Failed(detail) => return Some(child_fail(&detail)),
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn docx_writer_faults_are_failures_not_invalid_input() {
        assert!(matches!(
            docx_result(Err(DocxError::Pack("zip: io".into()))),
            ChildResult::Failed(m) if m.contains("zip: io")
        ));
        assert!(matches!(
            docx_result(Err(DocxError::TooLarge)),
            ChildResult::OutputTooLarge
        ));
        assert_eq!(panic_exit_code(MarkdownOp::TiptapToDocx), EXIT_FAILURE);
        for op in [
            MarkdownOp::MdToTiptap,
            MarkdownOp::MdToSafeHtml,
            MarkdownOp::TiptapToMd,
        ] {
            assert_eq!(panic_exit_code(op), EXIT_INVALID_INPUT, "{op:?}");
        }
    }

    #[test]
    fn docx_child_input_errors_stay_invalid_input() {
        assert!(matches!(
            tiptap_to_docx(b"not json"),
            ChildResult::InvalidInput(_)
        ));
        assert!(matches!(
            tiptap_to_docx(br#"{"title":"t","contentJson":{"type":"paragraph"}}"#),
            ChildResult::InvalidInput(_)
        ));
    }
}
