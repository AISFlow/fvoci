//! `xtask selected-library`: the Rust workflow's selected SQLite library
//! controls. One `cargo test --no-run` builds the root library test target,
//! then every registered filter runs alone as `<executable> <filter> --exact`,
//! so all runs share that single compile and each test keeps its own process.
//!
//! A run passes only when exactly the requested, nonignored test ran and
//! passed. A filter that matches nothing, an ignored test, a second test or a
//! failed build is a failure; the first failure stops the run.
//!
//! The filter list is `xtask/selected-library-filters.txt` in the checkout
//! (the working directory), one exact libtest name per line, read at run time
//! so it stays outside the SQLite build identity of the compiled sources.
//! `tools/ci/verify/rust-library.ts` reads the same file and pins its
//! content, so the runner and the workflow verifier share one list.
//!
//! Exit status: 0, 1 (refused or a control failed), or 2 (usage).

use crate::args::{self, Outcome};
use crate::rust_binaries::status_code;
use crate::rust_binaries_cohort::records;
use serde_json::{json, Value};
use std::collections::BTreeSet;
use std::ffi::OsString;
use std::fs;
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

pub const FILTERS_FILE: &str = "xtask/selected-library-filters.txt";

const USAGE: &str = "usage: xtask selected-library [-h] [--target-dir TARGET_DIR]";
const HELP: &str = "\
Build the root library tests once and run each selected SQLite library
control as one exact libtest filter

options:
  -h, --help                show this help message and exit
  --target-dir TARGET_DIR   Cargo target directory for the library build
";

/// The registered filters: LF-terminated lines, each a nonempty, unique
/// `ident(::ident)*` libtest name. Anything else refuses the whole list.
pub fn filters(text: &str) -> Result<Vec<&str>, String> {
    let body = text
        .strip_suffix('\n')
        .ok_or("selected library filters must end with a newline")?;
    let mut seen = BTreeSet::new();
    let mut list = Vec::new();
    for line in body.split('\n') {
        let ident = |part: &str| {
            part.chars()
                .next()
                .is_some_and(|c| c == '_' || c.is_ascii_alphabetic())
                && part.chars().all(|c| c == '_' || c.is_ascii_alphanumeric())
        };
        if !line.split("::").all(ident) {
            return Err(format!(
                "selected library filter is not a test path: {line:?}"
            ));
        }
        if !seen.insert(line) {
            return Err(format!("selected library filter is repeated: {line}"));
        }
        list.push(line);
    }
    Ok(list)
}

/// The single library test executable in Cargo's JSON records and the package
/// directory Cargo runs it from. Zero or several candidates are refused.
pub fn library_executable(records: &[Value]) -> Result<(PathBuf, PathBuf), String> {
    let candidates: Vec<&Value> = records
        .iter()
        .filter(|r| r["profile"]["test"] == true && r["target"]["kind"] == json!(["lib"]))
        .collect();
    let [record] = candidates.as_slice() else {
        return Err(format!(
            "selected library build must produce exactly one library test executable, found {}",
            candidates.len()
        ));
    };
    let executable = record["executable"].as_str().unwrap_or_default();
    let package = record["manifest_path"]
        .as_str()
        .map(Path::new)
        .and_then(Path::parent)
        .filter(|dir| dir.is_absolute())
        .ok_or("selected library record has no package directory")?;
    Ok((PathBuf::from(executable), package.to_path_buf()))
}

/// Python `str.split("\n")`-style lines with the oracle's `re.MULTILINE`
/// line shapes, parsed by hand. `\S` is `!char::is_whitespace`.
fn running_count(line: &str) -> Option<&str> {
    let rest = line.strip_prefix("running ")?;
    let rest = rest
        .strip_suffix(" tests")
        .or_else(|| rest.strip_suffix(" test"))?;
    (!rest.is_empty() && rest.bytes().all(|b| b.is_ascii_digit())).then_some(rest)
}

fn token(text: &str) -> Option<(&str, &str)> {
    let end = text.find(char::is_whitespace).unwrap_or(text.len());
    (end > 0).then(|| text.split_at(end))
}

fn executed_test(line: &str) -> Option<(&str, &str)> {
    let (name, rest) = token(line.strip_prefix("test ")?)?;
    let (status, _) = token(rest.strip_prefix(" ... ")?)?;
    Some((name, status))
}

/// The text after a nonempty run of ASCII digits.
fn after_digits(text: &str) -> Option<&str> {
    let end = text
        .find(|c: char| !c.is_ascii_digit())
        .unwrap_or(text.len());
    (end > 0).then(|| &text[end..])
}

/// `finished in [0-9]+(?:\.[0-9]+)?s` after the fixed counts.
fn passed_summary(summary: &str) -> bool {
    let Some(rest) = summary.strip_prefix("ok. 1 passed; 0 failed; 0 ignored; 0 measured; ") else {
        return false;
    };
    let Some(rest) = after_digits(rest).and_then(|r| r.strip_prefix(" filtered out; finished in "))
    else {
        return false;
    };
    let Some(rest) = after_digits(rest) else {
        return false;
    };
    let rest = match rest.strip_prefix('.') {
        Some(fraction) => match after_digits(fraction) {
            Some(rest) => rest,
            None => return false,
        },
        None => rest,
    };
    rest == "s"
}

/// Accept exactly the requested, nonignored library test, never compilation.
pub fn result_error(filter: &str, code: i32, output: &str) -> Option<&'static str> {
    if code != 0 {
        return Some("rust: selected library test command failed");
    }
    let lines: Vec<&str> = output.split('\n').collect();
    let counts: Vec<&str> = lines.iter().filter_map(|l| running_count(l)).collect();
    if counts != ["1"] {
        return Some("rust: selected library command must run exactly one test");
    }
    let executed: Vec<(&str, &str)> = lines.iter().filter_map(|l| executed_test(l)).collect();
    if executed != [(filter, "ok")] {
        return Some("rust: selected library result must be the exact requested test and ok");
    }
    let summaries: Vec<&str> = lines
        .iter()
        .filter_map(|l| l.strip_prefix("test result: "))
        .collect();
    if summaries.len() != 1 || !passed_summary(summaries[0]) {
        return Some("rust: selected library result must pass one test without failure or ignore");
    }
    None
}

/// `cargo test --no-run` of the root library with Cargo's JSON on stdout and
/// rendered diagnostics on stderr; returns the executable records.
fn build(target_dir: Option<&str>) -> Result<Vec<Value>, String> {
    let mut command = Command::new("cargo");
    command.args([
        "test",
        "--locked",
        "--offline",
        "--features",
        "db-tests",
        "--lib",
        "--no-run",
        "--message-format=json-render-diagnostics",
    ]);
    if let Some(dir) = target_dir {
        command.args(["--target-dir", dir]);
    }
    without_outer_package_env(&mut command);
    let output = command
        .stdin(Stdio::null())
        .stderr(Stdio::inherit())
        .output()
        .map_err(|e| format!("cargo: {e}"))?;
    if !output.status.success() {
        return Err(format!(
            "rust: selected library build failed with status {}",
            status_code(output.status)
        ));
    }
    let text = String::from_utf8(output.stdout).map_err(|_| "cargo output is not UTF-8")?;
    records(&text).map_err(|e| format!("selected library build: {e}"))
}

/// Drop the package variables an enclosing `cargo run` (the `cargo xtask` alias)
/// set for xtask itself. Build scripts declare some of them with
/// `rerun-if-env-changed` (ring: CARGO_PKG_NAME, CARGO_MANIFEST_DIR, ...), and
/// Cargo compares its own environment, so passing them on would rebuild ring
/// and its dependents in the shared target directory.
fn without_outer_package_env(command: &mut Command) {
    for (key, _) in std::env::vars_os() {
        let Some(key) = key.to_str() else { continue };
        if key.starts_with("CARGO_PKG_")
            || matches!(
                key,
                "CARGO_CRATE_NAME"
                    | "CARGO_BIN_NAME"
                    | "CARGO_PRIMARY_PACKAGE"
                    | "CARGO_MANIFEST_DIR"
                    | "CARGO_MANIFEST_PATH"
            )
        {
            command.env_remove(key);
        }
    }
}

/// The library tests get their own package directory and manifest, as
/// `cargo test` gave them.
fn package_env(command: &mut Command, package: &Path) {
    without_outer_package_env(command);
    command
        .env("CARGO_MANIFEST_DIR", package)
        .env("CARGO_MANIFEST_PATH", package.join("Cargo.toml"));
}

/// Run one exact filter with stdout and stderr on one pipe, copying the output
/// through as it arrives; returns the status and the captured bytes.
pub fn run_filter(executable: &Path, cwd: &Path, filter: &str) -> Result<(i32, Vec<u8>), String> {
    let spawn_error = |e: io::Error| format!("{}: {e}", executable.display());
    let (mut reader, writer) = io::pipe().map_err(spawn_error)?;
    let mut command = Command::new(executable);
    command
        .args([filter, "--exact"])
        .current_dir(cwd)
        .stdin(Stdio::null())
        .stdout(writer.try_clone().map_err(spawn_error)?)
        .stderr(writer);
    package_env(&mut command, cwd);
    let mut child = command.spawn().map_err(spawn_error)?;
    // The command holds the parent's write ends; EOF needs them closed.
    drop(command);
    let mut output = Vec::new();
    let mut stdout = io::stdout().lock();
    let mut chunk = [0; 8192];
    let copied = loop {
        match reader.read(&mut chunk) {
            Ok(0) => break stdout.flush(),
            Ok(n) => {
                output.extend_from_slice(&chunk[..n]);
                if let Err(error) = stdout.write_all(&chunk[..n]) {
                    break Err(error);
                }
            }
            Err(error) if error.kind() == io::ErrorKind::Interrupted => {}
            Err(error) => break Err(error),
        }
    };
    if copied.is_err() {
        let _ = child.kill();
    }
    let status = child.wait().map_err(spawn_error)?;
    copied.map_err(|e| format!("selected library output: {e}"))?;
    Ok((status_code(status), output))
}

/// Run every filter in order and stop at the first one that is not an exact pass.
pub fn run_filters(executable: &Path, cwd: &Path, filters: &[&str]) -> Result<(), String> {
    if filters.is_empty() {
        return Err("selected library filters are empty".into());
    }
    for filter in filters {
        let (code, output) = run_filter(executable, cwd, filter)?;
        let error = match std::str::from_utf8(&output) {
            Ok(text) => result_error(filter, code, text),
            Err(_) => Some("rust: selected library output is not UTF-8"),
        };
        if let Some(error) = error {
            return Err(format!("{error}: {filter}"));
        }
    }
    Ok(())
}

fn execute(target_dir: Option<&str>) -> Result<(), String> {
    let text = fs::read(FILTERS_FILE).map_err(|e| format!("{FILTERS_FILE}: {e}"))?;
    let text = String::from_utf8(text).map_err(|_| format!("{FILTERS_FILE} is not UTF-8"))?;
    let list = filters(&text)?;
    let (executable, package) = library_executable(&build(target_dir)?)?;
    run_filters(&executable, &package, &list)
}

fn usage(message: &str) -> i32 {
    eprintln!("{USAGE}\nxtask selected-library: error: {message}");
    2
}

/// Process entry: real environment, stdout and stderr.
pub fn main(argv: Vec<OsString>) -> i32 {
    let parsed = match args::parse(argv, &[args::value("target-dir")], false) {
        Outcome::Parsed(parsed) => parsed,
        Outcome::Help => {
            print!("{USAGE}\n\n{HELP}");
            return 0;
        }
        Outcome::Usage(message) => return usage(&message),
    };
    match execute(parsed.get("target-dir")) {
        Ok(()) => 0,
        Err(message) => {
            eprintln!("{message}");
            1
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn success(name: &str) -> String {
        format!(
            "running 1 test\ntest {name} ... ok\n\
             test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 99 filtered out; finished in 0.01s\n"
        )
    }

    fn registered() -> String {
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).parent().unwrap();
        fs::read_to_string(root.join(FILTERS_FILE)).unwrap()
    }

    #[test]
    fn registered_filters_parse_as_unique_test_paths() {
        let text = registered();
        let list = filters(&text).unwrap();
        assert!(!list.is_empty());
        assert_eq!(list.len(), text.lines().count());
    }

    #[test]
    fn malformed_filter_lists_are_refused() {
        for text in [
            "",
            "\n",
            "a::b",
            "a::b\n\n",
            "a::b\na::b\n",
            "a::b\r\n",
            "\u{feff}a::b\n",
            " a::b\n",
            "a::b \n",
            "a:::b\n",
            "a::\n",
            "::a\n",
            "1a::b\n",
            "a::b-c\n",
            "a::é\n",
        ] {
            assert!(filters(text).is_err(), "{text:?}");
        }
        assert_eq!(filters("a::b\n_c\n").unwrap(), ["a::b", "_c"]);
    }

    #[test]
    fn every_filter_passes_with_one_exact_result() {
        for name in filters(&registered()).unwrap() {
            assert_eq!(result_error(name, 0, &success(name)), None);
        }
        let name = "a::b";
        for finished in ["0s", "12s", "0.01s", "3.250s"] {
            let output = success(name).replace("0.01s", finished);
            assert_eq!(result_error(name, 0, &output), None, "{finished}");
        }
        // Lines outside the three checked shapes (Cargo, test output) are ignored.
        let noisy = format!("note: x\n{}trailing\n", success(name));
        assert_eq!(result_error(name, 0, &noisy), None);
    }

    #[test]
    fn zero_ignored_wrong_extra_and_failed_results_are_not_a_pass() {
        let name = "db::stars::t";
        let good = success(name);
        let cases: Vec<(i32, String)> = vec![
            (0, String::new()),
            (0, "running 1 test\n".into()),
            (0, good.split("test result:").next().unwrap().into()),
            (0, good.replace(&format!("test {name} ... ok\n"), "")),
            (1, good.clone()),
            (101, good.clone()),
            (128 + 9, good.clone()),
            (
                0,
                good.replace("running 1 test", "running 0 tests")
                    .replace("1 passed", "0 passed"),
            ),
            (
                0,
                good.replace("... ok", "... ignored")
                    .replace("1 passed", "0 passed")
                    .replace("0 ignored", "1 ignored"),
            ),
            (0, good.replace(name, "different::test")),
            (0, good.replace(name, "db::stars::t2")),
            (
                0,
                good.replace("running 1 test", "running 2 tests")
                    .replace("1 passed", "2 passed"),
            ),
            (0, format!("{good}test different::test ... ok\n")),
            (0, format!("{good}{good}")),
            (0, good.replace("0 measured", "1 measured")),
            (0, good.replace("0 failed", "1 failed")),
            (0, good.replace('\n', "\r\n")),
            (0, good.replace("... ok", "... FAILED")),
            (0, good.replace("finished in 0.01s", "finished in 0.s")),
            (0, good.replace("finished in 0.01s", "finished in .5s")),
            (0, good.replace("finished in 0.01s", "finished in 1s ")),
            (0, good.replace("99 filtered out", "filtered out")),
            (0, good.replace("test result: ok.", "test result: FAILED.")),
        ];
        for (code, output) in &cases {
            assert!(
                result_error(name, *code, output).is_some(),
                "{code} {output:?}"
            );
        }
        // Only the count line differs: the running-count rule alone must refuse it.
        assert_eq!(
            result_error(name, 0, &good.replace("running 1 test", "running 2 tests")),
            Some("rust: selected library command must run exactly one test")
        );
        assert_eq!(
            result_error(name, 0, &good.replace("... ok", "... ignored")),
            Some("rust: selected library result must be the exact requested test and ok")
        );
    }

    #[test]
    fn whitespace_inside_a_result_line_never_widens_a_match() {
        let name = "a::b";
        // A non-space separator makes the line a different test name, not a skip.
        let good = success(name);
        for line in ["test a::b\u{1c}... ok", "test a::b\u{a0}... ok"] {
            let output = good.replace(&format!("test {name} ... ok"), line);
            assert!(result_error(name, 0, &output).is_some(), "{line:?}");
        }
        let output = good.replace("... ok", "... ok\u{1c}x");
        assert!(result_error(name, 0, &output).is_some());
        let output = good.replace("... ok", "... ok (0.01s)");
        assert_eq!(result_error(name, 0, &output), None);
    }

    fn artifact(kind: &str, test: bool, executable: Option<&str>) -> Value {
        json!({
            "reason": "compiler-artifact",
            "manifest_path": "/w/Cargo.toml",
            "target": {"kind": [kind], "name": "fvoci_server"},
            "profile": {"test": test},
            "executable": executable,
        })
    }

    #[test]
    fn exactly_one_library_test_executable_is_selected() {
        let lib = artifact("lib", true, Some("/w/target/debug/deps/fvoci_server-1"));
        let selected = library_executable(&[artifact("bin", true, Some("/x")), lib.clone()]);
        assert_eq!(
            selected,
            Ok((
                PathBuf::from("/w/target/debug/deps/fvoci_server-1"),
                PathBuf::from("/w")
            ))
        );
        for records in [
            vec![],
            vec![artifact("lib", false, Some("/x"))],
            vec![artifact("test", true, Some("/x"))],
            vec![lib.clone(), lib.clone()],
        ] {
            assert!(library_executable(&records).is_err(), "{records:?}");
        }
        let mut relative = lib;
        relative["manifest_path"] = json!("Cargo.toml");
        assert!(library_executable(&[relative]).is_err());
    }
}
