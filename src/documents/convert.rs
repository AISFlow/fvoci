use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use base64::{engine::general_purpose::STANDARD as B64, Engine as _};
use serde::Deserialize;
use serde_json::{json, Value};

const CONVERT_TIMEOUT: Duration = Duration::from_secs(300);
const MAX_STDOUT_BYTES: usize = 32 * 1024 * 1024;

#[derive(Debug, Clone)]
pub struct ConvertClient {
    command: String,
    args: Vec<String>,
}

#[derive(Debug, thiserror::Error)]
pub enum ConvertError {
    #[error("document convert helper not configured")]
    NotConfigured,
    #[error("document convert helper failed: {0}")]
    Failed(String),
    #[error("invalid input")]
    InvalidInput,
    #[error("document too large")]
    TooLarge,
    #[error("timed out")]
    TimedOut,
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
        if parts.is_empty() {
            return None;
        }
        Some(Self {
            command: parts[0].to_string(),
            args: parts[1..].iter().map(|s| s.to_string()).collect(),
        })
    }

    pub fn from_parts(command: impl Into<String>, args: Vec<String>) -> Self {
        Self {
            command: command.into(),
            args,
        }
    }

    fn call(&self, body: Value) -> Result<ConvertResponse, ConvertError> {
        let payload =
            serde_json::to_string(&body).map_err(|e| ConvertError::Failed(e.to_string()))?;
        let started = Instant::now();
        let output = Command::new(&self.command)
            .args(&self.args)
            .env("FVOCI_CONVERT_PAYLOAD", payload)
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .output()
            .map_err(|e| ConvertError::Failed(e.to_string()))?;
        if started.elapsed() >= CONVERT_TIMEOUT {
            return Err(ConvertError::TimedOut);
        }
        let stdout = output.stdout;
        if stdout.len() > MAX_STDOUT_BYTES {
            return Err(ConvertError::TooLarge);
        }
        if !output.status.success() {
            return Err(ConvertError::Failed(format!(
                "convert helper exited with {} (stdout={})",
                output.status,
                String::from_utf8_lossy(&stdout)
                    .chars()
                    .take(200)
                    .collect::<String>()
            )));
        }
        let parsed: ConvertResponse =
            serde_json::from_slice(&stdout).map_err(|e| ConvertError::Failed(e.to_string()))?;
        Ok(parsed)
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

    pub fn md_to_tiptap(&self, markdown: &str) -> Result<Value, ConvertError> {
        let resp = Self::map_code(self.call(json!({
            "op": "md_to_tiptap",
            "markdown": markdown,
        }))?)?;
        resp.content_json.ok_or(ConvertError::Failed(
            "convert helper omitted contentJson".into(),
        ))
    }

    pub fn tiptap_to_yjs_update(&self, content_json: &Value) -> Result<Vec<u8>, ConvertError> {
        let resp = Self::map_code(self.call(json!({
            "op": "tiptap_to_yjs_update",
            "contentJson": content_json,
        }))?)?;
        let b64 = resp
            .update_b64
            .ok_or_else(|| ConvertError::Failed("convert helper omitted updateB64".into()))?;
        B64.decode(b64)
            .map_err(|e| ConvertError::Failed(e.to_string()))
    }

    pub fn export_binary(
        &self,
        op: &str,
        title: &str,
        content_json: &Value,
    ) -> Result<(Vec<u8>, String, String), ConvertError> {
        let resp = Self::map_code(self.call(json!({
            "op": op,
            "title": title,
            "contentJson": content_json,
        }))?)?;
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

pub fn validate_convert_bin(path: &std::path::Path) -> Result<(), String> {
    if path.as_os_str().is_empty() {
        return Err("FVOCI_DOCUMENT_CONVERT_BIN is empty".into());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn convert_helper_roundtrip_when_configured() {
        let client = match ConvertClient::from_env() {
            Some(client) => client,
            None => return,
        };
        let content = client
            .md_to_tiptap("# Imported note\n\nFrom zip.")
            .expect("md_to_tiptap");
        client
            .tiptap_to_yjs_update(&content)
            .expect("tiptap_to_yjs_update");
    }
}
