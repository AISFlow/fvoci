//! Member policy for the pinned SQLite amalgamation ZIP. ZIP structure,
//! decompression and CRC checks belong to the maintained `zip` crate; this
//! module only states which members are allowed. Two independent views from
//! the crate must agree: the central directory (`ZipArchive`) and the
//! sequential local headers (`read_zipfile_from_stream`). The second view
//! exists because `ZipArchive` keys entries by name, so a duplicated central
//! name would otherwise collapse silently.

use crate::sqlite::{archive_root, SIZE_LIMIT};
use std::collections::BTreeSet;
use std::io::{Cursor, Read};
use zip::read::read_zipfile_from_stream;
use zip::{CompressionMethod, ZipArchive};

const MEMBERS: [&str; 4] = ["sqlite3.c", "sqlite3.h", "sqlite3ext.h", "shell.c"];
const S_IFMT: u32 = 0o170_000;
const S_IFDIR: u32 = 0o040_000;
const S_IFREG: u32 = 0o100_000;

/// The two members the build consumes, read from the central directory view.
#[derive(Debug)]
pub struct Sources {
    pub c_source: Vec<u8>,
    pub header: Vec<u8>,
}

fn unexpected() -> String {
    "unexpected archive entries".to_owned()
}

fn unsafe_entry() -> String {
    "unsafe archive entry".to_owned()
}

fn bad_zip(error: impl std::fmt::Display) -> String {
    format!("bad zip archive: {error}")
}

fn read_bounded(reader: impl Read, declared: u64) -> Result<Vec<u8>, String> {
    let mut data = Vec::new();
    // Reading to EOF is what makes the crate compare the CRC-32.
    reader
        .take(SIZE_LIMIT + 1)
        .read_to_end(&mut data)
        .map_err(bad_zip)?;
    if data.len() as u64 != declared || declared > SIZE_LIMIT {
        return Err(unsafe_entry());
    }
    Ok(data)
}

fn u16_at(data: &[u8], offset: u64) -> Option<u64> {
    let offset = usize::try_from(offset).ok()?;
    let bytes = data.get(offset..offset.checked_add(2)?)?;
    Some(u64::from(u16::from_le_bytes([bytes[0], bytes[1]])))
}

fn u32_at(data: &[u8], offset: u64) -> Option<u64> {
    let offset = usize::try_from(offset).ok()?;
    let bytes = data.get(offset..offset.checked_add(4)?)?;
    Some(u64::from(u32::from_le_bytes([
        bytes[0], bytes[1], bytes[2], bytes[3],
    ])))
}

/// Layout rule on top of the crate's parse. The pinned archive is a plain
/// single-disk ZIP whose last 22 bytes are a comment-free classic EOCD, so
/// require exactly that, and require the central records the crate located to
/// tile the declared central directory with no gap or trailing bytes. Each
/// record's "version needed" low byte stays at most 63 (what Python zipfile
/// can extract). This reads only fixed EOCD fields and the three length fields
/// of records whose offsets the crate already returned; it is not a second
/// central directory parser.
fn check_layout(
    data: &[u8],
    directory_start: u64,
    mut record_starts: Vec<u64>,
    count: usize,
) -> Result<(), String> {
    let layout = || "unexpected archive layout".to_owned();
    let eocd = (data.len() as u64).checked_sub(22).ok_or_else(layout)?;
    let field = |offset: u64| u16_at(data, eocd + offset).ok_or_else(layout);
    if u32_at(data, eocd) != Some(0x0605_4b50)
        || field(4)? != 0
        || field(6)? != 0
        || field(8)? != count as u64
        || field(10)? != count as u64
        || field(20)? != 0
    {
        return Err(layout());
    }
    let size = u32_at(data, eocd + 12).ok_or_else(layout)?;
    let offset = u32_at(data, eocd + 16).ok_or_else(layout)?;
    if offset != directory_start || offset.checked_add(size) != Some(eocd) {
        return Err(layout());
    }
    record_starts.sort_unstable();
    let mut next = directory_start;
    for start in record_starts {
        if start != next || u32_at(data, start) != Some(0x0201_4b50) {
            return Err(layout());
        }
        let version_needed = u16_at(data, start + 6).ok_or_else(layout)?;
        if version_needed & 0xff > 63 {
            return Err(layout());
        }
        let variable = [28, 30, 32]
            .iter()
            .map(|o| u16_at(data, start + o).ok_or_else(layout))
            .sum::<Result<u64, String>>()?;
        next = start + 46 + variable;
    }
    if next != eocd {
        return Err(layout());
    }
    Ok(())
}

struct Entry {
    name: String,
    header_start: u64,
    data: Vec<u8>,
}

/// Authenticate the exact member set and return sqlite3.c and sqlite3.h.
/// Nothing is written anywhere; callers write only after every check passes.
pub fn read_sources(archive: &[u8]) -> Result<Sources, String> {
    let root = archive_root();
    let expected: BTreeSet<String> = std::iter::once(root.clone())
        .chain(MEMBERS.iter().map(|name| format!("{root}{name}")))
        .collect();

    let mut zip = ZipArchive::new(Cursor::new(archive)).map_err(bad_zip)?;
    if zip.len() != expected.len() || zip.offset() != 0 {
        return Err(unexpected());
    }
    let mut central = Vec::with_capacity(expected.len());
    for index in 0..zip.len() {
        let file = zip.by_index_raw(index).map_err(bad_zip)?;
        let name = std::str::from_utf8(file.name_raw())
            .map_err(|_| unexpected())?
            .to_owned();
        if !expected.contains(&name) {
            return Err(unexpected());
        }
        let mode = file.unix_mode().ok_or_else(unsafe_entry)?;
        let kind = mode & S_IFMT;
        let directory = name == root;
        let kind_ok = if directory {
            file.is_dir() && (kind == 0 || kind == S_IFDIR) && file.size() == 0
        } else {
            !file.is_dir() && (kind == 0 || kind == S_IFREG)
        };
        if !kind_ok
            || file.is_symlink()
            || file.encrypted()
            || file.size() > SIZE_LIMIT
            || file.compressed_size() > SIZE_LIMIT
            || !matches!(
                file.compression(),
                CompressionMethod::Stored | CompressionMethod::Deflated
            )
        {
            return Err(unsafe_entry());
        }
        central.push((name, file.header_start(), file.size()));
    }
    let names: BTreeSet<&String> = central.iter().map(|(name, ..)| name).collect();
    let starts: BTreeSet<u64> = central.iter().map(|(_, start, _)| *start).collect();
    if names.len() != expected.len() || starts.len() != expected.len() {
        return Err(unexpected());
    }
    let mut central_starts = Vec::with_capacity(expected.len());
    for index in 0..zip.len() {
        central_starts.push(
            zip.by_index_raw(index)
                .map_err(bad_zip)?
                .central_header_start(),
        );
    }
    check_layout(
        archive,
        zip.central_directory_start(),
        central_starts,
        expected.len(),
    )?;

    let mut central_data = Vec::with_capacity(central.len());
    for (index, (name, header_start, size)) in central.into_iter().enumerate() {
        let file = zip.by_index(index).map_err(bad_zip)?;
        central_data.push(Entry {
            name,
            header_start,
            data: read_bounded(file, size)?,
        });
    }

    // Local view: entries must start at offset 0, follow each other directly,
    // and end exactly where the central directory starts.
    let mut cursor = Cursor::new(archive);
    let mut local = Vec::new();
    loop {
        let start = cursor.position();
        let Some(file) = read_zipfile_from_stream(&mut cursor).map_err(bad_zip)? else {
            break;
        };
        if local.len() == expected.len() {
            return Err(unexpected());
        }
        let name = String::from_utf8(file.name_raw().to_vec()).map_err(|_| unexpected())?;
        if file.encrypted() {
            return Err(unsafe_entry());
        }
        let size = file.size();
        local.push(Entry {
            name,
            header_start: start,
            data: read_bounded(file, size)?,
        });
    }
    // The stream reader consumed the central directory signature.
    if cursor.position().checked_sub(4) != Some(zip.central_directory_start()) {
        return Err(unexpected());
    }
    central_data.sort_by_key(|entry| entry.header_start);
    let agree = local.len() == central_data.len()
        && local
            .iter()
            .zip(&central_data)
            .all(|(l, c)| l.name == c.name && l.header_start == c.header_start && l.data == c.data);
    if !agree {
        return Err(unexpected());
    }

    let take = |member: &str| {
        let wanted = format!("{root}{member}");
        central_data
            .iter()
            .find(|entry| entry.name == wanted)
            .map(|entry| entry.data.clone())
            .ok_or_else(unexpected)
    };
    Ok(Sources {
        c_source: take("sqlite3.c")?,
        header: take("sqlite3.h")?,
    })
}
