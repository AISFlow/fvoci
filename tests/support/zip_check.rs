//! Independent ZIP reader for export tests: the `zip` crate (not the
//! hand-written `export_zip` writer) opens the archive, checks that each local
//! header names the same entry as the central directory, then reads every
//! entry to EOF so its CRC-32 is verified (the checks `zipfile -t` made).

use std::io::{self, Cursor};

/// Opens `bytes` as a ZIP archive and reads every entry fully. Returns the
/// entry count, or the first open/read/CRC error with the entry it hit.
pub fn verify_zip(bytes: &[u8]) -> Result<usize, String> {
    let mut archive =
        zip::ZipArchive::new(Cursor::new(bytes)).map_err(|e| format!("open archive: {e}"))?;
    for i in 0..archive.len() {
        // `by_index` (not `by_index_raw`) decompresses and checks the CRC.
        let mut file = archive
            .by_index(i)
            .map_err(|e| format!("entry #{i}: {e}"))?;
        let name = file.name().to_owned();
        let local = local_header_name(bytes, file.header_start())
            .ok_or_else(|| format!("entry {name:?}: local header out of range"))?;
        if local != file.name_raw() {
            return Err(format!("entry {name:?}: local header name differs"));
        }
        io::copy(&mut file, &mut io::sink()).map_err(|e| format!("entry {name:?}: {e}"))?;
    }
    Ok(archive.len())
}

/// Name bytes of the local file header at `offset`, if it fits in `bytes`.
fn local_header_name(bytes: &[u8], offset: u64) -> Option<&[u8]> {
    let at = usize::try_from(offset).ok()?;
    let len = bytes.get(at + 26..at + 28)?;
    let len = usize::from(u16::from_le_bytes([len[0], len[1]]));
    bytes.get(at + 30..at + 30 + len)
}

/// Panics unless [`verify_zip`] accepts the archive.
pub fn external_zip_check(bytes: &[u8]) {
    if let Err(e) = verify_zip(bytes) {
        panic!("zip archive check failed: {e}");
    }
}

#[cfg(test)]
mod tests {
    use super::verify_zip;
    use std::io::{Cursor, Write};
    use zip::write::SimpleFileOptions;
    use zip::{CompressionMethod, ZipWriter};

    const PAYLOAD: &[u8] = b"zip-check-stored-payload-0123456789";

    fn archive() -> Vec<u8> {
        let mut writer = ZipWriter::new(Cursor::new(Vec::new()));
        let stored = SimpleFileOptions::default().compression_method(CompressionMethod::Stored);
        writer.start_file("a.txt", stored).unwrap();
        writer.write_all(PAYLOAD).unwrap();
        writer.start_file("b.json", stored).unwrap();
        writer.write_all(b"{}").unwrap();
        writer.finish().unwrap().into_inner()
    }

    #[test]
    fn zip_check_accepts_valid_archive() {
        assert_eq!(verify_zip(&archive()), Ok(2));
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
}
