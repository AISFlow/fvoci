//! Independent ZIP reader for export tests: the `zip` crate (not the
//! hand-written `export_zip` writer) opens the archive and reads every entry
//! to EOF so its CRC-32 is verified. On top of that the helper rejects
//! everything `python3 -m zipfile -t` rejects with a nonzero exit:
//!
//! - the central directory must run, record by record, exactly up to the end
//!   record, whose size and entry count must match the records walked;
//! - every central-directory record (including ones hidden by duplicate-name
//!   collapsing) must need extraction version <= 6.3, must not set flag bit 5
//!   (compressed patched data) or bit 6 (strong encryption), and must have a
//!   strict UTF-8 name when flag bit 11 is set;
//! - every entry's local header must carry the same raw name bytes as its
//!   central record, decoded the same way (bit 11 agrees for non-ASCII
//!   names), and strict UTF-8 when the local header sets bit 11.
//!
//! It is intentionally stricter than the CLI where the CLI only prints a
//! corruption diagnostic and exits 0 (bad CRC, bad local signature, name
//! mismatch), where CPython checks flag bits 5/6 only on the record it opens,
//! and where the end record's entry count disagrees with the records.
//! Duplicate names collapse to one counted entry (the last record wins, as
//! in Python), so the returned count is the number of distinct names.

use std::io::{self, Cursor};

const CENTRAL_SIGNATURE: &[u8] = b"PK\x01\x02";
const END_SIGNATURE: &[u8] = b"PK\x05\x06";
const ZIP64_END_SIGNATURE: &[u8] = b"PK\x06\x06";
const CENTRAL_HEADER_LEN: usize = 46;
const CENTRAL_VERSION_AT: usize = 6;
const CENTRAL_FLAGS_AT: usize = 8;
const CENTRAL_NAME_LEN_AT: usize = 28;
const LOCAL_HEADER_LEN: usize = 30;
const LOCAL_FLAGS_AT: usize = 6;
const LOCAL_NAME_LEN_AT: usize = 26;
/// CPython `zipfile.MAX_EXTRACT_VERSION` (6.3).
const MAX_EXTRACT_VERSION: u8 = 63;
const FLAG_COMPRESSED_PATCH: u16 = 1 << 5;
const FLAG_STRONG_ENCRYPTION: u16 = 1 << 6;
const FLAG_UTF8_NAME: u16 = 1 << 11;

/// Opens `bytes` as a ZIP archive and reads every entry fully. Returns the
/// distinct entry count, or the first metadata/open/read/CRC error with the
/// entry it hit.
pub fn verify_zip(bytes: &[u8]) -> Result<usize, String> {
    let mut archive =
        zip::ZipArchive::new(Cursor::new(bytes)).map_err(|e| format!("open archive: {e}"))?;
    check_central_directory(bytes, archive.central_directory_start())?;
    for i in 0..archive.len() {
        // `by_index` (not `by_index_raw`) decompresses and checks the CRC.
        let mut file = archive
            .by_index(i)
            .map_err(|e| format!("entry #{i}: {e}"))?;
        let name = file.name().to_owned();
        // On-disk central name: `name_raw()` is replaced by a 0x7075 extra.
        let (central_flags, central) = header_name(
            bytes,
            file.central_header_start(),
            CENTRAL_HEADER_LEN,
            CENTRAL_FLAGS_AT,
            CENTRAL_NAME_LEN_AT,
        )
        .ok_or_else(|| format!("entry {name:?}: central record out of range"))?;
        let (local_flags, local) = header_name(
            bytes,
            file.header_start(),
            LOCAL_HEADER_LEN,
            LOCAL_FLAGS_AT,
            LOCAL_NAME_LEN_AT,
        )
        .ok_or_else(|| format!("entry {name:?}: local header out of range"))?;
        if local_flags & FLAG_UTF8_NAME != 0 && std::str::from_utf8(local).is_err() {
            return Err(format!("entry {name:?}: local header name is not UTF-8"));
        }
        if local != central {
            return Err(format!("entry {name:?}: local header name differs"));
        }
        if !local.is_ascii() && (local_flags ^ central_flags) & FLAG_UTF8_NAME != 0 {
            return Err(format!(
                "entry {name:?}: local header name encoding differs"
            ));
        }
        io::copy(&mut file, &mut io::sink()).map_err(|e| format!("entry {name:?}: {e}"))?;
    }
    Ok(archive.len())
}

/// Walks every central-directory record from `dir_start` and applies the
/// per-record checks CPython makes while listing and opening members. The
/// records must end at the (ZIP64) end record, whose directory size and entry
/// count must match what was walked: CPython locates the directory as
/// `end record - size`, so a size mismatch makes it read the wrong bytes.
fn check_central_directory(bytes: &[u8], dir_start: u64) -> Result<(), String> {
    let start = usize::try_from(dir_start).map_err(|_| "central directory out of range")?;
    let mut at = start;
    let mut count: u64 = 0;
    while bytes
        .get(at..)
        .is_some_and(|b| b.starts_with(CENTRAL_SIGNATURE))
    {
        let truncated = || format!("central record #{count}: truncated");
        let header = bytes
            .get(at..at + CENTRAL_HEADER_LEN)
            .ok_or_else(truncated)?;
        let version = header[CENTRAL_VERSION_AT];
        let flags = u16_at(header, CENTRAL_FLAGS_AT);
        let name_len = usize::from(u16_at(header, CENTRAL_NAME_LEN_AT));
        let rest_len = usize::from(u16_at(header, 30)) + usize::from(u16_at(header, 32));
        let name_start = at + CENTRAL_HEADER_LEN;
        let name = bytes
            .get(name_start..name_start + name_len)
            .ok_or_else(truncated)?;
        let next = name_start + name_len + rest_len;
        if next > bytes.len() {
            return Err(truncated());
        }
        let label = String::from_utf8_lossy(name);
        if version > MAX_EXTRACT_VERSION {
            return Err(format!(
                "entry {label:?}: zip file version {}.{} is not supported",
                version / 10,
                version % 10
            ));
        }
        if flags & FLAG_COMPRESSED_PATCH != 0 {
            return Err(format!(
                "entry {label:?}: compressed patched data (flag bit 5)"
            ));
        }
        if flags & FLAG_STRONG_ENCRYPTION != 0 {
            return Err(format!("entry {label:?}: strong encryption (flag bit 6)"));
        }
        if flags & FLAG_UTF8_NAME != 0 && std::str::from_utf8(name).is_err() {
            return Err(format!("entry {label:?}: central name is not UTF-8"));
        }
        at = next;
        count += 1;
    }
    // The zip crate reads entries-on-this-disk (EOCD) or the ZIP64 total.
    let end = bytes.get(at..).unwrap_or_default();
    let (size, entries) = if end.starts_with(ZIP64_END_SIGNATURE) && end.len() >= 56 {
        (u64_at(end, 40), u64_at(end, 32))
    } else if end.starts_with(END_SIGNATURE) && end.len() >= 22 {
        (u64::from(u32_at(end, 12)), u64::from(u16_at(end, 8)))
    } else {
        return Err(format!(
            "central directory: record #{count} is neither a central record nor an end record"
        ));
    };
    let walked = (at - start) as u64;
    if size != walked {
        return Err(format!(
            "central directory: end record size {size}, records span {walked} bytes"
        ));
    }
    if entries != count {
        return Err(format!(
            "central directory: end record lists {entries} entries, found {count} records"
        ));
    }
    Ok(())
}

/// Flag bits and name bytes of the header at `offset`, if the fixed header
/// and the name fit in `bytes`.
fn header_name(
    bytes: &[u8],
    offset: u64,
    header_len: usize,
    flags_at: usize,
    name_len_at: usize,
) -> Option<(u16, &[u8])> {
    let at = usize::try_from(offset).ok()?;
    let header = bytes.get(at..at.checked_add(header_len)?)?;
    let len = usize::from(u16_at(header, name_len_at));
    let name = bytes.get(at + header_len..at + header_len + len)?;
    Some((u16_at(header, flags_at), name))
}

fn u16_at(bytes: &[u8], at: usize) -> u16 {
    u16::from_le_bytes([bytes[at], bytes[at + 1]])
}

fn u32_at(bytes: &[u8], at: usize) -> u32 {
    u32::from_le_bytes(bytes[at..at + 4].try_into().unwrap())
}

fn u64_at(bytes: &[u8], at: usize) -> u64 {
    u64::from_le_bytes(bytes[at..at + 8].try_into().unwrap())
}

/// Panics unless [`verify_zip`] accepts the archive.
pub fn external_zip_check(bytes: &[u8]) {
    if let Err(e) = verify_zip(bytes) {
        panic!("zip archive check failed: {e}");
    }
}

#[cfg(test)]
mod tests {
    use super::{
        u16_at, verify_zip, CENTRAL_FLAGS_AT, CENTRAL_HEADER_LEN, CENTRAL_VERSION_AT,
        FLAG_COMPRESSED_PATCH, FLAG_STRONG_ENCRYPTION, FLAG_UTF8_NAME, LOCAL_FLAGS_AT,
        LOCAL_HEADER_LEN,
    };
    use std::io::{Cursor, Write};
    use zip::write::SimpleFileOptions;
    use zip::{CompressionMethod, ZipArchive, ZipWriter};

    const PAYLOAD: &[u8] = b"zip-check-stored-payload-0123456789";
    /// Local header "version needed to extract" (CPython checks the central one).
    const LOCAL_VERSION_AT: usize = 4;

    fn build(entries: &[(&str, &[u8])]) -> Vec<u8> {
        let mut writer = ZipWriter::new(Cursor::new(Vec::new()));
        let stored = SimpleFileOptions::default().compression_method(CompressionMethod::Stored);
        for (name, body) in entries {
            writer.start_file(*name, stored).unwrap();
            writer.write_all(body).unwrap();
        }
        writer.finish().unwrap().into_inner()
    }

    fn archive() -> Vec<u8> {
        build(&[("a.txt", PAYLOAD), ("b.json", b"{}")])
    }

    /// (local header offset, central record offset) of entry `index`.
    fn headers(bytes: &[u8], index: usize) -> (usize, usize) {
        let mut archive = ZipArchive::new(Cursor::new(bytes)).unwrap();
        let file = archive.by_index_raw(index).unwrap();
        (
            usize::try_from(file.header_start()).unwrap(),
            usize::try_from(file.central_header_start()).unwrap(),
        )
    }

    fn set_u16(bytes: &mut [u8], at: usize, value: u16) {
        bytes[at..at + 2].copy_from_slice(&value.to_le_bytes());
    }

    /// Sets `bits` in both the local and central flags of entry 0.
    fn with_flag_bits(mut bytes: Vec<u8>, bits: u16) -> Vec<u8> {
        let (local, central) = headers(&bytes, 0);
        for at in [local + LOCAL_FLAGS_AT, central + CENTRAL_FLAGS_AT] {
            let value = u16_at(&bytes, at) | bits;
            set_u16(&mut bytes, at, value);
        }
        bytes
    }

    fn end_record(bytes: &[u8]) -> usize {
        bytes
            .windows(4)
            .rposition(|w| w == b"PK\x05\x06")
            .expect("end record present")
    }

    #[test]
    fn zip_check_accepts_valid_archive() {
        assert_eq!(verify_zip(&archive()), Ok(2));
        assert_eq!(verify_zip(&build(&[])), Ok(0));
    }

    #[test]
    fn zip_check_accepts_utf8_names() {
        let bytes = build(&[("\u{e9}t\u{e9}.txt", PAYLOAD)]);
        let (local, central) = headers(&bytes, 0);
        assert_ne!(u16_at(&bytes, local + LOCAL_FLAGS_AT) & FLAG_UTF8_NAME, 0);
        assert_ne!(
            u16_at(&bytes, central + CENTRAL_FLAGS_AT) & FLAG_UTF8_NAME,
            0
        );
        assert_eq!(verify_zip(&bytes), Ok(1));
    }

    #[test]
    fn zip_check_accepts_max_extract_version() {
        let mut bytes = archive();
        let (local, central) = headers(&bytes, 0);
        bytes[local + LOCAL_VERSION_AT] = 63;
        bytes[central + CENTRAL_VERSION_AT] = 63;
        assert_eq!(verify_zip(&bytes), Ok(2));
    }

    #[test]
    fn zip_check_rejects_crc_mismatch() {
        let mut bytes = archive();
        let at = bytes
            .windows(PAYLOAD.len())
            .position(|w| w == PAYLOAD)
            .expect("stored payload present");
        bytes[at] ^= 0x01;
        let err = verify_zip(&bytes).unwrap_err();
        assert!(err.contains("a.txt"), "{err}");
    }

    #[test]
    fn zip_check_rejects_truncated_archive() {
        let bytes = archive();
        let err = verify_zip(&bytes[..bytes.len() - 10]).unwrap_err();
        assert!(err.starts_with("open archive"), "{err}");
    }

    #[test]
    fn zip_check_rejects_local_header_name_mismatch() {
        let mut bytes = archive();
        // The first "b.json" is the local header; the central copy follows.
        let at = bytes
            .windows(6)
            .position(|w| w == b"b.json")
            .expect("local name present");
        bytes[at] = b'c';
        let err = verify_zip(&bytes).unwrap_err();
        assert!(err.contains("local header name differs"), "{err}");
    }

    #[test]
    fn zip_check_rejects_local_header_name_encoding_mismatch() {
        let mut bytes = build(&[("\u{e9}.txt", PAYLOAD)]);
        let (_, central) = headers(&bytes, 0);
        let value = u16_at(&bytes, central + CENTRAL_FLAGS_AT) & !FLAG_UTF8_NAME;
        set_u16(&mut bytes, central + CENTRAL_FLAGS_AT, value);
        let err = verify_zip(&bytes).unwrap_err();
        assert!(err.contains("name encoding differs"), "{err}");
    }

    #[test]
    fn zip_check_rejects_compressed_patched_data_flag() {
        let err = verify_zip(&with_flag_bits(archive(), FLAG_COMPRESSED_PATCH)).unwrap_err();
        assert!(err.contains("flag bit 5"), "{err}");
    }

    #[test]
    fn zip_check_rejects_strong_encryption_flag() {
        let err = verify_zip(&with_flag_bits(archive(), FLAG_STRONG_ENCRYPTION)).unwrap_err();
        assert!(err.contains("flag bit 6"), "{err}");
    }

    #[test]
    fn zip_check_rejects_unsupported_extract_version() {
        let mut bytes = archive();
        let (local, central) = headers(&bytes, 0);
        bytes[local + LOCAL_VERSION_AT] = 100;
        bytes[central + CENTRAL_VERSION_AT] = 100;
        let err = verify_zip(&bytes).unwrap_err();
        assert!(err.contains("version 10.0"), "{err}");
    }

    #[test]
    fn zip_check_rejects_invalid_utf8_central_name() {
        let mut bytes = with_flag_bits(archive(), FLAG_UTF8_NAME);
        let (local, central) = headers(&bytes, 0);
        bytes[local + LOCAL_HEADER_LEN] = 0xff;
        bytes[central + CENTRAL_HEADER_LEN] = 0xff;
        let err = verify_zip(&bytes).unwrap_err();
        assert!(err.contains("central name is not UTF-8"), "{err}");
    }

    #[test]
    fn zip_check_rejects_invalid_utf8_local_name() {
        let mut bytes = with_flag_bits(archive(), FLAG_UTF8_NAME);
        let (local, central) = headers(&bytes, 0);
        let value = u16_at(&bytes, central + CENTRAL_FLAGS_AT) & !FLAG_UTF8_NAME;
        set_u16(&mut bytes, central + CENTRAL_FLAGS_AT, value);
        bytes[local + LOCAL_HEADER_LEN] = 0xff;
        bytes[central + CENTRAL_HEADER_LEN] = 0xff;
        let err = verify_zip(&bytes).unwrap_err();
        assert!(err.contains("local header name is not UTF-8"), "{err}");
    }

    #[test]
    fn zip_check_checks_records_hidden_by_duplicate_names() {
        let mut bytes = build(&[("a.txt", PAYLOAD), ("b.txt", b"{}")]);
        let (_, first_central) = headers(&bytes, 0);
        let (local, central) = headers(&bytes, 1);
        bytes[local + LOCAL_HEADER_LEN] = b'a';
        bytes[central + CENTRAL_HEADER_LEN] = b'a';
        // Two records, one name: the reader keeps the last, as Python does.
        assert_eq!(verify_zip(&bytes), Ok(1));

        let mut hidden_version = bytes.clone();
        hidden_version[first_central + CENTRAL_VERSION_AT] = 100;
        let err = verify_zip(&hidden_version).unwrap_err();
        assert!(err.contains("version 10.0"), "{err}");

        // Python only opens the last record per name, so it accepts this one;
        // the helper is intentionally stricter and checks every record.
        let mut hidden_flag = bytes;
        let value = u16_at(&hidden_flag, first_central + CENTRAL_FLAGS_AT) | FLAG_COMPRESSED_PATCH;
        set_u16(&mut hidden_flag, first_central + CENTRAL_FLAGS_AT, value);
        let err = verify_zip(&hidden_flag).unwrap_err();
        assert!(err.contains("flag bit 5"), "{err}");
    }

    #[test]
    fn zip_check_rejects_central_directory_size_mismatch() {
        let mut bytes = archive();
        let end = end_record(&bytes);
        let size = u16_at(&bytes, end + 12) + 1;
        set_u16(&mut bytes, end + 12, size);
        let err = verify_zip(&bytes).unwrap_err();
        assert!(err.contains("end record size"), "{err}");
    }

    #[test]
    fn zip_check_rejects_records_beyond_end_record_count() {
        let mut bytes = archive();
        let end = end_record(&bytes);
        set_u16(&mut bytes, end + 8, 1);
        set_u16(&mut bytes, end + 10, 1);
        let err = verify_zip(&bytes).unwrap_err();
        assert!(err.contains("lists 1 entries, found 2"), "{err}");
    }
}
