//! Streaming stored (method 0) ZIP writer for exports.
//!
//! Source `zipStore` (packages/core/src/zip-store.ts) builds the whole archive
//! in memory. Here each entry is streamed with a data descriptor (general
//! purpose flag bit 3) so memory stays bounded by one chunk; the central
//! directory carries the real CRC and sizes. Entry names use the source
//! `zipEntryName` / `zipSafeName` rules and the UTF-8 flag (bit 11). Like the
//! source, the archive is zip32 only: an entry or offset past 4 GiB, or more
//! than 65 535 entries, fails instead of writing a corrupt archive.

use bytes::Bytes;

const LOCAL_SIG: u32 = 0x0403_4b50;
const DESCRIPTOR_SIG: u32 = 0x0807_4b50;
const CENTRAL_SIG: u32 = 0x0201_4b50;
const EOCD_SIG: u32 = 0x0605_4b50;
const VERSION: u16 = 20;
const FLAGS: u16 = 0x0808;
const ZIP32_MAX: u64 = 0xffff_ffff;
const MAX_ENTRIES: usize = 0xffff;

const CRC_TABLE: [u32; 256] = {
    let mut table = [0u32; 256];
    let mut i = 0;
    while i < 256 {
        let mut c = i as u32;
        let mut k = 0;
        while k < 8 {
            c = if c & 1 != 0 {
                0xedb8_8320 ^ (c >> 1)
            } else {
                c >> 1
            };
            k += 1;
        }
        table[i] = c;
        i += 1;
    }
    table
};

#[derive(Clone, Copy, Debug)]
pub struct Crc32(u32);

impl Default for Crc32 {
    fn default() -> Self {
        Self(0xffff_ffff)
    }
}

impl Crc32 {
    pub fn update(&mut self, data: &[u8]) {
        let mut c = self.0;
        for byte in data {
            c = CRC_TABLE[((c ^ u32::from(*byte)) & 0xff) as usize] ^ (c >> 8);
        }
        self.0 = c;
    }

    pub fn finish(self) -> u32 {
        self.0 ^ 0xffff_ffff
    }
}

pub fn crc32(data: &[u8]) -> u32 {
    let mut crc = Crc32::default();
    crc.update(data);
    crc.finish()
}

/// Source `zipEntryName`: backslashes to `/`, drop NUL, empty, `.` and `..`
/// segments.
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

/// Source `zipSafeName`: the last path segment, trimmed, never `.`/`..`/empty.
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

#[derive(Debug, thiserror::Error)]
pub enum ZipError {
    #[error("zip32 overflow")]
    Overflow,
    #[error("zip entry already open")]
    EntryOpen,
    #[error("no zip entry open")]
    NoEntry,
}

struct CentralEntry {
    name: Vec<u8>,
    crc: u32,
    size: u32,
    offset: u32,
}

struct OpenEntry {
    name: Vec<u8>,
    offset: u64,
    crc: Crc32,
    size: u64,
}

/// Produces archive bytes; the caller forwards each returned chunk.
#[derive(Default)]
pub struct ZipStream {
    offset: u64,
    entries: Vec<CentralEntry>,
    open: Option<OpenEntry>,
}

fn put_u16(out: &mut Vec<u8>, value: u16) {
    out.extend_from_slice(&value.to_le_bytes());
}

fn put_u32(out: &mut Vec<u8>, value: u32) {
    out.extend_from_slice(&value.to_le_bytes());
}

fn to_u32(value: u64) -> Result<u32, ZipError> {
    if value > ZIP32_MAX {
        return Err(ZipError::Overflow);
    }
    Ok(value as u32)
}

impl ZipStream {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn begin_entry(&mut self, raw_name: &str) -> Result<Bytes, ZipError> {
        if self.open.is_some() {
            return Err(ZipError::EntryOpen);
        }
        if self.entries.len() >= MAX_ENTRIES {
            return Err(ZipError::Overflow);
        }
        let name = zip_entry_name(raw_name).into_bytes();
        let name_len = u16::try_from(name.len()).map_err(|_| ZipError::Overflow)?;
        to_u32(self.offset)?;
        let mut out = Vec::with_capacity(30 + name.len());
        put_u32(&mut out, LOCAL_SIG);
        put_u16(&mut out, VERSION);
        put_u16(&mut out, FLAGS);
        put_u16(&mut out, 0); // stored
        put_u16(&mut out, 0); // time
        put_u16(&mut out, 0); // date
        put_u32(&mut out, 0); // crc (descriptor)
        put_u32(&mut out, 0); // compressed size (descriptor)
        put_u32(&mut out, 0); // size (descriptor)
        put_u16(&mut out, name_len);
        put_u16(&mut out, 0); // extra
        out.extend_from_slice(&name);
        self.open = Some(OpenEntry {
            name,
            offset: self.offset,
            crc: Crc32::default(),
            size: 0,
        });
        self.offset += out.len() as u64;
        Ok(Bytes::from(out))
    }

    /// Accounts for `data` inside the open entry; the caller forwards `data`.
    pub fn entry_data(&mut self, data: &[u8]) -> Result<(), ZipError> {
        let open = self.open.as_mut().ok_or(ZipError::NoEntry)?;
        open.crc.update(data);
        open.size += data.len() as u64;
        to_u32(open.size)?;
        self.offset += data.len() as u64;
        Ok(())
    }

    pub fn end_entry(&mut self) -> Result<Bytes, ZipError> {
        let open = self.open.take().ok_or(ZipError::NoEntry)?;
        let crc = open.crc.finish();
        let size = to_u32(open.size)?;
        let mut out = Vec::with_capacity(16);
        put_u32(&mut out, DESCRIPTOR_SIG);
        put_u32(&mut out, crc);
        put_u32(&mut out, size);
        put_u32(&mut out, size);
        self.entries.push(CentralEntry {
            name: open.name,
            crc,
            size,
            offset: to_u32(open.offset)?,
        });
        self.offset += out.len() as u64;
        Ok(Bytes::from(out))
    }

    pub fn finish(self) -> Result<Bytes, ZipError> {
        if self.open.is_some() {
            return Err(ZipError::EntryOpen);
        }
        let central_start = to_u32(self.offset)?;
        let mut out = Vec::new();
        for entry in &self.entries {
            put_u32(&mut out, CENTRAL_SIG);
            put_u16(&mut out, VERSION);
            put_u16(&mut out, VERSION);
            put_u16(&mut out, FLAGS);
            put_u16(&mut out, 0);
            put_u16(&mut out, 0);
            put_u16(&mut out, 0);
            put_u32(&mut out, entry.crc);
            put_u32(&mut out, entry.size);
            put_u32(&mut out, entry.size);
            put_u16(&mut out, entry.name.len() as u16);
            put_u16(&mut out, 0);
            put_u16(&mut out, 0);
            put_u16(&mut out, 0);
            put_u16(&mut out, 0);
            put_u32(&mut out, 0);
            put_u32(&mut out, entry.offset);
            out.extend_from_slice(&entry.name);
        }
        let central_len = to_u32(out.len() as u64)?;
        to_u32(self.offset + u64::from(central_len))?;
        let count = self.entries.len() as u16;
        put_u32(&mut out, EOCD_SIG);
        put_u16(&mut out, 0);
        put_u16(&mut out, 0);
        put_u16(&mut out, count);
        put_u16(&mut out, count);
        put_u32(&mut out, central_len);
        put_u32(&mut out, central_start);
        put_u16(&mut out, 0);
        Ok(Bytes::from(out))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn crc32_matches_reference() {
        assert_eq!(crc32(b""), 0);
        assert_eq!(crc32(b"123456789"), 0xcbf4_3926);
        let mut split = Crc32::default();
        split.update(b"1234");
        split.update(b"56789");
        assert_eq!(split.finish(), 0xcbf4_3926);
    }

    #[test]
    fn entry_names_follow_source_rules() {
        assert_eq!(zip_entry_name("../a\\b/./c"), "a/b/c");
        assert_eq!(zip_entry_name("\0"), "file");
        assert_eq!(zip_safe_name("dir/../보고서.hwp"), "보고서.hwp");
        assert_eq!(zip_safe_name(".."), "file");
        assert_eq!(zip_safe_name("  "), "file");
    }

    #[test]
    fn archive_layout_is_consistent() {
        let mut zip = ZipStream::new();
        let mut archive = Vec::new();
        archive.extend_from_slice(&zip.begin_entry("a.json").unwrap());
        zip.entry_data(b"{}\n").unwrap();
        archive.extend_from_slice(b"{}\n");
        archive.extend_from_slice(&zip.end_entry().unwrap());
        archive.extend_from_slice(&zip.finish().unwrap());
        let eocd = archive.len() - 22;
        assert_eq!(&archive[eocd..eocd + 4], &EOCD_SIG.to_le_bytes());
        let entries = u16::from_le_bytes([archive[eocd + 10], archive[eocd + 11]]);
        assert_eq!(entries, 1);
        let central = u32::from_le_bytes(archive[eocd + 16..eocd + 20].try_into().unwrap());
        let central = central as usize;
        assert_eq!(&archive[central..central + 4], &CENTRAL_SIG.to_le_bytes());
        let crc = u32::from_le_bytes(archive[central + 16..central + 20].try_into().unwrap());
        assert_eq!(crc, crc32(b"{}\n"));
    }
}
