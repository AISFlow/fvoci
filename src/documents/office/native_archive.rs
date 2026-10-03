//! All untrusted container work runs in the existing credential-free child.
use docx_zip::{
    read::{read_zipfile_from_stream, HasZipMetadata},
    write::SimpleFileOptions,
    CompressionMethod, ZipArchive, ZipWriter,
};
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet};
use std::io::{Cursor, Read, Write};

use crate::native_archive::{self as model, Archive, ArchiveError, EntryDigest, Graph, Manifest};

const MODELS: &[&str] = &[
    "project",
    "workflow",
    "status",
    "document",
    "task",
    "task-origin",
    "task-assignee",
    "task-creation-activity",
    "native-state-tail-receipt",
    "revision",
    "attachment",
    "task-collection-baseline",
    "task-label",
    "task-changed-activity",
    "comment",
    "wiki-document",
    "zotero-connector",
    "zotero-reference",
    "zotero-collection",
    "zotero-membership",
    "zotero-link",
    "task-origin-wiki",
    "personal-input-receipt-retired",
    "time-entry",
    "task-timer",
    "task-timer-receipt-retired",
    "task-timer-audit",
    "milestone",
    "task-dependency",
    "project-view",
    "collection-field",
    "collection-value",
    "collection-view",
    "document-tag",
];
const POLICY: &str = "preserve-content-ids-fresh-private-workspace";

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn native_archive_container_keeps_literal_unicode_and_content_ids() {
        let archive = crate::native_archive::tests::policy_fixture();
        let bytes = pack_archive(&archive).unwrap();
        let decoded = read_archive(&bytes).unwrap();
        assert_eq!(decoded.graph.project.name, "원본 프로젝트 🧪");
        assert_eq!(
            decoded.graph.documents[0].id.to_string(),
            "10000000-0000-4000-8000-000000000002"
        );
        assert_eq!(
            decoded.graph.activity[0].id.to_string(),
            "10000000-0000-4000-8000-000000000006"
        );
        assert_eq!(decoded.entries, archive.entries);
        // This tests the typed container only, never native history restoration.
    }

    #[test]
    fn native_archive_container_denies_alias_unknown_entry_and_manifest_version() {
        fn container(entries: &[(&str, &[u8])]) -> Vec<u8> {
            let mut zip = ZipWriter::new(Cursor::new(Vec::new()));
            for (name, data) in entries {
                zip.start_file(*name, SimpleFileOptions::default()).unwrap();
                zip.write_all(data).unwrap();
            }
            zip.finish().unwrap().into_inner()
        }
        for name in [
            "../graph.json",
            "graph\\json",
            "/graph.json",
            "unknown.json",
        ] {
            assert!(read_archive(&container(&[(name, b"{}")])).is_err());
        }
        let archive = crate::native_archive::tests::policy_fixture();
        let valid = pack_archive(&archive).unwrap();
        let mut original = ZipArchive::new(Cursor::new(valid)).unwrap();
        let mut entries = Vec::new();
        for index in 0..original.len() {
            let mut file = original.by_index(index).unwrap();
            let name = file.name().to_string();
            let mut data = Vec::new();
            file.read_to_end(&mut data).unwrap();
            if name == "manifest.json" {
                let mut manifest: serde_json::Value = serde_json::from_slice(&data).unwrap();
                manifest["format_version"] = serde_json::json!(99);
                data = serde_json::to_vec(&manifest).unwrap();
            }
            entries.push((name, data));
        }
        let refs: Vec<_> = entries
            .iter()
            .map(|(name, data)| (name.as_str(), data.as_slice()))
            .collect();
        assert!(read_archive(&container(&refs)).is_err());
    }

    #[test]
    fn native_archive_container_denies_duplicate_records_hidden_by_name_index() {
        let archive = crate::native_archive::tests::policy_fixture();
        let valid = pack_archive(&archive).unwrap();
        let mut original = ZipArchive::new(Cursor::new(valid)).unwrap();
        let mut entries = Vec::new();
        let mut manifest = Vec::new();
        for index in 0..original.len() {
            let mut file = original.by_index(index).unwrap();
            let name = file.name().to_string();
            let mut data = Vec::new();
            file.read_to_end(&mut data).unwrap();
            if name == "manifest.json" {
                manifest = data.clone();
            }
            entries.push((name, data));
        }
        let mut zip = ZipWriter::new(Cursor::new(Vec::new()));
        entries.push(("manifest.jsoN".to_owned(), manifest));
        for (name, data) in entries {
            zip.start_file(name, SimpleFileOptions::default()).unwrap();
            zip.write_all(&data).unwrap();
        }
        let mut bytes = zip.finish().unwrap().into_inner();
        // Deliberately malformed fixture only: equal-length local and central
        // names alias an additional record. Payloads/CRC remain unchanged.
        for offset in 0..bytes.len().saturating_sub(12) {
            if &bytes[offset..offset + 13] == b"manifest.jsoN" {
                bytes[offset + 12] = b'n';
            }
        }
        assert!(read_archive(&bytes).is_err());
    }
}

pub(super) fn process(
    source: &[u8],
    pack: bool,
    max_output: usize,
) -> Result<String, ArchiveError> {
    if pack {
        let archive: Archive = serde_json::from_slice(source)
            .map_err(|_| ArchiveError::Invalid("child input".into()))?;
        let output = model::encode(&pack_archive(&archive)?);
        if output.len() > max_output {
            return Err(ArchiveError::Limit);
        }
        Ok(output)
    } else {
        let archive = read_archive(source)?;
        let output = serde_json::to_string(&archive)
            .map_err(|_| ArchiveError::Invalid("child output".into()))?;
        if output.len() > max_output {
            return Err(ArchiveError::Limit);
        }
        Ok(output)
    }
}

pub(crate) fn pack_archive(archive: &Archive) -> Result<Vec<u8>, ArchiveError> {
    archive.validate()?;
    let mut entries = BTreeMap::new();
    entries.insert(
        "graph.json".to_string(),
        serde_json::to_vec(&archive.graph).map_err(|_| invalid())?,
    );
    for name in archive.entries.keys() {
        entries.insert(name.clone(), archive.bytes(name)?);
    }
    let manifest = Manifest {
        kind: model::KIND.into(),
        format_version: 1,
        native_encoding: 1,
        complete: true,
        required_models: MODELS.iter().map(|s| (*s).into()).collect(),
        mapping_policy: POLICY.into(),
        captured_at: archive.graph.captured_at.clone(),
        entries: entries
            .iter()
            .map(|(name, bytes)| {
                (
                    name.clone(),
                    EntryDigest {
                        size: bytes.len() as u64,
                        sha256: model::digest(bytes),
                    },
                )
            })
            .collect(),
    };
    entries.insert(
        "manifest.json".into(),
        serde_json::to_vec(&manifest).map_err(|_| invalid())?,
    );
    let mut zip = ZipWriter::new(Cursor::new(Vec::new()));
    let options = SimpleFileOptions::default()
        .compression_method(CompressionMethod::Stored)
        .large_file(false)
        .unix_permissions(0o600);
    let mut budget = 0usize;
    for (name, bytes) in entries {
        budget = budget.checked_add(bytes.len()).ok_or(ArchiveError::Limit)?;
        if budget > model::MAX_BYTES {
            return Err(ArchiveError::Limit);
        }
        zip.start_file(name, options).map_err(|_| invalid())?;
        zip.write_all(&bytes).map_err(|_| invalid())?;
    }
    let bytes = zip.finish().map_err(|_| invalid())?.into_inner();
    if bytes.len() > model::MAX_BYTES {
        return Err(ArchiveError::Limit);
    }
    Ok(bytes)
}

pub(crate) fn read_archive(bytes: &[u8]) -> Result<Archive, ArchiveError> {
    if bytes.len() > model::MAX_BYTES {
        return Err(ArchiveError::Limit);
    }
    let mut zip = ZipArchive::new(Cursor::new(bytes)).map_err(|_| invalid())?;
    // The locked reader exposes this deprecated nonstandard trailer accessor;
    // we use it solely to refuse that trailer, never to support its format.
    #[allow(deprecated)]
    let has_zip64_comment = zip.zip64_comment().is_some();
    if zip.offset() != 0
        || zip.len() > model::MAX_ENTRIES
        || zip.raw_zip64_extensible_data_sector().is_some()
        || has_zip64_comment
        || !zip.comment().is_empty()
    {
        return Err(invalid());
    }
    let mut entries = BTreeMap::new();
    let mut local_spans = Vec::new();
    let mut central_spans = Vec::new();
    let central_start = zip.central_directory_start();
    let mut total = 0u64;
    for index in 0..zip.len() {
        let mut file = zip.by_index(index).map_err(|_| invalid())?;
        let metadata = file.get_metadata();
        let name = file.name().to_string();
        if file.encrypted()
            || metadata.large_file
            || metadata.using_data_descriptor
            || !file.comment().is_empty()
            || file.extra_data_fields().next().is_some()
            || !file.extra_data().unwrap_or_default().is_empty()
            || !metadata
                .central_extra_field
                .as_deref()
                .unwrap_or_default()
                .is_empty()
            || file.is_dir()
            || file.is_symlink()
            || file
                .unix_mode()
                .is_some_and(|m| m & 0o170000 != 0 && m & 0o170000 != 0o100000)
            || !matches!(
                file.compression(),
                CompressionMethod::Stored | CompressionMethod::Deflated
            )
            || name.contains(['\\', '\0'])
            || name.starts_with('/')
            || name
                .split('/')
                .any(|part| part.is_empty() || part == "." || part == "..")
            || name.as_bytes() != file.name_raw()
            || entries.contains_key(&name)
        {
            return Err(invalid());
        }
        let data_start = file.data_start().ok_or_else(invalid)?;
        local_spans.push((
            file.header_start(),
            data_start
                .checked_add(file.compressed_size())
                .ok_or_else(invalid)?,
        ));
        central_spans.push((
            file.central_header_start(),
            file.central_header_start()
                .checked_add(46 + file.name_raw().len() as u64)
                .ok_or_else(invalid)?,
        ));
        let cap = if name == "manifest.json" {
            1024 * 1024
        } else if name == "graph.json" {
            model::MAX_GRAPH_BYTES
        } else {
            model::MAX_BYTES
        };
        total = total.checked_add(file.size()).ok_or(ArchiveError::Limit)?;
        if file.size() > cap as u64 || total > model::MAX_BYTES as u64 {
            return Err(ArchiveError::Limit);
        }
        let mut output = Vec::new();
        (&mut file)
            .take(cap as u64 + 1)
            .read_to_end(&mut output)
            .map_err(|_| invalid())?;
        if output.len() as u64 != file.size() || output.len() > cap {
            return Err(invalid());
        }
        entries.insert(name, output);
    }
    verify_spans(&mut local_spans, 0, central_start)?;
    let central_end = bytes.len().checked_sub(22).ok_or_else(invalid)? as u64;
    verify_spans(&mut central_spans, central_start, central_end)?;
    verify_record_inventory(bytes, &entries)?;
    let manifest: Manifest =
        serde_json::from_slice(&entries.remove("manifest.json").ok_or_else(invalid)?)
            .map_err(|_| invalid())?;
    if manifest.kind != model::KIND
        || manifest.format_version != 1
        || manifest.native_encoding != 1
        || !manifest.complete
        || manifest.mapping_policy != POLICY
        || manifest.required_models != MODELS.iter().map(|s| (*s).to_string()).collect::<Vec<_>>()
    {
        return Err(ArchiveError::Unsupported(
            "format version or required models".into(),
        ));
    }
    if manifest.entries.len() != entries.len() {
        return Err(invalid());
    }
    for (name, bytes) in &entries {
        let expected = manifest.entries.get(name).ok_or_else(invalid)?;
        if expected.size != bytes.len() as u64 || expected.sha256 != model::digest(bytes) {
            return Err(invalid());
        }
    }
    let graph: Graph = serde_json::from_slice(&entries.remove("graph.json").ok_or_else(invalid)?)
        .map_err(|_| invalid())?;
    if graph.captured_at != manifest.captured_at {
        return Err(invalid());
    }
    let archive = Archive {
        graph,
        entries: entries
            .into_iter()
            .map(|(name, bytes)| (name, model::encode(&bytes)))
            .collect(),
    };
    archive.validate()?;
    Ok(archive)
}

/// Require contiguous library-reported record spans in this strict ZIP32
/// profile (no comments, extras, data descriptors or trailing data). Fixed
/// 46-byte central and 22-byte end-record lengths are ZIP32 policy constants;
/// every field/offset/content was parsed by the maintained library.
fn verify_spans(spans: &mut [(u64, u64)], start: u64, end: u64) -> Result<(), ArchiveError> {
    spans.sort_unstable();
    let mut next = start;
    for &(begin, finish) in spans.iter() {
        if begin != next || finish <= begin {
            return Err(invalid());
        }
        next = finish;
    }
    if next != end {
        return Err(invalid());
    }
    Ok(())
}

/// ZipArchive's name index collapses duplicate central records. Contiguous
/// central span coverage above refuses those omissions; this maintained
/// streaming reader independently checks every local name and payload.
fn verify_record_inventory(
    bytes: &[u8],
    entries: &BTreeMap<String, Vec<u8>>,
) -> Result<(), ArchiveError> {
    let mut cursor = Cursor::new(bytes);
    let mut seen = BTreeSet::new();
    while let Some(mut file) = read_zipfile_from_stream(&mut cursor).map_err(|_| invalid())? {
        let name = file.name().to_owned();
        let expected = entries.get(&name).ok_or_else(invalid)?;
        if !seen.insert(name)
            || file.name_raw() != file.name().as_bytes()
            || file.size() != expected.len() as u64
            || file.encrypted()
            || file.get_metadata().large_file
            || file.get_metadata().using_data_descriptor
            || !file.extra_data().unwrap_or_default().is_empty()
            || file.extra_data_fields().next().is_some()
        {
            return Err(invalid());
        }
        let mut digest = Sha256::new();
        let mut count = 0usize;
        let mut buffer = [0u8; 8192];
        loop {
            let read = file.read(&mut buffer).map_err(|_| invalid())?;
            if read == 0 {
                break;
            }
            count = count.checked_add(read).ok_or_else(invalid)?;
            if count > expected.len() {
                return Err(ArchiveError::Limit);
            }
            digest.update(&buffer[..read]);
        }
        if count != expected.len() || hex::encode(digest.finalize()) != model::digest(expected) {
            return Err(invalid());
        }
    }
    if seen != entries.keys().cloned().collect() {
        return Err(invalid());
    }
    Ok(())
}

fn invalid() -> ArchiveError {
    ArchiveError::Invalid("container, entry or hash".into())
}
