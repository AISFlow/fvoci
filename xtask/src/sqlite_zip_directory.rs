//! Raw central-directory metadata only; no extraction or SQLite inventory policy.

const EOCD: &[u8; 4] = b"PK\x05\x06";
const ZIP64_EOCD: &[u8; 4] = b"PK\x06\x06";
const ZIP64_LOCATOR: &[u8; 4] = b"PK\x06\x07";
const CENTRAL: &[u8; 4] = b"PK\x01\x02";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DirectoryError {
    MissingEndRecord,
    InvalidSignature,
    Truncated,
    OutOfBounds,
    IntegerOverflow,
    CountMismatch,
    InvalidZip64,
    MultiDiskZip64,
    UnsupportedVersion,
}

#[derive(Debug, PartialEq, Eq)]
pub struct Entry<'a> {
    pub name_raw: &'a [u8],
    pub flags: u16,
    pub extra: &'a [u8],
    pub uncompressed_size: u64,
    pub external_attributes: u32,
    /// Absolute offset of the local header in `input` (prepend included).
    pub local_header_offset: u64,
    pub compression_method: u16,
    pub compressed_size: u64,
    /// Central-directory "version needed to extract" (offset 6), both bytes.
    /// Rejection compares only the low byte with 63. Offset 7 is reserved.
    pub version_needed: u16,
}

#[derive(Debug, PartialEq, Eq)]
pub struct Directory<'a> {
    pub entries: Vec<Entry<'a>>,
    pub declared_entry_count: u64,
    pub entries_on_disk: u64,
    pub disk_number: u32,
    pub directory_disk: u32,
}

struct End {
    count: u64,
    on_disk: u64,
    disk: u32,
    directory_disk: u32,
    size: u64,
    offset: u64,
    position: u64,
}

type Result<T> = std::result::Result<T, DirectoryError>;

fn extent(input: &[u8], offset: u64, length: u64) -> Result<&[u8]> {
    let end = offset
        .checked_add(length)
        .ok_or(DirectoryError::IntegerOverflow)?;
    let start = usize::try_from(offset).map_err(|_| DirectoryError::OutOfBounds)?;
    let end = usize::try_from(end).map_err(|_| DirectoryError::OutOfBounds)?;
    input.get(start..end).ok_or(DirectoryError::OutOfBounds)
}

fn number<const N: usize>(input: &[u8], offset: u64) -> Result<[u8; N]> {
    let bytes = extent(input, offset, N as u64)?;
    let mut value = [0; N];
    value.copy_from_slice(bytes);
    Ok(value)
}

fn u16_at(input: &[u8], offset: u64) -> Result<u16> {
    Ok(u16::from_le_bytes(number(input, offset)?))
}

fn u32_at(input: &[u8], offset: u64) -> Result<u32> {
    Ok(u32::from_le_bytes(number(input, offset)?))
}

fn u64_at(input: &[u8], offset: u64) -> Result<u64> {
    Ok(u64::from_le_bytes(number(input, offset)?))
}

fn take<'a>(input: &mut &'a [u8], length: usize) -> Result<&'a [u8]> {
    let (value, rest) = input
        .split_at_checked(length)
        .ok_or(DirectoryError::Truncated)?;
    *input = rest;
    Ok(value)
}

fn end_record(input: &[u8]) -> Result<End> {
    // Match zipfile's no-comment fast path and bounded last-signature search.
    let position = if input.len() >= 22
        && input.get(input.len() - 22..input.len() - 18) == Some(EOCD.as_slice())
        && input.get(input.len() - 2..) == Some(&[0, 0])
    {
        input.len() - 22
    } else {
        let start = input.len().saturating_sub(22 + usize::from(u16::MAX));
        let found = input[start..]
            .windows(4)
            .rposition(|bytes| bytes == EOCD)
            .ok_or(DirectoryError::MissingEndRecord)?;
        start
            .checked_add(found)
            .ok_or(DirectoryError::IntegerOverflow)?
    };
    let position = u64::try_from(position).map_err(|_| DirectoryError::OutOfBounds)?;
    let header = extent(input, position, 22)?;
    let comment_start = position
        .checked_add(22)
        .ok_or(DirectoryError::IntegerOverflow)?;
    extent(input, comment_start, u64::from(u16_at(header, 20)?))?;
    let mut end = End {
        count: u64::from(u16_at(header, 10)?),
        on_disk: u64::from(u16_at(header, 8)?),
        disk: u32::from(u16_at(header, 4)?),
        directory_disk: u32::from(u16_at(header, 6)?),
        size: u64::from(u32_at(header, 12)?),
        offset: u64::from(u32_at(header, 16)?),
        position,
    };
    if let Some(locator_start) = position.checked_sub(20) {
        let locator = extent(input, locator_start, 20)?;
        if locator.get(..4) == Some(ZIP64_LOCATOR.as_slice()) {
            if u32_at(locator, 4)? != 0 || u32_at(locator, 16)? > 1 {
                return Err(DirectoryError::MultiDiskZip64);
            }
            let relative = u64_at(locator, 8)?;
            let latest_start = locator_start
                .checked_sub(56)
                .ok_or(DirectoryError::InvalidZip64)?;
            if relative > latest_start {
                return Err(DirectoryError::OutOfBounds);
            }
            let mut physical = relative;
            let mut record = extent(input, physical, 56)?;
            if record.get(..4) != Some(ZIP64_EOCD.as_slice()) && relative != latest_start {
                // zipfile also accepts prepended data without ZIP64 extensible data.
                physical = latest_start;
                record = extent(input, physical, 56)?;
            }
            if record.get(..4) != Some(ZIP64_EOCD.as_slice()) {
                return Err(DirectoryError::InvalidSignature);
            }
            let record_length = u64_at(record, 4)?
                .checked_add(12)
                .ok_or(DirectoryError::IntegerOverflow)?;
            if physical.checked_add(record_length) != Some(locator_start) {
                return Err(DirectoryError::InvalidZip64);
            }
            let count = u64_at(record, 32)?;
            if end.count != u64::from(u16::MAX) && end.count != count {
                return Err(DirectoryError::CountMismatch);
            }
            end = End {
                count,
                on_disk: u64_at(record, 24)?,
                disk: u32_at(record, 16)?,
                directory_disk: u32_at(record, 20)?,
                size: u64_at(record, 40)?,
                offset: u64_at(record, 48)?,
                position: physical,
            };
            if end.offset.checked_add(end.size) != Some(relative) {
                return Err(DirectoryError::InvalidZip64);
            }
        }
    }
    Ok(end)
}

fn entry_size_and_offset(header: &[u8], mut extra: &[u8]) -> Result<(u64, u64, u64)> {
    let mut uncompressed = u64::from(u32_at(header, 24)?);
    let mut compressed = u64::from(u32_at(header, 20)?);
    let mut offset = u64::from(u32_at(header, 42)?);
    let mut need_uncompressed = uncompressed == u64::from(u32::MAX);
    let mut need_compressed = compressed == u64::from(u32::MAX);
    let mut need_offset = offset == u64::from(u32::MAX);
    let mut saw_zip64 = false;
    while !extra.is_empty() {
        let tag = take(&mut extra, 4)?;
        let mut value = take(&mut extra, usize::from(u16_at(tag, 2)?))?;
        if u16_at(tag, 0)? == 1 {
            // Python reads a later ZIP64 block only while a sentinel remains.
            // A second block is rejected here even when that read would succeed.
            if saw_zip64 {
                return Err(DirectoryError::InvalidZip64);
            }
            saw_zip64 = true;
            if need_uncompressed {
                uncompressed = u64_at(take(&mut value, 8)?, 0)?;
                need_uncompressed = false;
            }
            if need_compressed {
                compressed = u64_at(take(&mut value, 8)?, 0)?;
                need_compressed = false;
            }
            if need_offset {
                offset = u64_at(take(&mut value, 8)?, 0)?;
                need_offset = false;
            }
        }
    }
    if need_uncompressed || need_compressed || need_offset {
        return Err(DirectoryError::InvalidZip64);
    }
    Ok((uncompressed, compressed, offset))
}

/// Return every central entry, or an error without a partial inventory.
/// Names and encoding metadata remain raw for the caller's separate policy.
pub fn parse_directory(input: &[u8]) -> Result<Directory<'_>> {
    let end = end_record(input)?;
    let declared_end = end
        .offset
        .checked_add(end.size)
        .ok_or(DirectoryError::IntegerOverflow)?;
    let prefix = end
        .position
        .checked_sub(declared_end)
        .ok_or(DirectoryError::OutOfBounds)?;
    let start = prefix
        .checked_add(end.offset)
        .ok_or(DirectoryError::IntegerOverflow)?;
    let mut central = extent(input, start, end.size)?;
    let mut entries = Vec::new();
    // The bounded bytes, never an untrusted count, drive allocation and progress.
    while !central.is_empty() {
        let header = take(&mut central, 46)?;
        if header.get(..4) != Some(CENTRAL.as_slice()) {
            return Err(DirectoryError::InvalidSignature);
        }
        let version_needed = u16_at(header, 6)?;
        // ZIP stores the spec version in the low byte. The high byte is reserved.
        if (version_needed & 0xff) > 63 {
            return Err(DirectoryError::UnsupportedVersion);
        }
        let compression_method = u16_at(header, 10)?;
        let name_raw = take(&mut central, usize::from(u16_at(header, 28)?))?;
        let extra = take(&mut central, usize::from(u16_at(header, 30)?))?;
        take(&mut central, usize::from(u16_at(header, 32)?))?;
        let (uncompressed_size, compressed_size, local_offset) =
            entry_size_and_offset(header, extra)?;
        let local_header_offset = prefix
            .checked_add(local_offset)
            .ok_or(DirectoryError::IntegerOverflow)?;
        if local_header_offset
            >= u64::try_from(input.len()).map_err(|_| DirectoryError::OutOfBounds)?
        {
            return Err(DirectoryError::OutOfBounds);
        }
        entries.push(Entry {
            name_raw,
            flags: u16_at(header, 8)?,
            extra,
            uncompressed_size,
            external_attributes: u32_at(header, 38)?,
            local_header_offset,
            compression_method,
            compressed_size,
            version_needed,
        });
    }
    if u64::try_from(entries.len()).map_err(|_| DirectoryError::IntegerOverflow)? != end.count {
        return Err(DirectoryError::CountMismatch);
    }
    Ok(Directory {
        entries,
        declared_entry_count: end.count,
        entries_on_disk: end.on_disk,
        disk_number: end.disk,
        directory_disk: end.directory_disk,
    })
}

/// Reads every central-directory field so the non-test binary keeps this leaf live.
pub(crate) fn read_fields_fingerprint(directory: &Directory<'_>) -> u64 {
    let mut mixed = directory
        .declared_entry_count
        .wrapping_add(directory.entries_on_disk)
        .wrapping_add(u64::from(directory.disk_number))
        .wrapping_add(u64::from(directory.directory_disk));
    for entry in &directory.entries {
        mixed = mixed
            .wrapping_add(u64::from(entry.flags))
            .wrapping_add(entry.uncompressed_size)
            .wrapping_add(u64::from(entry.external_attributes))
            .wrapping_add(entry.local_header_offset)
            .wrapping_add(u64::from(entry.compression_method))
            .wrapping_add(entry.compressed_size)
            .wrapping_add(u64::from(entry.version_needed));
        for byte in entry.name_raw.iter().chain(entry.extra) {
            mixed = mixed.wrapping_add(u64::from(*byte));
        }
    }
    mixed
}

#[cfg(test)]
mod tests {
    use super::*;

    fn set16(bytes: &mut [u8], offset: usize, value: u16) {
        bytes[offset..offset + 2].copy_from_slice(&value.to_le_bytes());
    }

    fn set32(bytes: &mut [u8], offset: usize, value: u32) {
        bytes[offset..offset + 4].copy_from_slice(&value.to_le_bytes());
    }

    fn set64(bytes: &mut [u8], offset: usize, value: u64) {
        bytes[offset..offset + 8].copy_from_slice(&value.to_le_bytes());
    }

    // Empty stored files have CRC 0; no compression/CRC implementation is needed.
    fn archive(names: &[&[u8]]) -> (Vec<u8>, usize, usize) {
        let mut bytes = Vec::new();
        let mut central = Vec::new();
        for name in names {
            let offset = u32::try_from(bytes.len()).unwrap();
            let mut local = [0; 30];
            local[..4].copy_from_slice(b"PK\x03\x04");
            set16(&mut local, 4, 20);
            set16(&mut local, 26, u16::try_from(name.len()).unwrap());
            bytes.extend(local);
            bytes.extend_from_slice(name);
            let mut header = [0; 46];
            header[..4].copy_from_slice(CENTRAL);
            set16(&mut header, 4, 0x0314);
            set16(&mut header, 6, 20);
            set16(&mut header, 28, u16::try_from(name.len()).unwrap());
            set32(&mut header, 38, 0o100644 << 16);
            set32(&mut header, 42, offset);
            central.extend(header);
            central.extend_from_slice(name);
        }
        let start = bytes.len();
        bytes.extend(central);
        let footer = bytes.len();
        let mut end = [0; 22];
        end[..4].copy_from_slice(EOCD);
        set16(&mut end, 8, u16::try_from(names.len()).unwrap());
        set16(&mut end, 10, u16::try_from(names.len()).unwrap());
        set32(&mut end, 12, u32::try_from(footer - start).unwrap());
        set32(&mut end, 16, u32::try_from(start).unwrap());
        bytes.extend(end);
        (bytes, start, footer)
    }

    fn zip64(names: &[&[u8]]) -> (Vec<u8>, usize, usize) {
        let (mut bytes, start, footer) = archive(names);
        let mut end = bytes.split_off(footer);
        let mut record = [0; 56];
        record[..4].copy_from_slice(ZIP64_EOCD);
        set64(&mut record, 4, 44);
        set16(&mut record, 12, 45);
        set16(&mut record, 14, 45);
        set64(&mut record, 24, names.len() as u64);
        set64(&mut record, 32, names.len() as u64);
        set64(&mut record, 40, (footer - start) as u64);
        set64(&mut record, 48, start as u64);
        bytes.extend(record);
        let mut locator = [0; 20];
        locator[..4].copy_from_slice(ZIP64_LOCATOR);
        set64(&mut locator, 8, footer as u64);
        set32(&mut locator, 16, 1);
        bytes.extend(locator);
        set16(&mut end, 8, u16::MAX);
        set16(&mut end, 10, u16::MAX);
        set32(&mut end, 12, u32::MAX);
        set32(&mut end, 16, u32::MAX);
        bytes.extend(end);
        (bytes, start, footer)
    }

    #[test]
    fn normal_inventory_preserves_order_and_type_metadata() {
        let names: &[&[u8]] = &[
            b"sqlite-amalgamation-3530400/",
            b"sqlite-amalgamation-3530400/sqlite3.c",
            b"sqlite-amalgamation-3530400/sqlite3.h",
            b"sqlite-amalgamation-3530400/sqlite3ext.h",
            b"sqlite-amalgamation-3530400/shell.c",
        ];
        let (mut bytes, start, _) = archive(names);
        set32(&mut bytes, start + 38, 0o040755 << 16);
        let directory = parse_directory(&bytes).unwrap();
        assert_eq!(directory.declared_entry_count, 5);
        assert_eq!(directory.entries.len(), 5);
        assert_eq!(
            directory
                .entries
                .iter()
                .map(|entry| entry.name_raw)
                .collect::<Vec<_>>(),
            names
        );
        assert_eq!(directory.entries[0].external_attributes >> 16, 0o040755);
        assert_eq!(directory.entries[1].external_attributes >> 16, 0o100644);
        assert_eq!(directory.entries[0].compression_method, 0);
        assert_eq!(directory.entries[0].compressed_size, 0);
        assert_eq!(directory.entries[0].version_needed, 20);
        assert_eq!(
            &bytes[directory.entries[0].local_header_offset as usize..][..4],
            b"PK\x03\x04"
        );
        for entry in &directory.entries {
            assert_eq!(
                &bytes[entry.local_header_offset as usize..][..4],
                b"PK\x03\x04"
            );
        }
    }

    #[test]
    fn duplicate_entries_preserve_both_count_and_duplicate_names() {
        let (bytes, _, _) = archive(&[b"a", b"a", b"b"]);
        let directory = parse_directory(&bytes).unwrap();
        assert_eq!(directory.declared_entry_count, 3);
        assert_eq!(directory.entries.len(), 3);
        assert_eq!(directory.entries[0].name_raw, b"a");
        assert_eq!(directory.entries[1].name_raw, b"a");
        assert_eq!(directory.entries[2].name_raw, b"b");
    }

    #[test]
    fn missing_eocd_is_an_error() {
        let (bytes, _, footer) = archive(&[b"a"]);
        assert_eq!(
            parse_directory(&bytes[..footer]),
            Err(DirectoryError::MissingEndRecord)
        );
        assert_eq!(parse_directory(&[]), Err(DirectoryError::MissingEndRecord));
    }

    #[test]
    fn truncated_central_header_never_returns_a_partial_list() {
        let (mut bytes, start, footer) = archive(&[b"a", b"b"]);
        bytes.drain(start + 47 + 12..footer);
        let footer = start + 47 + 12;
        set32(&mut bytes, footer + 12, 59);
        assert_eq!(parse_directory(&bytes), Err(DirectoryError::Truncated));
    }

    #[test]
    fn truncated_variable_fields_are_errors() {
        for field in [28, 30, 32] {
            let (mut bytes, start, _) = archive(&[b"a"]);
            set16(&mut bytes, start + field, 100);
            assert_eq!(parse_directory(&bytes), Err(DirectoryError::Truncated));
        }
    }

    #[test]
    fn eocd_count_mismatches_in_both_directions_are_errors() {
        for count in [0, 1, 3, u16::MAX] {
            let (mut bytes, _, footer) = archive(&[b"a", b"a"]);
            set16(&mut bytes, footer + 10, count);
            assert_eq!(parse_directory(&bytes), Err(DirectoryError::CountMismatch));
        }
    }

    #[test]
    fn wrong_central_signature_is_an_error_even_after_a_valid_entry() {
        let (mut bytes, start, _) = archive(&[b"a", b"b"]);
        bytes[start + 47] = 0;
        assert_eq!(
            parse_directory(&bytes),
            Err(DirectoryError::InvalidSignature)
        );
    }

    #[test]
    fn directory_offset_and_length_out_of_bounds_are_errors() {
        for field in [12, 16] {
            let (mut bytes, _, footer) = archive(&[b"a"]);
            set32(&mut bytes, footer + field, u32::MAX);
            assert_eq!(parse_directory(&bytes), Err(DirectoryError::OutOfBounds));
        }
    }

    #[test]
    fn local_offset_out_of_bounds_is_an_error() {
        let (mut bytes, start, _) = archive(&[b"a"]);
        let length = bytes.len() as u32;
        set32(&mut bytes, start + 42, length);
        assert_eq!(parse_directory(&bytes), Err(DirectoryError::OutOfBounds));
    }

    #[test]
    fn comment_and_trailing_bytes_keep_zipfile_search_behavior() {
        let (mut bytes, _, footer) = archive(&[b"a"]);
        set16(&mut bytes, footer + 20, 3);
        bytes.extend(b"abcafter-comment");
        assert_eq!(parse_directory(&bytes).unwrap().entries.len(), 1);
        set16(&mut bytes, footer + 20, u16::MAX);
        assert_eq!(parse_directory(&bytes), Err(DirectoryError::OutOfBounds));
    }

    #[test]
    fn prepended_bytes_preserve_relative_offsets() {
        for is_zip64 in [false, true] {
            let (bytes, _, _) = if is_zip64 {
                zip64(&[b"a"])
            } else {
                archive(&[b"a"])
            };
            let mut prefixed = b"prefix".to_vec();
            prefixed.extend(bytes);
            let directory = parse_directory(&prefixed).unwrap();
            assert_eq!(directory.entries[0].name_raw, b"a");
            assert_eq!(directory.entries[0].local_header_offset, 6);
            assert_eq!(&prefixed[6..10], b"PK\x03\x04");
        }
    }

    #[test]
    fn classic_disk_labels_are_raw_metadata_not_a_new_rejection_policy() {
        let (mut bytes, _, footer) = archive(&[b"a"]);
        set16(&mut bytes, footer + 4, 2);
        set16(&mut bytes, footer + 6, 1);
        set16(&mut bytes, footer + 8, 0);
        let directory = parse_directory(&bytes).unwrap();
        assert_eq!(directory.disk_number, 2);
        assert_eq!(directory.directory_disk, 1);
        assert_eq!(directory.entries_on_disk, 0);
        assert_eq!(directory.declared_entry_count, 1);
    }

    #[test]
    fn raw_names_are_not_decoded_or_normalized() {
        let names: &[&[u8]] = &[b"a\0suffix", b"\x82", "한글".as_bytes()];
        let (mut bytes, start, _) = archive(names);
        set16(&mut bytes, start + 8, 0x800);
        let directory = parse_directory(&bytes).unwrap();
        assert_eq!(
            directory
                .entries
                .iter()
                .map(|entry| entry.name_raw)
                .collect::<Vec<_>>(),
            names
        );
        assert_eq!(directory.entries[0].flags, 0x800);
    }

    #[test]
    fn valid_zip64_footer_preserves_duplicates_and_large_counts_fail_without_allocation() {
        let (bytes, _, footer) = zip64(&[b"a", b"a"]);
        let directory = parse_directory(&bytes).unwrap();
        assert_eq!(directory.entries.len(), 2);
        assert_eq!(directory.declared_entry_count, 2);
        assert_eq!(directory.entries[0].name_raw, directory.entries[1].name_raw);
        let mut bytes = bytes;
        set64(&mut bytes, footer + 32, u64::MAX);
        assert_eq!(parse_directory(&bytes), Err(DirectoryError::CountMismatch));
    }

    #[test]
    fn zip64_multidisk_locator_retains_zipfile_rejection() {
        for (field, value) in [(4, 1), (16, 2)] {
            let (mut bytes, _, footer) = zip64(&[b"a"]);
            set32(&mut bytes, footer + 56 + field, value);
            assert_eq!(parse_directory(&bytes), Err(DirectoryError::MultiDiskZip64));
        }
    }

    #[test]
    fn zip64_bad_signature_locator_extent_and_record_length_are_errors() {
        let (bytes, _, footer) = zip64(&[b"a"]);
        let mut bad = bytes.clone();
        bad[footer] = 0;
        assert_eq!(parse_directory(&bad), Err(DirectoryError::InvalidSignature));
        let mut bad = bytes.clone();
        set64(&mut bad, footer + 56 + 8, u64::MAX);
        assert_eq!(parse_directory(&bad), Err(DirectoryError::OutOfBounds));
        let mut bad = bytes;
        set64(&mut bad, footer + 4, u64::MAX);
        assert_eq!(parse_directory(&bad), Err(DirectoryError::IntegerOverflow));
    }

    #[test]
    fn zip64_offset_length_overflow_is_an_error() {
        let (mut bytes, _, footer) = zip64(&[b"a"]);
        set64(&mut bytes, footer + 40, u64::MAX);
        assert_eq!(parse_directory(&bytes), Err(DirectoryError::InvalidZip64));
        assert_eq!(
            extent(&[], u64::MAX, 1),
            Err(DirectoryError::IntegerOverflow)
        );
    }

    #[test]
    fn zip64_extra_resolves_size_and_preserves_raw_extra_and_symlink_type() {
        let (mut bytes, start, footer) = archive(&[b"a"]);
        let mut extra = [0; 12];
        set16(&mut extra, 0, 1);
        set16(&mut extra, 2, 8);
        set64(&mut extra, 4, 1_u64 << 40);
        bytes.splice(footer..footer, extra);
        set32(&mut bytes, start + 24, u32::MAX);
        set16(&mut bytes, start + 30, 12);
        set32(&mut bytes, start + 38, 0o120777 << 16);
        set32(&mut bytes, footer + 12 + 12, (footer - start + 12) as u32);
        let directory = parse_directory(&bytes).unwrap();
        assert_eq!(directory.entries[0].uncompressed_size, 1_u64 << 40);
        assert_eq!(directory.entries[0].compressed_size, 0);
        assert_eq!(directory.entries[0].compression_method, 0);
        assert_eq!(directory.entries[0].version_needed, 20);
        assert_eq!(directory.entries[0].local_header_offset, 0);
        assert_eq!(directory.entries[0].external_attributes >> 16, 0o120777);
        assert_eq!(directory.entries[0].extra, extra);
    }

    #[test]
    fn missing_or_truncated_zip64_extra_is_an_error() {
        let (mut bytes, start, _) = archive(&[b"a"]);
        set32(&mut bytes, start + 24, u32::MAX);
        assert_eq!(parse_directory(&bytes), Err(DirectoryError::InvalidZip64));
        let (mut bytes, start, footer) = archive(&[b"a"]);
        bytes.splice(footer..footer, [1, 0, 8, 0]);
        set16(&mut bytes, start + 30, 4);
        set32(&mut bytes, footer + 4 + 12, (footer - start + 4) as u32);
        assert_eq!(parse_directory(&bytes), Err(DirectoryError::Truncated));
    }

    #[test]
    fn every_truncated_archive_prefix_is_an_error() {
        for is_zip64 in [false, true] {
            let (bytes, _, _) = if is_zip64 {
                zip64(&[b"a", b"a"])
            } else {
                archive(&[b"a", b"a"])
            };
            for length in 0..bytes.len() {
                assert!(
                    parse_directory(&bytes[..length]).is_err(),
                    "length={length}"
                );
            }
        }
    }

    #[test]
    fn malformed_extra_envelope_including_short_tail_is_an_error() {
        for extra in [&[0x75, 0x70, 20, 0][..], &[0x75, 0x70, 0][..]] {
            let (mut bytes, start, footer) = archive(&[b"a"]);
            bytes.splice(footer..footer, extra.iter().copied());
            set16(&mut bytes, start + 30, extra.len() as u16);
            set32(
                &mut bytes,
                footer + extra.len() + 12,
                (footer - start + extra.len()) as u32,
            );
            assert_eq!(parse_directory(&bytes), Err(DirectoryError::Truncated));
        }
    }

    #[test]
    fn zip64_extra_field_order_and_actual_maximum_32_bit_size_are_preserved() {
        let (mut bytes, start, footer) = archive(&[b"a"]);
        let mut extra = [0; 28];
        set16(&mut extra, 0, 1);
        set16(&mut extra, 2, 24);
        set64(&mut extra, 4, u64::from(u32::MAX));
        set64(&mut extra, 12, 0);
        set64(&mut extra, 20, 0);
        bytes.splice(footer..footer, extra);
        for field in [20, 24, 42] {
            set32(&mut bytes, start + field, u32::MAX);
        }
        set16(&mut bytes, start + 30, 28);
        set32(&mut bytes, footer + 28 + 12, (footer - start + 28) as u32);
        let directory = parse_directory(&bytes).unwrap();
        assert_eq!(directory.entries[0].uncompressed_size, u64::from(u32::MAX));
        assert_eq!(directory.entries[0].compressed_size, 0);
        assert_eq!(directory.entries[0].local_header_offset, 0);
        set64(&mut bytes, footer + 20, u64::MAX);
        assert_eq!(parse_directory(&bytes), Err(DirectoryError::OutOfBounds));
    }

    #[test]
    fn zip64_extensible_data_and_conflicting_classic_count_are_checked() {
        let (mut bytes, _, footer) = zip64(&[b"a"]);
        let extension = [0x23, 0, 0, 0, 0, 0];
        bytes.splice(footer + 56..footer + 56, extension);
        set64(&mut bytes, footer + 4, 50);
        assert_eq!(parse_directory(&bytes).unwrap().declared_entry_count, 1);
        set16(&mut bytes, footer + 56 + 6 + 20 + 10, 2);
        assert_eq!(parse_directory(&bytes), Err(DirectoryError::CountMismatch));
    }

    #[test]
    fn empty_directory_is_valid_metadata() {
        let (bytes, _, _) = archive(&[]);
        let directory = parse_directory(&bytes).unwrap();
        assert!(directory.entries.is_empty());
        assert_eq!(directory.declared_entry_count, 0);
    }

    fn with_central(
        name: &[u8],
        version: u16,
        compressed: u32,
        uncompressed: u32,
        extra: &[u8],
    ) -> Vec<u8> {
        let (mut bytes, start, footer) = archive(&[name]);
        bytes.splice(footer..footer, extra.iter().copied());
        set16(&mut bytes, start + 6, version);
        set32(&mut bytes, start + 20, compressed);
        set32(&mut bytes, start + 24, uncompressed);
        set16(&mut bytes, start + 30, u16::try_from(extra.len()).unwrap());
        let eocd = footer + extra.len();
        set32(
            &mut bytes,
            eocd + 12,
            u32::try_from(footer - start + extra.len()).unwrap(),
        );
        bytes
    }

    fn zip64_then_empty(payload: u64) -> Vec<u8> {
        let mut extra = vec![1, 0, 8, 0];
        extra.extend(payload.to_le_bytes());
        extra.extend_from_slice(&[1, 0, 0, 0]);
        extra
    }

    #[test]
    fn duplicate_zip64_extra_blocks_are_rejected() {
        // Hand cases 38, 38b, and 38d: a second ZIP64 extra follows a short or sentinel copy.
        let cases = [
            with_central(
                b"a",
                20,
                0,
                u32::MAX,
                &zip64_then_empty(u64::from(u32::MAX)),
            ),
            with_central(
                b"a",
                20,
                u32::MAX,
                0,
                &zip64_then_empty(u64::from(u32::MAX)),
            ),
            with_central(
                b"d/",
                20,
                u32::MAX,
                0,
                &zip64_then_empty(u64::from(u32::MAX)),
            ),
            // G3/G4 shapes: Python accepts these; the second block is still rejected.
            with_central(b"a", 20, 0, u32::MAX, &zip64_then_empty(1_u64 << 40)),
            with_central(b"a", 20, 0, 0, &zip64_then_empty(7)),
        ];
        for bytes in cases {
            assert_eq!(parse_directory(&bytes), Err(DirectoryError::InvalidZip64));
        }
    }

    #[test]
    fn version_needed_compares_only_the_low_byte_with_63() {
        assert_eq!(
            parse_directory(&with_central(b"a", 64, 0, 0, &[])).unwrap_err(),
            DirectoryError::UnsupportedVersion
        );
        assert_eq!(
            parse_directory(&with_central(b"a", u16::MAX, 0, 0, &[])).unwrap_err(),
            DirectoryError::UnsupportedVersion
        );
        let (mut bytes, start, _) = archive(&[b"dir/", b"dir/", b"a"]);
        set16(&mut bytes, start + 6, 64);
        assert_eq!(
            parse_directory(&bytes),
            Err(DirectoryError::UnsupportedVersion)
        );
        let (mut bytes, start, _) = archive(&[b"dir/", b"a"]);
        let second = start + 46 + b"dir/".len();
        set16(&mut bytes, second + 6, 64);
        assert_eq!(
            parse_directory(&bytes),
            Err(DirectoryError::UnsupportedVersion)
        );

        let high_byte_bytes = with_central(b"a", 0x0314, 0, 0, &[]);
        let high_byte = parse_directory(&high_byte_bytes).unwrap();
        assert_eq!(high_byte.entries[0].version_needed, 0x0314);
        assert_eq!(high_byte.entries[0].compression_method, 0);
        let at_limit_bytes = with_central(b"a", 63, 0, 0, &[]);
        let at_limit = parse_directory(&at_limit_bytes).unwrap();
        assert_eq!(at_limit.entries[0].version_needed, 63);
    }

    fn fixture(dir: &str, name: &str) -> Vec<u8> {
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("tests/fixtures/sqlite-zip")
            .join(dir)
            .join(name);
        std::fs::read(&path).unwrap_or_else(|error| panic!("read {}: {error}", path.display()))
    }

    #[test]
    fn probe_fixtures_accept_g1_g2_g5_and_reject_the_rest_of_the_review_set() {
        assert_eq!(
            parse_directory(&fixture("probe", "D-B1-extract-version-64.zip")).unwrap_err(),
            DirectoryError::UnsupportedVersion
        );
        assert_eq!(
            parse_directory(&fixture("probe", "D-B1-extract-version-255.zip")).unwrap_err(),
            DirectoryError::UnsupportedVersion
        );
        assert_eq!(
            parse_directory(&fixture(
                "probe",
                "D-B2-zip64-extra-twice-size-ffffffff.zip"
            ))
            .unwrap_err(),
            DirectoryError::InvalidZip64
        );
        assert_eq!(
            parse_directory(&fixture("probe", "D-B2-zip64-extra-twice-size-u64max.zip"))
                .unwrap_err(),
            DirectoryError::InvalidZip64
        );
        assert_eq!(
            parse_directory(&fixture(
                "probe",
                "G3-zip64-extra-twice-resolved-1TiB-must-accept.zip"
            ))
            .unwrap_err(),
            DirectoryError::InvalidZip64
        );
        assert_eq!(
            parse_directory(&fixture(
                "probe",
                "G4-zip64-extra-twice-unneeded-must-accept.zip"
            ))
            .unwrap_err(),
            DirectoryError::InvalidZip64
        );
        assert_eq!(
            parse_directory(&fixture("probe", "D-N3-zip64-sentinel-no-extra.zip")).unwrap_err(),
            DirectoryError::InvalidZip64
        );

        let g1_bytes = fixture("probe", "G1-version-0x0314-must-accept.zip");
        let g1 = parse_directory(&g1_bytes).unwrap();
        assert_eq!(g1.entries.len(), 1);
        assert_eq!(g1.entries[0].version_needed, 0x0314);
        assert_eq!(g1.entries[0].name_raw, b"a");
        assert_eq!(g1.entries[0].local_header_offset, 0);
        assert_eq!(g1.entries[0].compression_method, 0);
        assert_eq!(g1.entries[0].compressed_size, 0);

        let g2_bytes = fixture("probe", "G2-extract-version-63-must-accept.zip");
        let g2 = parse_directory(&g2_bytes).unwrap();
        assert_eq!(g2.entries[0].version_needed, 63);

        let g5_bytes = fixture("probe", "G5-duplicate-entry-count6.zip");
        let g5 = parse_directory(&g5_bytes).unwrap();
        assert_eq!(g5.declared_entry_count, 6);
        assert_eq!(g5.entries.len(), 6);
        assert_eq!(
            g5.entries
                .iter()
                .filter(|entry| entry.name_raw.ends_with(b"sqlite3.c"))
                .count(),
            2
        );
    }

    #[test]
    fn d_n1_current_results_stay_accepted_until_a_later_b_commit() {
        let utf8_bytes = fixture("probe", "D-N1-utf8-flag-invalid-name.zip");
        let utf8 = parse_directory(&utf8_bytes).unwrap();
        assert_eq!(utf8.entries.len(), 1);
        assert_eq!(utf8.entries[0].name_raw, &[0xff, 0xfe]);
        assert_eq!(utf8.entries[0].flags, 0x800);
        assert!(utf8.entries[0].extra.is_empty());
        assert_eq!(utf8.entries[0].version_needed, 20);

        let short_bytes = fixture("probe", "D-N1-unicode-path-short.zip");
        let short = parse_directory(&short_bytes).unwrap();
        assert_eq!(short.entries.len(), 1);
        assert_eq!(short.entries[0].name_raw, b"a");
        assert_eq!(short.entries[0].extra, [0x75, 0x70, 1, 0, 1]);

        let bad_bytes = fixture("probe", "D-N1-unicode-path-bad-utf8.zip");
        let bad = parse_directory(&bad_bytes).unwrap();
        assert_eq!(bad.entries.len(), 1);
        assert_eq!(bad.entries[0].name_raw, b"a");
        assert_eq!(
            bad.entries[0].extra,
            [0x75, 0x70, 7, 0, 1, 0x43, 0xbe, 0xb7, 0xe8, 0xff, 0xfe]
        );
    }

    #[test]
    fn every_hand_case_is_classified_and_the_known_counterexamples_are_rejected() {
        let directory = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("tests/fixtures/sqlite-zip/hand/cases");
        let mut names = Vec::new();
        let mut accepted = Vec::new();
        for entry in std::fs::read_dir(&directory).unwrap() {
            let entry = entry.unwrap();
            let name = entry.file_name().into_string().unwrap();
            assert!(name.ends_with(".zip"), "{name}");
            names.push(name.clone());
            if parse_directory(&std::fs::read(entry.path()).unwrap()).is_ok() {
                accepted.push(name);
            }
        }
        names.sort();
        accepted.sort();
        assert_eq!(names.len(), 76, "hand fixture count");
        for required in [
            "38_zip64_extra_dup_block_max.zip",
            "38b_zip64_dup_block_csize_max.zip",
            "38c_zip64_dup_block_loff_max.zip",
            "38d_zip64_dup_block_csize_max_dir.zip",
            "39_zip64_extra_dup_block_max_ok.zip",
            "42_extract_ver_64.zip",
            "44_extract_ver_65535.zip",
            "69_ver_dir_64_dup.zip",
        ] {
            assert!(
                !accepted.iter().any(|name| name == required),
                "{required} accepted"
            );
        }
        let expected = [
            "01_valid5.zip",
            "02_valid5_pyzipfile.zip",
            "03_dup6.zip",
            "04_dup5_replaces.zip",
            "05_dup_dir.zip",
            "16_comment_len_lt_trailing.zip",
            "17_trailing_junk.zip",
            "18_classic_multidisk.zip",
            "19_classic_ondisk_mismatch.zip",
            "23_zip64_valid.zip",
            "32_zip64_multidisk_record_only.zip",
            "33_zip64_extensible_data.zip",
            "36_zip64_extra_ok.zip",
            "37_zip64_extra_big_size.zip",
            "43_extract_ver_63.zip",
            "45_utf8flag_invalid_name.zip",
            "46_utf8flag_invalid_name_6.zip",
            "47_up7075_short.zip",
            "48_up7075_crc_match_bad_utf8.zip",
            "49_up7075_rename_to_expected.zip",
            "55_local_off_into_cd.zip",
            "56_local_off_same_two.zip",
            "57_local_off_into_eocd.zip",
            "58_prepended.zip",
            "59_prepended_zip64.zip",
            "60_symlink.zip",
            "61_oversize.zip",
            "62_empty.zip",
            "64_eocd_edge_65557.zip",
            "65_two_eocd_last_bogus.zip",
            "67_nul_in_name.zip",
            "68_flag_encrypted.zip",
            "70_pyzip_force_zip64.zip",
        ];
        assert_eq!(accepted, expected);
    }
}
