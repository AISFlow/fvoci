//! Reviewed SQLite source pins shared by `sqlite-build` and `sqlite-ci`.
//! This module is the single owner of the version, source ID and hashes.

use crate::host::sha256_hex;

pub const ARCHIVE_HASH: &str = "1e71ddf93849c6a6ecf58b827c0692073d2dd7ee40196158068f7b29f422e87d";
pub const C_HASH: &str = "67f423e9ebbbdc473cbc4772c872ee6b89f31fde4ed0279a5c25d5f65c043a16";
pub const HEADER_HASH: &str = "919e7f2e8ed1d8f56ac17b412b8971c76aa5d1a879752cc6058f75e7d5910e1d";
pub const SOURCE_ID: &str =
    "2026-07-24 19:02:57 bf7c7f30031888f4e796e429ab3978879485813aaca6f641c7b33e4e09459bcc";
pub const VERSION: &str = "3.53.4";
/// Archive and every member are bounded to 16 MiB.
pub const SIZE_LIMIT: u64 = 16 * 1024 * 1024;

pub const ENV_NAMES: [&str; 4] = [
    "SQLITE3_LIB_DIR",
    "SQLITE3_INCLUDE_DIR",
    "SQLITE3_STATIC",
    "SQLITE3_NO_PKG_CONFIG",
];

/// `sqlite-amalgamation-3530400.zip`, derived from VERSION like the wrapper did.
pub fn archive_name() -> String {
    let mut parts = VERSION.split('.').map(|p| p.parse::<u32>().unwrap_or(0));
    let (major, minor, patch) = (
        parts.next().unwrap_or(0),
        parts.next().unwrap_or(0),
        parts.next().unwrap_or(0),
    );
    format!(
        "sqlite-amalgamation-{}.zip",
        major * 1_000_000 + minor * 10_000 + patch * 100
    )
}

/// Official download URL: the year directory comes from the source ID.
pub fn archive_url() -> String {
    format!(
        "https://www.sqlite.org/{}/{}",
        &SOURCE_ID[..4],
        archive_name()
    )
}

/// Root directory inside the official archive.
pub fn archive_root() -> String {
    archive_name().trim_end_matches(".zip").to_owned() + "/"
}

/// Hash of the build policy sources compiled into this binary. It replaces
/// the former hash of the Python helper file in the build input identity, so
/// any policy, flag, smoke or ZIP rule change yields a new cache identity.
pub fn helper_sha256() -> String {
    let mut sources = Vec::new();
    for (name, body) in [
        ("sqlite.rs", include_bytes!("sqlite.rs").as_slice()),
        ("sqlite_build.rs", include_bytes!("sqlite_build.rs")),
        ("sqlite_zip.rs", include_bytes!("sqlite_zip.rs")),
        ("process.rs", include_bytes!("process.rs")),
        ("shell.rs", include_bytes!("shell.rs")),
        ("Cargo.lock", include_bytes!("../Cargo.lock")),
    ] {
        sources.extend_from_slice(name.as_bytes());
        sources.push(0);
        sources.extend_from_slice(&(body.len() as u64).to_le_bytes());
        sources.extend_from_slice(body);
    }
    sha256_hex(&sources)
}

/// Hash of the wrapper policy source, replacing the former wrapper file hash.
pub fn wrapper_sha256() -> String {
    sha256_hex(include_bytes!("sqlite_ci.rs"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn derived_names_match_the_reviewed_download() {
        assert_eq!(archive_name(), "sqlite-amalgamation-3530400.zip");
        assert_eq!(
            archive_url(),
            "https://www.sqlite.org/2026/sqlite-amalgamation-3530400.zip"
        );
        assert_eq!(archive_root(), "sqlite-amalgamation-3530400/");
    }
}
