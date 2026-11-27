//! Reading the zip archive of an import: an archive opened once and read at positions by
//! every task that unpacks an entry, its entries counted and named before anything is
//! read, each read no larger than its limit.

use std::borrow::Cow;
use std::io::{self, Read, Seek, SeekFrom, Write as _};
use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use bytes::Bytes;

use super::{MAX_ENTRIES, MAX_JSONL_BYTES, MAX_VERSION_BYTES};
use crate::error::{AppError, Reason};

/// The signature each entry of a ZIP's central directory begins with (APPNOTE 4.3.12).
pub(super) const CENTRAL_HEADER: &[u8; 4] = b"PK\x01\x02";

/// End of Central Directory signature (APPNOTE 4.3.16, 22 bytes + up to 65,535-byte comment).
const EOCD_SIGNATURE: &[u8; 4] = b"PK\x05\x06";

/// Zip64 End of Central Directory Locator signature (APPNOTE 4.3.15, 20 bytes).
const ZIP64_LOCATOR_SIGNATURE: &[u8; 4] = b"PK\x06\x07";

/// Zip64 End of Central Directory Record signature (APPNOTE 4.3.14, >= 56 bytes).
const ZIP64_EOCD_SIGNATURE: &[u8; 4] = b"PK\x06\x06";

/// Maximum tail window scanned to locate the EOCD and optional 20-byte Zip64 locator.
const MAX_EOCD_TAIL_BYTES: u64 = 22 + 65_535 + 20;

/// How much of an archive is read at a time to count its central headers.
pub(super) const SCAN_CHUNK_BYTES: usize = 1024 * 1024;

/// The archive as it is read while it is imported. Cloning it copies no content.
pub(super) type Archive = zip::ZipArchive<SharedFile>;

/// A file read at positions of its own: each clone has its own position over the one
/// open file, so that the archive can be cloned into each task that reads an entry.
#[derive(Debug, Clone)]
pub(super) struct SharedFile {
    file: Arc<std::fs::File>,
    len: u64,
    position: u64,
    /// Cumulative raw bytes read through this handle (and its clones), used by the
    /// external bundle reader to verify actual compressed bytes read rather than trusting
    /// spoofable ZIP `compressed_size` headers.
    bytes_read: Arc<AtomicU64>,
}

impl SharedFile {
    /// Creates a position-independent reader over `file` of `len` bytes starting at offset 0.
    pub(super) fn new(file: Arc<std::fs::File>, len: u64) -> Self {
        Self {
            file,
            len,
            position: 0,
            bytes_read: Arc::new(AtomicU64::new(0)),
        }
    }

    /// Resets this handle's read position back to the start of the file.
    pub(super) fn rewind_start(&mut self) {
        self.position = 0;
    }

    /// Returns an independent clone with a fresh per-reader byte counter sharing the
    /// underlying open file descriptor.
    pub(super) fn with_fresh_counter(&self) -> (Self, Arc<AtomicU64>) {
        let counter = Arc::new(AtomicU64::new(0));
        let cloned = Self {
            file: Arc::clone(&self.file),
            len: self.len,
            position: self.position,
            bytes_read: Arc::clone(&counter),
        };
        (cloned, counter)
    }
}

impl Read for SharedFile {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        #[cfg(unix)]
        let read = std::os::unix::fs::FileExt::read_at(&*self.file, buf, self.position)?;
        #[cfg(windows)]
        let read = std::os::windows::fs::FileExt::seek_read(&*self.file, buf, self.position)?;
        #[cfg(not(any(unix, windows)))]
        compile_error!("an archive is read at positions, which unix and windows have");
        let read_u64 = u64::try_from(read).unwrap_or(u64::MAX);
        self.position = self.position.saturating_add(read_u64);
        self.bytes_read.fetch_add(read_u64, Ordering::Relaxed);
        Ok(read)
    }
}

impl Seek for SharedFile {
    fn seek(&mut self, to: SeekFrom) -> io::Result<u64> {
        let position = match to {
            SeekFrom::Start(offset) => Some(offset),
            SeekFrom::End(offset) => self.len.checked_add_signed(offset),
            SeekFrom::Current(offset) => self.position.checked_add_signed(offset),
        };
        self.position = position.ok_or_else(|| {
            io::Error::new(io::ErrorKind::InvalidInput, "a seek before the start")
        })?;
        Ok(self.position)
    }
}

/// Opens the archive in the file at `path` and reads its directory, off the runtime's
/// workers.
pub(super) async fn open_archive(path: &Path) -> Result<Archive, AppError> {
    let path = path.to_owned();
    tokio::task::spawn_blocking(move || {
        let file = std::fs::File::open(&path).map_err(|error| {
            AppError::internal(format_args!("cannot open the archive: {error}"))
        })?;
        let len = file
            .metadata()
            .map_err(|error| AppError::internal(format_args!("cannot read the archive: {error}")))?
            .len();
        let mut file = SharedFile::new(Arc::new(file), len);
        // Counted before the directory is read, as reading it holds every entry in memory.
        // Every entry the reader keeps begins with the central header's signature, so it
        // keeps at most as many entries as signatures, whatever the end records say. The
        // capacity it reserves first follows the declared count, which the crate bounds
        // only by the file's length (about a 47th of it, as entries): that reservation is
        // not bounded here.
        let headers = count_central_headers(&mut file, MAX_ENTRIES).map_err(|error| {
            AppError::internal(format_args!("cannot read the archive: {error}"))
        })?;
        if headers > MAX_ENTRIES {
            return Err(too_many_entries());
        }
        check_declared_eocd_entries(&mut file, len, headers, MAX_ENTRIES)?;
        file.position = 0;
        zip::ZipArchive::new(file).map_err(|error| zip_error(&error))
    })
    .await
    .map_err(|error| AppError::internal(format_args!("archive task failed: {error}")))?
}

/// Maps a [`zip::result::ZipError`] to a sanitized client-facing 400 error without
/// leaking internal parser messages.
pub(super) fn zip_error(error: &zip::result::ZipError) -> AppError {
    use zip::result::ZipError;
    let detail = match error {
        ZipError::UnsupportedArchive(_) => "zip: unsupported archive feature",
        ZipError::FileNotFound => "zip: archive entry not found",
        ZipError::InvalidPassword => "zip: encrypted archive entries are not supported",
        ZipError::Io(_) => "zip: archive could not be read",
        _ => "zip: invalid archive",
    };
    AppError::bad_request(detail)
}

/// Inspects the EOCD (and Zip64 EOCD if present) before `ZipArchive::new` runs so a
/// spoofed declared entry count cannot pre-allocate a huge central-directory `Vec` or
/// bypass `max_entries` (`H-5`).
pub(super) fn check_declared_eocd_entries(
    reader: &mut SharedFile,
    file_len: u64,
    counted_headers: u64,
    max_entries: u64,
) -> Result<(), AppError> {
    if file_len < 22 {
        return Err(AppError::bad_request("zip: invalid archive"));
    }
    let tail_len = usize::try_from(file_len.min(MAX_EOCD_TAIL_BYTES)).unwrap_or(0);
    let tail_start = file_len - u64::try_from(tail_len).unwrap_or(0);
    reader
        .seek(SeekFrom::Start(tail_start))
        .map_err(|error| AppError::internal(format_args!("cannot seek the archive: {error}")))?;
    let mut tail = vec![0_u8; tail_len];
    reader
        .read_exact(&mut tail)
        .map_err(|error| AppError::internal(format_args!("cannot read the archive: {error}")))?;

    let eocd_pos = (0..=tail.len().saturating_sub(22))
        .rev()
        .find(|&pos| {
            &tail[pos..pos + 4] == EOCD_SIGNATURE && {
                let comment_len = usize::from(u16::from_le_bytes([tail[pos + 20], tail[pos + 21]]));
                pos + 22 + comment_len == tail.len()
            }
        })
        .ok_or_else(|| AppError::bad_request("zip: invalid archive"))?;

    let disk_16 = u16::from_le_bytes([tail[eocd_pos + 8], tail[eocd_pos + 9]]);
    let total_16 = u16::from_le_bytes([tail[eocd_pos + 10], tail[eocd_pos + 11]]);

    let declared =
        if eocd_pos >= 20 && &tail[eocd_pos - 20..eocd_pos - 16] == ZIP64_LOCATOR_SIGNATURE {
            let loc = &tail[eocd_pos - 20..eocd_pos];
            let z64_offset = u64::from_le_bytes([
                loc[8], loc[9], loc[10], loc[11], loc[12], loc[13], loc[14], loc[15],
            ]);
            if z64_offset.checked_add(56).is_none_or(|end| end > file_len) {
                return Err(AppError::bad_request("zip: invalid archive"));
            }
            reader.seek(SeekFrom::Start(z64_offset)).map_err(|error| {
                AppError::internal(format_args!("cannot seek the archive: {error}"))
            })?;
            let mut z64 = [0_u8; 56];
            reader.read_exact(&mut z64).map_err(|error| {
                AppError::internal(format_args!("cannot read the archive: {error}"))
            })?;
            if &z64[0..4] != ZIP64_EOCD_SIGNATURE {
                return Err(AppError::bad_request("zip: invalid archive"));
            }
            let disk_64 = u64::from_le_bytes([
                z64[24], z64[25], z64[26], z64[27], z64[28], z64[29], z64[30], z64[31],
            ]);
            let total_64 = u64::from_le_bytes([
                z64[32], z64[33], z64[34], z64[35], z64[36], z64[37], z64[38], z64[39],
            ]);
            let mut max_decl = disk_64.max(total_64);
            if disk_16 != u16::MAX {
                max_decl = max_decl.max(u64::from(disk_16));
            }
            if total_16 != u16::MAX {
                max_decl = max_decl.max(u64::from(total_16));
            }
            max_decl
        } else {
            u64::from(disk_16).max(u64::from(total_16))
        };

    if declared > max_entries {
        return Err(AppError::too_large(
            format_args!("the archive has more than {max_entries} entries"),
            max_entries,
        )
        .with_reason(Reason::ArchiveTooLarge));
    }
    if declared > counted_headers {
        return Err(AppError::bad_request(
            "zip: central directory entry count mismatch",
        ));
    }
    Ok(())
}

/// The refusal of an archive of more than [`MAX_ENTRIES`] entries.
pub(super) fn too_many_entries() -> AppError {
    AppError::too_large(
        format_args!("the archive has more than {MAX_ENTRIES} entries"),
        MAX_ENTRIES,
    )
}

/// Counts the central headers' signatures in what `reader` gives, up to the first count
/// over `limit`, where it stops reading.
pub(super) fn count_central_headers(reader: &mut impl Read, limit: u64) -> io::Result<u64> {
    let mut chunk = vec![0; SCAN_CHUNK_BYTES];
    // The last bytes of the chunk before: a signature may span two chunks.
    let mut carried = 0;
    let mut count: u64 = 0;
    loop {
        let read = match reader.read(chunk.get_mut(carried..).unwrap_or_default()) {
            Ok(0) => return Ok(count),
            Ok(read) => read,
            Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
            Err(error) => return Err(error),
        };
        let filled = chunk.get(..carried + read).unwrap_or_default();
        let found = filled
            .windows(CENTRAL_HEADER.len())
            .filter(|window| window == CENTRAL_HEADER)
            .count();
        count = count.saturating_add(u64::try_from(found).unwrap_or(u64::MAX));
        if count > limit {
            return Ok(count);
        }
        // Three bytes are kept: four would count a signature at the end twice.
        let keep = filled.len().min(CENTRAL_HEADER.len() - 1);
        let start = filled.len() - keep;
        chunk.copy_within(start..start + keep, 0);
        carried = keep;
    }
}

/// The most an entry may unpack to: a `container.jsonl`, the archive's `version.json`
/// (the one at its root), or a file, which is held to the limit of an upload.
pub(super) fn entry_limit(dir: &str, filename: &str, largest_file: u64) -> u64 {
    match filename {
        "container.jsonl" => MAX_JSONL_BYTES,
        "version.json" if dir == "." => MAX_VERSION_BYTES,
        _ => largest_file,
    }
}

/// Returns the cleaned directory and the base file name of an archive entry, without
/// reading its content.
pub(super) fn entry_name(zip: &Archive, index: usize) -> Result<(String, String), AppError> {
    let name = zip
        .name_for_index(index)
        .ok_or_else(|| AppError::bad_request("invalid archive entry"))?
        .map_err(|_| AppError::bad_request("invalid archive entry name"))?;
    Ok(split_entry_name(&name))
}

/// Reads the content of an archive entry, at most `limit` bytes of it. Decompression is
/// done off the runtime's workers, which it would hold up.
pub(super) async fn read_entry(zip: &Archive, index: usize, limit: u64) -> Result<Bytes, AppError> {
    let mut zip = zip.clone();
    tokio::task::spawn_blocking(move || {
        let mut bytes = Vec::new();
        decompress(&mut zip, index, limit, &mut bytes)?;
        Ok(Bytes::from(bytes))
    })
    .await
    .map_err(|error| AppError::internal(format_args!("archive task failed: {error}")))?
}

/// Unpacks an archive entry, at most `limit` bytes of it, into the file `to`, off the
/// runtime's workers.
pub(super) async fn unpack_entry(
    zip: &Archive,
    index: usize,
    limit: u64,
    to: &Path,
) -> Result<(), AppError> {
    let mut zip = zip.clone();
    let to = to.to_owned();
    tokio::task::spawn_blocking(move || {
        let file = std::fs::File::create(&to).map_err(|error| {
            AppError::internal(format_args!("cannot unpack an archive entry: {error}"))
        })?;
        let mut out = io::BufWriter::new(file);
        decompress(&mut zip, index, limit, &mut out)?;
        out.flush().map_err(|error| {
            AppError::internal(format_args!("cannot unpack an archive entry: {error}"))
        })
    })
    .await
    .map_err(|error| AppError::internal(format_args!("archive task failed: {error}")))?
}

/// Reads an archive entry through without keeping it, as [`read_entry`] would read it:
/// its checksum and its bound are checked. Returns how much it unpacks to.
pub(super) async fn check_entry(zip: &Archive, index: usize, limit: u64) -> Result<u64, AppError> {
    let mut zip = zip.clone();
    tokio::task::spawn_blocking(move || decompress(&mut zip, index, limit, &mut io::sink()))
        .await
        .map_err(|error| AppError::internal(format_args!("archive task failed: {error}")))?
}

/// Decompresses an archive entry into `out`, at most `limit` bytes of it, and returns
/// how many. The reader checks the entry's checksum when it reaches the end.
pub(super) fn decompress(
    zip: &mut Archive,
    index: usize,
    limit: u64,
    out: &mut impl io::Write,
) -> Result<u64, AppError> {
    let mut entry = zip.by_index(index).map_err(|error| zip_error(&error))?;
    let name = entry.name().map(Cow::into_owned).unwrap_or_default();
    let read = io::copy(&mut entry.by_ref().take(limit.saturating_add(1)), out)
        .map_err(|_| AppError::bad_request("cannot read archive entry"))?;
    if read > limit {
        return Err(AppError::too_large(
            format_args!("the archive entry {name} unpacks to more than {limit} bytes"),
            limit,
        )
        .with_reason(Reason::ArchiveEntryTooLarge));
    }
    Ok(read)
}

/// Splits an archive entry name into its normalized directory and its base file name.
/// The directory is only ever a map key, and the file name never has a path separator,
/// so a `..` segment in the name cannot escape the storage root.
pub(super) fn split_entry_name(name: &str) -> (String, String) {
    match name.rsplit_once('/') {
        Some((dir, filename)) => (clean_dir(dir), filename.to_owned()),
        None => (".".to_owned(), name.to_owned()),
    }
}

/// Normalises a directory path lexically: drops empty and `.` segments, and resolves
/// `..` against the segment before it (keeping a leading `..` that has none). The
/// result is used only as a map key.
pub(super) fn clean_dir(dir: &str) -> String {
    let mut parts: Vec<&str> = Vec::new();
    for segment in dir.split('/') {
        match segment {
            "" | "." => {}
            ".." => {
                if matches!(parts.last(), Some(&last) if last != "..") {
                    parts.pop();
                } else {
                    parts.push("..");
                }
            }
            other => parts.push(other),
        }
    }
    if parts.is_empty() {
        ".".to_owned()
    } else {
        parts.join("/")
    }
}
