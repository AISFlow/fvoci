//! Sealed tar hand-off of finished executables: `manifest.json` (identity
//! context plus one entry per cohort member) followed by the executables.
//!
//! Unpacking validates every member, the manifest and every destination before
//! anything is written, and never falls back to a rebuild or cache. Tar header
//! parsing (checksums, PAX and GNU long names) is the `tar` crate's; only
//! member kinds and paths are policy here: regular files with workspace-relative
//! normal paths under a Cargo target directory.

use crate::host::{first_symlink, hex};
use crate::rust_binaries_cohort::{expectation, record_matches, Entry};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet};
use std::fs::{self, File, OpenOptions, Permissions};
use std::io::{self, Read, Seek, SeekFrom, Write};
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
use std::path::{Component, Path, PathBuf};
use tar::{Archive, Builder, EntryType, Header, HeaderMode};

pub const MANIFEST: &str = "manifest.json";
const MANIFEST_LIMIT: u64 = 4 * 1024 * 1024;
const TARGET_PREFIXES: [&str; 2] = ["target/", "crates/collab-engine/target/"];

/// Reader or writer adapter that hashes every byte passing through.
struct Hashing<T> {
    inner: T,
    hasher: Sha256,
}

impl<T> Hashing<T> {
    fn new(inner: T) -> Self {
        Self {
            inner,
            hasher: Sha256::new(),
        }
    }

    fn hex(self) -> String {
        hex(&self.hasher.finalize())
    }
}

impl<R: Read> Read for Hashing<R> {
    fn read(&mut self, buffer: &mut [u8]) -> io::Result<usize> {
        let read = self.inner.read(buffer)?;
        self.hasher.update(&buffer[..read]);
        Ok(read)
    }
}

impl<W: Write> Write for Hashing<W> {
    fn write(&mut self, buffer: &[u8]) -> io::Result<usize> {
        let written = self.inner.write(buffer)?;
        self.hasher.update(&buffer[..written]);
        Ok(written)
    }

    fn flush(&mut self) -> io::Result<()> {
        self.inner.flush()
    }
}

/// Stream `path`'s SHA-256. The final component must be a regular file, not a
/// symlink; a FIFO is refused without blocking on its open.
pub fn file_sha256(path: &Path) -> io::Result<String> {
    let file = OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
        .open(path)?;
    if !file.metadata()?.file_type().is_file() {
        return Err(io::Error::other("not a regular file"));
    }
    let mut reader = Hashing::new(file);
    io::copy(&mut reader, &mut io::sink())?;
    Ok(reader.hex())
}

/// Tar read errors can quote raw header bytes; report only the error kind.
fn unreadable(error: io::Error) -> String {
    format!("Rust binary archive unreadable ({:?})", error.kind())
}

fn io_error(error: io::Error) -> String {
    error.to_string()
}

/// Write a new archive at `output` (refused when it exists). Each executable
/// is hashed again while it is copied and must still match its entry; a failed
/// pack removes the partial archive.
pub fn pack(
    output: &Path,
    context: &Value,
    workspace: &Path,
    entries: &BTreeMap<String, Entry>,
) -> Result<(), String> {
    let file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(output)
        .map_err(|e| format!("{}: {e}", output.display()))?;
    let result = write_archive(file, context, workspace, entries);
    if result.is_err() {
        let _ = fs::remove_file(output);
    }
    result
}

fn write_archive(
    file: File,
    context: &Value,
    workspace: &Path,
    entries: &BTreeMap<String, Entry>,
) -> Result<(), String> {
    let entries_json: serde_json::Map<String, Value> = entries
        .iter()
        .map(|(name, entry)| (name.clone(), entry.to_json()))
        .collect();
    let manifest = json!({"version": 1, "context": context, "entries": entries_json});
    let raw = serde_json::to_vec(&manifest).map_err(|e| e.to_string())?;
    let mut builder = Builder::new(file);
    builder.mode(HeaderMode::Deterministic);
    let mut header = Header::new_gnu();
    header.set_entry_type(EntryType::Regular);
    header.set_size(raw.len() as u64);
    header.set_mode(0o644);
    builder
        .append_data(&mut header, MANIFEST, raw.as_slice())
        .map_err(io_error)?;
    for entry in entries.values() {
        let source = workspace.join(&entry.path);
        let file = File::open(&source).map_err(|e| format!("{}: {e}", source.display()))?;
        let metadata = file.metadata().map_err(io_error)?;
        if !metadata.file_type().is_file() {
            return Err("Rust binary executable is not physical regular file".into());
        }
        let mut header = Header::new_gnu();
        header.set_metadata_in_mode(&metadata, HeaderMode::Deterministic);
        let mut reader = Hashing::new(file.take(metadata.len()));
        builder
            .append_data(&mut header, &entry.path, &mut reader)
            .map_err(io_error)?;
        if reader.hex() != entry.sha256 {
            return Err("Rust binary changed after validation".into());
        }
    }
    builder
        .into_inner()
        .and_then(|file| file.sync_all())
        .map_err(io_error)
}

const NONREGULAR: &str = "Rust binary archive has duplicate or nonregular members";

/// Member name -> SHA-256 (empty for the manifest).
type Digests = BTreeMap<String, String>;

/// One pass over the archive: every member must be a uniquely named regular
/// file; returns each member's digest and the manifest bytes.
fn scan(file: &File) -> Result<(Digests, Option<Vec<u8>>), String> {
    let mut archive = Archive::new(file);
    let mut digests = Digests::new();
    let mut manifest = None;
    for entry in archive.entries_with_seek().map_err(unreadable)? {
        let entry = entry.map_err(unreadable)?;
        let name = String::from_utf8(entry.path_bytes().into_owned()).map_err(|_| NONREGULAR)?;
        if entry.header().entry_type() != EntryType::Regular || digests.contains_key(&name) {
            return Err(NONREGULAR.into());
        }
        if name == MANIFEST {
            if entry.size() > MANIFEST_LIMIT {
                return Err("Rust binary manifest too large".into());
            }
            let mut raw = Vec::new();
            entry
                .take(MANIFEST_LIMIT + 1)
                .read_to_end(&mut raw)
                .map_err(unreadable)?;
            manifest = Some(raw);
            digests.insert(name, String::new());
        } else {
            let mut reader = Hashing::new(entry);
            io::copy(&mut reader, &mut io::sink()).map_err(unreadable)?;
            digests.insert(name, reader.hex());
        }
    }
    if digests.is_empty() {
        return Err(NONREGULAR.into());
    }
    Ok((digests, manifest))
}

/// Normal relative components only, written canonically, under a Cargo target
/// directory.
fn safe_relative(path: &str) -> bool {
    let canonical = Path::new(path)
        .components()
        .map(|c| match c {
            Component::Normal(part) => part.to_str(),
            _ => None,
        })
        .collect::<Option<Vec<_>>>()
        .map(|parts| parts.join("/"));
    canonical.as_deref() == Some(path) && TARGET_PREFIXES.iter().any(|p| path.starts_with(p))
}

/// Validate `archive` against the expected `context` and cohort `names`, then
/// restore every executable under `root` (mode 0755, never overwriting).
/// Returns the manifest.
pub fn unpack(
    archive_path: &Path,
    context: &Value,
    names: &BTreeSet<String>,
    products: &[String],
    root: &Path,
) -> Result<Value, String> {
    let mut file =
        File::open(archive_path).map_err(|e| format!("{}: {e}", archive_path.display()))?;
    let (members, manifest) = scan(&file)?;
    let raw = manifest.ok_or("Rust binary archive has no manifest.json")?;
    let manifest: Value = serde_json::from_slice(&raw)
        .map_err(|e| format!("Rust binary manifest is not JSON: {e}"))?;
    if manifest["version"].as_u64() != Some(1) || manifest["context"] != *context {
        return Err("Rust binary source/platform/native inputs mismatch".into());
    }
    let entries = manifest["entries"]
        .as_object()
        .ok_or("Rust binary cohort missing or foreign executable")?;
    if entries.keys().cloned().collect::<BTreeSet<_>>() != *names {
        return Err("Rust binary cohort missing or foreign executable".into());
    }
    let inventory_mismatch = || "Rust binary archive member inventory mismatch".to_owned();
    let paths = entries
        .values()
        .map(|entry| entry["path"].as_str().map(str::to_owned))
        .collect::<Option<BTreeSet<_>>>()
        .ok_or_else(inventory_mismatch)?;
    let mut inventory = paths.clone();
    inventory.insert(MANIFEST.to_owned());
    if paths.len() != entries.len() || members.keys().cloned().collect::<BTreeSet<_>>() != inventory
    {
        return Err(inventory_mismatch());
    }
    let mut destinations: BTreeMap<String, PathBuf> = BTreeMap::new();
    for (name, entry) in entries {
        let relative = entry["path"].as_str().unwrap_or_default();
        if !safe_relative(relative) {
            return Err("Rust binary archive path escape".into());
        }
        let destination = root.join(relative);
        if first_symlink(&destination).is_some() || fs::symlink_metadata(&destination).is_ok() {
            return Err("Rust binary destination occupied or symlinked".into());
        }
        let expected = expectation(name, products);
        if entry["sha256"].as_str() != Some(members[relative].as_str())
            || !record_matches(&entry["record"], &expected)
            || entry["record"]["executable"].as_str() != destination.to_str()
        {
            return Err("Rust binary digest/feature/profile/path mismatch".into());
        }
        destinations.insert(relative.to_owned(), destination);
    }
    file.seek(SeekFrom::Start(0)).map_err(io_error)?;
    restore(&file, &members, &destinations)?;
    Ok(manifest)
}

/// Second pass: write each validated member and check its digest again; on any
/// failure the files written by this call are removed.
fn restore(
    file: &File,
    members: &Digests,
    destinations: &BTreeMap<String, PathBuf>,
) -> Result<(), String> {
    let mut written: Vec<PathBuf> = Vec::new();
    let result = (|| {
        let mut archive = Archive::new(file);
        for entry in archive.entries_with_seek().map_err(unreadable)? {
            let mut entry = entry.map_err(unreadable)?;
            let name = String::from_utf8_lossy(&entry.path_bytes()).into_owned();
            let Some(destination) = destinations.get(&name) else {
                continue;
            };
            if let Some(parent) = destination.parent() {
                fs::create_dir_all(parent).map_err(io_error)?;
            }
            if first_symlink(destination).is_some() {
                return Err("Rust binary destination occupied or symlinked".into());
            }
            let output = OpenOptions::new()
                .write(true)
                .create_new(true)
                .mode(0o755)
                .custom_flags(libc::O_NOFOLLOW)
                .open(destination)
                .map_err(|e| format!("{}: {e}", destination.display()))?;
            written.push(destination.clone());
            let mut writer = Hashing::new(output);
            // Separate archive read errors from destination write errors.
            let mut buffer = vec![0; 64 * 1024];
            loop {
                let read = match entry.read(&mut buffer) {
                    Err(e) if e.kind() == io::ErrorKind::Interrupted => continue,
                    result => result.map_err(unreadable)?,
                };
                if read == 0 {
                    break;
                }
                writer
                    .write_all(&buffer[..read])
                    .map_err(|e| format!("{}: {e}", destination.display()))?;
            }
            if writer.hex() != members[&name] {
                return Err("Rust binary digest/feature/profile/path mismatch".into());
            }
            fs::set_permissions(destination, Permissions::from_mode(0o755)).map_err(io_error)?;
        }
        if written.len() != destinations.len() {
            return Err("Rust binary archive member inventory mismatch".into());
        }
        Ok(())
    })();
    if result.is_err() {
        for path in &written {
            let _ = fs::remove_file(path);
        }
    }
    result
}

#[cfg(test)]
mod tests {
    use super::safe_relative;

    #[test]
    fn member_paths_must_be_canonical_and_under_target() {
        for good in [
            "target/db-tests/debug/deps/x",
            "crates/collab-engine/target/debug/collab-engine",
        ] {
            assert!(safe_relative(good), "{good}");
        }
        for bad in [
            "",
            "/target/x",
            "target/../target/x",
            "target/./x",
            "./target/x",
            "target//x",
            "target/x/",
            "src/x",
            "targetx/y",
            "target",
            "crates/collab-engine/../../target/x",
        ] {
            assert!(!safe_relative(bad), "{bad}");
        }
    }
}
