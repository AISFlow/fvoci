//! Offline backup manifest creation and restore preflight. No database or server setup.
use std::collections::BTreeMap;
use std::fs::{self, File};
use std::io::{self, Write};
use std::path::{Component, Path};

use chrono::{DateTime, Duration, NaiveDateTime, Utc};
use hmac::{Hmac, Mac};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::auth::password::Keyring;

type Result<T> = std::result::Result<T, String>;
const LABEL: &[u8] = b"fvoci:encryption-key-fingerprint:v1:";
const PEPPER_NOTE: &str = "SHA-256 of the canonical keyring and active id; keys are not stored. Restore refuses a different keyring because existing password hashes could not be verified.";
const ENCRYPTION_NOTE: &str = "Per key id HMAC-SHA256(key, label||id); keys are not stored. Restore needs every key id listed here with the same key (extra keys are fine) and then opens every sealed secret (fvoci-migrate --verify-secrets).";
const NO_ENCRYPTION_NOTE: &str = "ENCRYPTION_KEYS was not set; nothing could be sealed with it. Restore runs fvoci-migrate --verify-secrets regardless.";
const SEARCH_REASON: &str = "Meilisearch is derived. Restore runs fvoci-migrate --ensure-meili-key (scoped key and index settings) and --rebuild-search from PostgreSQL.";

#[derive(Serialize)]
struct CanonicalRing<'a> {
    keys: BTreeMap<&'a str, &'a str>,
    active: &'a str,
}

fn pepper_fingerprint(raw: &str, active: &str) -> Result<String> {
    Keyring::parse(raw, active).map_err(|_| "invalid PASSWORD_PEPPER_KEYS".to_string())?;
    // The v1 Python fingerprint hashes the original hex spelling, including case.
    let keys: BTreeMap<String, String> =
        serde_json::from_str(raw).map_err(|_| "invalid PASSWORD_PEPPER_KEYS".to_string())?;
    // The v1 Python serializer puts keys before active and sorts key ids.
    let canonical = serde_json::to_vec(&CanonicalRing {
        keys: keys.iter().map(|(k, v)| (k.as_str(), v.as_str())).collect(),
        active,
    })
    .map_err(|_| "could not canonicalize pepper ring".to_string())?;
    Ok(hex::encode(Sha256::digest(&canonical)))
}

fn encryption_ring() -> Result<Option<Keyring>> {
    crate::identity::encryption_keys_from_env().map(|ring| ring.map(|ring| (*ring).clone()))
}

fn encryption_entry(ring: Option<&Keyring>) -> serde_json::Value {
    let Some(ring) = ring else {
        return serde_json::json!({"configured": false, "note": NO_ENCRYPTION_NOTE});
    };
    let keys: BTreeMap<String, String> = ring
        .keys
        .iter()
        .map(|(id, key)| (id.clone(), hex::encode(key)))
        .collect();
    let canonical = serde_json::to_vec(&CanonicalRing {
        keys: keys.iter().map(|(k, v)| (k.as_str(), v.as_str())).collect(),
        active: &ring.active_id,
    })
    .expect("serializing key ids");
    let fingerprints: BTreeMap<String, String> = ring
        .keys
        .iter()
        .map(|(id, key)| {
            let mut mac = Hmac::<Sha256>::new_from_slice(key).expect("HMAC key");
            mac.update(LABEL);
            mac.update(id.as_bytes());
            (id.clone(), hex::encode(mac.finalize().into_bytes()))
        })
        .collect();
    serde_json::json!({
        "configured": true,
        "fingerprint": hex::encode(Sha256::digest(canonical)),
        "activeKeyId": ring.active_id,
        "keyFingerprints": fingerprints,
        "note": ENCRYPTION_NOTE,
    })
}

fn check_encryption(entry: &serde_json::Value, ring: Option<&Keyring>) -> Result<()> {
    if !entry.is_object() {
        return Err("backup manifest encryptionKeys entry is malformed".into());
    }
    match entry.get("configured").and_then(serde_json::Value::as_bool) {
        Some(false) => Ok(()),
        Some(true) => {
            let expected = entry
                .get("keyFingerprints")
                .and_then(serde_json::Value::as_object)
                .filter(|map| !map.is_empty())
                .ok_or("backup manifest encryptionKeys.keyFingerprints is missing")?;
            let ring =
                ring.ok_or("backed-up install had ENCRYPTION_KEYS; configure the original ring")?;
            for (id, value) in expected {
                let expected_bytes = value
                    .as_str()
                    .and_then(|s| hex::decode(s).ok())
                    .filter(|v| v.len() == 32)
                    .ok_or("backup manifest encryptionKeys fingerprint is invalid")?;
                let key = ring
                    .keys
                    .get(id)
                    .ok_or_else(|| format!("ENCRYPTION_KEYS lacks backed-up key id: {id}"))?;
                let mut mac = Hmac::<Sha256>::new_from_slice(key).expect("HMAC key");
                mac.update(LABEL);
                mac.update(id.as_bytes());
                mac.verify_slice(&expected_bytes)
                    .map_err(|_| format!("ENCRYPTION_KEYS has a different key for id: {id}"))?;
            }
            Ok(())
        }
        None => Err("backup manifest encryptionKeys entry is malformed".into()),
    }
}

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct FileEntry {
    path: String,
    size_bytes: u64,
    sha256: String,
}

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Manifest {
    format_version: u32,
    created_at: String,
    source_project: String,
    schema: String,
    postgres: serde_json::Value,
    password_pepper: serde_json::Value,
    #[serde(default)]
    encryption_keys: Option<serde_json::Value>,
    search: serde_json::Value,
    database: FileEntry,
    storage: FileEntry,
}

fn file_entry(path: &Path, expected_name: &str) -> Result<FileEntry> {
    let metadata = fs::symlink_metadata(path).map_err(|_| format!("missing {expected_name}"))?;
    if !metadata.file_type().is_file() || metadata.len() == 0 {
        return Err(format!("{expected_name} is not a nonempty regular file"));
    }
    let mut input = File::open(path).map_err(|_| format!("cannot read {expected_name}"))?;
    let mut hash = Sha256::new();
    io::copy(&mut input, &mut hash).map_err(|_| format!("cannot hash {expected_name}"))?;
    Ok(FileEntry {
        path: expected_name.into(),
        size_bytes: metadata.len(),
        sha256: hex::encode(hash.finalize()),
    })
}

fn check_file(expected: &FileEntry, path: &Path, name: &str) -> Result<()> {
    let actual = file_entry(path, name)?;
    if expected.path != name
        || expected.size_bytes != actual.size_bytes
        || expected.sha256 != actual.sha256
    {
        return Err(format!("backup integrity check failed for {name}"));
    }
    Ok(())
}

fn parse_date(raw: &str) -> Result<DateTime<Utc>> {
    let time = NaiveDateTime::parse_from_str(raw, "%Y-%m-%dT%H:%M:%SZ")
        .map_err(|_| "invalid backup createdAt".to_string())?;
    if time.format("%Y-%m-%dT%H:%M:%SZ").to_string() != raw {
        return Err("invalid backup createdAt".into());
    }
    Ok(time.and_utc())
}

fn valid_project(project: &str) -> bool {
    project.len() <= 63
        && project
            .as_bytes()
            .first()
            .is_some_and(|b| b.is_ascii_lowercase() || b.is_ascii_digit())
        && project
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'_' || b == b'-')
}

pub fn create(
    manifest_path: &Path,
    project: &str,
    created_at: &str,
    pg_version: &str,
    dump: &Path,
    storage: &Path,
) -> Result<()> {
    if !valid_project(project) {
        return Err("invalid backup source project".into());
    }
    parse_date(created_at)?;
    let version = pg_version
        .parse::<u32>()
        .map_err(|_| "invalid PostgreSQL version")?;
    let pepper_raw =
        std::env::var("PASSWORD_PEPPER_KEYS").map_err(|_| "PASSWORD_PEPPER_KEYS required")?;
    let pepper_active = std::env::var("PASSWORD_PEPPER_ACTIVE_KEY_ID")
        .map_err(|_| "PASSWORD_PEPPER_ACTIVE_KEY_ID required")?;
    let manifest = Manifest {
        format_version: 1,
        created_at: created_at.into(),
        source_project: project.into(),
        schema: "fvoci".into(),
        postgres: serde_json::json!({"serverVersionNum": version}),
        password_pepper: serde_json::json!({"fingerprint": pepper_fingerprint(&pepper_raw, &pepper_active)?, "note": PEPPER_NOTE}),
        encryption_keys: Some(encryption_entry(encryption_ring()?.as_ref())),
        search: serde_json::json!({"included": false, "reason": SEARCH_REASON}),
        database: file_entry(dump, "database.dump")?,
        storage: file_entry(storage, "storage.tar")?,
    };
    check_dump_magic(dump)?;
    validate_tar(storage)?;
    let mut file = fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(manifest_path)
        .map_err(|_| "cannot create manifest".to_string())?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        file.set_permissions(fs::Permissions::from_mode(0o600))
            .map_err(|_| "cannot protect manifest")?;
    }
    serde_json::to_writer_pretty(&mut file, &manifest).map_err(|_| "cannot write manifest")?;
    file.write_all(b"\n").map_err(|_| "cannot write manifest")?;
    Ok(())
}

pub fn preflight(
    manifest_path: &Path,
    dump: &Path,
    storage: &Path,
    target: &str,
) -> Result<(String, String)> {
    if !valid_project(target) {
        return Err("invalid restore target project".into());
    }
    let metadata = fs::symlink_metadata(manifest_path).map_err(|_| "missing manifest")?;
    if !metadata.file_type().is_file() || metadata.len() == 0 || metadata.len() > 1024 * 1024 {
        return Err("invalid manifest file".into());
    }
    let input = File::open(manifest_path).map_err(|_| "cannot read manifest")?;
    let value: serde_json::Value =
        serde_json::from_reader(input).map_err(|_| "invalid backup manifest")?;
    let manifest: Manifest =
        serde_json::from_value(value.clone()).map_err(|_| "invalid backup manifest")?;
    if manifest.format_version != 1 {
        return Err("unsupported backup format".into());
    }
    if manifest.schema != "fvoci" {
        return Err("backup schema is not fvoci".into());
    }
    if !valid_project(&manifest.source_project) || manifest.source_project == target {
        return Err("restore target must differ from backup source".into());
    }
    // The operational Compose image in this slice is PostgreSQL 18.3.
    if manifest
        .postgres
        .get("serverVersionNum")
        .and_then(serde_json::Value::as_u64)
        .is_none_or(|version| version / 10_000 != 18)
    {
        return Err("backup PostgreSQL major version differs from restore image".into());
    }
    if manifest
        .search
        .get("included")
        .and_then(serde_json::Value::as_bool)
        != Some(false)
    {
        return Err("backup search metadata is invalid".into());
    }
    let created = parse_date(&manifest.created_at)?;
    check_file(&manifest.database, dump, "database.dump")?;
    check_dump_magic(dump)?;
    check_file(&manifest.storage, storage, "storage.tar")?;
    validate_tar(storage)?;
    let pepper_raw =
        std::env::var("PASSWORD_PEPPER_KEYS").map_err(|_| "PASSWORD_PEPPER_KEYS required")?;
    let pepper_active = std::env::var("PASSWORD_PEPPER_ACTIVE_KEY_ID")
        .map_err(|_| "PASSWORD_PEPPER_ACTIVE_KEY_ID required")?;
    let expected = manifest
        .password_pepper
        .get("fingerprint")
        .and_then(serde_json::Value::as_str)
        .ok_or("backup manifest has no password pepper fingerprint")?;
    if pepper_fingerprint(&pepper_raw, &pepper_active)? != expected {
        return Err("password pepper keyring differs from backed-up install".into());
    }
    if let Some(entry) = value.get("encryptionKeys") {
        check_encryption(entry, encryption_ring()?.as_ref())?;
    } else {
        eprintln!("backup manifest predates the ENCRYPTION_KEYS fingerprint; --verify-secrets remains required after restore");
    }
    let snapshot = created
        .checked_add_signed(Duration::seconds(1))
        .ok_or("invalid backup createdAt")?;
    let since = snapshot
        .checked_sub_signed(Duration::days(29))
        .ok_or("invalid backup createdAt")?;
    Ok((
        snapshot.format("%Y-%m-%dT%H:%M:%SZ").to_string(),
        since.format("%Y-%m-%dT%H:%M:%SZ").to_string(),
    ))
}

fn check_dump_magic(dump: &Path) -> Result<()> {
    let mut magic = [0u8; 5];
    File::open(dump)
        .and_then(|mut file| std::io::Read::read_exact(&mut file, &mut magic))
        .map_err(|_| "cannot read database.dump")?;
    if &magic != b"PGDMP" {
        return Err("database.dump is not PostgreSQL custom format".into());
    }
    Ok(())
}

fn validate_tar(path: &Path) -> Result<()> {
    let file = File::open(path).map_err(|_| "cannot read storage.tar")?;
    let mut archive = tar::Archive::new(file);
    let entries = archive
        .entries()
        .map_err(|_| "invalid storage.tar")?
        .raw(true);
    let mut count = 0usize;
    for entry in entries {
        let entry = entry.map_err(|_| "invalid storage.tar entry")?;
        let kind = entry.header().entry_type();
        if !(kind.is_file() || kind.is_dir()) || entry.link_name_bytes().is_some() {
            return Err("storage.tar has an unsupported link or entry type".into());
        }
        let bytes = entry.path_bytes();
        let raw = bytes.as_ref();
        if raw.is_empty() || raw[0] == b'/' || raw.contains(&0) || raw.contains(&b'\\') {
            return Err("storage.tar has an unsafe path".into());
        }
        // GNU tar emits `./` and `./objects/...`; accept those canonical roots.
        if (raw == b"." || raw == b"./") && kind.is_dir() {
            count += 1;
            continue;
        }
        let mut parts = raw.split(|byte| *byte == b'/').peekable();
        if parts.peek() == Some(&&b"."[..]) {
            parts.next();
        }
        let remaining: Vec<&[u8]> = parts.collect();
        if remaining.is_empty() || remaining.iter().any(|p| *p == b".." || p.is_empty()) {
            // A final slash denotes a directory in tar; the parser normalizes it below.
            if !(kind.is_dir()
                && remaining.last() == Some(&&b""[..])
                && remaining[..remaining.len().saturating_sub(1)]
                    .iter()
                    .all(|p| !p.is_empty() && *p != b".."))
            {
                return Err("storage.tar has an unsafe path".into());
            }
        }
        let clean = raw.strip_prefix(b"./").unwrap_or(raw);
        if !clean.starts_with(b"objects/")
            && clean != b"objects"
            && clean != b"objects/"
            && !clean.starts_with(b"tmp/")
            && clean != b"tmp"
            && clean != b"tmp/"
        {
            return Err("storage.tar has an unexpected path".into());
        }
        // Path components catch platform-specific normalization surprises.
        if Path::new(std::str::from_utf8(clean).map_err(|_| "storage.tar path is not UTF-8")?)
            .components()
            .any(|component| {
                matches!(
                    component,
                    Component::ParentDir | Component::RootDir | Component::Prefix(_)
                )
            })
        {
            return Err("storage.tar has an unsafe path".into());
        }
        count += 1;
    }
    if count == 0 {
        return Err("storage.tar has no entries".into());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    fn ring(raw: &str, active: &str) -> Keyring {
        Keyring::parse_named(raw, active, "ENCRYPTION_KEYS").unwrap()
    }

    #[test]
    fn encryption_fingerprints_preserve_keys_and_accept_rotation_without_secret_output() {
        let k1 = "11".repeat(32);
        let k2 = "22".repeat(32);
        let k3 = "33".repeat(32);
        let backed = ring(&format!(r#"{{"k1":"{k1}","k2":"{k2}"}}"#), "k2");
        let entry = encryption_entry(Some(&backed));
        let output = entry.to_string();
        assert!(!output.contains(&k1) && !output.contains(&k2));
        let rotated = ring(
            &format!(
                r#"{{"k3":"{k3}","k2":"{}","k1":"{k1}"}}"#,
                k2.to_uppercase()
            ),
            "k3",
        );
        assert!(check_encryption(&entry, Some(&rotated)).is_ok());
        let changed = ring(&format!(r#"{{"k1":"{k3}","k2":"{k2}"}}"#), "k2");
        let error = check_encryption(&entry, Some(&changed)).unwrap_err();
        assert!(error.contains("k1") && !error.contains(&k3));
        let missing = ring(&format!(r#"{{"k2":"{k2}"}}"#), "k2");
        assert!(check_encryption(&entry, Some(&missing))
            .unwrap_err()
            .contains("k1"));
        assert!(check_encryption(&entry, None).is_err());
        assert!(check_encryption(&encryption_entry(None), None).is_ok());
        assert!(check_encryption(&serde_json::json!({"configured": true}), Some(&backed)).is_err());
        // The old pepper format hashes raw hex spelling, unlike the encryption ring.
        let letters = "ab".repeat(32);
        assert_ne!(
            pepper_fingerprint(&format!(r#"{{"k1":"{letters}"}}"#), "k1").unwrap(),
            pepper_fingerprint(&format!(r#"{{"k1":"{}"}}"#, letters.to_uppercase()), "k1").unwrap()
        );
    }

    #[test]
    fn python_v1_fingerprint_vectors() {
        let upper = "AB".repeat(32);
        let lower = "cd".repeat(32);
        let raw = format!(r#"{{"z":"{upper}","a":"{lower}"}}"#);
        // Fixed values from the original Python v1 serializer and HMAC label.
        assert_eq!(
            pepper_fingerprint(&raw.to_lowercase(), "z").unwrap(),
            "ad719e6cf13e153c2e3607564d89dfd07a63400ed0aa6045f18a78a89f6ebcef"
        );
        let entry = encryption_entry(Some(&ring(&raw, "z")));
        assert_eq!(
            entry["keyFingerprints"]["z"],
            "bdd607ec8e30e34b96f730a8c46895ee598221896fb9165ab7f717b4052308df"
        );
        assert_eq!(
            entry["keyFingerprints"]["a"],
            "2a8926208ff0f91ed933e8c13774b91384086680177711303a7676ede2a45358"
        );
    }

    fn scratch() -> std::path::PathBuf {
        let path = std::env::temp_dir().join(format!("fvoci-backup-test-{}", uuid::Uuid::now_v7()));
        fs::create_dir(&path).unwrap();
        path
    }

    fn write_tar(path: &Path, name: &str, kind: tar::EntryType) {
        let output = File::create(path).unwrap();
        let mut builder = tar::Builder::new(output);
        let mut header = tar::Header::new_gnu();
        header.set_entry_type(kind);
        header.set_mode(0o600);
        header.set_size(if kind.is_file() { 3 } else { 0 });
        header.set_path(name).unwrap();
        if kind.is_symlink() {
            header.set_link_name("../../outside").unwrap();
        }
        header.set_cksum();
        builder
            .append(
                &header,
                Cursor::new(if kind.is_file() {
                    b"abc".as_slice()
                } else {
                    b"".as_slice()
                }),
            )
            .unwrap();
        builder.finish().unwrap();
    }

    #[test]
    fn tar_rejects_links_traversal_and_unexpected_paths() {
        let root = scratch();
        let archive = root.join("storage.tar");
        write_tar(&archive, "./objects/item/payload", tar::EntryType::Regular);
        assert!(validate_tar(&archive).is_ok());
        write_tar(&archive, "./objects/item/payload", tar::EntryType::Symlink);
        assert!(validate_tar(&archive).is_err());
        write_tar(&archive, "./objects/item/payload", tar::EntryType::Link);
        assert!(validate_tar(&archive).is_err());
        write_tar(&archive, "./database.dump", tar::EntryType::Regular);
        assert!(validate_tar(&archive).is_err());
        // Bypass Builder's path safeguards to simulate a malicious tar header.
        let mut bytes = fs::read(&archive).unwrap();
        bytes[..100].fill(0);
        bytes[..18].copy_from_slice(b"objects/../evil\0\0\0");
        let mut header = tar::Header::new_gnu();
        header.as_mut_bytes().copy_from_slice(&bytes[..512]);
        header.set_cksum();
        bytes[..512].copy_from_slice(header.as_bytes());
        fs::write(&archive, bytes).unwrap();
        assert!(validate_tar(&archive).is_err());
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn preflight_rejects_bad_manifest_and_mismatched_files_before_restore() {
        let root = scratch();
        let dump = root.join("database.dump");
        let storage = root.join("storage.tar");
        let manifest = root.join("manifest.json");
        fs::write(&dump, b"PGDMP example").unwrap();
        write_tar(&storage, "./objects/item/payload", tar::EntryType::Regular);
        let key = "11".repeat(32);
        std::env::set_var("PASSWORD_PEPPER_KEYS", format!(r#"{{"install":"{key}"}}"#));
        std::env::set_var("PASSWORD_PEPPER_ACTIVE_KEY_ID", "install");
        std::env::remove_var("ENCRYPTION_KEYS");
        std::env::remove_var("ENCRYPTION_ACTIVE_KEY_ID");
        create(
            &manifest,
            "source",
            "2026-09-27T00:00:00Z",
            "180003",
            &dump,
            &storage,
        )
        .unwrap();
        assert_eq!(
            preflight(&manifest, &dump, &storage, "target").unwrap(),
            ("2026-09-27T00:00:01Z".into(), "2026-08-29T00:00:01Z".into())
        );
        assert!(preflight(&manifest, &dump, &storage, "source").is_err());
        let original = fs::read(&manifest).unwrap();
        let mut value: serde_json::Value = serde_json::from_slice(&original).unwrap();
        value.as_object_mut().unwrap().remove("encryptionKeys");
        fs::write(&manifest, serde_json::to_vec(&value).unwrap()).unwrap();
        assert!(preflight(&manifest, &dump, &storage, "target").is_ok());
        value["encryptionKeys"] = serde_json::Value::Null;
        fs::write(&manifest, serde_json::to_vec(&value).unwrap()).unwrap();
        assert!(preflight(&manifest, &dump, &storage, "target").is_err());
        value = serde_json::from_slice(&original).unwrap();
        value["formatVersion"] = 2.into();
        fs::write(&manifest, serde_json::to_vec(&value).unwrap()).unwrap();
        assert!(preflight(&manifest, &dump, &storage, "target").is_err());
        value["formatVersion"] = 1.into();
        value["createdAt"] = "invalid".into();
        fs::write(&manifest, serde_json::to_vec(&value).unwrap()).unwrap();
        assert!(preflight(&manifest, &dump, &storage, "target").is_err());
        value["createdAt"] = "2026-09-27T00:00:00Z".into();
        value["postgres"]["serverVersionNum"] = 170001.into();
        fs::write(&manifest, serde_json::to_vec(&value).unwrap()).unwrap();
        assert!(preflight(&manifest, &dump, &storage, "target").is_err());
        fs::write(&manifest, &original).unwrap();
        fs::write(&dump, b"PGDMP changed").unwrap();
        assert!(preflight(&manifest, &dump, &storage, "target")
            .unwrap_err()
            .contains("integrity"));
        fs::write(&dump, b"PGDMP example").unwrap();
        write_tar(
            &storage,
            "./objects/another/payload",
            tar::EntryType::Regular,
        );
        assert!(preflight(&manifest, &dump, &storage, "target")
            .unwrap_err()
            .contains("integrity"));
        fs::remove_dir_all(root).unwrap();
    }
}
