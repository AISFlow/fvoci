//! Reviewed SQLite source pins shared by `sqlite-build` and `sqlite-ci`.
//! This module is the single owner of the version, source ID and hashes.

use crate::host::sha256_hex;
use serde_json::{json, Map, Value};
use std::collections::BTreeSet;
use std::fs;
use std::path::Path;
use std::process::Command;
use std::time::Duration;

/// Record physical bytes and symlink resolution, including the dynamic ELF
/// closure. Package names/versions are diagnostic, never a substitute for this.
pub fn native_files(paths: &[&Path]) -> Result<Value, String> {
    fn add(
        path: &Path,
        files: &mut Map<String, Value>,
        visited: &mut BTreeSet<std::path::PathBuf>,
    ) -> Result<(), String> {
        let resolved = fs::canonicalize(path).map_err(|e| format!("{}: {e}", path.display()))?;
        if resolved.is_dir() {
            files.insert(
                path.to_string_lossy().into_owned(),
                json!({"resolved": resolved, "kind": "directory"}),
            );
            // Follow SDK directory symlinks as well as ordinary directories.
            if !visited.insert(resolved.clone()) {
                return Ok(());
            }
            let mut entries = fs::read_dir(path)
                .map_err(|e| e.to_string())?
                .map(|entry| entry.map(|e| e.path()))
                .collect::<Result<Vec<_>, _>>()
                .map_err(|e| e.to_string())?;
            entries.sort();
            for entry in entries {
                add(&entry, files, visited)?;
            }
            return Ok(());
        }
        if !resolved.is_file() {
            return Err(format!(
                "native input is not a regular file: {}",
                path.display()
            ));
        }
        let bytes = fs::read(&resolved).map_err(|e| e.to_string())?;
        files.insert(
            path.to_string_lossy().into_owned(),
            json!({"resolved": resolved, "sha256": sha256_hex(&bytes)}),
        );
        if !visited.insert(resolved.clone()) || !bytes.starts_with(b"\x7fELF") {
            return Ok(());
        }
        let mut command = Command::new("/usr/bin/ldd");
        command
            .arg(&resolved)
            .env_clear()
            .envs(crate::sqlite_build::ENV);
        let done = crate::process::run_owned(&mut command, Duration::from_secs(30))
            .map_err(|e| e.to_string())?;
        let output = String::from_utf8(done.stdout).map_err(|e| e.to_string())?
            + &String::from_utf8(done.stderr).map_err(|e| e.to_string())?;
        let deps = elf_dependencies(crate::process::returncode(done.status), &output)?;
        for dependency in deps {
            add(Path::new(&dependency), files, visited)?;
        }
        Ok(())
    }
    let mut files = Map::new();
    let mut visited = BTreeSet::new();
    for path in paths {
        add(path, &mut files, &mut visited)?;
    }
    Ok(Value::Object(files))
}

fn elf_dependencies(code: i64, output: &str) -> Result<Vec<String>, String> {
    if code == 1
        && (output.contains("statically linked") || output.contains("not a dynamic executable"))
    {
        return Ok(Vec::new());
    }
    if code != 0 {
        return Err(format!("native dependency inspection failed: {output}"));
    }
    let mut paths = Vec::new();
    for line in output.lines().map(str::trim).filter(|l| !l.is_empty()) {
        let words: Vec<_> = line.split_whitespace().collect();
        let path = if words.get(1) == Some(&"=>") {
            words.get(2).copied().filter(|p| p.starts_with('/'))
        } else if words[0].starts_with('/') {
            Some(words[0])
        } else if words[0].starts_with("linux-vdso.") || words[0].starts_with("linux-gate.") {
            continue;
        } else {
            None
        };
        paths.push(
            path.ok_or_else(|| format!("unresolved native dependency: {line}"))?
                .to_owned(),
        );
    }
    if paths.is_empty() {
        return Err("dynamic native input has no resolved loader/dependencies".to_owned());
    }
    Ok(paths)
}

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
const SOURCES: [(&str, &[u8]); 31] = [
    (
        "../tests/support/official_sqld.rs",
        include_bytes!("../../tests/support/official_sqld.rs"),
    ),
    ("Cargo.toml", include_bytes!("../Cargo.toml")),
    ("Cargo.lock", include_bytes!("../Cargo.lock")),
    ("src/args.rs", include_bytes!("args.rs")),
    ("src/ci_fixture.rs", include_bytes!("ci_fixture.rs")),
    ("src/host.rs", include_bytes!("host.rs")),
    ("src/install_image.rs", include_bytes!("install_image.rs")),
    ("src/lib.rs", include_bytes!("lib.rs")),
    ("src/main.rs", include_bytes!("main.rs")),
    ("src/native_library.rs", include_bytes!("native_library.rs")),
    (
        "src/native_library_pg.rs",
        include_bytes!("native_library_pg.rs"),
    ),
    (
        "src/native_library_sqld.rs",
        include_bytes!("native_library_sqld.rs"),
    ),
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
    (
        "src/task_scalar_qualification.rs",
        include_bytes!("task_scalar_qualification.rs"),
    ),
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
    fn physical_native_inputs_change_for_tool_header_library_and_symlink_bytes() {
        let root = crate::host::mkdtemp("sqlite-native-files-", &std::env::temp_dir()).unwrap();
        for name in ["cc1", "as", "ld", "crt.o", "header.h", "libclang.so"] {
            std::fs::write(root.join(name), name).unwrap();
        }
        let first = native_files(&[&root]).unwrap();
        assert_eq!(first, native_files(&[&root]).unwrap());
        for name in ["cc1", "as", "ld", "crt.o", "header.h", "libclang.so"] {
            std::fs::write(root.join(name), "changed without package version change").unwrap();
            assert_ne!(first, native_files(&[&root]).unwrap(), "{name}");
            std::fs::write(root.join(name), name).unwrap();
        }
        std::os::unix::fs::symlink("missing", root.join("bad-sdk")).unwrap();
        assert!(native_files(&[&root]).is_err());
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn dynamic_dependency_output_requires_concrete_paths_on_both_native_architectures() {
        assert_eq!(elf_dependencies(0, "linux-vdso.so.1 (0x1)\nlibc.so.6 => /lib/aarch64-linux-gnu/libc.so.6 (0x2)\n/lib/ld-linux-aarch64.so.1 (0x3)\n").unwrap(), vec!["/lib/aarch64-linux-gnu/libc.so.6", "/lib/ld-linux-aarch64.so.1"]);
        assert_eq!(elf_dependencies(0, "libc.so.6 => /lib/x86_64-linux-gnu/libc.so.6 (0x2)\n/lib64/ld-linux-x86-64.so.2 (0x3)\n").unwrap().len(), 2);
        assert!(elf_dependencies(0, "libclang.so => not found\n").is_err());
        assert!(elf_dependencies(0, "unexpected output\n").is_err());
        assert!(elf_dependencies(2, "statically linked").is_err());
        assert!(elf_dependencies(1, "not a dynamic executable")
            .unwrap()
            .is_empty());
    }

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
