//! Official sqld release inputs. Both tarball and executable digests come
//! from libsql-server-v0.24.32's published assets; no mutable tag/cache trust.

#[path = "../../tests/support/official_sqld.rs"]
mod official;
use crate::ci_fixture::{capture, Streams};
use crate::rust_binaries_archive::file_sha256;
use official::pin;
use std::fs;
use std::path::Path;
use std::process::Command;

fn command(command: &mut Command, phase: &str) -> Result<Vec<u8>, String> {
    let done = capture(command, None, Streams::Separate, false)
        .map_err(|_| format!("official sqld {phase} child failed"))?;
    if !done.status.success() {
        return Err(format!("official sqld {phase} failed"));
    }
    Ok(done.stdout)
}

pub(super) fn verify(path: &Path) -> Result<(), String> {
    let digest = file_sha256(path).map_err(|_| "official sqld executable digest unavailable")?;
    official::verify(path, &digest, std::env::consts::ARCH)?;
    let version = command(Command::new(path).arg("--version"), "version")?;
    if version != b"sqld sqld 0.24.32 (40c272de 2025-02-14)\n" {
        return Err("official sqld version mismatch".into());
    }
    Ok(())
}

pub(super) fn prepare(directory: &Path) -> Result<(), String> {
    if !directory.is_absolute() {
        return Err("official sqld preparation needs an absolute new directory".into());
    }
    let pin = pin(std::env::consts::ARCH)?;
    fs::create_dir(directory)
        .map_err(|_| "official sqld preparation directory already exists or unavailable")?;
    let archive = directory.join("sqld.tar.xz");
    let member = format!("libsql-server-{}/sqld", pin.triple);
    let url = format!("https://github.com/tursodatabase/libsql/releases/download/libsql-server-v0.24.32/libsql-server-{}.tar.xz", pin.triple);
    let result = (|| {
        command(
            Command::new("curl")
                .args([
                    "--fail",
                    "--silent",
                    "--show-error",
                    "--location",
                    "--proto",
                    "=https",
                    "--proto-redir",
                    "=https",
                    "--output",
                ])
                .arg(&archive)
                .arg(&url),
            "download",
        )?;
        if file_sha256(&archive).ok().as_deref() != Some(pin.archive) {
            return Err("official sqld archive digest mismatch".into());
        }
        // A single named regular executable from the authenticated archive;
        // the standard tar reader owns xz/header decoding.
        command(
            Command::new("tar")
                .arg("--extract")
                .arg("--xz")
                .arg("--file")
                .arg(&archive)
                .arg("--directory")
                .arg(directory)
                .arg("--no-same-owner")
                .arg("--no-same-permissions")
                .arg(&member),
            "extract",
        )?;
        fs::rename(directory.join(&member), directory.join("sqld"))
            .map_err(|_| "official sqld extraction failed")?;
        verify(&directory.join("sqld"))?;
        println!("official sqld 0.24.32 native archive and executable digests verified");
        Ok(())
    })();
    if let Err(error) = result {
        return match fs::remove_dir_all(directory) {
            Ok(()) => Err(error),
            Err(_) => Err(format!("{error}; official sqld cleanup failed")),
        };
    }
    result
}

#[cfg(test)]
mod tests {
    use super::official;
    use std::fs;

    #[test]
    fn official_pins_require_the_selected_architecture_and_exact_digest() {
        let root = crate::host::mkdtemp("sqld-identity-", &std::env::temp_dir()).unwrap();
        let path = root.join("sqld");
        for (arch, machine, other) in [("x86_64", 62u16, "aarch64"), ("aarch64", 183, "x86_64")] {
            let pin = official::pin(arch).unwrap();
            let mut bytes = [0u8; 20];
            bytes[..6].copy_from_slice(b"\x7fELF\x02\x01");
            bytes[18..20].copy_from_slice(&machine.to_le_bytes());
            fs::write(&path, bytes).unwrap();
            official::verify(&path, pin.binary, arch).unwrap();
            assert!(official::verify(&path, "wrong", arch)
                .unwrap_err()
                .contains("digest"));
            assert!(official::verify(&path, official::pin(other).unwrap().binary, arch).is_err());
            assert!(
                official::verify(&path, official::pin(other).unwrap().binary, other)
                    .unwrap_err()
                    .contains("ELF64")
            );
            for (offset, value) in [(0, 0), (4, 1), (5, 2), (18, 0)] {
                let mut bad = bytes;
                bad[offset] = value;
                fs::write(&path, bad).unwrap();
                assert!(official::verify(&path, pin.binary, arch)
                    .unwrap_err()
                    .contains("ELF64"));
            }
        }
        assert!(official::pin("riscv64").is_err());
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn official_identity_refuses_symlink_and_relative_inputs() {
        let root = crate::host::mkdtemp("sqld-path-", &std::env::temp_dir()).unwrap();
        let path = root.join("sqld");
        fs::write(&path, b"unused").unwrap();
        let link = root.join("link");
        std::os::unix::fs::symlink(&path, &link).unwrap();
        let pin = official::pin(std::env::consts::ARCH).unwrap();
        for input in [&link, std::path::Path::new("sqld")] {
            assert!(official::verify(input, pin.binary, std::env::consts::ARCH)
                .unwrap_err()
                .contains("physical absolute"));
        }
        fs::remove_dir_all(root).unwrap();
    }
}
