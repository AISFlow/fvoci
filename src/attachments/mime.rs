/// MIME type of the leading bytes, `application/octet-stream` when nothing
/// matches. Two `infer` 0.22 additions keep the earlier result, as the source
/// `file-type` reads them: `%PDF` counts only at offset 0, and an AutoCAD
/// drawing stays a download instead of an image the viewer cannot show.
pub fn sniff_mime_from_bytes(buf: &[u8]) -> String {
    let kind = match infer::get(buf) {
        Some(kind) if kind.mime_type() == "application/pdf" && !buf.starts_with(PDF_MAGIC) => {
            infer::get(&without_late_pdf_magic(buf))
        }
        kind => kind,
    };
    match kind.map(|kind| kind.mime_type()) {
        Some("image/vnd.dwg") | None => "application/octet-stream".to_string(),
        Some(mime) => mime.to_string(),
    }
}

pub fn is_image_mime(mime: &str) -> bool {
    mime.starts_with("image/") && mime != "image/svg+xml"
}

const PDF_MAGIC: &[u8] = b"%PDF";

/// `infer` finds `%PDF` anywhere in the first KiB. With every such match past
/// offset 0 broken, the matchers after it (none of which reads `%PDF`) decide
/// as they did under the offset-0 rule.
fn without_late_pdf_magic(buf: &[u8]) -> Vec<u8> {
    let mut out = buf.to_vec();
    let end = out.len().min(1024);
    for at in 1..end.saturating_sub(PDF_MAGIC.len() - 1) {
        if out[at..].starts_with(PDF_MAGIC) {
            out[at + PDF_MAGIC.len() - 1] = b'x';
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    /// AutoCAD 2018 file header: version `AC1032`, five zero bytes,
    /// maintenance release, preview image address, application version,
    /// code page 30 (ANSI 1252).
    const DWG: &[u8] = b"AC1032\0\0\0\0\0\0\x01\x60\x01\0\0\x21\0\x1e\0\0\0\0\0\0\0\0\0";

    #[test]
    fn autocad_drawing_stays_a_download_not_an_image() {
        assert_eq!(
            infer::get(DWG).map(|kind| kind.mime_type()),
            Some("image/vnd.dwg")
        );
        let mime = sniff_mime_from_bytes(DWG);
        assert_eq!(mime, "application/octet-stream");
        assert!(!is_image_mime(&mime));
        assert_eq!(
            crate::attachments::initial_extract_status("plan.dwg", &mime),
            "skipped"
        );
    }

    #[test]
    fn pdf_signature_counts_only_at_offset_zero() {
        let pdf = b"%PDF-1.7\n%\xe2\xe3\xcf\xd3\n";
        assert_eq!(sniff_mime_from_bytes(pdf), "application/pdf");
        for offset in [1, 512] {
            let mut late = vec![b'x'; offset];
            late.extend_from_slice(pdf);
            assert_eq!(
                infer::get(&late).map(|kind| kind.mime_type()),
                Some("application/pdf")
            );
            assert_eq!(
                sniff_mime_from_bytes(&late),
                "application/octet-stream",
                "{offset}"
            );
        }
        // The matchers after `%PDF` still decide: HTML quoting the signature.
        let mut html = b"<html><body>".to_vec();
        html.resize(512, b' ');
        html.extend_from_slice(b"%PDF-1.7 sample</body></html>");
        assert_eq!(sniff_mime_from_bytes(&html), "text/html");
    }
}
