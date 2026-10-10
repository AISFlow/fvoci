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

/// Every source, manifest and lockfile compiled into this binary. Adding a
/// module to `src/` requires listing it here (checked by a test).
const SOURCES: [(&str, &[u8]); 25] = [
    ("Cargo.toml", include_bytes!("../Cargo.toml")),
    ("Cargo.lock", include_bytes!("../Cargo.lock")),
    ("src/args.rs", include_bytes!("args.rs")),
    ("src/ci_fixture.rs", include_bytes!("ci_fixture.rs")),
    ("src/host.rs", include_bytes!("host.rs")),
    ("src/lib.rs", include_bytes!("lib.rs")),
    ("src/main.rs", include_bytes!("main.rs")),
    ("src/process.rs", include_bytes!("process.rs")),
    ("src/rust_binaries.rs", include_bytes!("rust_binaries.rs")),
    (
        "src/rust_binaries_archive.rs",
        include_bytes!("rust_binaries_archive.rs"),
    ),
    (
        "src/rust_binaries_cohort.rs",
        include_bytes!("rust_binaries_cohort.rs"),
    ),
    (
        "src/rustup_ci_metadata/guard.rs",
        include_bytes!("rustup_ci_metadata/guard.rs"),
    ),
    (
        "src/rustup_ci_metadata/json.rs",
        include_bytes!("rustup_ci_metadata/json.rs"),
    ),
    (
        "src/rustup_ci_metadata/mod.rs",
        include_bytes!("rustup_ci_metadata/mod.rs"),
    ),
    (
        "src/rustup_ci_metadata/prepare.rs",
        include_bytes!("rustup_ci_metadata/prepare.rs"),
    ),
    (
        "src/rustup_ci_metadata/prepare/tests.rs",
        include_bytes!("rustup_ci_metadata/prepare/tests.rs"),
    ),
    (
        "src/rustup_ci_metadata/scope.rs",
        include_bytes!("rustup_ci_metadata/scope.rs"),
    ),
    (
        "src/schema_baseline.rs",
        include_bytes!("schema_baseline.rs"),
    ),
    (
        "src/selected_install.rs",
        include_bytes!("selected_install.rs"),
    ),
    (
        "src/selected_library.rs",
        include_bytes!("selected_library.rs"),
    ),
    ("src/shell.rs", include_bytes!("shell.rs")),
    ("src/sqlite.rs", include_bytes!("sqlite.rs")),
    ("src/sqlite_build.rs", include_bytes!("sqlite_build.rs")),
    ("src/sqlite_ci.rs", include_bytes!("sqlite_ci.rs")),
    ("src/sqlite_zip.rs", include_bytes!("sqlite_zip.rs")),
];

fn sources_sha256(role: &str) -> String {
    let mut data = role.as_bytes().to_vec();
    for (name, body) in SOURCES {
        data.push(0);
        data.extend_from_slice(name.as_bytes());
        data.push(0);
        data.extend_from_slice(&(body.len() as u64).to_le_bytes());
        data.extend_from_slice(body);
    }
    sha256_hex(&data)
}

/// Replaces the former hash of the Python helper file in the build input
/// identity: any change to the compiled xtask (policy, flags, smoke program,
/// ZIP rules, dependencies) yields a new identity.
pub fn helper_sha256() -> String {
    sources_sha256("sqlite-build")
}

/// Replaces the former hash of the Python wrapper file, over the same sources.
pub fn wrapper_sha256() -> String {
    sources_sha256("sqlite-ci")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn identity_sources_cover_every_module() {
        fn files(dir: &std::path::Path, prefix: &str, found: &mut Vec<String>) {
            for entry in std::fs::read_dir(dir).unwrap() {
                let entry = entry.unwrap();
                let name = format!("{prefix}/{}", entry.file_name().to_string_lossy());
                if entry.file_type().unwrap().is_dir() {
                    files(&entry.path(), &name, found);
                } else {
                    found.push(name);
                }
            }
        }
        let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
        let mut on_disk = Vec::new();
        files(&dir, "src", &mut on_disk);
        on_disk.sort();
        let listed: Vec<String> = SOURCES
            .iter()
            .map(|(n, _)| (*n).to_owned())
            .filter(|n| n.starts_with("src/"))
            .collect();
        assert_eq!(listed, on_disk);
        assert_ne!(helper_sha256(), wrapper_sha256());
    }

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
