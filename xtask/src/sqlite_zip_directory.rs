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
}

#[derive(Debug, PartialEq, Eq)]
pub struct Entry<'a> {
    pub name_raw: &'a [u8],
    pub flags: u16,
    pub extra: &'a [u8],
    pub uncompressed_size: u64,
    pub external_attributes: u32,
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
    extent(input, offset, N as u64)?
        .try_into()
        .map_err(|_| DirectoryError::Truncated)
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
        let found = input
            .get(start..)
            .ok_or(DirectoryError::OutOfBounds)?
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
            extent(input, physical, record_length)?;
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

fn entry_size_and_offset(header: &[u8], mut extra: &[u8]) -> Result<(u64, u64)> {
    let mut size = u64::from(u32_at(header, 24)?);
    let mut offset = u64::from(u32_at(header, 42)?);
    let mut need_size = size == u64::from(u32::MAX);
    let mut need_compressed = u32_at(header, 20)? == u32::MAX;
    let mut need_offset = offset == u64::from(u32::MAX);
    while !extra.is_empty() {
        let tag = take(&mut extra, 4)?;
        let mut value = take(&mut extra, usize::from(u16_at(tag, 2)?))?;
        if u16_at(tag, 0)? == 1 {
            if need_size {
                size = u64_at(take(&mut value, 8)?, 0)?;
                need_size = false;
            }
            if need_compressed {
                take(&mut value, 8)?;
                need_compressed = false;
            }
            if need_offset {
                offset = u64_at(take(&mut value, 8)?, 0)?;
                need_offset = false;
            }
        }
    }
    if need_size || need_compressed || need_offset {
        return Err(DirectoryError::InvalidZip64);
    }
    Ok((size, offset))
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
        let name_raw = take(&mut central, usize::from(u16_at(header, 28)?))?;
        let extra = take(&mut central, usize::from(u16_at(header, 30)?))?;
        take(&mut central, usize::from(u16_at(header, 32)?))?;
        let (uncompressed_size, local_offset) = entry_size_and_offset(header, extra)?;
        let local_position = prefix
            .checked_add(local_offset)
            .ok_or(DirectoryError::IntegerOverflow)?;
        if local_position >= u64::try_from(input.len()).map_err(|_| DirectoryError::OutOfBounds)? {
            return Err(DirectoryError::OutOfBounds);
        }
        entries.push(Entry {
            name_raw,
            flags: u16_at(header, 8)?,
            extra,
            uncompressed_size,
            external_attributes: u32_at(header, 38)?,
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
            assert_eq!(
                parse_directory(&prefixed).unwrap().entries[0].name_raw,
                b"a"
            );
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
        assert_eq!(
            parse_directory(&bytes).unwrap().entries[0].uncompressed_size,
            u64::from(u32::MAX)
        );
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
}
