use serde_json::Value;

use crate::collab::derived_body::DOCUMENT_MAX_BODY_BYTES;
use crate::documents::convert::{ConvertClient, ConvertError};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExportFormat {
    Markdown,
    Pdf,
    Docx,
    Pptx,
}

impl ExportFormat {
    pub fn op(self) -> &'static str {
        match self {
            Self::Markdown => "export_md",
            Self::Pdf => "export_pdf",
            Self::Docx => "export_docx",
            Self::Pptx => "export_pptx",
        }
    }
}

#[derive(Debug, thiserror::Error)]
pub enum ExportRenderError {
    #[error("convert unavailable")]
    Unavailable,
    #[error("invalid document body")]
    InvalidInput,
    #[error("document too large")]
    TooLarge,
    #[error("convert failed")]
    Failed,
}

pub struct RenderedExport {
    pub bytes: Vec<u8>,
    pub content_type: String,
    pub ext: String,
}

pub fn render_document_export(
    client: &ConvertClient,
    format: ExportFormat,
    title: &str,
    content_json: &Value,
) -> Result<RenderedExport, ExportRenderError> {
    let serialized =
        serde_json::to_vec(content_json).map_err(|_| ExportRenderError::InvalidInput)?;
    if serialized.len() > DOCUMENT_MAX_BODY_BYTES {
        return Err(ExportRenderError::TooLarge);
    }
    let (bytes, content_type, ext) = match client.export_binary(format.op(), title, content_json) {
        Ok(v) => v,
        Err(ConvertError::InvalidInput) => return Err(ExportRenderError::InvalidInput),
        Err(ConvertError::TooLarge) => return Err(ExportRenderError::TooLarge),
        Err(ConvertError::NotConfigured) => return Err(ExportRenderError::Unavailable),
        Err(_) => return Err(ExportRenderError::Failed),
    };
    Ok(RenderedExport {
        bytes,
        content_type,
        ext,
    })
}

pub fn export_filename(title: &str, ext: &str) -> String {
    let base = title
        .trim()
        .replace(['/', '\\', '?', '%', '*', ':', '|', '"', '<', '>'], "_");
    let clipped: String = base.chars().take(80).collect();
    let stem = if clipped.is_empty() {
        "export"
    } else {
        clipped.as_str()
    };
    format!("{stem}.{ext}")
}
