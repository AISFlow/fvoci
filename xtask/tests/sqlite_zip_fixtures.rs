//! PR #381's ZIP fixtures as the oracle for the SQLite member policy.
//!
//! `PYTHON_ACCEPTED` is the set the former Python policy
//! (scripts/prepare-sqlite-build.sh:211-221 at 431c7f91, with the archive
//! SHA-256 pin skipped) accepted under CPython 3.14.4; every other fixture it
//! rejected. The Rust policy must reject everything Python rejected. Of the
//! Python-accepted fixtures only well-formed archives stay accepted; the rest
//! are intentional, stricter rejections listed in the PR intent table.

use std::fs;
use std::path::{Path, PathBuf};
use xtask::sqlite_zip::read_sources;

const PYTHON_ACCEPTED: &[&str] = &[
    "01_valid5.zip",
    "02_valid5_pyzipfile.zip",
    "14_local_off_oob.zip",
    "15_comment_len_gt.zip",
    "16_comment_len_lt_trailing.zip",
    "17_trailing_junk.zip",
    "18_classic_multidisk.zip",
    "19_classic_ondisk_mismatch.zip",
    "20_eocd_total_more.zip",
    "21_eocd_total_less.zip",
    "22_eocd_total_zero.zip",
    "23_zip64_valid.zip",
    "29_zip64_classic_count_conflict.zip",
    "30_zip64_count_more.zip",
    "32_zip64_multidisk_record_only.zip",
    "33_zip64_extensible_data.zip",
    "36_zip64_extra_ok.zip",
    "39_zip64_extra_dup_block_max_ok.zip",
    "41_extra_tail_3.zip",
    "43_extract_ver_63.zip",
    "49_up7075_rename_to_expected.zip",
    "50_central_comment_truncated.zip",
    "51_central_name_truncated.zip",
    "55_local_off_into_cd.zip",
    "56_local_off_same_two.zip",
    "57_local_off_into_eocd.zip",
    "58_prepended.zip",
    "59_prepended_zip64.zip",
    "64_eocd_edge_65557.zip",
    "67_nul_in_name.zip",
    "68_flag_encrypted.zip",
    "70_pyzip_force_zip64.zip",
];

/// Fixtures the Rust policy accepts, each a subset of `PYTHON_ACCEPTED`.
const RUST_ACCEPTED: &[&str] = &[
    "01_valid5.zip",
    "02_valid5_pyzipfile.zip",
    "41_extra_tail_3.zip",
    "43_extract_ver_63.zip",
    "70_pyzip_force_zip64.zip",
];

fn fixtures() -> Vec<PathBuf> {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/sqlite-zip");
    let mut out = Vec::new();
    for dir in ["hand/cases", "probe"] {
        for entry in fs::read_dir(root.join(dir)).unwrap() {
            let path = entry.unwrap().path();
            if path.extension().is_some_and(|e| e == "zip") {
                out.push(path);
            }
        }
    }
    out.sort();
    assert_eq!(out.len(), 89, "fixture inventory changed");
    out
}

#[test]
fn rust_policy_is_never_looser_than_python() {
    for name in RUST_ACCEPTED {
        assert!(PYTHON_ACCEPTED.contains(name), "{name}");
    }
    let mut mismatches = Vec::new();
    for path in fixtures() {
        let name = path.file_name().unwrap().to_str().unwrap().to_owned();
        let result = read_sources(&fs::read(&path).unwrap());
        if result.is_ok() != RUST_ACCEPTED.contains(&name.as_str()) {
            mismatches.push(format!("{name}: {:?}", result.err()));
        }
    }
    assert!(mismatches.is_empty(), "{mismatches:#?}");
}

fn valid5() -> Vec<u8> {
    fs::read(
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("tests/fixtures/sqlite-zip/hand/cases/01_valid5.zip"),
    )
    .unwrap()
}

#[test]
fn accepted_archive_returns_the_two_consumed_members() {
    let sources = read_sources(&valid5()).unwrap();
    assert_eq!(sources.c_source.len(), 111);
    assert_eq!(sources.header.len(), 111);
}

#[test]
fn crc_is_verified_for_every_member() {
    // Stored data offsets in 01_valid5.zip: local header + 30 + name length.
    for (member, offset) in [("sqlite3.c", 58 + 30 + 37), ("shell.c", 604 + 30 + 35)] {
        let mut data = valid5();
        data[offset] ^= 0x01;
        let error = read_sources(&data).unwrap_err();
        assert!(error.contains("bad zip archive"), "{member}: {error}");
    }
}

#[test]
fn truncation_and_trailing_bytes_are_refused() {
    let data = valid5();
    assert!(read_sources(&data[..data.len() - 1]).is_err());
    let mut longer = data.clone();
    longer.push(0);
    assert!(read_sources(&longer).is_err());
    assert!(read_sources(&[]).is_err());
}
