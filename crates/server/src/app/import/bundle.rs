//! Hardened ZIP and raw-byte bundle reading for external imports (Trello JSON, Notion ZIP).
//!
//! Treats every uploaded byte as untrusted: rejects path traversal (`..`, absolute paths,
//! backslashes, drive prefixes, NUL/control characters), symlinks and special Unix modes,
//! encrypted entries, excessive path depth or component length, excessive entry counts,
//! single-entry and total decompressed byte ceilings, per-entry and global compression
//! ratio violations (defeating spoofed headers, overlapping entries, and nested ZIP
//! amplification), excessive nested ZIP counts/bytes/depth, and FD leaks from empty
//! nested archives. Untrusted archive entry paths never touch `Path::join`.

use std::collections::HashSet;
use std::io::{self, Read as _, Write as _};
use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use super::archive::{
    Archive, SharedFile, check_declared_eocd_entries, count_central_headers, zip_error,
};
use super::summary::BundleLimits;
use crate::app::doomed::Doomed;
use crate::app::file_name::is_control_or_bidi;
use crate::auth::secret::new_secret;
use crate::error::{AppError, Reason};

/// Chunk size used during streaming decompression and bounded file reads.
const STREAM_CHUNK_BYTES: usize = 64 * 1024;

/// An opened ZIP archive paired with its actual backing file byte length and raw-read
/// byte counter so compression ratios never trust spoofable ZIP header metadata.
#[derive(Debug, Clone)]
struct OpenedArchive {
    archive: Archive,
    file_len: u64,
    bytes_read: Arc<AtomicU64>,
}

/// One validated file entry inside a [`SafeBundle`].
#[derive(Debug, Clone)]
pub(crate) struct BundleEntry {
    /// Normalized forward-slash relative path within the bundle.
    pub(crate) path: String,
    /// Index of the owning ZIP archive in [`SafeBundle::archives`].
    archive_index: usize,
    /// Index of the entry inside that ZIP archive.
    entry_index: usize,
    /// Verified uncompressed byte count measured while streaming.
    pub(crate) unpacked_bytes: u64,
}

/// A validated, read-only handle to a ZIP bundle and any safely unpacked nested ZIPs.
///
/// Temporary files holding nested ZIPs live under `_workdir` and are deleted on drop.
#[derive(Debug)]
pub(crate) struct SafeBundle {
    archives: Vec<OpenedArchive>,
    entries: Vec<BundleEntry>,
    limits: BundleLimits,
    _workdir: Option<Doomed>,
}

/// Shared accounting budget across outer and nested ZIP archives.
#[derive(Debug, Default)]
struct BundleBudget {
    entries: u64,
    total_unpacked: u64,
    outer_file_len: u64,
    nested_counter: usize,
    nested_zip_bytes: u64,
}

/// Classification of a validated raw ZIP entry path.
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum PathCheck {
    /// A regular relative file path (normalized with `/` separators).
    File(String),
    /// A directory entry (validated for safety, holds no file bytes).
    Directory,
    /// macOS archive metadata (`__MACOSX/...`, `.DS_Store`), validated for safety then ignored.
    MetadataIgnored,
}

impl SafeBundle {
    /// Opens, scans, and validates a ZIP bundle at `path` off the async runtime.
    ///
    /// Nested `.zip` archives are unpacked into sequentially named files inside a
    /// temporary directory under `temp_root` (up to `limits.max_nested_zip_depth`,
    /// `limits.max_nested_zips`, and `limits.max_total_nested_zip_bytes`) and validated
    /// under the same global entry, decompressed-byte, and compression-ratio budget.
    ///
    /// # Errors
    ///
    /// Returns a 400 error for invalid ZIPs, path traversal, symlinks/special modes,
    /// encrypted entries, excessive path depth/name length, or excessive nested ZIP depth;
    /// and a 413 error for entry count, nested ZIP count/size, entry size, total unpacked
    /// size, or per-entry/global compression ratio violations.
    pub(crate) async fn open(
        path: &Path,
        temp_root: &Path,
        limits: BundleLimits,
    ) -> Result<Self, AppError> {
        let path = path.to_owned();
        let temp_root = temp_root.to_owned();
        tokio::task::spawn_blocking(move || open_blocking(&path, &temp_root, limits))
            .await
            .map_err(|error| AppError::internal(format_args!("bundle scan task failed: {error}")))?
    }

    /// All validated regular file entries in deterministic bundle order.
    #[must_use]
    pub(crate) fn entries(&self) -> &[BundleEntry] {
        &self.entries
    }

    /// Reads a text entry (`.md`, `.csv`, `.json`) into memory up to `limit` bytes,
    /// rejecting invalid UTF-8 and U+0000 characters.
    ///
    /// # Errors
    ///
    /// Returns a 413 error if the entry exceeds `limit` or compression ratio bounds, and
    /// a 400 error if the entry cannot be decompressed, is not UTF-8, or holds U+0000.
    pub(crate) async fn read_text(
        &self,
        entry: &BundleEntry,
        limit: u64,
    ) -> Result<String, AppError> {
        let mut opened = self.archives[entry.archive_index].clone();
        let entry_index = entry.entry_index;
        let limits = self.limits;
        tokio::task::spawn_blocking(move || {
            let mut bytes = Vec::new();
            let mut budget = BundleBudget::default();
            stream_decompress(
                &mut opened,
                entry_index,
                limit,
                &limits,
                &mut budget,
                &mut bytes,
            )?;
            let text = String::from_utf8(bytes).map_err(|error| {
                AppError::bad_request(format_args!(
                    "archive text entry is not valid UTF-8: {error}"
                ))
            })?;
            if text.contains('\0') {
                return Err(AppError::bad_request(
                    "archive text entry contains U+0000 character",
                ));
            }
            Ok(text)
        })
        .await
        .map_err(|error| AppError::internal(format_args!("bundle read task failed: {error}")))?
    }

    /// Streams a validated entry directly to a file at `dest` without buffering it in RAM.
    ///
    /// # Errors
    ///
    /// Returns a 413 error if the entry exceeds `limit` or compression ratio bounds, a 400
    /// error if decompression fails, or a 500 error if writing `dest` fails.
    pub(crate) async fn unpack_to_file(
        &self,
        entry: &BundleEntry,
        limit: u64,
        dest: &Path,
    ) -> Result<u64, AppError> {
        let mut opened = self.archives[entry.archive_index].clone();
        let entry_index = entry.entry_index;
        let limits = self.limits;
        let dest = dest.to_owned();
        tokio::task::spawn_blocking(move || {
            let file = std::fs::File::create(&dest).map_err(|error| {
                AppError::internal(format_args!(
                    "cannot create temporary file for entry: {error}"
                ))
            })?;
            let mut out = io::BufWriter::new(file);
            let mut budget = BundleBudget::default();
            let written = stream_decompress(
                &mut opened,
                entry_index,
                limit,
                &limits,
                &mut budget,
                &mut out,
            )?;
            out.flush().map_err(|error| {
                AppError::internal(format_args!(
                    "cannot flush temporary file for entry: {error}"
                ))
            })?;
            Ok(written)
        })
        .await
        .map_err(|error| AppError::internal(format_args!("bundle unpack task failed: {error}")))?
    }
}

/// Shared mutable state threaded through recursive ZIP scanning.
struct ScanState {
    budget: BundleBudget,
    archives: Vec<OpenedArchive>,
    entries: Vec<BundleEntry>,
    seen_paths: HashSet<String>,
    workdir: Option<Doomed>,
}

/// Blocking implementation of [`SafeBundle::open`].
fn open_blocking(
    path: &Path,
    temp_root: &Path,
    limits: BundleLimits,
) -> Result<SafeBundle, AppError> {
    let mut state = ScanState {
        budget: BundleBudget::default(),
        archives: Vec::new(),
        entries: Vec::new(),
        seen_paths: HashSet::new(),
        workdir: None,
    };

    let _ = scan_zip(path, "", 0, temp_root, &limits, &mut state)?;

    Ok(SafeBundle {
        archives: state.archives,
        entries: state.entries,
        limits,
        _workdir: state.workdir,
    })
}

/// Opens a ZIP at `path`, validates every entry against `limits` and `state.budget`, and
/// recursively scans nested `.zip` files up to `limits.max_nested_zip_depth`.
///
/// Returns whether this archive contributed at least one leaf [`BundleEntry`]; nested
/// archives that hold no leaf entries are dropped immediately to avoid holding open file
/// descriptors.
fn scan_zip(
    path: &Path,
    prefix: &str,
    depth: usize,
    temp_root: &Path,
    limits: &BundleLimits,
    state: &mut ScanState,
) -> Result<bool, AppError> {
    let remaining_entries = limits.max_entries.saturating_sub(state.budget.entries);
    let mut opened = open_zip_bounded(path, remaining_entries)?;
    if depth == 0 {
        state.budget.outer_file_len = opened.file_len;
        if opened.archive.is_empty() {
            return Err(AppError::bad_request("zip: archive has no entries"));
        }
    }
    let mut retained_index: Option<usize> = None;

    for entry_index in 0..opened.archive.len() {
        state.budget.entries = state.budget.entries.saturating_add(1);
        if state.budget.entries > limits.max_entries {
            return Err(too_many_bundle_entries(limits.max_entries));
        }
        let raw_name = validate_entry_metadata(&mut opened.archive, entry_index)?;
        let checked = validate_bundle_path(&raw_name, limits)?;
        let normalized = match checked {
            PathCheck::Directory => continue,
            PathCheck::MetadataIgnored => {
                // Still stream-check metadata entries against zip-bomb limits.
                stream_decompress(
                    &mut opened,
                    entry_index,
                    limits.max_text_entry_bytes,
                    limits,
                    &mut state.budget,
                    &mut io::sink(),
                )?;
                continue;
            }
            PathCheck::File(file) => join_bundle_prefix(prefix, &file, limits)?,
        };

        if is_zip_path(&normalized) {
            scan_nested_zip(
                &mut opened,
                entry_index,
                &normalized,
                depth,
                temp_root,
                limits,
                state,
            )?;
            continue;
        }

        let entry_limit = entry_byte_limit(&normalized, limits);
        let unpacked_bytes = stream_decompress(
            &mut opened,
            entry_index,
            entry_limit,
            limits,
            &mut state.budget,
            &mut io::sink(),
        )?;
        if state.seen_paths.insert(normalized.clone()) {
            let archive_index = *retained_index.get_or_insert_with(|| {
                let idx = state.archives.len();
                state.archives.push(opened.clone());
                idx
            });
            state.entries.push(BundleEntry {
                path: normalized,
                archive_index,
                entry_index,
                unpacked_bytes,
            });
        }
    }
    Ok(retained_index.is_some())
}

/// Unpacks a nested `.zip` entry into a generated safe filename inside `workdir` and scans it.
fn scan_nested_zip(
    opened: &mut OpenedArchive,
    entry_index: usize,
    normalized: &str,
    depth: usize,
    temp_root: &Path,
    limits: &BundleLimits,
    state: &mut ScanState,
) -> Result<(), AppError> {
    if depth >= limits.max_nested_zip_depth {
        return Err(AppError::bad_request(format_args!(
            "nested ZIP archive exceeds maximum depth of {}",
            limits.max_nested_zip_depth
        )));
    }
    if state.budget.nested_counter >= limits.max_nested_zips {
        return Err(AppError::too_large(
            format_args!(
                "archive contains more than {} nested ZIP files",
                limits.max_nested_zips
            ),
            u64::try_from(limits.max_nested_zips).unwrap_or(u64::MAX),
        )
        .with_reason(Reason::ArchiveTooLarge));
    }
    let dir = if let Some(existing) = &state.workdir {
        existing.path().to_path_buf()
    } else {
        let created = temp_root.join(format!("import-bundle-{}", new_secret()));
        std::fs::create_dir_all(&created).map_err(|error| {
            AppError::internal(format_args!("cannot create bundle work directory: {error}"))
        })?;
        state.workdir = Some(Doomed::directory(created.clone()));
        created
    };
    state.budget.nested_counter = state.budget.nested_counter.saturating_add(1);
    let safe_file = dir.join(format!("nested-{:04}.zip", state.budget.nested_counter));
    let remaining_nested_bytes = limits
        .max_total_nested_zip_bytes
        .saturating_sub(state.budget.nested_zip_bytes);
    let nested_entry_limit = limits.max_nested_zip_bytes.min(remaining_nested_bytes);
    let unpacked_zip_bytes = {
        let file = std::fs::File::create(&safe_file).map_err(|error| {
            AppError::internal(format_args!("cannot create nested ZIP temp file: {error}"))
        })?;
        let mut out = io::BufWriter::new(file);
        let written = stream_decompress(
            opened,
            entry_index,
            nested_entry_limit,
            limits,
            &mut state.budget,
            &mut out,
        )?;
        out.flush().map_err(|error| {
            AppError::internal(format_args!("cannot flush nested ZIP temp file: {error}"))
        })?;
        written
    };
    state.budget.nested_zip_bytes = state
        .budget
        .nested_zip_bytes
        .saturating_add(unpacked_zip_bytes);
    let nested_prefix = normalized.rsplit_once('/').map_or("", |(parent, _)| parent);
    let retained = scan_zip(
        &safe_file,
        nested_prefix,
        depth + 1,
        temp_root,
        limits,
        state,
    )?;
    if !retained {
        let _ = std::fs::remove_file(&safe_file);
    }
    Ok(())
}

/// Opens a ZIP archive after pre-counting central directory headers to stop entry-count bombs.
fn open_zip_bounded(path: &Path, max_entries: u64) -> Result<OpenedArchive, AppError> {
    let file = std::fs::File::open(path)
        .map_err(|error| AppError::internal(format_args!("cannot open import archive: {error}")))?;
    let file_len = file
        .metadata()
        .map_err(|error| AppError::internal(format_args!("cannot read import archive: {error}")))?
        .len();
    let mut shared = SharedFile::new(Arc::new(file), file_len);
    let headers = count_central_headers(&mut shared, max_entries)
        .map_err(|error| AppError::internal(format_args!("cannot scan import archive: {error}")))?;
    if headers > max_entries {
        return Err(too_many_bundle_entries(max_entries));
    }
    check_declared_eocd_entries(&mut shared, file_len, headers, max_entries)?;
    shared.rewind_start();
    let (counted_shared, bytes_read) = shared.with_fresh_counter();
    let archive = zip::ZipArchive::new(counted_shared).map_err(|error| zip_error(&error))?;
    if u64::try_from(archive.len())
        .ok()
        .is_none_or(|count| count > max_entries)
    {
        return Err(too_many_bundle_entries(max_entries));
    }
    Ok(OpenedArchive {
        archive,
        file_len,
        bytes_read,
    })
}

/// Checks entry header metadata (encryption, symlinks, special Unix file modes) and
/// returns the raw entry name.
fn validate_entry_metadata(archive: &mut Archive, index: usize) -> Result<String, AppError> {
    let entry = archive
        .by_index_raw(index)
        .map_err(|error| zip_error(&error))?;
    if entry.encrypted() {
        return Err(AppError::bad_request(
            "encrypted ZIP entries are not supported",
        ));
    }
    if entry.is_symlink() {
        return Err(AppError::bad_request(
            "symbolic links are not allowed in import archives",
        ));
    }
    if let Some(mode) = entry.unix_mode() {
        let file_type = mode & 0o170_000;
        // 0 = unspecified (DOS/Windows); 0o100_000 = regular file; 0o040_000 = directory.
        // Reject symlinks (0o120_000), FIFOs (0o010_000), devices (0o020_000 / 0o060_000),
        // and sockets (0o140_000).
        if file_type != 0 && file_type != 0o100_000 && file_type != 0o040_000 {
            return Err(AppError::bad_request(
                "special file entries are not allowed in import archives",
            ));
        }
    }
    let name = entry
        .name()
        .map_err(|_| AppError::bad_request("invalid ZIP entry name"))?;
    Ok(name.into_owned())
}

/// Validates a raw ZIP entry path against traversal, absolute paths, drive prefixes,
/// backslashes, control characters, depth, and name length limits.
///
/// # Errors
///
/// Returns a 400 error if the path violates any security or size constraint.
pub(crate) fn validate_bundle_path(
    raw: &str,
    limits: &BundleLimits,
) -> Result<PathCheck, AppError> {
    if raw.is_empty() {
        return Err(AppError::bad_request("archive entry has an empty name"));
    }
    if raw.len() > limits.max_path_bytes {
        return Err(AppError::bad_request(format_args!(
            "archive entry path exceeds {} bytes",
            limits.max_path_bytes
        )));
    }
    if raw.chars().any(is_control_or_bidi) {
        return Err(AppError::bad_request(
            "archive entry path contains forbidden control, bidirectional, or NUL characters",
        ));
    }
    if raw.contains('\\') {
        return Err(AppError::bad_request(
            "archive entry path must not contain backslashes",
        ));
    }
    if raw.starts_with('/') {
        return Err(AppError::bad_request(
            "absolute paths are not allowed in import archives",
        ));
    }
    if has_drive_prefix(raw) {
        return Err(AppError::bad_request(
            "drive prefixes are not allowed in import archives",
        ));
    }

    let is_dir = raw.ends_with('/');
    let trimmed = raw.trim_end_matches('/');
    let mut parts = Vec::new();
    for segment in trimmed.split('/') {
        match segment {
            "" => {
                if !trimmed.is_empty() {
                    return Err(AppError::bad_request(
                        "archive entry path contains empty path segments",
                    ));
                }
            }
            "." => {}
            ".." => {
                return Err(AppError::bad_request(
                    "path traversal (`..`) is not allowed in import archives",
                ));
            }
            other => {
                if has_drive_prefix(other) {
                    return Err(AppError::bad_request(
                        "drive prefixes are not allowed in import archives",
                    ));
                }
                if other.chars().count() > limits.max_entry_name_chars {
                    return Err(AppError::bad_request(format_args!(
                        "archive entry name exceeds {} characters",
                        limits.max_entry_name_chars
                    )));
                }
                parts.push(other);
            }
        }
    }
    if parts.len() > limits.max_path_depth {
        return Err(AppError::bad_request(format_args!(
            "archive entry path depth exceeds {} levels",
            limits.max_path_depth
        )));
    }
    if is_dir || parts.is_empty() {
        return Ok(PathCheck::Directory);
    }
    if is_macos_metadata(&parts) {
        return Ok(PathCheck::MetadataIgnored);
    }
    Ok(PathCheck::File(parts.join("/")))
}

/// Joins a nested ZIP entry's validated relative path onto its enclosing directory prefix,
/// re-checking combined path length and depth bounds.
fn join_bundle_prefix(prefix: &str, path: &str, limits: &BundleLimits) -> Result<String, AppError> {
    if prefix.is_empty() {
        return Ok(path.to_owned());
    }
    let joined = format!("{prefix}/{path}");
    if joined.len() > limits.max_path_bytes {
        return Err(AppError::bad_request(format_args!(
            "nested archive path exceeds {} bytes",
            limits.max_path_bytes
        )));
    }
    let depth = joined.split('/').count();
    if depth > limits.max_path_depth {
        return Err(AppError::bad_request(format_args!(
            "nested archive path depth exceeds {} levels",
            limits.max_path_depth
        )));
    }
    Ok(joined)
}

/// Streaming decompression enforcing per-entry byte limit, global decompressed byte budget,
/// per-entry actual-bytes-read compression ratio, and overall outer-file compression ratio.
/// Never trusts ZIP header `size()` or `compressed_size()`.
fn stream_decompress(
    opened: &mut OpenedArchive,
    index: usize,
    entry_limit: u64,
    limits: &BundleLimits,
    budget: &mut BundleBudget,
    out: &mut impl io::Write,
) -> Result<u64, AppError> {
    let raw_before = opened.bytes_read.load(Ordering::Relaxed);
    let mut entry = opened
        .archive
        .by_index(index)
        .map_err(|error| zip_error(&error))?;
    let header_compressed = entry.compressed_size().min(opened.file_len);
    let mut buf = vec![0_u8; STREAM_CHUNK_BYTES];
    let mut entry_read: u64 = 0;
    loop {
        let count = match entry.read(&mut buf) {
            Ok(0) => break,
            Ok(count) => count,
            Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
            Err(error) => {
                return Err(AppError::bad_request(format_args!(
                    "cannot read archive entry: {error}"
                )));
            }
        };
        let count_u64 = u64::try_from(count).unwrap_or(u64::MAX);
        entry_read = entry_read.saturating_add(count_u64);
        if entry_read > entry_limit {
            return Err(AppError::too_large(
                format_args!("archive entry unpacks to more than {entry_limit} bytes"),
                entry_limit,
            )
            .with_reason(Reason::ArchiveEntryTooLarge));
        }
        budget.total_unpacked = budget.total_unpacked.saturating_add(count_u64);
        if budget.total_unpacked > limits.max_total_unpacked_bytes {
            return Err(AppError::too_large(
                format_args!(
                    "archive entries unpack to more than {} bytes",
                    limits.max_total_unpacked_bytes
                ),
                limits.max_total_unpacked_bytes,
            )
            .with_reason(Reason::ArchiveTooLarge));
        }
        let raw_read = opened
            .bytes_read
            .load(Ordering::Relaxed)
            .saturating_sub(raw_before);
        check_compression_ratios(entry_read, header_compressed, raw_read, budget, limits)?;
        out.write_all(&buf[..count]).map_err(|error| {
            AppError::internal(format_args!("cannot write unpacked archive entry: {error}"))
        })?;
    }
    Ok(entry_read)
}

/// Checks both per-entry ratio (against the minimum of header and actual raw bytes read)
/// and overall archive ratio (against `budget.outer_file_len`, defeating overlapping and
/// multi-layer nested ZIP bombs).
fn check_compression_ratios(
    entry_read: u64,
    header_compressed: u64,
    raw_read: u64,
    budget: &BundleBudget,
    limits: &BundleLimits,
) -> Result<(), AppError> {
    if entry_read > limits.min_ratio_check_bytes {
        let effective_compressed = if raw_read > 0 {
            header_compressed.min(raw_read)
        } else {
            header_compressed
        };
        let max_by_ratio = effective_compressed
            .max(1)
            .saturating_mul(limits.max_compression_ratio);
        if entry_read > max_by_ratio {
            return Err(AppError::too_large(
                format_args!(
                    "archive entry exceeds maximum compression ratio ({}:1)",
                    limits.max_compression_ratio
                ),
                limits.max_compression_ratio,
            )
            .with_reason(Reason::ArchiveEntryTooLarge));
        }
    }
    if budget.outer_file_len > 0 && budget.total_unpacked > limits.min_ratio_check_bytes {
        let max_global = budget
            .outer_file_len
            .max(1)
            .saturating_mul(limits.max_compression_ratio);
        if budget.total_unpacked > max_global {
            return Err(AppError::too_large(
                format_args!(
                    "archive exceeds maximum overall compression ratio ({}:1)",
                    limits.max_compression_ratio
                ),
                limits.max_compression_ratio,
            )
            .with_reason(Reason::ArchiveTooLarge));
        }
    }
    Ok(())
}

/// Reads a standalone file (such as Trello JSON) into memory up to `limit` bytes.
///
/// # Errors
///
/// Returns 413 (`Reason::ArchiveTooLarge`) if the file is larger than `limit`, or 500 on
/// I/O failure.
pub(crate) async fn read_bounded_file(path: &Path, limit: u64) -> Result<Vec<u8>, AppError> {
    let path = path.to_owned();
    tokio::task::spawn_blocking(move || {
        let mut file = std::fs::File::open(&path).map_err(|error| {
            AppError::internal(format_args!("cannot open import file: {error}"))
        })?;
        let mut out = Vec::new();
        let mut buf = vec![0_u8; STREAM_CHUNK_BYTES];
        let mut total: u64 = 0;
        loop {
            let read = match file.read(&mut buf) {
                Ok(0) => break,
                Ok(read) => read,
                Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
                Err(error) => {
                    return Err(AppError::internal(format_args!(
                        "cannot read import file: {error}"
                    )));
                }
            };
            total = total.saturating_add(u64::try_from(read).unwrap_or(u64::MAX));
            if total > limit {
                return Err(AppError::too_large(
                    format_args!("import file is larger than {limit} bytes"),
                    limit,
                )
                .with_reason(Reason::ArchiveTooLarge));
            }
            out.extend_from_slice(&buf[..read]);
        }
        Ok(out)
    })
    .await
    .map_err(|error| AppError::internal(format_args!("import file read task failed: {error}")))?
}

/// Verifies that a JSON byte buffer contains neither raw U+0000 bytes nor `\u0000`
/// escapes and nests `{`/`[` at most `max_depth` levels deep, before handing it to
/// `serde_json`.
///
/// # Errors
///
/// Returns a 400 error if a NUL byte / `\u0000` escape is present or nesting exceeds `max_depth`.
pub(crate) fn check_json_safety(bytes: &[u8], max_depth: usize) -> Result<(), AppError> {
    if bytes.contains(&0) {
        return Err(AppError::bad_request(
            "JSON input contains forbidden U+0000 byte",
        ));
    }
    let mut depth: usize = 0;
    let mut in_string = false;
    let mut escaped = false;
    for (idx, &byte) in bytes.iter().enumerate() {
        if in_string {
            if escaped {
                escaped = false;
                if byte == b'u' && bytes.get(idx + 1..idx + 5) == Some(b"0000") {
                    return Err(AppError::bad_request(
                        "JSON input contains forbidden \\u0000 escape",
                    ));
                }
            } else if byte == b'\\' {
                escaped = true;
            } else if byte == b'"' {
                in_string = false;
            }
            continue;
        }
        match byte {
            b'"' => in_string = true,
            b'{' | b'[' => {
                depth = depth.saturating_add(1);
                if depth > max_depth {
                    return Err(AppError::bad_request(format_args!(
                        "JSON nesting depth exceeds {max_depth} levels"
                    )));
                }
            }
            b'}' | b']' => {
                depth = depth.saturating_sub(1);
            }
            _ => {}
        }
    }
    Ok(())
}

/// Safely resolves a relative Markdown link or image target against `base_dir` within a
/// bundle. Returns `None` for external URLs, absolute paths, drive prefixes, backslashes,
/// NUL/control bytes, invalid percent-encoding, or any `..` escaping above the bundle root.
#[must_use]
pub(crate) fn resolve_relative_bundle_path(base_dir: &str, raw_target: &str) -> Option<String> {
    let target = raw_target
        .trim()
        .split(['#', '?'])
        .next()
        .unwrap_or_default();
    if target.is_empty()
        || target.starts_with('/')
        || target.contains('\\')
        || has_drive_prefix(target)
        || has_url_scheme(target)
    {
        return None;
    }
    let decoded = percent_decode_utf8(target)?;
    if decoded.is_empty()
        || decoded.starts_with('/')
        || decoded.contains('\\')
        || has_drive_prefix(&decoded)
        || decoded.chars().any(is_control_or_bidi)
    {
        return None;
    }

    let mut stack: Vec<&str> = if base_dir.is_empty() {
        Vec::new()
    } else {
        base_dir
            .split('/')
            .filter(|part| !part.is_empty())
            .collect()
    };
    for segment in decoded.split('/') {
        match segment {
            "" | "." => {}
            ".." => {
                stack.pop()?;
            }
            other => {
                if has_drive_prefix(other) {
                    return None;
                }
                stack.push(other);
            }
        }
    }
    (!stack.is_empty()).then(|| stack.join("/"))
}

/// Percent-decodes a UTF-8 URI path component. Returns `None` on malformed escapes or
/// non-UTF-8 byte sequences.
pub(crate) fn percent_decode_utf8(input: &str) -> Option<String> {
    let bytes = input.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut idx = 0;
    while idx < bytes.len() {
        if bytes[idx] == b'%' {
            let hi = hex_nibble(*bytes.get(idx + 1)?)?;
            let lo = hex_nibble(*bytes.get(idx + 2)?)?;
            out.push((hi << 4) | lo);
            idx += 3;
        } else {
            out.push(bytes[idx]);
            idx += 1;
        }
    }
    String::from_utf8(out).ok()
}

fn hex_nibble(byte: u8) -> Option<u8> {
    match byte {
        b'0'..=b'9' => Some(byte - b'0'),
        b'a'..=b'f' => Some(byte - b'a' + 10),
        b'A'..=b'F' => Some(byte - b'A' + 10),
        _ => None,
    }
}

fn has_drive_prefix(segment: &str) -> bool {
    let bytes = segment.as_bytes();
    bytes.len() >= 2 && bytes[0].is_ascii_alphabetic() && bytes[1] == b':'
}

/// Whether `text` begins with a URI scheme (`^[A-Za-z][A-Za-z0-9+.-]*:`).
pub(crate) fn has_url_scheme(text: &str) -> bool {
    let Some((scheme, _)) = text.split_once(':') else {
        return false;
    };
    let mut chars = scheme.chars();
    let Some(first) = chars.next() else {
        return false;
    };
    first.is_ascii_alphabetic()
        && chars.all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '+' | '.' | '-'))
}

fn is_macos_metadata(parts: &[&str]) -> bool {
    parts.first().is_some_and(|&first| first == "__MACOSX")
        || parts.last().is_some_and(|&last| last == ".DS_Store")
}

fn is_zip_path(path: &str) -> bool {
    Path::new(path)
        .extension()
        .is_some_and(|ext| ext.eq_ignore_ascii_case("zip"))
}

fn entry_byte_limit(path: &str, limits: &BundleLimits) -> u64 {
    let is_text = Path::new(path).extension().is_some_and(|ext| {
        ext.eq_ignore_ascii_case("md")
            || ext.eq_ignore_ascii_case("markdown")
            || ext.eq_ignore_ascii_case("csv")
            || ext.eq_ignore_ascii_case("json")
    });
    if is_text {
        limits.max_text_entry_bytes
    } else {
        limits.max_file_bytes
    }
}

fn too_many_bundle_entries(limit: u64) -> AppError {
    AppError::too_large(
        format_args!("import archive has more than {limit} entries"),
        limit,
    )
    .with_reason(Reason::ArchiveTooLarge)
}
