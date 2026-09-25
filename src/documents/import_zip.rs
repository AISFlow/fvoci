use std::io::{Cursor, Read};

use zip::read::ZipArchive;

pub const ZIP_MAX_ENTRIES: usize = 10_000;
pub const ZIP_MAX_UNCOMPRESSED_BYTES: u64 = 200 * 1024 * 1024;

#[derive(Debug, Clone)]
pub struct ZipEntry {
    pub name: String,
    pub data: Vec<u8>,
}

#[derive(Debug, thiserror::Error)]
pub enum ZipImportError {
    #[error("zip required")]
    Empty,
    #[error("zip too large")]
    TooLarge,
    #[error("zip entry too large")]
    EntryTooLarge,
    #[error("zip read failed")]
    ReadFailed,
}

pub fn unzip_bounded(bytes: &[u8]) -> Result<Vec<ZipEntry>, ZipImportError> {
    if bytes.is_empty() {
        return Err(ZipImportError::Empty);
    }
    let cursor = Cursor::new(bytes);
    let mut archive = ZipArchive::new(cursor).map_err(|_| ZipImportError::ReadFailed)?;
    if archive.len() > ZIP_MAX_ENTRIES {
        return Err(ZipImportError::TooLarge);
    }
    let mut total = 0u64;
    let mut out = Vec::new();
    for i in 0..archive.len() {
        let mut file = archive
            .by_index(i)
            .map_err(|_| ZipImportError::ReadFailed)?;
        if file.is_dir() {
            continue;
        }
        let name = zip_safe_name(file.name());
        let mut data = Vec::new();
        file.read_to_end(&mut data)
            .map_err(|_| ZipImportError::ReadFailed)?;
        total += data.len() as u64;
        if total > ZIP_MAX_UNCOMPRESSED_BYTES {
            return Err(ZipImportError::TooLarge);
        }
        out.push(ZipEntry { name, data });
    }
    Ok(out)
}

pub fn zip_safe_name(raw: &str) -> String {
    let normalized = raw.replace('\\', "/");
    let last = normalized
        .split('/')
        .next_back()
        .unwrap_or("file")
        .replace('\0', "")
        .trim()
        .to_string();
    if last.is_empty() || last == "." || last == ".." {
        "file".to_string()
    } else {
        last
    }
}

pub fn title_from_file_name(name: &str) -> String {
    let base = zip_safe_name(name);
    let stem = base
        .rsplit_once('.')
        .map(|(s, _)| s)
        .unwrap_or(base.as_str())
        .trim();
    if stem.is_empty() {
        "untitled".to_string()
    } else {
        stem.to_string()
    }
}
