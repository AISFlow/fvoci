//! Exact db-tests library cohorts, executed with the inputs owned by their
//! existing CI jobs. The libtest result contract is selected-library's.

#[path = "native_library_pg.rs"]
mod pg;
#[path = "native_library_sqld.rs"]
mod sqld;

use crate::args::{self, Outcome};
use crate::rust_binaries::{context, host, product_binaries};
use crate::rust_binaries_archive::file_sha256;
use crate::rust_binaries_cohort::{expectation, record_matches, HELPER, LIBRARY, SCHEMA};
use crate::selected_library::filters;
use serde_json::Value;
use std::collections::BTreeSet;
use std::ffi::OsString;
use std::fs::{self, File};
use std::io::Read;
use std::path::{Path, PathBuf};

pub const COHORTS_FILE: &str = "xtask/native-library-cohorts.tsv";
pub const COHORTS: &[(&str, usize)] = &[
    ("no-input", 105),
    ("collaboration", 28),
    ("preview", 6),
    ("postgres", 1),
    ("s3", 1),
    ("sqld", 15),
    ("sqld-ignored", 15),
    ("sqld-migration", 6),
    ("pg-app", 1),
];
const USAGE: &str = "usage: xtask native-library {run --cohort NAME --directory DIRECTORY --sqlite-identity HASH | prepare-sqld --directory DIRECTORY}";

/// Validate the entire registry before selecting any member. Duplicate names
/// across cohorts, unknown cohorts, empty selections and malformed paths fail.
pub fn parse_cohort(text: &str, name: &str) -> Result<Vec<String>, String> {
    let text = text
        .strip_suffix('\n')
        .ok_or("native library registry must end with LF")?;
    let mut seen = BTreeSet::new();
    let mut selected = Vec::new();
    let mut counts = std::collections::BTreeMap::new();
    for line in text.split('\n') {
        let (cohort, test) = line
            .split_once('\t')
            .ok_or("native library registry needs cohort and exact test")?;
        if !COHORTS.iter().any(|(known, _)| *known == cohort) {
            return Err("unknown native library cohort".into());
        }
        filters(&format!("{test}\n"))?;
        if !seen.insert(test) {
            return Err("duplicate native library test".into());
        }
        *counts.entry(cohort).or_insert(0) += 1;
        if cohort == name {
            selected.push(test.to_owned());
        }
    }
    if COHORTS
        .iter()
        .any(|(name, count)| counts.get(name) != Some(count))
        || selected.is_empty()
    {
        return Err("native library cohort inventory/count mismatch".into());
    }
    Ok(selected)
}

pub fn cohort_filters(name: &str) -> Result<Vec<String>, String> {
    let text =
        fs::read_to_string(COHORTS_FILE).map_err(|_| "native library registry unavailable")?;
    parse_cohort(&text, name)
}

/// Reject foreign/non-ELF inputs before exec; binfmt is not native coverage.
pub fn native_elf(path: &Path) -> Result<(), String> {
    if path.canonicalize().ok().as_deref() != Some(path) {
        return Err("native library input must be a physical absolute path".into());
    }
    let mut header = [0u8; 20];
    File::open(path)
        .and_then(|mut file| file.read_exact(&mut header))
        .map_err(|_| "native library ELF header unavailable")?;
    let machine = match std::env::consts::ARCH {
        "x86_64" => 62,
        "aarch64" => 183,
        _ => return Err("unsupported native library architecture".into()),
    };
    if &header[..6] != b"\x7fELF\x02\x01" || u16::from_le_bytes([header[18], header[19]]) != machine
    {
        return Err("native library input is not native ELF64".into());
    }
    Ok(())
}

fn required_env(name: &str) -> Result<String, String> {
    std::env::var(name)
        .ok()
        .filter(|s| !s.is_empty())
        .ok_or_else(|| format!("native library missing required input: {name}"))
}

pub fn input_names(cohort: &str) -> &'static [&'static str] {
    match cohort {
        "collaboration" => &[
            "FVOCI_COLLAB_ENGINE",
            "FVOCI_IMPORT_TEST_SERVER_BIN",
            "TEST_DATABASE_URL",
        ],
        "preview" => &["FVOCI_PREVIEW_TEST_HELPER"],
        "postgres" => &["TEST_DATABASE_URL"],
        "pg-app" => &["PG_CONTAINER", "PREPARATION_DATABASE_URL"],
        "s3" => &[
            "S3_ENDPOINT",
            "S3_REGION",
            "S3_BUCKET",
            "S3_ACCESS_KEY_ID",
            "S3_SECRET_ACCESS_KEY",
        ],
        "sqld" => &["FVOCI_TEST_SQLD"],
        "sqld-ignored" => &["FVOCI_TEST_SQLD", "FVOCI_COLLAB_ENGINE"],
        "sqld-migration" => &["FVOCI_TEST_SQLD"],
        _ => &[],
    }
}

/// Every run rechecks the complete native identity and requested executable
/// hashes from the unpacked receipt, including Cargo features/profile.
pub fn checked_entry(
    manifest: &Value,
    name: &str,
    root: &Path,
    products: &[String],
) -> Result<PathBuf, String> {
    let entry = &manifest["entries"][name];
    let relative = entry["path"]
        .as_str()
        .ok_or("native library executable missing")?;
    if !crate::rust_binaries_archive::safe_relative(relative) {
        return Err("native library executable path escape".into());
    }
    let path = root.join(relative);
    if entry["record"]["executable"].as_str() != path.to_str()
        || !record_matches(&entry["record"], &expectation(name, products))
        || file_sha256(&path).ok().as_deref() != entry["sha256"].as_str()
        || entry["sha256"].as_str().is_none()
    {
        return Err("native library digest/feature/profile/path mismatch".into());
    }
    native_elf(&path)?;
    Ok(path)
}

pub fn validate_context(manifest: &Value, expected: &Value) -> Result<(), String> {
    if manifest["version"] != 1 || manifest["context"] != *expected {
        return Err("native library source/run/attempt/platform/native identity mismatch".into());
    }
    Ok(())
}

fn run(cohort: &str, directory: &Path, sqlite: &str) -> Result<(), String> {
    let selected = cohort_filters(cohort)?;
    if cohort == "no-input" {
        return Err("no-input cohort belongs to selected-library's existing SQLite build".into());
    }
    for name in input_names(cohort) {
        required_env(name)?;
    }
    let root = std::env::current_dir()
        .and_then(|p| p.canonicalize())
        .map_err(|_| "native library workspace unavailable")?;
    let archive = if matches!(cohort, "postgres" | "s3" | "pg-app") {
        "postgres"
    } else {
        "helper"
    };
    let manifest: Value = serde_json::from_slice(
        &fs::read(directory.join(format!("{archive}-manifest.json")))
            .map_err(|_| "native library unpacked receipt unavailable")?,
    )
    .map_err(|_| "native library receipt is not JSON")?;
    validate_context(&manifest, &context(&host(&root)?, Some(sqlite))?)?;
    let products = product_binaries(&root)?;
    let executable = checked_entry(&manifest, LIBRARY, &root, &products)?;
    for (env, member) in [
        ("FVOCI_COLLAB_ENGINE", HELPER),
        ("FVOCI_IMPORT_TEST_SERVER_BIN", "fvoci-server"),
        ("FVOCI_PREVIEW_TEST_HELPER", "fvoci-server"),
    ] {
        if input_names(cohort).contains(&env) {
            let path = checked_entry(&manifest, member, &root, &products)?;
            if std::env::var_os(env).as_deref() != Some(path.as_os_str()) {
                return Err(format!("native library sealed input mismatch: {env}"));
            }
        }
    }
    if cohort.starts_with("sqld") {
        sqld::verify(Path::new(&required_env("FVOCI_TEST_SQLD")?))?;
    }
    let selection: Vec<&str> = selected.iter().map(String::as_str).collect();
    let execute = || {
        for filter in &selection {
            checked_entry(&manifest, LIBRARY, &root, &products)?;
            crate::selected_library::run_filters_with_mode(
                &executable,
                &root,
                &[*filter],
                matches!(cohort, "sqld-ignored" | "sqld-migration"),
                cohort == "sqld-migration",
            )?;
        }
        Ok(())
    };
    if cohort == "pg-app" {
        let migrator = checked_entry(&manifest, SCHEMA, &root, &products)?;
        pg::run(&migrator, execute)?;
    } else {
        execute()?;
    }
    println!(
        "native library cohort={cohort} passed={} ignored=0",
        selected.len()
    );
    Ok(())
}

pub fn main(argv: Vec<OsString>) -> i32 {
    if argv.as_slice() == [OsString::from("--help")] || argv.as_slice() == [OsString::from("-h")] {
        println!("{USAGE}");
        return 0;
    }
    let Some((action, rest)) = argv.split_first() else {
        eprintln!("{USAGE}");
        return 2;
    };
    let parsed = match args::parse(
        rest.to_vec(),
        &[
            args::value("cohort"),
            args::value("directory"),
            args::value("sqlite-identity"),
        ],
        false,
    ) {
        Outcome::Parsed(p) => p,
        _ => {
            eprintln!("{USAGE}");
            return 2;
        }
    };
    let Some(directory) = parsed.get("directory") else {
        eprintln!("{USAGE}");
        return 2;
    };
    let result = match action.to_str() {
        Some("prepare-sqld")
            if parsed.get("cohort").is_none() && parsed.get("sqlite-identity").is_none() =>
        {
            sqld::prepare(Path::new(directory))
        }
        Some("run") => match (parsed.get("cohort"), parsed.get("sqlite-identity")) {
            (Some(name), Some(sqlite)) => run(name, Path::new(directory), sqlite),
            _ => {
                eprintln!("{USAGE}");
                return 2;
            }
        },
        _ => {
            eprintln!("{USAGE}");
            return 2;
        }
    };
    match result {
        Ok(()) => 0,
        Err(error) => {
            eprintln!("native-library: {error}");
            1
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::selected_library::{result_error, run_filter, run_filters_with_mode};
    use serde_json::json;
    use std::io::{Seek, SeekFrom, Write};

    fn registry() -> String {
        fs::read_to_string(Path::new(env!("CARGO_MANIFEST_DIR")).join("native-library-cohorts.tsv"))
            .unwrap()
    }

    #[test]
    fn exact_registry_rejects_empty_unknown_duplicate_and_cross_cohort_selection() {
        let text = registry();
        for (cohort, count) in COHORTS {
            assert_eq!(parse_cohort(&text, cohort).unwrap().len(), *count);
        }
        assert!(parse_cohort(&text, "unknown").is_err());
        assert!(parse_cohort("", "collaboration").is_err());
        assert!(parse_cohort(&text.replace("preview\t", "unknown\t"), "collaboration").is_err());
        let first = text.lines().next().unwrap();
        assert!(parse_cohort(&format!("{text}{first}\n"), "collaboration").is_err());
        assert!(parse_cohort(&text.replacen(first, "", 1), "collaboration").is_err());
        assert!(parse_cohort(&text.replace('\n', "\r\n"), "collaboration").is_err());
    }

    #[test]
    fn changed_source_run_attempt_arch_features_toolchain_and_native_context_refuse() {
        let context = json!({"sha":"a", "tree":"b", "run_id":"1", "run_attempt":"2", "arch":"X64", "workspace":"/w", "rustc":"pinned", "sqlite":"c", "build_environment":{}, "profile":"dev-test-nodebug", "os":"Ubuntu"});
        let good = json!({"version":1,"context": context});
        validate_context(&good, &context).unwrap();
        for key in context.as_object().unwrap().keys() {
            let mut bad = good.clone();
            bad["context"][key] = json!("foreign");
            assert!(validate_context(&bad, &context).is_err(), "{key}");
        }
        let mut bad = good;
        bad["version"] = json!(2);
        assert!(validate_context(&bad, &context).is_err());
    }

    #[test]
    fn real_native_libtest_fixture_passes() {}

    #[test]
    #[ignore = "counterexample fixture selected by the parent test"]
    fn real_native_libtest_fixture_ignored_or_fails() {
        panic!("expected failed-body counterexample");
    }

    #[test]
    fn real_native_child_zero_ignored_failed_and_duplicate_never_pass() {
        let executable = std::env::current_exe().unwrap();
        let cwd = executable.parent().unwrap();
        let pass = "native_library::tests::real_native_libtest_fixture_passes";
        let ignored = "native_library::tests::real_native_libtest_fixture_ignored_or_fails";
        for (name, expected) in [(pass, true), ("unknown::test", false), (ignored, false)] {
            let (code, output) = run_filter(&executable, cwd, name).unwrap();
            assert_eq!(
                result_error(name, code, std::str::from_utf8(&output).unwrap()).is_none(),
                expected
            );
        }
        assert!(run_filters_with_mode(&executable, cwd, &[ignored], true, false).is_err());
        assert!(run_filters_with_mode(&executable, cwd, &[], false, false).is_err());
        assert!(run_filters_with_mode(&executable, cwd, &[pass, pass], false, false).is_err());
    }

    #[test]
    fn actual_elf_digest_feature_and_architecture_counterexamples() {
        let root = crate::host::mkdtemp("native-library-", &std::env::temp_dir())
            .unwrap()
            .canonicalize()
            .unwrap();
        let path = root.join("target/libtest");
        fs::create_dir(root.join("target")).unwrap();
        fs::copy(std::env::current_exe().unwrap(), &path).unwrap();
        let record = json!({"features":["db-tests"], "target":{"name":"fvoci_server","kind":["lib"]}, "profile":{"test":true,"opt_level":"0","debuginfo":0},"executable":path});
        let manifest = json!({"entries":{LIBRARY:{"path":"target/libtest","sha256":file_sha256(&path).unwrap(),"record":record}}});
        assert_eq!(checked_entry(&manifest, LIBRARY, &root, &[]).unwrap(), path);
        for (key, value) in [
            ("features", json!([])),
            ("target", json!({"kind":["test"],"name":"fvoci_server"})),
            (
                "profile",
                json!({"test":true,"opt_level":"0","debuginfo":2}),
            ),
            ("executable", json!("/foreign")),
        ] {
            let mut bad = manifest.clone();
            bad["entries"][LIBRARY]["record"][key] = value;
            assert!(checked_entry(&bad, LIBRARY, &root, &[]).is_err(), "{key}");
        }
        let mut bad = manifest.clone();
        bad["entries"][LIBRARY]["sha256"] = json!("0".repeat(64));
        assert!(checked_entry(&bad, LIBRARY, &root, &[]).is_err());
        let mut file = fs::OpenOptions::new().write(true).open(&path).unwrap();
        file.seek(SeekFrom::Start(18)).unwrap();
        file.write_all(&0u16.to_le_bytes()).unwrap();
        drop(file);
        let mut foreign = manifest;
        foreign["entries"][LIBRARY]["sha256"] = json!(file_sha256(&path).unwrap());
        assert!(checked_entry(&foreign, LIBRARY, &root, &[])
            .unwrap_err()
            .contains("ELF64"));
        assert!(sqld::verify(&path).unwrap_err().contains("digest"));
        fs::remove_dir_all(root).unwrap();
    }
}
