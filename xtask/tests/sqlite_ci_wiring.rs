//! Port of the process controls in scripts/fixtures/sqlite-ci/test-wiring.py.
//! Build wiring only: owned bash tool/helper stubs; no C/Rust/network
//! execution. `sqlite_ci::run` is driven with an explicit environment and a
//! stub helper program, the way the Python test replaced the sibling helper.

use std::collections::BTreeMap;
use std::ffi::OsString;
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use xtask::sqlite_ci::{run, Context};

const NAMES: [&str; 4] = [
    "SQLITE3_LIB_DIR",
    "SQLITE3_INCLUDE_DIR",
    "SQLITE3_STATIC",
    "SQLITE3_NO_PKG_CONFIG",
];

fn target() -> &'static str {
    match std::env::consts::ARCH {
        "x86_64" => "x86_64-unknown-linux-gnu",
        "aarch64" => "aarch64-unknown-linux-gnu",
        other => panic!("unsupported test host {other}"),
    }
}

/// Stub helper: same argv contract as `xtask sqlite-build`.
const HELPER_STUB: &str = r#"#!/usr/bin/env bash
set -euo pipefail
archive= prefix= target= identity=0
while [ $# -gt 0 ]; do
  case "$1" in
    --archive) archive=$2; shift 2 ;;
    --prefix) prefix=$2; shift 2 ;;
    --target) target=$2; shift 2 ;;
    --identity-only) identity=1; shift ;;
    *) exit 2 ;;
  esac
done
if [ "$identity" = 1 ]; then echo "inspect $target" >>"$TRACE"; else echo "helper $target" >>"$TRACE"; fi
[ -z "${HELPER_FAIL:-}" ] || exit 9
[ "$(cat -- "$archive")" = "fixture source" ] || exit 7
exports="export SQLITE3_LIB_DIR='$prefix/lib'
export SQLITE3_INCLUDE_DIR='$prefix/include'
export SQLITE3_STATIC=1
export SQLITE3_NO_PKG_CONFIG=1
"
json_exports=$(printf '%s' "$exports" | sed ':a;N;$!ba;s/\n/\\n/g')
inputs="{\"helper_sha256\": \"$(sha256sum <"$0" | cut -d' ' -f1)\", \"target\": \"$target\", \"flags\": [\"fixture profile\"]}"
if [ "$identity" = 1 ]; then
  printf '{"inputs": %s, "exports": "%s\\n", "timings": {}}\n' "$inputs" "$json_exports"
  exit 0
fi
manifest() {
  printf '{"inputs": %s, "outputs": {' "$inputs"
  printf '"env.sh": "%s", ' "$(sha256sum <"$prefix/env.sh" | cut -d' ' -f1)"
  printf '"lib/libsqlite3.a": "%s", ' "$(sha256sum <"$prefix/lib/libsqlite3.a" | cut -d' ' -f1)"
  printf '"include/sqlite3.h": "%s"}}' "$(sha256sum <"$prefix/include/sqlite3.h" | cut -d' ' -f1)"
}
if [ ! -e "$prefix" ]; then
  mkdir -p "$prefix/lib" "$prefix/include"
  printf 'fixture static archive' >"$prefix/lib/libsqlite3.a"
  printf 'fixture exact header' >"$prefix/include/sqlite3.h"
  printf '%s' "$exports" >"$prefix/env.sh"
  manifest >"$prefix/manifest.json"
else
  [ "$(manifest)" = "$(cat "$prefix/manifest.json")" ] || exit 8
fi
printf '{"archive": {"elapsed_seconds": 0.002, "timeout_seconds": 180, "exit_code": 0}}' >"$prefix.prepare-timings.json"
[ -z "${BAD_EXPORT:-}" ] || exports="${exports}export UNEXPECTED=bad
"
printf '%s' "$exports"
"#;

struct Fixture {
    _temp: TempDir,
    root: PathBuf,
    parent: PathBuf,
    clang: PathBuf,
    trace: PathBuf,
    github_env: PathBuf,
    github_output: PathBuf,
    consumer_env: PathBuf,
    env: BTreeMap<OsString, OsString>,
    helper: Vec<OsString>,
}

struct TempDir(PathBuf);

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

struct Outcome {
    code: i32,
    stdout: String,
    stderr: String,
}

fn write_tool(path: &Path, body: &str) {
    fs::write(path, format!("#!/usr/bin/env bash\n{body}")).unwrap();
    fs::set_permissions(path, fs::Permissions::from_mode(0o755)).unwrap();
}

impl Fixture {
    fn new() -> Self {
        let root = xtask::host::mkdtemp("fvoci-sqlite-wiring-", &std::env::temp_dir()).unwrap();
        let temp = TempDir(root.clone());
        let parent = root.join("owned parent");
        let bin = root.join("bin");
        let clang = root.join("clang");
        for dir in [&parent, &bin, &clang] {
            fs::create_dir(dir).unwrap();
        }
        fs::write(clang.join("libclang.so"), b"fixture libclang").unwrap();
        let trace = root.join("trace");
        fs::write(&trace, "").unwrap();
        write_tool(
            &bin.join("curl"),
            r#"echo "curl $*" >>"$TRACE"
[ -z "${CURL_FAIL:-}" ] || exit 22
while [ $# -gt 0 ]; do if [ "$1" = --output ]; then printf 'fixture source' >"$2"; fi; shift; done
"#,
        );
        write_tool(
            &bin.join("rustc"),
            r#"echo "rustc $*" >>"$TRACE"
[ -z "${RUSTC_FAIL:-}" ] || exit 2
echo "${RUSTC_VERSION:-fixture rustc native target}"
echo "host: $RUSTC_HOST"
"#,
        );
        for tool in ["cc", "ar"] {
            write_tool(
                &bin.join(tool),
                "echo 'native tool must not execute in FAST fixtures' >&2\nexit 99\n",
            );
        }
        write_tool(
            &bin.join("consumer"),
            r#"echo "consumer $*" >>"$TRACE"
env | grep '^SQLITE3_' | sort >"$CONSUMER_ENV"
exit "${CONSUMER_EXIT:-0}"
"#,
        );
        let helper = root.join("prepare-sqlite-build-stub");
        write_tool(
            &helper,
            HELPER_STUB.trim_start_matches("#!/usr/bin/env bash\n"),
        );
        let mut env: BTreeMap<OsString, OsString> = std::env::vars_os()
            .filter(|(k, _)| {
                let k = k.to_string_lossy();
                !NAMES.contains(&k.as_ref())
                    && k != "CARGO_BUILD_TARGET"
                    && !k.starts_with("GITHUB_")
            })
            .collect();
        let path = format!(
            "{}:{}",
            bin.display(),
            std::env::var("PATH").unwrap_or_default()
        );
        let consumer_env = root.join("consumer.env");
        for (k, v) in [
            ("PATH", path.as_str()),
            ("TRACE", trace.to_str().unwrap()),
            ("LIBCLANG_PATH", clang.to_str().unwrap()),
            ("RUSTC_HOST", target()),
            ("CONSUMER_ENV", consumer_env.to_str().unwrap()),
        ] {
            env.insert(k.into(), v.into());
        }
        let github_env = root.join("github-env");
        let github_output = root.join("github-output");
        fs::write(&github_env, "").unwrap();
        fs::write(&github_output, "").unwrap();
        Self {
            _temp: temp,
            root,
            parent,
            clang,
            trace,
            github_env,
            github_output,
            consumer_env,
            env,
            helper: vec![helper.into()],
        }
    }

    fn set(&mut self, key: &str, value: impl Into<OsString>) {
        self.env.insert(key.into(), value.into());
    }

    fn unset(&mut self, key: &str) {
        self.env.remove(OsString::from(key).as_os_str());
    }

    fn invoke_with(&self, parent: Option<&Path>, extra: &[&str]) -> Outcome {
        let mut argv: Vec<OsString> = Vec::new();
        if let Some(parent) = parent {
            argv.extend(["--parent".into(), parent.into()]);
        }
        argv.extend([
            "--github-env".into(),
            self.github_env.clone().into(),
            "--github-output".into(),
            self.github_output.clone().into(),
        ]);
        argv.extend(extra.iter().map(OsString::from));
        let context = Context {
            env: self.env.clone(),
            helper: self.helper.clone(),
        };
        let (mut out, mut err) = (Vec::new(), Vec::new());
        let code = run(argv, &context, &mut out, &mut err);
        Outcome {
            code,
            stdout: String::from_utf8(out).unwrap(),
            stderr: String::from_utf8(err).unwrap(),
        }
    }

    fn invoke(&self, extra: &[&str]) -> Outcome {
        self.invoke_with(Some(&self.parent), extra)
    }

    fn trace(&self) -> String {
        fs::read_to_string(&self.trace).unwrap()
    }

    fn rejected(&self, outcome: &Outcome) {
        assert_ne!(outcome.code, 0, "{}", outcome.stderr);
        assert_eq!(fs::read_to_string(&self.github_env).unwrap(), "");
        assert_eq!(fs::read_to_string(&self.github_output).unwrap(), "");
        assert!(!self.consumer_env.exists());
    }

    fn last_output_line(&self) -> String {
        fs::read_to_string(&self.github_output)
            .unwrap()
            .lines()
            .last()
            .unwrap()
            .to_owned()
    }

    fn prefix(&self) -> PathBuf {
        self.parent.join(target())
    }

    fn github_env_map(&self) -> BTreeMap<String, String> {
        fs::read_to_string(&self.github_env)
            .unwrap()
            .lines()
            .map(|l| {
                let (k, v) = l.split_once('=').unwrap();
                (k.to_owned(), v.to_owned())
            })
            .collect()
    }

    fn json(&self, name: &str) -> serde_json::Value {
        serde_json::from_str(&fs::read_to_string(self.parent.join(name)).unwrap()).unwrap()
    }
}

#[test]
fn verified_env_before_consumer_and_bounded_official_download() {
    let f = Fixture::new();
    let outcome = f.invoke(&["--", "consumer", "--fixture-argument"]);
    assert_eq!(outcome.code, 0, "{}", outcome.stderr);
    assert_eq!(outcome.stdout, "");
    let consumer = fs::read_to_string(&f.consumer_env).unwrap();
    let exports: BTreeMap<String, String> = consumer
        .lines()
        .map(|l| {
            let (k, v) = l.split_once('=').unwrap();
            (k.to_owned(), v.to_owned())
        })
        .collect();
    let prefix = f.prefix();
    let expected: BTreeMap<String, String> = [
        (
            "SQLITE3_INCLUDE_DIR",
            prefix.join("include").display().to_string(),
        ),
        ("SQLITE3_LIB_DIR", prefix.join("lib").display().to_string()),
        ("SQLITE3_NO_PKG_CONFIG", "1".to_owned()),
        ("SQLITE3_STATIC", "1".to_owned()),
    ]
    .into_iter()
    .map(|(k, v)| (k.to_owned(), v))
    .collect();
    assert_eq!(exports, expected);
    assert_eq!(f.github_env_map(), expected);
    let trace = f.trace();
    let lines: Vec<&str> = trace.lines().collect();
    assert!(lines[0].starts_with("curl "));
    assert_eq!(lines[1], format!("inspect {}", target()));
    assert!(lines
        .last()
        .unwrap()
        .starts_with("consumer --fixture-argument"));
    assert!(lines[0].contains("https://www.sqlite.org/2026/sqlite-amalgamation-3530400.zip"));
    for flag in [
        "--proto =https",
        "--max-time 90",
        "--retry 0",
        "--max-filesize 16777216",
    ] {
        assert!(lines[0].contains(flag), "{flag}");
    }
    assert!(outcome
        .stderr
        .contains("cache: MISS; building pinned source"));
    let identity = f.json("consumer-inputs.json");
    assert!(identity["manifest"].get("outputs").is_some());
    assert!(identity["manifest"]["inputs"].get("flags").is_some());
    assert_eq!(
        identity["exports"],
        serde_json::to_value(&expected).unwrap()
    );
    assert_eq!(
        identity["os_release"],
        fs::read_to_string("/etc/os-release").unwrap()
    );
    assert_eq!(identity["architecture"], std::env::consts::ARCH);
    let output = fs::read_to_string(&f.github_output).unwrap();
    let cache_identity = output
        .lines()
        .find_map(|l| l.strip_prefix("cache_identity="))
        .unwrap();
    assert_eq!(cache_identity.len(), 64);
}

#[test]
fn download_failure_never_runs_helper_or_consumer() {
    let mut f = Fixture::new();
    f.set("CURL_FAIL", "1");
    f.rejected(&f.invoke(&["--", "consumer"]));
    assert!(!f.trace().contains("helper "));
    let names: Vec<_> = fs::read_dir(&f.parent)
        .unwrap()
        .map(|e| e.unwrap().file_name())
        .collect();
    assert_eq!(names, ["preparation-timings.json"]);
}

#[test]
fn helper_failure_never_exports_or_runs_consumer() {
    let mut f = Fixture::new();
    f.set("HELPER_FAIL", "1");
    f.rejected(&f.invoke(&["--", "consumer"]));
    assert!(!f.trace().contains("rustc "));
}

#[test]
fn real_helper_rejects_wrong_pin_before_native_tools() {
    let mut f = Fixture::new();
    f.helper = vec![env!("CARGO_BIN_EXE_xtask").into(), "sqlite-build".into()];
    let outcome = f.invoke(&["--", "consumer"]);
    f.rejected(&outcome);
    assert!(
        outcome.stderr.contains("archive SHA-256 mismatch"),
        "{}",
        outcome.stderr
    );
    assert!(!f.prefix().exists());
    // A second invocation must reject retained bad bytes, without downloading again.
    f.rejected(&f.invoke(&["--", "consumer"]));
    assert_eq!(f.trace().matches("curl ").count(), 1);
}

#[test]
fn bad_exports_never_reach_github_or_consumer() {
    let mut f = Fixture::new();
    f.set("BAD_EXPORT", "1");
    f.rejected(&f.invoke(&["--", "consumer"]));
}

#[test]
fn missing_clang_fails_before_download() {
    let f = Fixture::new();
    fs::remove_file(f.clang.join("libclang.so")).unwrap();
    f.rejected(&f.invoke(&["--", "consumer"]));
    assert_eq!(f.trace(), "");
}

#[test]
fn symlink_parent_never_downloads() {
    let f = Fixture::new();
    let link = f.root.join("link");
    std::os::unix::fs::symlink(&f.parent, &link).unwrap();
    f.rejected(&f.invoke_with(Some(&link), &["--", "consumer"]));
    assert_eq!(f.trace(), "");
}

#[test]
fn partial_environment_refused() {
    let mut f = Fixture::new();
    f.set("SQLITE3_STATIC", "1");
    f.rejected(&f.invoke_with(None, &["--", "consumer"]));
    assert_eq!(f.trace(), "");
}

/// An explicit parent skips the partial-environment check; a missing
/// `SQLITE3_LIB_DIR` is still a handled failure (exit 1), never a panic.
#[test]
fn explicit_parent_with_partial_environment_fails_without_panic() {
    let mut f = Fixture::new();
    f.set("SQLITE3_STATIC", "1");
    let outcome = f.invoke(&["--", "consumer"]);
    assert_eq!(outcome.code, 1, "{}", outcome.stderr);
    assert!(
        outcome
            .stderr
            .contains("prepare-sqlite-ci: 'SQLITE3_LIB_DIR'"),
        "{}",
        outcome.stderr
    );
    f.rejected(&outcome);
    assert_eq!(f.trace(), "");
}

#[test]
fn cross_cargo_target_fails_before_download() {
    let mut f = Fixture::new();
    f.set("CARGO_BUILD_TARGET", "unsupported-cross-target");
    f.rejected(&f.invoke(&["--", "consumer"]));
    assert_eq!(f.trace(), "");
    f.unset("CARGO_BUILD_TARGET");
    f.rejected(&f.invoke(&["--", "consumer", "--target=unsupported-cross-target"]));
    f.rejected(&f.invoke(&["--", "consumer", "--target", "unsupported-cross-target"]));
    f.rejected(&f.invoke(&["--", "consumer", "--target"]));
    assert_eq!(f.trace(), "");
}

#[test]
fn rustc_host_mismatch_never_exports_or_consumes() {
    let mut f = Fixture::new();
    f.set("RUSTC_HOST", "unsupported-host");
    f.rejected(&f.invoke(&["--", "consumer"]));
}

#[test]
fn rustc_failure_never_exports_or_consumes() {
    let mut f = Fixture::new();
    f.set("RUSTC_FAIL", "1");
    f.rejected(&f.invoke(&["--", "consumer"]));
}

#[test]
fn unmatched_prefix_never_downloads_or_overwrites() {
    let mut f = Fixture::new();
    for (k, v) in [
        ("SQLITE3_LIB_DIR", "/unowned/lib"),
        ("SQLITE3_INCLUDE_DIR", "/unowned/include"),
        ("SQLITE3_STATIC", "1"),
        ("SQLITE3_NO_PKG_CONFIG", "1"),
    ] {
        f.set(k, v);
    }
    f.rejected(&f.invoke(&["--", "consumer"]));
    assert_eq!(f.trace(), "");
}

#[test]
fn cache_identity_changes_with_clang_and_rust_toolchain() {
    let mut f = Fixture::new();
    assert_eq!(f.invoke(&[]).code, 0);
    let first = f.last_output_line();
    fs::write(f.clang.join("libclang.so"), b"different fixture libclang").unwrap();
    assert_eq!(f.invoke(&[]).code, 0);
    let second = f.last_output_line();
    assert_ne!(first, second);
    f.set("RUSTC_VERSION", "different fixture toolchain");
    assert_eq!(f.invoke(&[]).code, 0);
    assert_ne!(second, f.last_output_line());
    assert_eq!(f.trace().matches("curl ").count(), 1);
}

#[test]
fn cache_identity_tracks_bindgen_environment_without_disclosing_values() {
    let mut f = Fixture::new();
    assert_eq!(f.invoke(&[]).code, 0);
    let first = f.last_output_line();
    f.set(
        "BINDGEN_EXTRA_CLANG_ARGS",
        "-Dfixture_private_build_override",
    );
    assert_eq!(f.invoke(&[]).code, 0);
    assert_ne!(first, f.last_output_line());
    let inputs = fs::read_to_string(f.parent.join("consumer-inputs.json")).unwrap();
    assert!(!inputs.contains("fixture_private_build_override"));
    assert!(inputs.contains("build_environment_sha256"));
}

#[test]
fn unknown_prefix_is_preserved() {
    let f = Fixture::new();
    let prefix = f.prefix();
    fs::create_dir(&prefix).unwrap();
    fs::write(prefix.join("sentinel"), b"keep").unwrap();
    f.rejected(&f.invoke(&["--", "consumer"]));
    let names: Vec<_> = fs::read_dir(&prefix)
        .unwrap()
        .map(|e| e.unwrap().file_name())
        .collect();
    assert_eq!(names, ["sentinel"]);
    assert_eq!(fs::read(prefix.join("sentinel")).unwrap(), b"keep");
}

#[test]
fn corrupt_static_archive_not_replaced_or_consumed() {
    let f = Fixture::new();
    assert_eq!(f.invoke(&[]).code, 0);
    fs::write(&f.github_env, "").unwrap();
    fs::write(&f.github_output, "").unwrap();
    let archive = f.prefix().join("lib/libsqlite3.a");
    fs::write(&archive, b"corrupt").unwrap();
    f.rejected(&f.invoke(&["--", "consumer"]));
    assert_eq!(fs::read(&archive).unwrap(), b"corrupt");
}

#[test]
fn verified_existing_environment_revalidates_helper() {
    let mut f = Fixture::new();
    assert_eq!(f.invoke(&[]).code, 0);
    for (k, v) in f.github_env_map() {
        f.set(&k, v);
    }
    assert_eq!(f.invoke_with(None, &["--", "consumer"]).code, 0);
    assert_eq!(f.trace().matches("curl ").count(), 1);
    assert_eq!(f.trace().matches("helper ").count(), 2);
}

#[test]
fn preflight_exports_no_build_paths_and_matches_verified_identity() {
    let f = Fixture::new();
    assert_eq!(f.invoke(&["--identity-only"]).code, 0);
    assert_eq!(fs::read_to_string(&f.github_env).unwrap(), "");
    assert!(!f.prefix().exists());
    assert!(f.parent.join("prepare-inputs.json").is_file());
    let expected = f
        .last_output_line()
        .strip_prefix("cache_identity=")
        .unwrap()
        .to_owned();
    assert_eq!(f.invoke(&["--expected-cache-identity", &expected]).code, 0);
    let consumer = f.json("consumer-inputs.json");
    assert_eq!(consumer["cache_identity"], expected.as_str());
    assert!(consumer["helper_timings"].get("archive").is_some());
}

#[test]
fn identity_only_refuses_consumer_or_env_file() {
    let f = Fixture::new();
    f.rejected(&f.invoke(&["--identity-only", "--", "consumer"]));
    let env_file = f.root.join("env");
    f.rejected(&f.invoke(&["--identity-only", "--env-file", env_file.to_str().unwrap()]));
    assert!(!env_file.exists());
    assert_eq!(f.trace(), "");
}

#[test]
fn wrong_expected_identity_cannot_export_or_build() {
    let f = Fixture::new();
    f.rejected(&f.invoke(&["--expected-cache-identity", &"f".repeat(64)]));
    assert!(!f.prefix().exists());
}

#[test]
fn timings_do_not_change_cache_identity() {
    let f = Fixture::new();
    assert_eq!(f.invoke(&[]).code, 0);
    let first = f.last_output_line();
    let timing_path = f.parent.join(format!("{}.prepare-timings.json", target()));
    fs::write(&timing_path, r#"{"archive": {"elapsed_seconds": 30.1}}"#).unwrap();
    let manifest_path = f.prefix().join("manifest.json");
    let before = fs::read(&manifest_path).unwrap();
    let outcome = f.invoke(&[]);
    assert_eq!(outcome.code, 0, "{}", outcome.stderr);
    assert!(outcome.stderr.contains("cache: HIT;"));
    assert_eq!(first, f.last_output_line());
    assert_eq!(before, fs::read(&manifest_path).unwrap());
    for timing in f.json("consumer-inputs.json")["timings"]
        .as_object()
        .unwrap()
        .values()
    {
        let elapsed = timing["elapsed_seconds"].as_f64().unwrap();
        assert!((0.0..=86_400.0).contains(&elapsed));
        assert!(timing["timeout_seconds"].as_u64().unwrap() > 0);
    }
}

#[test]
fn explicit_cache_fallback_preserves_rejected_prefix_and_logs_build() {
    let f = Fixture::new();
    assert_eq!(f.invoke(&[]).code, 0);
    let archive = f.prefix().join("lib/libsqlite3.a");
    fs::write(&archive, b"rejected bytes").unwrap();
    let outcome = f.invoke(&["--cache-fallback"]);
    assert_eq!(outcome.code, 0, "{}", outcome.stderr);
    assert!(outcome.stderr.contains("cache: REJECTED;"));
    assert!(outcome.stderr.contains("cache: MISS/BUILD;"));
    let rejected: Vec<PathBuf> = fs::read_dir(&f.parent)
        .unwrap()
        .map(|e| e.unwrap().path())
        .filter(|p| {
            p.file_name()
                .unwrap()
                .to_string_lossy()
                .starts_with(&format!("{}.rejected-", target()))
        })
        .collect();
    assert_eq!(rejected.len(), 1);
    assert_eq!(
        fs::read(rejected[0].join("lib/libsqlite3.a")).unwrap(),
        b"rejected bytes"
    );
    assert_eq!(fs::read(&archive).unwrap(), b"fixture static archive");
}

#[test]
fn cache_fallback_cannot_bypass_source_authentication_or_symlink() {
    let f = Fixture::new();
    let archive = f.parent.join("sqlite-amalgamation-3530400.zip");
    fs::write(&archive, b"wrong source").unwrap();
    f.rejected(&f.invoke(&["--cache-fallback"]));
    fs::remove_file(&archive).unwrap();
    std::os::unix::fs::symlink(&f.root, f.prefix()).unwrap();
    f.rejected(&f.invoke(&["--cache-fallback"]));
}

#[test]
fn consumer_exit_is_propagated() {
    let mut f = Fixture::new();
    f.set("CONSUMER_EXIT", "23");
    assert_eq!(f.invoke(&["--", "consumer"]).code, 23);
}

#[test]
fn consumer_signal_is_python_negative_returncode() {
    let f = Fixture::new();
    let outcome = f.invoke(&["--", "bash", "-c", "kill -TERM $$"]);
    assert_eq!(outcome.code, -15);
}

#[test]
fn exports_are_printed_only_without_env_destinations() {
    let f = Fixture::new();
    let context = Context {
        env: f.env.clone(),
        helper: f.helper.clone(),
    };
    let (mut out, mut err) = (Vec::new(), Vec::new());
    let code = run(
        vec!["--parent".into(), f.parent.clone().into()],
        &context,
        &mut out,
        &mut err,
    );
    assert_eq!(code, 0, "{}", String::from_utf8_lossy(&err));
    let printed = String::from_utf8(out).unwrap();
    assert_eq!(
        printed,
        fs::read_to_string(f.prefix().join("env.sh")).unwrap()
    );
    let env_file = f.root.join("sqlite.env");
    let outcome = f.invoke(&["--env-file", env_file.to_str().unwrap()]);
    assert_eq!(outcome.code, 0);
    assert_eq!(outcome.stdout, "");
    assert_eq!(fs::read_to_string(env_file).unwrap(), printed);
}

#[test]
fn usage_errors_exit_two_before_any_work() {
    let f = Fixture::new();
    for extra in [
        &["--parnet", "x"][..],
        &["--env-file"],
        &["--identity-only=yes"],
        &["--env-file", "--", "consumer"],
        &["--expected-cache-identity", "-h"],
    ] {
        let outcome = f.invoke(extra);
        assert_eq!(outcome.code, 2, "{extra:?}");
        assert!(outcome.stderr.contains("usage:"));
    }
    assert_eq!(f.trace(), "");
}
