//! Client for the Node document convert helper (`scripts/document-convert`).
//!
//! Each call is one child process: the JSON request goes to stdin, the JSON
//! response is read from stdout up to a fixed cap, and the child is killed when
//! the deadline passes. The child starts from an empty environment plus a short
//! allowlist, so database URLs and secrets never reach it. A semaphore bounds
//! concurrent helpers per server process.

use std::process::Stdio;
use std::sync::Arc;
use std::time::Duration;

use base64::{engine::general_purpose::STANDARD as B64, Engine as _};
use serde::Deserialize;
use serde_json::{json, Value};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::process::Command;
use tokio::sync::Semaphore;

const DEFAULT_TIMEOUT: Duration = Duration::from_secs(300);
const DEFAULT_CONCURRENCY: usize = 2;
/// Helpers anonymous callers (public share PDFs) may run at once. They never
/// wait for a permit, so they cannot queue ahead of members or import jobs.
const PUBLIC_CONCURRENCY: usize = 1;
const MAX_STDOUT_BYTES: u64 = 32 * 1024 * 1024;
const MAX_STDERR_BYTES: u64 = 16 * 1024;
/// Variables the helper may inherit. Everything else (DATABASE_URL, peppers,
/// SMTP credentials, ...) is withheld by `env_clear`.
const ENV_ALLOWLIST: &[&str] = &[
    "PATH",
    "HOME",
    "TMPDIR",
    "LANG",
    "LC_ALL",
    "TZ",
    "NODE_OPTIONS",
];

#[derive(Debug, Clone)]
pub struct ConvertClient {
    command: String,
    args: Vec<String>,
    timeout: Duration,
    permits: Arc<Semaphore>,
    /// Separate pool for anonymous work; see [`ConvertClient::for_public`].
    public_permits: Arc<Semaphore>,
    fail_fast: bool,
}

#[derive(Debug, thiserror::Error)]
pub enum ConvertError {
    #[error("document convert helper failed: {0}")]
    Failed(String),
    #[error("invalid input")]
    InvalidInput,
    #[error("document too large")]
    TooLarge,
    #[error("timed out")]
    TimedOut,
    #[error("document convert helper busy")]
    Busy,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ConvertResponse {
    ok: bool,
    code: Option<String>,
    detail: Option<String>,
    content_json: Option<Value>,
    update_b64: Option<String>,
    content_type: Option<String>,
    ext: Option<String>,
    data_b64: Option<String>,
}

impl ConvertClient {
    pub fn from_env() -> Option<Self> {
        let raw = std::env::var("FVOCI_DOCUMENT_CONVERT_BIN")
            .ok()
            .filter(|v| !v.trim().is_empty())?;
        let parts: Vec<&str> = raw.split_whitespace().collect();
        let concurrency = std::env::var("FVOCI_DOCUMENT_CONVERT_CONCURRENCY")
            .ok()
            .and_then(|v| v.trim().parse::<usize>().ok())
            .filter(|v| *v > 0)
            .unwrap_or(DEFAULT_CONCURRENCY);
        Some(
            Self::from_parts(parts[0], parts[1..].iter().map(|s| s.to_string()).collect())
                .with_limits(DEFAULT_TIMEOUT, concurrency),
        )
    }

    pub fn from_parts(command: impl Into<String>, args: Vec<String>) -> Self {
        Self {
            command: command.into(),
            args,
            timeout: DEFAULT_TIMEOUT,
            permits: Arc::new(Semaphore::new(DEFAULT_CONCURRENCY)),
            public_permits: Arc::new(Semaphore::new(PUBLIC_CONCURRENCY)),
            fail_fast: false,
        }
    }

    /// The same helper on its own small pool that refuses with
    /// [`ConvertError::Busy`] instead of waiting, for unauthenticated callers.
    pub fn for_public(&self) -> Self {
        Self {
            permits: self.public_permits.clone(),
            fail_fast: true,
            ..self.clone()
        }
    }

    pub fn with_limits(mut self, timeout: Duration, concurrency: usize) -> Self {
        self.timeout = timeout;
        self.permits = Arc::new(Semaphore::new(concurrency.max(1)));
        self
    }

    async fn call(&self, body: Value) -> Result<ConvertResponse, ConvertError> {
        let payload = serde_json::to_vec(&body).map_err(|e| ConvertError::Failed(e.to_string()))?;
        let _permit = if self.fail_fast {
            self.permits
                .clone()
                .try_acquire_owned()
                .map_err(|_| ConvertError::Busy)?
        } else {
            self.permits
                .clone()
                .acquire_owned()
                .await
                .map_err(|_| ConvertError::Failed("convert helper closed".into()))?
        };

        let mut command = Command::new(&self.command);
        command
            .args(&self.args)
            .env_clear()
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true);
        for name in ENV_ALLOWLIST {
            if let Some(value) = std::env::var_os(name) {
                command.env(name, value);
            }
        }
        let mut child = command
            .spawn()
            .map_err(|e| ConvertError::Failed(format!("spawn failed: {e}")))?;
        let mut stdin = child.stdin.take().expect("piped stdin");
        let stdout = child.stdout.take().expect("piped stdout");
        let stderr = child.stderr.take().expect("piped stderr");

        let run = async {
            // Write and read concurrently: a large request must not deadlock
            // against a helper that already fills its stdout pipe.
            let write = async move {
                let result = stdin.write_all(&payload).await;
                drop(stdin);
                result
            };
            let read_out = async move {
                let mut out = Vec::new();
                stdout
                    .take(MAX_STDOUT_BYTES + 1)
                    .read_to_end(&mut out)
                    .await
                    .map(|_| out)
            };
            let read_err = async move {
                let mut err = Vec::new();
                let mut limited = stderr.take(MAX_STDERR_BYTES);
                let _ = limited.read_to_end(&mut err).await;
                // Drain the rest so the helper never blocks on a full pipe.
                let _ = tokio::io::copy(&mut limited.into_inner(), &mut tokio::io::sink()).await;
                err
            };
            let (written, out, err) = tokio::join!(write, read_out, read_err);
            (written, out, err)
        };

        let (written, out, err) = match tokio::time::timeout(self.timeout, run).await {
            Ok(result) => result,
            Err(_) => {
                let _ = child.start_kill();
                let _ = child.wait().await;
                return Err(ConvertError::TimedOut);
            }
        };
        let stdout = match out {
            Ok(bytes) if bytes.len() as u64 > MAX_STDOUT_BYTES => {
                let _ = child.start_kill();
                let _ = child.wait().await;
                return Err(ConvertError::TooLarge);
            }
            Ok(bytes) => bytes,
            Err(e) => {
                let _ = child.start_kill();
                let _ = child.wait().await;
                return Err(ConvertError::Failed(format!("stdout read failed: {e}")));
            }
        };
        let status = match tokio::time::timeout(self.timeout, child.wait()).await {
            Ok(Ok(status)) => status,
            Ok(Err(e)) => return Err(ConvertError::Failed(format!("wait failed: {e}"))),
            Err(_) => {
                let _ = child.start_kill();
                let _ = child.wait().await;
                return Err(ConvertError::TimedOut);
            }
        };
        if let Err(e) = written {
            return Err(ConvertError::Failed(format!("stdin write failed: {e}")));
        }
        if !status.success() {
            tracing::warn!(
                %status,
                stderr = %String::from_utf8_lossy(&err).chars().take(500).collect::<String>(),
                "document convert helper exited unsuccessfully"
            );
            return Err(ConvertError::Failed(format!(
                "convert helper exited with {status}"
            )));
        }
        serde_json::from_slice(&stdout).map_err(|e| ConvertError::Failed(e.to_string()))
    }

    fn map_code(resp: ConvertResponse) -> Result<ConvertResponse, ConvertError> {
        if resp.ok {
            return Ok(resp);
        }
        match resp.code.as_deref() {
            Some("document_body_exceeds_document_max_body_bytes") => Err(ConvertError::TooLarge),
            Some("invalid_input") => Err(ConvertError::InvalidInput),
            _ => Err(ConvertError::Failed(
                resp.detail.unwrap_or_else(|| "convert failed".into()),
            )),
        }
    }

    pub async fn md_to_tiptap(&self, markdown: &str) -> Result<Value, ConvertError> {
        let resp = Self::map_code(
            self.call(json!({
                "op": "md_to_tiptap",
                "markdown": markdown,
            }))
            .await?,
        )?;
        resp.content_json.ok_or(ConvertError::Failed(
            "convert helper omitted contentJson".into(),
        ))
    }

    pub async fn tiptap_to_yjs_update(
        &self,
        content_json: &Value,
    ) -> Result<Vec<u8>, ConvertError> {
        let resp = Self::map_code(
            self.call(json!({
                "op": "tiptap_to_yjs_update",
                "contentJson": content_json,
            }))
            .await?,
        )?;
        let b64 = resp
            .update_b64
            .ok_or_else(|| ConvertError::Failed("convert helper omitted updateB64".into()))?;
        B64.decode(b64)
            .map_err(|e| ConvertError::Failed(e.to_string()))
    }

    pub async fn export_binary(
        &self,
        op: &str,
        title: &str,
        content_json: &Value,
    ) -> Result<(Vec<u8>, String, String), ConvertError> {
        let resp = Self::map_code(
            self.call(json!({
                "op": op,
                "title": title,
                "contentJson": content_json,
            }))
            .await?,
        )?;
        let data_b64 = resp
            .data_b64
            .ok_or_else(|| ConvertError::Failed("convert helper omitted dataB64".into()))?;
        let bytes = B64
            .decode(data_b64)
            .map_err(|e| ConvertError::Failed(e.to_string()))?;
        let content_type = resp
            .content_type
            .ok_or_else(|| ConvertError::Failed("convert helper omitted contentType".into()))?;
        let ext = resp
            .ext
            .ok_or_else(|| ConvertError::Failed("convert helper omitted ext".into()))?;
        Ok((bytes, content_type, ext))
    }
}

#[cfg(all(test, unix))]
mod tests {

    #[tokio::test]
    async fn public_calls_refuse_instead_of_queueing_and_keep_member_permits() {
        let client = ConvertClient::from_parts("/bin/false", vec![]);
        let public = client.for_public();
        let _held = public.permits.clone().try_acquire_owned().unwrap();
        let err = public.call(serde_json::json!({})).await.unwrap_err();
        assert!(matches!(err, ConvertError::Busy), "{err:?}");
        // A second public view shares the same pool.
        let err = client
            .for_public()
            .call(serde_json::json!({}))
            .await
            .unwrap_err();
        assert!(matches!(err, ConvertError::Busy), "{err:?}");
        assert_eq!(client.permits.available_permits(), DEFAULT_CONCURRENCY);
    }
    use super::*;

    fn script(dir: &std::path::Path, name: &str, body: &str) -> String {
        use std::os::unix::fs::PermissionsExt;
        let path = dir.join(name);
        std::fs::write(&path, format!("#!/bin/sh\n{body}\n")).unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
        path.display().to_string()
    }

    fn temp_dir(tag: &str) -> std::path::PathBuf {
        let dir =
            std::env::temp_dir().join(format!("fvoci-convert-{tag}-{}", uuid::Uuid::now_v7()));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[tokio::test]
    async fn large_payload_goes_through_stdin_and_env_is_cleared() {
        let dir = temp_dir("stdin");
        // Echo the byte count of stdin and whether a secret leaked into the env.
        let bin = script(
            &dir,
            "echo.sh",
            r#"n=$(wc -c | tr -d ' '); leak=${CARGO_MANIFEST_DIR:-none}; printf '{"ok":true,"contentJson":{"n":%s,"leak":"%s"}}' "$n" "$leak""#,
        );
        // cargo sets this for the test process; the helper must not see it.
        assert!(std::env::var_os("CARGO_MANIFEST_DIR").is_some());
        let client = ConvertClient::from_parts(bin, vec![]);
        let markdown = "가".repeat(200 * 1024);
        let value = client.md_to_tiptap(&markdown).await.unwrap();
        assert!(value["n"].as_u64().unwrap() > 600 * 1024);
        assert_eq!(value["leak"], "none");
        let _ = std::fs::remove_dir_all(dir);
    }

    #[tokio::test]
    async fn hung_helper_is_killed_at_the_deadline() {
        let dir = temp_dir("hang");
        let bin = script(&dir, "hang.sh", "exec sleep 30");
        let client =
            ConvertClient::from_parts(bin, vec![]).with_limits(Duration::from_millis(300), 1);
        let started = std::time::Instant::now();
        let err = client.md_to_tiptap("x").await.unwrap_err();
        assert!(matches!(err, ConvertError::TimedOut), "{err:?}");
        assert!(started.elapsed() < Duration::from_secs(5));
        let _ = std::fs::remove_dir_all(dir);
    }

    #[tokio::test]
    async fn oversized_stdout_is_rejected_without_buffering_it_all() {
        let dir = temp_dir("big");
        let bin = script(&dir, "big.sh", "cat >/dev/null; exec yes aaaaaaaaaaaaaaaa");
        let client = ConvertClient::from_parts(bin, vec![]);
        let err = client.md_to_tiptap("x").await.unwrap_err();
        assert!(matches!(err, ConvertError::TooLarge), "{err:?}");
        let _ = std::fs::remove_dir_all(dir);
    }
}
