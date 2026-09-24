pub fn sniff_mime_from_bytes(buf: &[u8]) -> String {
    infer::get(buf)
        .map(|kind| kind.mime_type().to_string())
        .unwrap_or_else(|| "application/octet-stream".to_string())
}

pub fn is_image_mime(mime: &str) -> bool {
    mime.starts_with("image/") && mime != "image/svg+xml"
}
