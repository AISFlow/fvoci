//! Canonicalize `lib/rustlib/components` in place while proving that the
//! public installed set, the Rustup executable and every other toolchain
//! entry (bytes and identity) are unchanged, writing receipts before and
//! after the single in-place write.

use super::guard::{self, identity, Identity, Owner};
use super::scope::Context;
use super::{canonical, json, require, Refusal, PUBLIC_ROWS, RELATIVE, ROWS, TOOLCHAIN};
use crate::host::sha256_hex;
use serde_json::{json, Map, Value};
use sha2::{Digest, Sha256};
use std::collections::BTreeSet;
use std::fs::{self, DirBuilder, File, OpenOptions};
use std::io::{self, Read, Seek, SeekFrom, Write};
use std::os::unix::fs::{DirBuilderExt, MetadataExt, OpenOptionsExt};
use std::path::{Path, PathBuf};

const TOOLCHAIN_BYTE_BOUND: u64 = 4 * 1024 * 1024 * 1024;
const TOOLCHAIN_ENTRY_BOUND: usize = 50_000;

/// Points between steps where tests interleave a concurrent change.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Point {
    ComponentOpen,
    Receipt(&'static str),
    ReceiptWritten(&'static str),
}

/// `rustup component list --installed` and the step points; the process
/// implementation runs the real Rustup and ignores the points.
pub trait Inspect {
    fn component_list(&mut self) -> Result<String, Refusal>;
    fn checkpoint(&mut self, _point: Point) -> Result<(), Refusal> {
        Ok(())
    }
}

/// The 136-byte LF-terminated components file holding exactly the four rows.
fn exact_rows(raw: &[u8]) -> Result<Vec<String>, Refusal> {
    require(
        raw.len() == 136 && raw.ends_with(b"\n") && !raw.contains(&b'\r') && raw.is_ascii(),
        "components-byte-format",
    )?;
    let text = std::str::from_utf8(&raw[..raw.len() - 1]).expect("ASCII is UTF-8");
    let rows: Vec<String> = text.split('\n').map(str::to_owned).collect();
    let set: BTreeSet<&str> = rows.iter().map(String::as_str).collect();
    require(
        rows.len() == 4 && set.len() == 4 && set == BTreeSet::from(ROWS),
        "components-set",
    )?;
    Ok(rows)
}

/// Diagnostic JSON line (fixed public labels, counts and the raw hash only,
/// never unknown output) and the sorted public installed set.
fn installed_rows(text: &str) -> (String, Result<Vec<String>, Refusal>) {
    let rows: Vec<&str> = text
        .strip_suffix('\n')
        .unwrap_or(text)
        .split('\n')
        .collect();
    let set: BTreeSet<&str> = rows.iter().copied().collect();
    let public = BTreeSet::from(PUBLIC_ROWS);
    let recognized: Vec<String> = set
        .intersection(&public)
        .map(|row| format!("\"{row}\""))
        .collect();
    let unknown = rows.iter().filter(|row| !public.contains(*row)).count();
    let line = format!(
        "{{\"rustup_installed\": {{\"recognized\": [{}], \"row_count\": {}, \"unknown_count\": {}, \"raw_sha256\": \"{}\"}}}}",
        recognized.join(", "),
        rows.len(),
        unknown,
        sha256_hex(text.as_bytes())
    );
    let result = require(
        rows.len() == 4 && set.len() == 4 && set == public,
        "public-installed-set",
    )
    .map(|()| {
        let mut sorted: Vec<String> = rows.iter().map(|row| (*row).to_owned()).collect();
        sorted.sort();
        sorted
    });
    (line, result)
}

fn installed(inspect: &mut dyn Inspect, out: &mut dyn Write) -> Result<Vec<String>, Refusal> {
    let (line, rows) = installed_rows(&inspect.component_list()?);
    writeln!(out, "{line}")?;
    rows
}

#[derive(Debug, PartialEq)]
pub struct Closure {
    entries: Vec<Value>,
    regular_file_bytes: u64,
    sha256: String,
}

impl Closure {
    fn to_json(&self) -> Value {
        json!({
            "entries": self.entries,
            "entry_count": self.entries.len(),
            "regular_file_bytes": self.regular_file_bytes,
            "sha256": self.sha256,
        })
    }
}

/// Every path under `root`, without following symlinks; any unreadable
/// directory refuses.
fn walk(directory: &Path, found: &mut Vec<PathBuf>) -> io::Result<()> {
    for entry in fs::read_dir(directory)? {
        let entry = entry?;
        let path = entry.path();
        let is_directory = entry.file_type()?.is_dir();
        found.push(path.clone());
        if is_directory {
            walk(&path, found)?;
        }
    }
    Ok(())
}

fn sha256_file(path: &Path, info: &fs::Metadata) -> Result<String, Refusal> {
    let mut stream = OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
        .open(path)?;
    require(
        identity(&stream.metadata()?) == identity(info),
        "closure-file-race",
    )?;
    let mut digest = Sha256::new();
    let mut chunk = vec![0; 1024 * 1024];
    loop {
        let read = stream.read(&mut chunk)?;
        if read == 0 {
            break;
        }
        digest.update(&chunk[..read]);
    }
    require(
        identity(&fs::symlink_metadata(path)?) == identity(info),
        "closure-file-race",
    )?;
    Ok(crate::host::hex(&digest.finalize()))
}

/// Path, identity and content (regular files) or target (symlinks) of every
/// toolchain entry except the components file, in component-wise path order.
pub fn closure(root: &Path) -> Result<Closure, Refusal> {
    let mut paths = Vec::new();
    walk(root, &mut paths)?;
    paths.sort();
    let (mut entries, mut total) = (Vec::new(), 0u64);
    for path in paths {
        let relative = path
            .strip_prefix(root)
            .expect("walked under root")
            .to_str()
            .ok_or(Refusal::Reason("toolchain-entry-name"))?
            .to_owned();
        if relative == RELATIVE {
            continue;
        }
        let info = fs::symlink_metadata(&path)?;
        let mut facts = vec![Value::from(relative)];
        facts.extend(identity(&info).map(Value::from));
        let kind = info.file_type();
        if kind.is_file() {
            total += info.size();
            require(total <= TOOLCHAIN_BYTE_BOUND, "toolchain-byte-bound")?;
            let digest = sha256_file(&path, &info)?;
            facts.extend([Value::from(info.size()), Value::from(digest)]);
        } else if kind.is_symlink() {
            // Dangling, looping and outside targets all refuse.
            let inside = fs::canonicalize(&path).is_ok_and(|target| target.starts_with(root));
            require(inside, "external-toolchain-symlink")?;
            let target = fs::read_link(&path)?
                .into_os_string()
                .into_string()
                .map_err(|_| Refusal::Reason("toolchain-entry-name"))?;
            facts.push(Value::from(target));
        } else {
            require(kind.is_dir(), "unsupported-toolchain-entry")?;
        }
        entries.push(Value::Array(facts));
        require(
            entries.len() <= TOOLCHAIN_ENTRY_BOUND,
            "toolchain-entry-bound",
        )?;
    }
    let sha256 = sha256_hex(json::compact_list(&entries).as_bytes());
    Ok(Closure {
        entries,
        regular_file_bytes: total,
        sha256,
    })
}

fn receipt_file(directory: &Path, name: &str, raw: &[u8]) -> Result<(), Refusal> {
    let mut stream = OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .custom_flags(libc::O_NOFOLLOW)
        .open(directory.join(name))?;
    require(stream.metadata()?.mode() & 0o7777 == 0o600, "receipt-mode")?;
    stream.write_all(raw)?;
    stream.flush()?;
    stream.sync_all()?;
    Ok(())
}

fn receipt(
    inspect: &mut dyn Inspect,
    directory: &Path,
    name: &'static str,
    raw: &[u8],
) -> Result<(), Refusal> {
    inspect.checkpoint(Point::Receipt(name))?;
    receipt_file(directory, name, raw)?;
    inspect.checkpoint(Point::ReceiptWritten(name))
}

fn read_upto(stream: &mut File, limit: u64) -> io::Result<Vec<u8>> {
    let mut raw = Vec::new();
    stream.take(limit).read_to_end(&mut raw)?;
    Ok(raw)
}

fn read_all(stream: &mut File) -> io::Result<Vec<u8>> {
    stream.seek(SeekFrom::Start(0))?;
    let mut raw = Vec::new();
    stream.read_to_end(&mut raw)?;
    Ok(raw)
}

fn open_component(path: &Path, write: bool) -> io::Result<File> {
    OpenOptions::new()
        .read(true)
        .write(write)
        .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
        .open(path)
}

fn rustup_facts(rustup: &Path) -> Result<(String, Identity), Refusal> {
    guard::file_facts(rustup)
}

fn object(value: Value) -> Map<String, Value> {
    match value {
        Value::Object(map) => map,
        _ => unreachable!("json! object"),
    }
}

/// Returns the `after.json` fields.
pub fn prepare(
    root: &Path,
    output: &Path,
    rustup: &Path,
    context: &Context,
    owner: Owner,
    inspect: &mut dyn Inspect,
    out: &mut dyn Write,
) -> Result<Map<String, Value>, Refusal> {
    guard::owned_directory(root, owner)?;
    guard::owned_directory(&root.join("lib"), owner)?;
    guard::owned_directory(&root.join("lib/rustlib"), owner)?;
    let component = root.join(RELATIVE);
    let original = identity(&guard::regular(&component, Some(0o644), Some(owner))?);
    let schema = root.join("lib/rustlib/rust-installer-version");
    guard::regular(&schema, None, Some(owner))?;
    let schema_bytes = read_upto(&mut open_component(&schema, false)?, 4096)?;
    require(
        schema_bytes == b"3" || schema_bytes == b"3\n",
        "unsupported-installer-schema",
    )?;
    for name in ROWS {
        let manifest = root.join("lib/rustlib").join(format!("manifest-{name}"));
        let info = guard::regular(&manifest, None, Some(owner))?;
        require(info.size() > 0, "missing-installed-manifest")?;
    }
    let before_set = installed(inspect, out)?;
    let before_closure = closure(root)?;
    let before_rustup = rustup_facts(rustup)?;
    require(
        before_rustup == (context.rustup_sha256.clone(), context.rustup_identity),
        "rustup-identity-drift",
    )?;
    guard::owned_directory(output.parent().unwrap_or(Path::new("")), owner)?;
    let canonical = canonical();
    inspect.checkpoint(Point::ComponentOpen)?;
    let raw = {
        let mut current = open_component(&component, true)?;
        require(
            identity(&current.metadata()?) == original,
            "components-fd-race",
        )?;
        let raw = read_upto(&mut current, 4096)?;
        let order = exact_rows(&raw)?;
        // A plain mkdir refuses every existing destination, including
        // symlinks and files.
        DirBuilder::new().mode(0o700).create(output)?;
        let receipts = guard::owned_directory(output, owner)?;
        require(
            fs::symlink_metadata(&receipts)?.mode() & 0o7777 == 0o700,
            "receipt-directory-mode",
        )?;
        receipt(inspect, output, "original-components.txt", &raw)?;
        let mut before = context.fields();
        before.extend(object(json!({
            "toolchain": TOOLCHAIN,
            "installer_schema": 3,
            "original_order": order,
            "original_sha256": sha256_hex(&raw),
            "component_identity": original,
            "public_installed": before_set,
            "other_toolchain_inputs": before_closure.to_json(),
        })));
        let before = json::indented(&Value::Object(before));
        receipt(inspect, output, "before.json", before.as_bytes())?;
        require(
            identity(&fs::symlink_metadata(&component)?) == original,
            "components-path-race",
        )?;
        require(read_all(&mut current)? == raw, "components-byte-race")?;
        if raw != canonical {
            current.seek(SeekFrom::Start(0))?;
            current.write_all(&canonical)?;
            current.flush()?;
            current.sync_all()?;
        }
        require(
            read_all(&mut current)? == canonical,
            "components-write-verification",
        )?;
        require(
            identity(&current.metadata()?) == original
                && identity(&fs::symlink_metadata(&component)?) == original,
            "components-identity-drift",
        )?;
        raw
    };
    require(
        installed(inspect, out)? == before_set,
        "public-installed-set-drift",
    )?;
    require(
        rustup_facts(rustup)? == before_rustup,
        "rustup-identity-drift",
    )?;
    let after_closure = closure(root)?;
    require(
        after_closure == before_closure,
        "compiled-toolchain-input-drift",
    )?;
    let mut last = open_component(&component, false)?;
    require(
        identity(&last.metadata()?) == original
            && identity(&fs::symlink_metadata(&component)?) == original,
        "components-final-identity-drift",
    )?;
    require(
        read_upto(&mut last, 4096)? == canonical,
        "components-final-byte-drift",
    )?;
    let mut after = context.fields();
    after.extend(object(json!({
        "changed": raw != canonical,
        "canonical_order": ROWS,
        "canonical_sha256": sha256_hex(&canonical),
        "component_identity": identity(&fs::symlink_metadata(&component)?),
        "other_toolchain_inputs_sha256": after_closure.sha256,
        "other_toolchain_entry_count": after_closure.entries.len(),
        "other_toolchain_regular_file_bytes": after_closure.regular_file_bytes,
        "public_installed_set_unchanged": true,
        "compiled_toolchain_inputs_unchanged": true,
    })));
    receipt(
        inspect,
        output,
        "after.json",
        json::indented(&Value::Object(after.clone())).as_bytes(),
    )?;
    Ok(after)
}

#[cfg(test)]
mod tests;
