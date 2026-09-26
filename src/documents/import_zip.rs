//! Untrusted zip reader for imports, mirroring source `zip-store.ts` `unzipStore`.
//!
//! Entries are enumerated from the central directory (what real parsers use),
//! and each local header must agree with its central record. zip64, streamed
//! data-descriptor entries, traversal names and methods other than
//! store/deflate reject the whole archive. Inflation is bounded per entry by the
//! remaining total budget, so a small archive cannot expand past the cap in
//! memory before the check runs.

use std::io::Read;

use flate2::read::DeflateDecoder;

pub const ZIP_MAX_ENTRIES: usize = 10_000;
pub const ZIP_MAX_UNCOMPRESSED_BYTES: u64 = 200 * 1024 * 1024;

const LOCAL_SIG: u32 = 0x0403_4b50;
const CENTRAL_SIG: u32 = 0x0201_4b50;
const EOCD_SIG: u32 = 0x0605_4b50;
const LOCAL_SIZE: usize = 30;
const CENTRAL_SIZE: usize = 46;
const EOCD_SIZE: usize = 22;
const ZIP32_MAX: u32 = 0xffff_ffff;
const ZIP64_EXTRA_ID: u16 = 0x0001;
const FLAG_DATA_DESCRIPTOR: u16 = 0x0008;
const MAX_EOCD_COMMENT: usize = 65_535;

#[derive(Debug, Clone)]
pub struct ZipEntry {
    /// Normalized relative path (`zipEntryName`): `/` separated, no empty,
    /// `.` or `..` segments. Traversal names never reach this point.
    pub name: String,
    pub data: Vec<u8>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum ZipImportError {
    #[error("zip required")]
    Empty,
    #[error("invalid zip")]
    Invalid,
    #[error("zip entry count")]
    TooManyEntries,
    #[error("zip too large")]
    TooLarge,
    #[error("zip entry escapes")]
    Traversal,
    #[error("zip64 unsupported")]
    Zip64,
    #[error("zip method unsupported")]
    Method,
    #[error("zip data descriptor unsupported")]
    DataDescriptor,
    #[error("zip local/central mismatch")]
    Mismatch,
}

fn u16_at(buf: &[u8], at: usize) -> Result<u16, ZipImportError> {
    let bytes = buf.get(at..at + 2).ok_or(ZipImportError::Invalid)?;
    Ok(u16::from_le_bytes([bytes[0], bytes[1]]))
}

fn u32_at(buf: &[u8], at: usize) -> Result<u32, ZipImportError> {
    let bytes = buf.get(at..at + 4).ok_or(ZipImportError::Invalid)?;
    Ok(u32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]))
}

fn slice(buf: &[u8], from: usize, to: usize) -> Result<&[u8], ZipImportError> {
    if from > to {
        return Err(ZipImportError::Invalid);
    }
    buf.get(from..to).ok_or(ZipImportError::Invalid)
}

fn find_eocd(buf: &[u8]) -> Result<usize, ZipImportError> {
    let last = buf.len() - EOCD_SIZE;
    // Only a comment of at most 65 535 bytes may follow the EOCD record.
    let first = last.saturating_sub(MAX_EOCD_COMMENT);
    (first..=last)
        .rev()
        .find(|&at| u32_at(buf, at).ok() == Some(EOCD_SIG))
        .ok_or(ZipImportError::Invalid)
}

fn assert_supported(method: u16, compressed: u32, size: u32) -> Result<(), ZipImportError> {
    if method != 0 && method != 8 {
        return Err(ZipImportError::Method);
    }
    if compressed == ZIP32_MAX || size == ZIP32_MAX {
        return Err(ZipImportError::Zip64);
    }
    Ok(())
}

fn assert_no_zip64_extra(extra: &[u8]) -> Result<(), ZipImportError> {
    let mut at = 0usize;
    while at + 4 <= extra.len() {
        let id = u16::from_le_bytes([extra[at], extra[at + 1]]);
        let size = u16::from_le_bytes([extra[at + 2], extra[at + 3]]) as usize;
        if id == ZIP64_EXTRA_ID {
            return Err(ZipImportError::Zip64);
        }
        at += 4 + size;
    }
    Ok(())
}

/// Source `assertNoTraversal`: absolute, drive-letter and `..` names reject
/// the archive instead of being rewritten.
fn assert_no_traversal(raw: &str) -> Result<(), ZipImportError> {
    let norm = raw.replace('\\', "/").replace('\0', "");
    let bytes = norm.as_bytes();
    if norm.starts_with('/')
        || (bytes.len() >= 2 && bytes[0].is_ascii_alphabetic() && bytes[1] == b':')
    {
        return Err(ZipImportError::Traversal);
    }
    if norm.split('/').any(|part| part == "..") {
        return Err(ZipImportError::Traversal);
    }
    Ok(())
}

/// Source `zipEntryName`.
pub fn zip_entry_name(raw: &str) -> String {
    let normalized = raw.replace('\\', "/").replace('\0', "");
    let joined = normalized
        .split('/')
        .filter(|part| !part.is_empty() && *part != "." && *part != "..")
        .collect::<Vec<_>>()
        .join("/");
    if joined.is_empty() {
        "file".to_string()
    } else {
        joined
    }
}

/// Source `zipSafeName`: the last path segment only.
pub fn zip_safe_name(raw: &str) -> String {
    let entry = zip_entry_name(raw);
    let last = entry.rsplit('/').next().unwrap_or("file");
    let cleaned = last.replace('\0', "");
    let cleaned = cleaned.trim();
    if cleaned.is_empty() || cleaned == "." || cleaned == ".." {
        "file".to_string()
    } else {
        cleaned.to_string()
    }
}

/// Source `titleFromFileName`: strips one extension of 1..=12 characters.
pub fn title_from_file_name(name: &str) -> String {
    let base = zip_safe_name(name);
    let stem = match base.rfind('.') {
        Some(dot) => {
            let ext = &base[dot + 1..];
            let len = ext.chars().count();
            if (1..=12).contains(&len) {
                &base[..dot]
            } else {
                base.as_str()
            }
        }
        None => base.as_str(),
    };
    let stem = stem.trim();
    if stem.is_empty() {
        "untitled".to_string()
    } else {
        stem.to_string()
    }
}

struct CentralEntry<'a> {
    name: &'a [u8],
    method: u16,
    compressed: u32,
    size: u32,
}

/// Payload slice located by the local header, which must match the central record.
fn local_payload<'a>(
    buf: &'a [u8],
    at: usize,
    entry: &CentralEntry<'_>,
) -> Result<&'a [u8], ZipImportError> {
    if at + LOCAL_SIZE > buf.len() || u32_at(buf, at)? != LOCAL_SIG {
        return Err(ZipImportError::Invalid);
    }
    let flags = u16_at(buf, at + 6)?;
    let method = u16_at(buf, at + 8)?;
    let compressed = u32_at(buf, at + 18)?;
    let size = u32_at(buf, at + 22)?;
    assert_supported(method, compressed, size)?;
    // A streamed entry has no sizes in its local header to compare against.
    if flags & FLAG_DATA_DESCRIPTOR != 0 && compressed == 0 {
        return Err(ZipImportError::DataDescriptor);
    }
    let name_at = at + LOCAL_SIZE;
    let extra_at = name_at + u16_at(buf, at + 26)? as usize;
    let data_at = extra_at + u16_at(buf, at + 28)? as usize;
    assert_no_zip64_extra(slice(buf, extra_at, data_at)?)?;
    if method != entry.method
        || compressed != entry.compressed
        || size != entry.size
        || slice(buf, name_at, extra_at)? != entry.name
    {
        return Err(ZipImportError::Mismatch);
    }
    let stored = if method == 0 { size } else { compressed } as usize;
    slice(buf, data_at, data_at + stored)
}

/// Inflates at most `limit` bytes; one more byte means the budget is exceeded.
fn inflate_bounded(payload: &[u8], limit: u64) -> Result<Vec<u8>, ZipImportError> {
    let mut out = Vec::new();
    DeflateDecoder::new(payload)
        .take(limit.saturating_add(1))
        .read_to_end(&mut out)
        .map_err(|_| ZipImportError::Invalid)?;
    if out.len() as u64 > limit {
        return Err(ZipImportError::TooLarge);
    }
    Ok(out)
}

pub fn unzip_bounded(buf: &[u8]) -> Result<Vec<ZipEntry>, ZipImportError> {
    unzip_bounded_with_limit(buf, ZIP_MAX_UNCOMPRESSED_BYTES)
}

pub fn unzip_bounded_with_limit(
    buf: &[u8],
    max_total: u64,
) -> Result<Vec<ZipEntry>, ZipImportError> {
    unzip_bounded_select(buf, max_total, |_| true)
}

/// Like [`unzip_bounded_with_limit`], but only entries whose normalized name
/// passes `select` are inflated and returned. Every entry's headers are still
/// validated, so a malformed or traversal entry rejects the whole archive.
pub fn unzip_bounded_select(
    buf: &[u8],
    max_total: u64,
    select: impl Fn(&str) -> bool,
) -> Result<Vec<ZipEntry>, ZipImportError> {
    if buf.is_empty() {
        return Err(ZipImportError::Empty);
    }
    if buf.len() < EOCD_SIZE {
        return Err(ZipImportError::Invalid);
    }
    let eocd = find_eocd(buf)?;
    let entries = u16_at(buf, eocd + 10)?;
    if entries as usize > ZIP_MAX_ENTRIES {
        return Err(ZipImportError::TooManyEntries);
    }
    let at = u32_at(buf, eocd + 16)?;
    if entries == 0xffff || at == ZIP32_MAX {
        return Err(ZipImportError::Zip64);
    }
    let mut at = at as usize;
    let mut total = 0u64;
    let mut files = Vec::new();
    for _ in 0..entries {
        if at + CENTRAL_SIZE > buf.len() || u32_at(buf, at)? != CENTRAL_SIG {
            return Err(ZipImportError::Invalid);
        }
        let method = u16_at(buf, at + 10)?;
        let compressed = u32_at(buf, at + 20)?;
        let size = u32_at(buf, at + 24)?;
        assert_supported(method, compressed, size)?;
        let name_at = at + CENTRAL_SIZE;
        let extra_at = name_at + u16_at(buf, at + 28)? as usize;
        let comment_at = extra_at + u16_at(buf, at + 30)? as usize;
        let raw_name = slice(buf, name_at, extra_at)?;
        let name = String::from_utf8_lossy(raw_name);
        assert_no_traversal(&name)?;
        assert_no_zip64_extra(slice(buf, extra_at, comment_at)?)?;
        let payload = local_payload(
            buf,
            u32_at(buf, at + 42)? as usize,
            &CentralEntry {
                name: raw_name,
                method,
                compressed,
                size,
            },
        )?;
        if !name.ends_with('/') && select(&zip_entry_name(&name)) {
            let remaining = max_total.saturating_sub(total);
            let data = if method == 0 {
                if payload.len() as u64 > remaining {
                    return Err(ZipImportError::TooLarge);
                }
                payload.to_vec()
            } else {
                inflate_bounded(payload, remaining)?
            };
            total += data.len() as u64;
            files.push(ZipEntry {
                name: zip_entry_name(&name),
                data,
            });
        }
        at = comment_at + u16_at(buf, at + 32)? as usize;
    }
    Ok(files)
}

#[cfg(test)]
mod tests {
    use super::*;
    use flate2::write::DeflateEncoder;
    use flate2::Compression;
    use std::io::Write;

    struct Spec<'a> {
        name: &'a str,
        data: &'a [u8],
        deflate: bool,
    }

    /// Minimal writer so tests can shape headers that library writers refuse.
    fn build_zip(files: &[Spec<'_>]) -> Vec<u8> {
        let mut locals = Vec::new();
        let mut centrals = Vec::new();
        for file in files {
            let (method, stored) = if file.deflate {
                let mut enc = DeflateEncoder::new(Vec::new(), Compression::best());
                enc.write_all(file.data).unwrap();
                (8u16, enc.finish().unwrap())
            } else {
                (0u16, file.data.to_vec())
            };
            let offset = locals.len() as u32;
            let name = file.name.as_bytes();
            let mut local = Vec::new();
            local.extend_from_slice(&LOCAL_SIG.to_le_bytes());
            local.extend_from_slice(&20u16.to_le_bytes());
            local.extend_from_slice(&0x0800u16.to_le_bytes());
            local.extend_from_slice(&method.to_le_bytes());
            local.extend_from_slice(&[0; 8]);
            local.extend_from_slice(&(stored.len() as u32).to_le_bytes());
            local.extend_from_slice(&(file.data.len() as u32).to_le_bytes());
            local.extend_from_slice(&(name.len() as u16).to_le_bytes());
            local.extend_from_slice(&0u16.to_le_bytes());
            local.extend_from_slice(name);
            local.extend_from_slice(&stored);
            locals.extend_from_slice(&local);
            let mut central = Vec::new();
            central.extend_from_slice(&CENTRAL_SIG.to_le_bytes());
            central.extend_from_slice(&20u16.to_le_bytes());
            central.extend_from_slice(&20u16.to_le_bytes());
            central.extend_from_slice(&0x0800u16.to_le_bytes());
            central.extend_from_slice(&method.to_le_bytes());
            central.extend_from_slice(&[0; 8]);
            central.extend_from_slice(&(stored.len() as u32).to_le_bytes());
            central.extend_from_slice(&(file.data.len() as u32).to_le_bytes());
            central.extend_from_slice(&(name.len() as u16).to_le_bytes());
            central.extend_from_slice(&[0; 12]);
            central.extend_from_slice(&offset.to_le_bytes());
            central.extend_from_slice(name);
            centrals.extend_from_slice(&central);
        }
        let mut out = locals.clone();
        out.extend_from_slice(&centrals);
        out.extend_from_slice(&EOCD_SIG.to_le_bytes());
        out.extend_from_slice(&[0; 4]);
        out.extend_from_slice(&(files.len() as u16).to_le_bytes());
        out.extend_from_slice(&(files.len() as u16).to_le_bytes());
        out.extend_from_slice(&(centrals.len() as u32).to_le_bytes());
        out.extend_from_slice(&(locals.len() as u32).to_le_bytes());
        out.extend_from_slice(&0u16.to_le_bytes());
        out
    }

    #[test]
    fn keeps_relative_paths_for_hierarchy() {
        let zip = build_zip(&[
            Spec {
                name: "Root abc/Child 0123456789abcdef.md",
                data: b"# child",
                deflate: true,
            },
            Spec {
                name: "Root abc.md",
                data: b"# root",
                deflate: false,
            },
        ]);
        let entries = unzip_bounded(&zip).unwrap();
        assert_eq!(entries[0].name, "Root abc/Child 0123456789abcdef.md");
        assert_eq!(entries[0].data, b"# child");
        assert_eq!(entries[1].name, "Root abc.md");
    }

    #[test]
    fn rejects_traversal_instead_of_rewriting() {
        for name in [
            "../evil.md",
            "a/../../evil.md",
            "/abs.md",
            "C:evil.md",
            "a\\..\\b.md",
        ] {
            let zip = build_zip(&[Spec {
                name,
                data: b"x",
                deflate: false,
            }]);
            assert_eq!(
                unzip_bounded(&zip).unwrap_err(),
                ZipImportError::Traversal,
                "{name}"
            );
        }
    }

    #[test]
    fn deflate_bomb_stops_at_budget_without_full_inflation() {
        // 64 MiB of zeros deflates to about 64 KiB; the budget is 1 MiB.
        let zeros = vec![0u8; 64 * 1024 * 1024];
        let zip = build_zip(&[Spec {
            name: "bomb.md",
            data: &zeros,
            deflate: true,
        }]);
        assert!(zip.len() < 1024 * 1024);
        assert_eq!(
            unzip_bounded_with_limit(&zip, 1024 * 1024).unwrap_err(),
            ZipImportError::TooLarge
        );
    }

    #[test]
    fn total_budget_spans_entries() {
        let chunk = vec![7u8; 600 * 1024];
        let zip = build_zip(&[
            Spec {
                name: "a.md",
                data: &chunk,
                deflate: true,
            },
            Spec {
                name: "b.md",
                data: &chunk,
                deflate: false,
            },
        ]);
        assert_eq!(
            unzip_bounded_with_limit(&zip, 1024 * 1024).unwrap_err(),
            ZipImportError::TooLarge
        );
        assert_eq!(
            unzip_bounded_with_limit(&zip, 2 * 1024 * 1024)
                .unwrap()
                .len(),
            2
        );
    }

    #[test]
    fn rejects_zip64_descriptor_mismatch_and_method() {
        let base = build_zip(&[Spec {
            name: "a.md",
            data: b"hello",
            deflate: false,
        }]);
        let central = base.len() - EOCD_SIZE - (CENTRAL_SIZE + 4);

        let mut zip64 = base.clone();
        zip64[central + 20..central + 24].copy_from_slice(&ZIP32_MAX.to_le_bytes());
        assert_eq!(unzip_bounded(&zip64).unwrap_err(), ZipImportError::Zip64);

        let mut method = base.clone();
        method[central + 10..central + 12].copy_from_slice(&12u16.to_le_bytes());
        assert_eq!(unzip_bounded(&method).unwrap_err(), ZipImportError::Method);

        let mut mismatch = base.clone();
        mismatch[LOCAL_SIZE] = b'b';
        assert_eq!(
            unzip_bounded(&mismatch).unwrap_err(),
            ZipImportError::Mismatch
        );

        let streamed = {
            let mut z = base.clone();
            z[6..8].copy_from_slice(&FLAG_DATA_DESCRIPTOR.to_le_bytes());
            z[18..22].copy_from_slice(&0u32.to_le_bytes());
            z
        };
        assert_eq!(
            unzip_bounded(&streamed).unwrap_err(),
            ZipImportError::DataDescriptor
        );
    }

    #[test]
    fn names_and_titles_follow_source() {
        assert_eq!(zip_entry_name("./a//b/../c.md"), "a/b/c.md");
        assert_eq!(zip_safe_name("dir/sub/file.md"), "file.md");
        assert_eq!(title_from_file_name("dir/회의록.md"), "회의록");
        assert_eq!(title_from_file_name("archive.tar.gz"), "archive.tar");
        assert_eq!(
            title_from_file_name("notes.averyveryverylongext"),
            "notes.averyveryverylongext"
        );
        assert_eq!(title_from_file_name(".md"), "untitled");
    }
}
