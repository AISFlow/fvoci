//! Official sqld release inputs. Both tarball and executable digests come
//! from libsql-server-v0.24.32's published assets; no mutable tag/cache trust.

use super::native_elf;
use crate::ci_fixture::{capture, Streams};
use crate::rust_binaries_archive::file_sha256;
use std::fs;
use std::path::Path;
use std::process::Command;

pub(super) struct Pin {
    pub triple: &'static str,
    pub archive: &'static str,
    pub binary: &'static str,
}

pub(super) fn pin(arch: &str) -> Result<Pin, String> {
    match arch {
        "x86_64" => Ok(Pin {
            triple: "x86_64-unknown-linux-gnu",
            archive: "71720fc8648c19efef416efebd47145ef59b62e198770533530a858e1336879f",
            binary: "0863c3fbe68ac9714bca2cec1330def7a0ba5e4a29f199bf60ef46fa0c95b895",
        }),
        "aarch64" => Ok(Pin {
            triple: "aarch64-unknown-linux-gnu",
            archive: "37f9eee45b388a30192907ecf4565b93df945c079331657073b5b3caf8bb1cd0",
            binary: "54039931c1088483706790e6cf73444ad88b843a9bb0ca8285b82fc309ad4810",
        }),
        _ => Err("official sqld is not pinned for this architecture".into()),
    }
}

fn command(command: &mut Command, phase: &str) -> Result<Vec<u8>, String> {
    let done = capture(command, None, Streams::Separate, false)
        .map_err(|_| format!("official sqld {phase} child failed"))?;
    if !done.status.success() {
        return Err(format!("official sqld {phase} failed"));
    }
    Ok(done.stdout)
}

pub(super) fn verify(path: &Path) -> Result<(), String> {
    let pin = pin(std::env::consts::ARCH)?;
    if file_sha256(path).ok().as_deref() != Some(pin.binary) {
        return Err("official sqld executable digest mismatch".into());
    }
    native_elf(path)?;
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
