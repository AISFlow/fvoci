//! Shared producer/SDK-fixture identity for official libsql-server-v0.24.32.

use std::fs::File;
use std::io::Read;
use std::path::Path;

pub struct Pin {
    // Download metadata is consumed by xtask; the SDK fixture verifies the
    // already-extracted executable through the same pin.
    #[allow(dead_code)]
    pub triple: &'static str,
    #[allow(dead_code)]
    pub archive: &'static str,
    pub binary: &'static str,
    machine: u16,
}

pub fn pin(arch: &str) -> Result<Pin, &'static str> {
    match arch {
        "x86_64" => Ok(Pin {
            triple: "x86_64-unknown-linux-gnu",
            archive: "71720fc8648c19efef416efebd47145ef59b62e198770533530a858e1336879f",
            binary: "0863c3fbe68ac9714bca2cec1330def7a0ba5e4a29f199bf60ef46fa0c95b895",
            machine: 62,
        }),
        "aarch64" => Ok(Pin {
            triple: "aarch64-unknown-linux-gnu",
            archive: "37f9eee45b388a30192907ecf4565b93df945c079331657073b5b3caf8bb1cd0",
            binary: "54039931c1088483706790e6cf73444ad88b843a9bb0ca8285b82fc309ad4810",
            machine: 183,
        }),
        _ => Err("official sqld is not pinned for this architecture"),
    }
}

pub fn verify(path: &Path, digest: &str, arch: &str) -> Result<(), &'static str> {
    let pin = pin(arch)?;
    if path.canonicalize().ok().as_deref() != Some(path) {
        return Err("official sqld input must be a physical absolute path");
    }
    if digest != pin.binary {
        return Err("official sqld executable digest mismatch");
    }
    let mut header = [0u8; 20];
    File::open(path)
        .and_then(|mut file| file.read_exact(&mut header))
        .map_err(|_| "official sqld ELF header unavailable")?;
    if &header[..6] != b"\x7fELF\x02\x01"
        || u16::from_le_bytes([header[18], header[19]]) != pin.machine
    {
        return Err("official sqld input is not native ELF64");
    }
    Ok(())
}
