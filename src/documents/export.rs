use serde_json::Value;

use crate::collab::derived_body::DOCUMENT_MAX_BODY_BYTES;
use crate::documents::docx::DOCX_CONTENT_TYPE;
use crate::documents::markdown_helper::{MarkdownError, MarkdownHelper};
use crate::documents::pdf::PDF_CONTENT_TYPE;
use crate::documents::pptx::PPTX_CONTENT_TYPE;

/// Source `convert.mjs` `export_md`.
pub const MARKDOWN_CONTENT_TYPE: &str = "text/markdown; charset=utf-8";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExportFormat {
    Markdown,
    Pdf,
    Docx,
    Pptx,
}

#[derive(Debug, thiserror::Error)]
pub enum ExportRenderError {
    #[error("invalid document body")]
    InvalidInput,
    #[error("document too large")]
    TooLarge,
    #[error("export failed")]
    Failed,
    #[error("export helper busy")]
    Busy,
}

pub struct RenderedExport {
    pub bytes: Vec<u8>,
    pub content_type: String,
    pub ext: String,
}

/// Every member export (source `renderDocument*` over the Node helper's
/// `export_*` ops), now written by this binary's `--internal-markdown` child:
/// the 1 MiB stored-body check, then the format's op on the shared pool.
pub async fn render_document_export(
    helper: &MarkdownHelper,
    format: ExportFormat,
    title: &str,
    content_json: &Value,
) -> Result<RenderedExport, ExportRenderError> {
    match format {
        ExportFormat::Docx => render_docx_export(helper, title, content_json).await,
        ExportFormat::Pdf => render_pdf_export(helper, title, content_json, false).await,
        ExportFormat::Pptx | ExportFormat::Markdown => {
            check_body_size(content_json)?;
            let (bytes, content_type, ext) = if format == ExportFormat::Pptx {
                (
                    helper.tiptap_to_pptx(title, content_json).await?,
                    PPTX_CONTENT_TYPE,
                    "pptx",
                )
            } else {
                (
                    helper.tiptap_to_md_export(title, content_json).await?,
                    MARKDOWN_CONTENT_TYPE,
                    "md",
                )
            };
            Ok(RenderedExport {
                bytes,
                content_type: content_type.to_string(),
                ext: ext.to_string(),
            })
        }
    }
}

/// Source: the stored body is at most `DOCUMENT_MAX_BODY_BYTES` serialized
/// (413 otherwise, before any child).
fn check_body_size(content_json: &Value) -> Result<(), ExportRenderError> {
    let serialized =
        serde_json::to_vec(content_json).map_err(|_| ExportRenderError::InvalidInput)?;
    if serialized.len() > DOCUMENT_MAX_BODY_BYTES {
        return Err(ExportRenderError::TooLarge);
    }
    Ok(())
}

/// DOCX in Rust (source `export_docx`): the same 1 MiB body check, then the
/// `--internal-markdown` child writes the file.
pub async fn render_docx_export(
    helper: &MarkdownHelper,
    title: &str,
    content_json: &Value,
) -> Result<RenderedExport, ExportRenderError> {
    check_body_size(content_json)?;
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
    check_body_size(content_json)?;
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
