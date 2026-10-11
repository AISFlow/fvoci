//! Which finished executables form the `rust-binaries` hand-off cohorts, and
//! how each one is selected from Cargo's `--message-format=json` records.
//!
//! The expected cohort always comes from the checked-out sources (rust.yml's
//! PostgreSQL matrix catalog and the root package's binaries), never from the
//! archive being validated.

use crate::rust_binaries_archive::file_sha256;
use serde_json::Value;
use std::collections::BTreeSet;
use std::path::Path;
use yaml_rust2::YamlLoader;

pub const WORKFLOW: &str = ".github/workflows/rust.yml";
pub const CATALOG_ENV: &str = "FVOCI_POSTGRES_MATRIX_CATALOG";
/// Test executables run outside the matrix catalog rows.
pub const EXTRA_TESTS: [&str; 3] = [
    "attachment_s3_integration",
    "schema_baseline_integration",
    "selected_install_lifetime",
];
/// Cohort name of the production collaboration helper (`collab-engine` crate).
pub const HELPER: &str = "collab-engine";
/// Cohort name of the default-feature migrator, built as target `fvoci-migrate`.
pub const SCHEMA: &str = "schema-migrate";
pub const SCHEMA_TARGET: &str = "fvoci-migrate";
pub const DB_TESTS: &str = "db-tests";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    Test,
    Bin,
}

impl Kind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Test => "test",
            Self::Bin => "bin",
        }
    }
}

/// What a cohort member's Cargo record must say.
#[derive(Debug, PartialEq, Eq)]
pub struct Expectation {
    pub target: String,
    pub kind: Kind,
    /// Sorted feature list of the compiled package.
    pub features: &'static [&'static str],
}

/// The record rule for cohort member `name`; `products` are the root
/// package's db-tests binaries.
pub fn expectation(name: &str, products: &[String]) -> Expectation {
    let (target, features): (&str, &'static [&'static str]) = match name {
        HELPER => (HELPER, &["default", "worker"]),
        SCHEMA => (SCHEMA_TARGET, &[]),
        _ => (name, &[DB_TESTS]),
    };
    let kind = if name == HELPER || name == SCHEMA || products.iter().any(|p| p == name) {
        Kind::Bin
    } else {
        Kind::Test
    };
    Expectation {
        target: target.to_owned(),
        kind,
        features,
    }
}

/// Every `--test NAME` of the PostgreSQL matrix catalog in rust.yml plus
/// `EXTRA_TESTS`, sorted and unique.
pub fn test_targets(workflow: &str) -> Result<Vec<String>, String> {
    let documents =
        YamlLoader::load_from_str(workflow).map_err(|e| format!("rust: YAML parse failed: {e}"))?;
    let [document] = documents.as_slice() else {
        return Err("rust: workflow must be one YAML document".into());
    };
    let jobs = &document["jobs"];
    if jobs.as_hash().is_none() {
        return Err("rust: jobs mapping missing".into());
    }
    let Some(raw) = jobs["postgres"]["env"][CATALOG_ENV]
        .as_str()
        .filter(|raw| !raw.trim().is_empty())
    else {
        return Err("rust: postgres matrix catalog missing".into());
    };
    let rows: Value =
        serde_json::from_str(raw).map_err(|_| "rust: postgres matrix catalog is not JSON")?;
    let rows = rows
        .as_array()
        .filter(|rows| !rows.is_empty())
        .ok_or("rust: postgres matrix catalog must be a non-empty list")?;
    let mut names: BTreeSet<String> = EXTRA_TESTS.iter().map(|n| (*n).to_owned()).collect();
    for row in rows {
        let row = row
            .as_object()
            .ok_or("rust: postgres matrix catalog row must be a mapping")?;
        let tests = row
            .get("tests")
            .and_then(Value::as_str)
            .ok_or("rust: postgres matrix catalog row tests must be a string")?;
        names.extend(test_flags(tests)?);
    }
    Ok(names.into_iter().collect())
}

/// `tests` is shell-split into the `run` command line, so it must be exactly
/// `--test NAME` pairs; anything else is refused rather than skipped.
fn test_flags(tests: &str) -> Result<Vec<String>, String> {
    let words: Vec<&str> = tests.split_ascii_whitespace().collect();
    let valid = |name: &str| {
        !name.starts_with('-')
            && name
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')
    };
    if words.is_empty() || !words.len().is_multiple_of(2) {
        return Err(format!(
            "rust: postgres matrix tests are not --test pairs: {tests:?}"
        ));
    }
    words
        .chunks(2)
        .map(|pair| match pair {
            ["--test", name] if valid(name) => Ok((*name).to_owned()),
            _ => Err(format!(
                "rust: postgres matrix tests are not --test pairs: {tests:?}"
            )),
        })
        .collect()
}

/// Binaries of the root package (`cargo metadata --no-deps` output) whose
/// required features are a subset of `{db-tests}`, sorted.
pub fn product_binaries(metadata: &str, manifest_path: &Path) -> Result<Vec<String>, String> {
    let metadata: Value =
        serde_json::from_str(metadata).map_err(|e| format!("cargo metadata is not JSON: {e}"))?;
    let manifest = manifest_path.to_str().ok_or("manifest path is not UTF-8")?;
    let packages = metadata["packages"]
        .as_array()
        .ok_or("cargo metadata has no packages")?;
    let mut matching = packages
        .iter()
        .filter(|p| p["manifest_path"].as_str() == Some(manifest));
    let (Some(package), None) = (matching.next(), matching.next()) else {
        return Err("cargo metadata root package missing or ambiguous".into());
    };
    let targets = package["targets"]
        .as_array()
        .ok_or("cargo metadata package has no targets")?;
    let mut names = BTreeSet::new();
    for target in targets {
        if target["kind"] != serde_json::json!(["bin"]) {
            continue;
        }
        let name = target["name"].as_str().ok_or("cargo target without name")?;
        let required = match &target["required-features"] {
            Value::Null => Vec::new(),
            Value::Array(list) => list
                .iter()
                .map(Value::as_str)
                .collect::<Option<Vec<_>>>()
                .ok_or("cargo target required-features must be strings")?,
            _ => return Err("cargo target required-features must be a list".into()),
        };
        if required.iter().all(|feature| *feature == DB_TESTS) && !names.insert(name.to_owned()) {
            return Err(format!("duplicate cargo binary target: {name}"));
        }
    }
    Ok(names.into_iter().collect())
}

/// `compiler-artifact` records with an executable from one Cargo JSON stream
/// that ended in a successful `build-finished` record.
pub fn records(text: &str) -> Result<Vec<Value>, String> {
    let records = text
        .lines()
        .map(serde_json::from_str::<Value>)
        .collect::<Result<Vec<_>, _>>()
        .map_err(|e| format!("Rust binary producer output is not JSON lines: {e}"))?;
    if records.last() != Some(&serde_json::json!({"reason": "build-finished", "success": true})) {
        return Err("Rust binary producer did not finish successfully".into());
    }
    Ok(records
        .into_iter()
        .filter(|r| {
            r["reason"] == "compiler-artifact"
                && r["executable"]
                    .as_str()
                    .is_some_and(|path| !path.is_empty())
        })
        .collect())
}

/// Profile part of the record rule: unoptimised, no debug info, and a libtest
/// harness exactly for test targets.
pub fn record_matches(record: &Value, expectation: &Expectation) -> bool {
    let features = record["features"].as_array().and_then(|list| {
        let mut list = list.iter().map(Value::as_str).collect::<Option<Vec<_>>>()?;
        list.sort_unstable();
        Some(list)
    });
    let debuginfo = &record["profile"]["debuginfo"];
    features.as_deref() == Some(expectation.features)
        && record["profile"]["opt_level"] == "0"
        && (debuginfo.is_null() && record["profile"].get("debuginfo").is_some()
            || debuginfo.as_u64() == Some(0))
        && record["profile"]["test"] == (expectation.kind == Kind::Test)
        && record["target"]["kind"] == serde_json::json!([expectation.kind.as_str()])
        && record["target"]["name"] == expectation.target.as_str()
}

#[derive(Clone, Debug, PartialEq)]
pub struct Entry {
    /// Workspace-relative POSIX path, also the archive member name.
    pub path: String,
    pub sha256: String,
    pub record: Value,
}

impl Entry {
    pub fn to_json(&self) -> Value {
        serde_json::json!({"path": self.path, "sha256": self.sha256, "record": self.record})
    }
}

/// The single record selecting `name` (`expectation.target`) and its hashed
/// executable, which must be a physical regular file under `root`.
pub fn entry(
    records: &[Value],
    name: &str,
    expectation: &Expectation,
    root: &Path,
) -> Result<Entry, String> {
    let kind = serde_json::json!([expectation.kind.as_str()]);
    let mut matches = records.iter().filter(|r| {
        r["target"]["name"] == expectation.target.as_str()
            && r["target"]["kind"] == kind
            && r["profile"]["test"] == (expectation.kind == Kind::Test)
    });
    let (Some(record), None) = (matches.next(), matches.next()) else {
        return Err(format!(
            "Rust binary executable missing or ambiguous: {name}"
        ));
    };
    if !record_matches(record, expectation) {
        return Err(format!("Rust binary feature/profile mismatch: {name}"));
    }
    let not_physical = || "Rust binary executable is not physical regular file".to_owned();
    let executable = Path::new(record["executable"].as_str().ok_or_else(not_physical)?);
    let relative = executable
        .strip_prefix(root)
        .ok()
        .and_then(Path::to_str)
        .filter(|_| executable.is_absolute())
        .ok_or_else(not_physical)?;
    if executable.canonicalize().ok().as_deref() != Some(executable) {
        return Err(not_physical());
    }
    let sha256 = file_sha256(executable).map_err(|_| not_physical())?;
    Ok(Entry {
        path: relative.to_owned(),
        sha256,
        record: record.clone(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn workflow(tests: &[&str]) -> String {
        let rows: Vec<Value> = tests
            .iter()
            .map(|t| json!({"runner": "x", "tests": t}))
            .collect();
        let catalog = serde_json::to_string_pretty(&rows).unwrap();
        let indented: String = catalog.lines().map(|l| format!("        {l}\n")).collect();
        format!("jobs:\n  postgres:\n    env:\n      {CATALOG_ENV}: |\n{indented}")
    }

    #[test]
    fn catalog_tests_union_with_fixed_targets() {
        let names = test_targets(&workflow(&["--test b --test a", "--test\ta  --test c"])).unwrap();
        assert_eq!(
            names,
            [
                "a",
                "attachment_s3_integration",
                "b",
                "c",
                "schema_baseline_integration",
                "selected_install_lifetime"
            ]
        );
    }

    #[test]
    fn catalog_shape_errors_refuse() {
        for (text, message) in [
            ("jobs: [", "YAML parse failed"),
            ("a: 1\n---\nb: 2\n", "one YAML document"),
            ("on: push\n", "jobs mapping missing"),
            ("jobs:\n  postgres:\n    env: {}\n", "catalog missing"),
            ("jobs:\n  postgres: {}\n  postgres: {}\n", "duplicated key"),
            (
                &format!("jobs:\n  postgres:\n    env:\n      {CATALOG_ENV}: '[1'\n"),
                "not JSON",
            ),
            (
                &format!("jobs:\n  postgres:\n    env:\n      {CATALOG_ENV}: '[]'\n"),
                "non-empty list",
            ),
            (
                &format!("jobs:\n  postgres:\n    env:\n      {CATALOG_ENV}: '[1]'\n"),
                "must be a mapping",
            ),
            (
                &format!("jobs:\n  postgres:\n    env:\n      {CATALOG_ENV}: '[{{}}]'\n"),
                "tests must be a string",
            ),
        ] {
            let error = test_targets(text).unwrap_err();
            assert!(error.contains(message), "{text:?}: {error}");
        }
        for tests in [
            "",
            "--test",
            "--test a b",
            "--test --test",
            "--tests a",
            "--test a.b",
            "x --test a",
        ] {
            assert!(test_targets(&workflow(&[tests])).is_err(), "{tests:?}");
        }
    }

    #[test]
    fn product_binaries_follow_required_features() {
        let metadata = json!({"packages": [
            {"manifest_path": "/w/crates/x/Cargo.toml", "targets": [{"kind": ["bin"], "name": "other"}]},
            {"manifest_path": "/w/Cargo.toml", "targets": [
                {"kind": ["bin"], "name": "server", "required-features": []},
                {"kind": ["bin"], "name": "fixture", "required-features": ["db-tests"]},
                {"kind": ["bin"], "name": "openapi", "required-features": ["api-schema"]},
                {"kind": ["bin"], "name": "mixed", "required-features": ["db-tests", "x"]},
                {"kind": ["lib"], "name": "fvoci", "required-features": []},
                {"kind": ["test"], "name": "db_integration"}
            ]}
        ]})
        .to_string();
        assert_eq!(
            product_binaries(&metadata, Path::new("/w/Cargo.toml")).unwrap(),
            ["fixture", "server"]
        );
        assert!(product_binaries(&metadata, Path::new("/w/missing/Cargo.toml")).is_err());
    }

    #[test]
    fn records_require_successful_finish() {
        let finished = "{\"reason\":\"build-finished\",\"success\":true}";
        let artifact = "{\"reason\":\"compiler-artifact\",\"executable\":\"/x\"}";
        let library = "{\"reason\":\"compiler-artifact\",\"executable\":null}";
        assert_eq!(
            records(&format!("{artifact}\n{library}\n{finished}\n"))
                .unwrap()
                .len(),
            1
        );
        for text in [
            String::new(),
            artifact.to_owned(),
            format!("{artifact}\n{{\"reason\":\"build-finished\",\"success\":false}}"),
            format!("{artifact}\n\n{finished}"),
        ] {
            assert!(records(&text).is_err(), "{text:?}");
        }
    }

    #[test]
    fn expectations_by_cohort_member() {
        let products = ["fvoci-server".to_owned()];
        assert_eq!(
            expectation("collab-engine", &products).features,
            ["default", "worker"]
        );
        assert_eq!(
            expectation("schema-migrate", &products).target,
            "fvoci-migrate"
        );
        assert_eq!(expectation("schema-migrate", &products).kind, Kind::Bin);
        assert_eq!(expectation("fvoci-server", &products).kind, Kind::Bin);
        assert_eq!(expectation("db_integration", &products).kind, Kind::Test);
        assert_eq!(
            expectation("db_integration", &products).features,
            ["db-tests"]
        );
    }

    #[test]
    fn debuginfo_must_be_zero_or_null() {
        let expect = expectation("t", &[]);
        let record = |debuginfo: Value| {
            json!({"features": ["db-tests"], "target": {"name": "t", "kind": ["test"]},
                   "profile": {"test": true, "opt_level": "0", "debuginfo": debuginfo}})
        };
        assert!(record_matches(&record(json!(0)), &expect));
        assert!(record_matches(&record(Value::Null), &expect));
        for bad in [json!(2), json!(false), json!(0.0), json!("0")] {
            assert!(!record_matches(&record(bad.clone()), &expect), "{bad}");
        }
        let mut missing = record(json!(0));
        missing["profile"]
            .as_object_mut()
            .unwrap()
            .remove("debuginfo");
        assert!(!record_matches(&missing, &expect));
    }
}
