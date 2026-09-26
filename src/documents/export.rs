use serde_json::Value;

use crate::collab::derived_body::DOCUMENT_MAX_BODY_BYTES;
use crate::documents::convert::{ConvertClient, ConvertError};
use crate::documents::docx::DOCX_CONTENT_TYPE;
use crate::documents::markdown_helper::{MarkdownError, MarkdownHelper};
use crate::documents::pdf::PDF_CONTENT_TYPE;

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

/// DOCX in Rust (source `export_docx`): the same 1 MiB body check, then the
/// `--internal-markdown` child writes the file.
pub async fn render_docx_export(
    helper: &MarkdownHelper,
    title: &str,
    content_json: &Value,
) -> Result<RenderedExport, ExportRenderError> {
    let serialized =
        serde_json::to_vec(content_json).map_err(|_| ExportRenderError::InvalidInput)?;
    if serialized.len() > DOCUMENT_MAX_BODY_BYTES {
        return Err(ExportRenderError::TooLarge);
    }
    let bytes = match helper.tiptap_to_docx(title, content_json).await {
        Ok(bytes) => bytes,
        Err(MarkdownError::InvalidInput(_)) => return Err(ExportRenderError::InvalidInput),
        Err(MarkdownError::TooLarge) => return Err(ExportRenderError::TooLarge),
        Err(MarkdownError::Failed(detail)) => {
            tracing::error!(%detail, "docx export child failed");
            return Err(ExportRenderError::Failed);
        }
    };
    Ok(RenderedExport {
        bytes,
        content_type: DOCX_CONTENT_TYPE.to_string(),
        ext: "docx".to_string(),
    })
}

/// PDF in Rust (source `export_pdf`): the same 1 MiB body check, then the
/// `--internal-markdown` child writes the file. `public` (anonymous share
/// PDFs) uses the fail-fast pool and answers `Busy` instead of waiting.
pub async fn render_pdf_export(
    helper: &MarkdownHelper,
    title: &str,
    content_json: &Value,
    public: bool,
) -> Result<RenderedExport, ExportRenderError> {
    let serialized =
        serde_json::to_vec(content_json).map_err(|_| ExportRenderError::InvalidInput)?;
    if serialized.len() > DOCUMENT_MAX_BODY_BYTES {
        return Err(ExportRenderError::TooLarge);
    }
    let bytes = if public {
        helper.tiptap_to_pdf_public(title, content_json).await?
    } else {
        helper.tiptap_to_pdf(title, content_json).await?
    };
    Ok(RenderedExport {
        bytes,
        content_type: PDF_CONTENT_TYPE.to_string(),
        ext: "pdf".to_string(),
    })
}

/// Source `renderDocument*`: `${title}.${ext}`; quoting and RFC 5987
/// encoding happen in `content_disposition_attachment`.
pub fn export_filename(title: &str, ext: &str) -> String {
    format!("{title}.{ext}")
}
