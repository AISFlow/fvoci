use crate::limits::Limits;
use crate::outcome::{ExtractReport, ExtractStatus, LimitKind, UnsupportedReason};

const CFB_MAGIC: [u8; 8] = [0xD0, 0xCF, 0x11, 0xE0, 0xA1, 0xB1, 0x1A, 0xE1];
const ZIP_MAGIC: [u8; 4] = [0x50, 0x4B, 0x03, 0x04];
const FASOO_DRM: &[u8] = b"\x9b DRMONE";
const SCDSA: &[u8] = b"SCDSA";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HintedKind {
    Hwp,
    Hwpx,
    Other,
}

pub fn hint_from_name(name: &str) -> HintedKind {
    let ext = name.rsplit('.').next().unwrap_or("").to_ascii_lowercase();
    match ext.as_str() {
        "hwp" => HintedKind::Hwp,
        "hwpx" => HintedKind::Hwpx,
        _ => HintedKind::Other,
    }
}

pub fn reject_oversize(bytes: &[u8], limits: &Limits) -> Option<ExtractReport> {
    if bytes.len() as u64 > limits.max_input_bytes {
        return Some(ExtractReport::new(ExtractStatus::ResourceLimit {
            kind: LimitKind::Input,
            detail: format!(
                "input {} bytes exceeds {}-byte limit",
                bytes.len(),
                limits.max_input_bytes
            ),
        }));
    }
    None
}

pub fn reject_drm(bytes: &[u8]) -> Option<ExtractReport> {
    if bytes.starts_with(FASOO_DRM) || bytes.starts_with(SCDSA) {
        return Some(ExtractReport::new(ExtractStatus::Unsupported {
            reason: UnsupportedReason::Drm,
            detail: "DRM/security container is not decrypted".to_string(),
        }));
    }
    None
}

pub fn reject_empty(bytes: &[u8]) -> Option<ExtractReport> {
    if bytes.is_empty() {
        return Some(ExtractReport::new(ExtractStatus::Unsupported {
            reason: UnsupportedReason::EmptyFile,
            detail: "empty file".to_string(),
        }));
    }
    None
}

/// Inspect zip central directory without inflating payloads.
pub fn inspect_zip_limits(bytes: &[u8], limits: &Limits) -> Option<ExtractReport> {
    if bytes.len() < 4 || bytes[0..4] != ZIP_MAGIC {
        return None;
    }
    walk_zip_cd(bytes, limits).err()
}

fn walk_zip_cd(bytes: &[u8], limits: &Limits) -> Result<(), ExtractReport> {
    let Some(eocd) = find_eocd(bytes) else {
        return Err(ExtractReport::new(ExtractStatus::Corrupt {
            detail: "zip end-of-central-directory not found".to_string(),
        }));
    };
    let cd_offset = u32::from_le_bytes(eocd[16..20].try_into().unwrap()) as usize;
    let cd_size = u32::from_le_bytes(eocd[12..16].try_into().unwrap()) as usize;
    let declared_entries = u16::from_le_bytes(eocd[10..12].try_into().unwrap()) as usize;
    if declared_entries > limits.max_zip_entries {
        return Err(ExtractReport::new(ExtractStatus::ResourceLimit {
            kind: LimitKind::ZipEntries,
            detail: format!(
                "{declared_entries} zip entries exceeds {}",
                limits.max_zip_entries
            ),
        }));
    }
    if cd_offset.saturating_add(cd_size) > bytes.len() {
        return Err(ExtractReport::new(ExtractStatus::Corrupt {
            detail: "zip central directory truncated".to_string(),
        }));
    }
    let mut pos = cd_offset;
    let end = cd_offset + cd_size;
    let mut entries = 0usize;
    let mut uncompressed = 0u64;
    while pos + 46 <= end {
        if &bytes[pos..pos + 4] != b"PK\x01\x02" {
            return Err(ExtractReport::new(ExtractStatus::Corrupt {
                detail: "zip central directory signature mismatch".to_string(),
            }));
        }
        let uncomp = u32::from_le_bytes(bytes[pos + 24..pos + 28].try_into().unwrap()) as u64;
        let name_len = u16::from_le_bytes(bytes[pos + 28..pos + 30].try_into().unwrap()) as usize;
        let extra_len = u16::from_le_bytes(bytes[pos + 30..pos + 32].try_into().unwrap()) as usize;
        let comment_len =
            u16::from_le_bytes(bytes[pos + 32..pos + 34].try_into().unwrap()) as usize;
        let name_at = pos + 46;
        let name_end = name_at.saturating_add(name_len);
        if name_end > end {
            return Err(ExtractReport::new(ExtractStatus::Corrupt {
                detail: "zip entry name truncated".to_string(),
            }));
        }
        let name = std::str::from_utf8(&bytes[name_at..name_end]).unwrap_or("");
        if name.contains("..") || name.starts_with('/') || name.contains('\\') {
            return Err(ExtractReport::new(ExtractStatus::Corrupt {
                detail: format!("zip entry path rejected: {name}"),
            }));
        }
        entries += 1;
        if entries > limits.max_zip_entries {
            return Err(ExtractReport::new(ExtractStatus::ResourceLimit {
                kind: LimitKind::ZipEntries,
                detail: format!("{entries} zip entries exceeds {}", limits.max_zip_entries),
            }));
        }
        uncompressed = uncompressed.saturating_add(uncomp);
        if uncompressed > limits.max_zip_uncompressed_bytes {
            return Err(ExtractReport::new(ExtractStatus::ResourceLimit {
                kind: LimitKind::ZipUncompressed,
                detail: format!(
                    "zip uncompressed {uncompressed} exceeds {}",
                    limits.max_zip_uncompressed_bytes
                ),
            }));
        }
        pos = name_end + extra_len + comment_len;
    }
    Ok(())
}

fn find_eocd(bytes: &[u8]) -> Option<&[u8]> {
    if bytes.len() < 22 {
        return None;
    }
    let start = bytes.len().saturating_sub(22 + 65535);
    let window = &bytes[start..];
    window
        .windows(22)
        .rev()
        .find(|w| w.starts_with(b"PK\x05\x06"))
}

pub fn is_cfb(bytes: &[u8]) -> bool {
    bytes.len() >= 8 && bytes[0..8] == CFB_MAGIC
}

pub fn is_zip(bytes: &[u8]) -> bool {
    bytes.len() >= 4 && bytes[0..4] == ZIP_MAGIC
}
