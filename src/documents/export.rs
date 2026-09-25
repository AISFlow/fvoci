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
    #[error("invalid document body")]
    InvalidInput,
    #[error("document too large")]
    TooLarge,
    #[error("convert failed")]
    Failed,
    #[error("convert helper busy")]
    Busy,
}

pub struct RenderedExport {
    pub bytes: Vec<u8>,
    pub content_type: String,
    pub ext: String,
}

pub async fn render_document_export(
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
    let (bytes, content_type, ext) =
        match client.export_binary(format.op(), title, content_json).await {
            Ok(v) => v,
            Err(ConvertError::InvalidInput) => return Err(ExportRenderError::InvalidInput),
            Err(ConvertError::TooLarge) => return Err(ExportRenderError::TooLarge),
            Err(ConvertError::Busy) => return Err(ExportRenderError::Busy),
            Err(_) => return Err(ExportRenderError::Failed),
        };
    Ok(RenderedExport {
        bytes,
        content_type,
        ext,
    })
}

/// Source `renderDocument*`: `${title}.${ext}`; quoting and RFC 5987
/// encoding happen in `content_disposition_attachment`.
pub fn export_filename(title: &str, ext: &str) -> String {
    format!("{title}.{ext}")
}
