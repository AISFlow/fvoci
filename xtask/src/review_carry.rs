//! Replace manual stable patch-id, single-commit range-diff and fresh local checks.
//! This report never carries a reviewer ACCEPT or a user's SHA approval.

use std::ffi::OsString;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, ExitCode, Output, Stdio};

const VERIFY_SOURCE: &str = ".agents/skills/fvoci-fast-verify/SKILL.md";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Args {
    old: String,
    new: String,
}

pub fn parse_args(mut args: impl Iterator<Item = OsString>) -> Result<Args, String> {
    let old = parse_sha(args.next())?;
    let new = parse_sha(args.next())?;
    if args.next().is_some() {
        return Err("review-carry requires exactly two full commit SHAs".into());
    }
    Ok(Args { old, new })
}

fn parse_sha(value: Option<OsString>) -> Result<String, String> {
    let value = value
        .and_then(|value| value.into_string().ok())
        .ok_or("review-carry requires two lowercase 40-character commit SHAs")?;
    if value.len() != 40
        || !value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    {
        return Err("review-carry requires lowercase 40-character commit SHAs".into());
    }
    Ok(value)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum State {
    Pass,
    Fail,
    NotRun,
    Missing,
}

impl State {
    fn label(self) -> &'static str {
        match self {
            Self::Pass => "PASS",
            Self::Fail => "FAIL",
            Self::NotRun => "NOTRUN",
            Self::Missing => "MISSING",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Count {
    NotApplicable,
    Tests,
}

#[derive(Debug, Clone)]
struct Check {
    program: &'static str,
    args: Vec<String>,
    count: Count,
    source: &'static str,
    available: bool,
}

impl Check {
    fn command(&self) -> String {
        std::iter::once(self.program.to_owned())
            .chain(self.args.iter().map(|arg| {
                if arg
                    .bytes()
                    .all(|byte| byte.is_ascii_alphanumeric() || b"-_/.:^".contains(&byte))
                {
                    arg.clone()
                } else {
                    format!("{arg:?}")
                }
            }))
            .collect::<Vec<_>>()
            .join(" ")
    }
}

#[derive(Debug)]
struct CheckResult {
    check: Check,
    sha: String,
    state: State,
    exit: Option<i32>,
    count: Option<usize>,
    failures: Vec<String>,
    first_error: Option<String>,
}

#[derive(Debug)]
struct Comparison {
    old_patch: Option<String>,
    new_patch: Option<String>,
    range_diff: String,
    parent: String,
    paths: Vec<String>,
}

impl Comparison {
    fn identical(&self) -> bool {
        self.old_patch.is_some() && self.old_patch == self.new_patch
    }
}

fn command(cwd: &Path, program: &str) -> Command {
    let mut command = Command::new(program);
    // Verify and execute against the actual cwd, not an inherited Git override.
    for name in [
        "GIT_DIR",
        "GIT_WORK_TREE",
        "GIT_INDEX_FILE",
        "GIT_COMMON_DIR",
        "GIT_OBJECT_DIRECTORY",
        "GIT_ALTERNATE_OBJECT_DIRECTORIES",
        "GIT_NAMESPACE",
    ] {
        command.env_remove(name);
    }
    command
        .current_dir(cwd)
        .env("GIT_OPTIONAL_LOCKS", "0")
        .env("GIT_NO_REPLACE_OBJECTS", "1");
    command
}

fn git(cwd: &Path, args: &[&str]) -> Result<Output, String> {
    command(cwd, "git")
        .args(["--no-pager", "-c", "core.hooksPath=/dev/null"])
        .args(args)
        .output()
        .map_err(|_| "MISSING: git could not be started".into())
}

fn git_text(cwd: &Path, args: &[&str]) -> Result<String, String> {
    let output = git(cwd, args)?;
    if !output.status.success() {
        return Err(format!(
            "FAIL: git {} exit={} first-error={}",
            args.first().unwrap_or(&""),
            output
                .status
                .code()
                .map_or("signal".into(), |code| code.to_string()),
            redact(
                String::from_utf8_lossy(&output.stderr)
                    .lines()
                    .next()
                    .unwrap_or("MISSING diagnostic"),
                cwd
            )
        ));
    }
    String::from_utf8(output.stdout).map_err(|_| "MISSING: non-UTF-8 git output".into())
}

fn parent(cwd: &Path, sha: &str) -> Result<String, String> {
    let object_type = git_text(cwd, &["cat-file", "-t", sha])?;
    if object_type.trim() != "commit" {
        return Err("FAIL: SHA must name a commit object".into());
    }
    let parents = git_text(cwd, &["rev-list", "--parents", "-n", "1", sha])?;
    let fields: Vec<_> = parents.split_whitespace().collect();
    if fields.len() != 2 || fields[0] != sha {
        return Err("MISSING: review-carry requires a non-root, non-merge exact commit".into());
    }
    Ok(fields[1].to_owned())
}

fn validate_checkout(cwd: &Path, sha: &str) -> Result<(), String> {
    let root = git_text(cwd, &["rev-parse", "--show-toplevel"])?;
    let root = PathBuf::from(root.trim());
    if cwd.canonicalize().ok() != root.canonicalize().ok() || !root.is_dir() {
        return Err("NOTRUN: run from the current worktree's repository root".into());
    }
    if git_text(cwd, &["rev-parse", "HEAD"])?.trim() != sha {
        return Err(
            "NOTRUN: current worktree HEAD is not the exact new SHA; no checkout performed".into(),
        );
    }
    if !git_text(cwd, &["status", "--porcelain=v1", "--untracked-files=all"])?.is_empty() {
        return Err("NOTRUN: current worktree has tracked or untracked changes".into());
    }
    Ok(())
}

fn patch_id(cwd: &Path, before: &str, sha: &str) -> Result<Option<String>, String> {
    let patch = git_text(
        cwd,
        &[
            "diff",
            "--no-ext-diff",
            "--no-textconv",
            "--binary",
            "--full-index",
            before,
            sha,
            "--",
        ],
    )?;
    let mut child = command(cwd, "git")
        .args(["patch-id", "--stable"])
        .current_dir(cwd)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|_| "MISSING: git patch-id could not be started")?;
    // Close the pipe before waiting, including on a write failure, and reap the child.
    let written = child
        .stdin
        .take()
        .ok_or("MISSING: patch-id stdin")?
        .write_all(patch.as_bytes());
    let output = child
        .wait_with_output()
        .map_err(|_| "FAIL: git patch-id could not be reaped")?;
    if written.is_err() || !output.status.success() {
        return Err("FAIL: git patch-id --stable".into());
    }
    let text = String::from_utf8(output.stdout).map_err(|_| "MISSING: patch-id encoding")?;
    let Some(id) = text.split_whitespace().next() else {
        return Ok(None);
    };
    parse_sha(Some(OsString::from(id)))?;
    Ok(Some(id.to_owned()))
}

fn compare(cwd: &Path, args: &Args) -> Result<Comparison, String> {
    let old_parent = parent(cwd, &args.old)?;
    let new_parent = parent(cwd, &args.new)?;
    let range_diff = git_text(
        cwd,
        &[
            "range-diff",
            "--no-color",
            "--no-ext-diff",
            &format!("{old_parent}..{}", args.old),
            &format!("{new_parent}..{}", args.new),
        ],
    )?;
    let paths = git_text(
        cwd,
        &[
            "diff",
            "--no-renames",
            "--name-only",
            "-z",
            &new_parent,
            &args.new,
            "--",
        ],
    )?
    .split('\0')
    .filter(|path| !path.is_empty())
    .map(str::to_owned)
    .collect();
    Ok(Comparison {
        old_patch: patch_id(cwd, &old_parent, &args.old)?,
        new_patch: patch_id(cwd, &new_parent, &args.new)?,
        range_diff,
        parent: new_parent,
        paths,
    })
}

fn source_contains(cwd: &Path, sha: &str, path: &str, command: &str) -> bool {
    git_text(cwd, &["show", &format!("{sha}:{path}")]).is_ok_and(|text| text.contains(command))
}

fn select_checks(cwd: &Path, sha: &str, comparison: &Comparison) -> (Vec<Check>, Vec<String>) {
    let mut checks = vec![Check {
        program: "git",
        args: vec![
            "diff".into(),
            "--check".into(),
            comparison.parent.clone(),
            sha.into(),
            "--".into(),
        ],
        count: Count::NotApplicable,
        source: "git diff --check (patch hygiene only)",
        available: true,
    }];
    let mut missing = Vec::new();
    let selector_paths = [
        "scripts/ci_selection.py",
        "scripts/test_ci_selection.py",
        "scripts/test-ci-selection.sh",
        "scripts/ci_selection_requirements.txt",
    ];
    let selector = comparison.paths.iter().any(|path| {
        selector_paths.contains(&path.as_str()) || path.starts_with(".github/workflows/")
    });
    if selector {
        checks.push(Check {
            program: "bash",
            args: vec!["scripts/test-ci-selection.sh".into()],
            count: Count::Tests,
            source: VERIFY_SOURCE,
            available: source_contains(
                cwd,
                sha,
                VERIFY_SOURCE,
                "bash scripts/test-ci-selection.sh",
            ) && git_text(
                cwd,
                &[
                    "cat-file",
                    "-t",
                    &format!("{sha}:scripts/test-ci-selection.sh"),
                ],
            )
            .is_ok_and(|kind| kind.trim() == "blob"),
        });
    }
    if comparison
        .paths
        .iter()
        .any(|path| path.starts_with("xtask/"))
    {
        // No invented registry: commands become runnable only when the actual CI
        // source registers them. At the scaffold SHA these are honestly MISSING.
        for (args, count) in [
            (
                vec!["fmt", "--manifest-path", "xtask/Cargo.toml", "--check"],
                Count::NotApplicable,
            ),
            (
                vec![
                    "check",
                    "--manifest-path",
                    "xtask/Cargo.toml",
                    "--locked",
                    "--offline",
                ],
                Count::NotApplicable,
            ),
            (
                vec![
                    "test",
                    "--manifest-path",
                    "xtask/Cargo.toml",
                    "--locked",
                    "--offline",
                ],
                Count::Tests,
            ),
            (
                vec![
                    "clippy",
                    "--manifest-path",
                    "xtask/Cargo.toml",
                    "--locked",
                    "--offline",
                    "--all-targets",
                    "--",
                    "-D",
                    "warnings",
                ],
                Count::NotApplicable,
            ),
        ] {
            let command = format!("cargo {}", args.join(" "));
            checks.push(Check {
                program: "cargo",
                args: args.into_iter().map(str::to_owned).collect(),
                count,
                source: ".github/workflows/rust.yml",
                available: source_contains(cwd, sha, ".github/workflows/rust.yml", &command),
            });
        }
        missing.push("xtask required-check selection is not established by the existing selector; local Cargo candidates do not prove complete coverage".into());
    }
    for path in &comparison.paths {
        if !selector_paths.contains(&path.as_str()) {
            missing.push(format!(
                "required local check selection unavailable for {path:?}"
            ));
        }
    }
    if comparison.paths.is_empty() {
        missing.push("empty commit has no patch identity or supported changed scope".into());
    }
    (checks, missing)
}

fn test_count(text: &str) -> Option<usize> {
    let mut total = None;
    for line in text.lines() {
        let fields: Vec<_> = line.split_whitespace().collect();
        let count = if fields.first() == Some(&"Ran") && fields.get(2) == Some(&"tests") {
            fields.get(1).and_then(|field| field.parse::<usize>().ok())
        } else if line.starts_with("test result:") {
            fields
                .iter()
                .position(|field| *field == "passed;")
                .and_then(|index| {
                    let passed = fields.get(index.checked_sub(1)?)?.parse::<usize>().ok()?;
                    let failed = fields.iter().position(|field| *field == "failed;")?;
                    Some(passed + fields.get(failed.checked_sub(1)?)?.parse::<usize>().ok()?)
                })
        } else {
            None
        };
        if let Some(count) = count {
            total = Some(total.unwrap_or(0) + count);
        }
    }
    total
}

fn redact(line: &str, cwd: &Path) -> String {
    let lower = line.to_ascii_lowercase();
    if [
        "bearer ",
        "token=",
        "token:",
        "password=",
        "secret=",
        "authorization:",
    ]
    .iter()
    .any(|marker| lower.contains(marker))
    {
        return "[credential diagnostic redacted]".into();
    }
    let line = line.replace(&cwd.to_string_lossy().to_string(), "<worktree>");
    line.split_whitespace()
        .map(|word| {
            let trimmed = word.trim_start_matches(['\'', '"', '(', '[']);
            if trimmed.starts_with('/') || trimmed.as_bytes().get(1) == Some(&b':') {
                "<absolute-path>"
            } else {
                word
            }
        })
        .collect::<Vec<_>>()
        .join(" ")
}

fn summarize_output(check: Check, sha: &str, output: Output, cwd: &Path) -> CheckResult {
    let text = format!(
        "{}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    let count = test_count(&text);
    let failures: Vec<String> = text
        .lines()
        .filter(|line| {
            (line.starts_with("test ") && line.ends_with(" FAILED"))
                || line.starts_with("FAIL: ")
                || line.starts_with("ERROR: ")
        })
        .map(|line| redact(line, cwd))
        .collect();
    let first_error = text
        .lines()
        .find(|line| {
            let line = line.trim_start();
            line.starts_with("error")
                || line.contains("panicked at")
                || line.starts_with("AssertionError")
                || line.starts_with("fatal:")
                || line.starts_with("FAIL:")
                || line.starts_with("ERROR:")
        })
        .or_else(|| {
            (!output.status.success())
                .then(|| text.lines().find(|line| !line.trim().is_empty()))
                .flatten()
        })
        .map(|line| redact(line, cwd));
    let summary_failed = text.lines().any(|line| {
        if line.starts_with("test result:") {
            let fields: Vec<_> = line.split_whitespace().collect();
            fields
                .iter()
                .position(|field| *field == "failed;")
                .is_some_and(|index| {
                    index
                        .checked_sub(1)
                        .and_then(|index| fields[index].parse::<usize>().ok())
                        .is_none_or(|failed| failed != 0)
                })
        } else {
            line.starts_with("FAILED (")
        }
    });
    let state = if !output.status.success() || !failures.is_empty() || summary_failed {
        State::Fail
    } else if check.count == Count::Tests && count.is_none_or(|count| count == 0) {
        State::Missing
    } else {
        State::Pass
    };
    CheckResult {
        check,
        sha: sha.into(),
        state,
        exit: output.status.code(),
        count,
        failures,
        first_error,
    }
}

fn run_check(cwd: &Path, sha: &str, check: Check) -> CheckResult {
    let unavailable = |check, state, error| CheckResult {
        check,
        sha: sha.into(),
        state,
        exit: None,
        count: None,
        failures: Vec::new(),
        first_error: Some(error),
    };
    if !check.available {
        return unavailable(
            check,
            State::Missing,
            "command or required-check source missing; NOTRUN".into(),
        );
    }
    if let Err(error) = validate_checkout(cwd, sha) {
        return unavailable(check, State::NotRun, error);
    }
    let mut process = command(cwd, check.program);
    process
        .args(&check.args)
        .current_dir(cwd)
        .env("CARGO_TARGET_DIR", cwd.join("target/review-carry"))
        .env("GIT_OPTIONAL_LOCKS", "0")
        .env("GIT_NO_REPLACE_OBJECTS", "1");
    if check.program == "bash"
        && check.args.len() == 1
        && check.args[0] == "scripts/test-ci-selection.sh"
    {
        process.env("PYTHONDONTWRITEBYTECODE", "1");
    }
    let output = process.output();
    let mut result = match output {
        Ok(output) => summarize_output(check, sha, output, cwd),
        Err(_) => unavailable(
            check,
            State::Missing,
            "command could not be started; NOTRUN".into(),
        ),
    };
    if let Err(error) = validate_checkout(cwd, sha) {
        result.state = State::NotRun;
        result.first_error = Some(error);
    }
    result
}

fn print_result(result: &CheckResult) {
    println!(
        "{} command={} sha={} cwd=<current-worktree> source={} exit={} count={}",
        result.state.label(),
        result.check.command(),
        result.sha,
        result.check.source,
        result
            .exit
            .map_or("NOTRUN/signal".into(), |exit| exit.to_string()),
        match result.check.count {
            Count::NotApplicable => "N/A (no tests)".into(),
            Count::Tests => result
                .count
                .map_or("MISSING".into(), |count| count.to_string()),
        }
    );
    for failure in &result.failures {
        println!("failure: {failure}");
    }
    if let Some(error) = &result.first_error {
        println!("first-error: {error}");
    }
}

pub fn run(args: Args) -> ExitCode {
    let outcome = (|| {
        let cwd = std::env::current_dir().map_err(|_| "MISSING: current worktree path")?;
        validate_checkout(&cwd, &args.new)?;
        let comparison = compare(&cwd, &args)?;
        println!(
            "old={} new={} parent={}",
            args.old, args.new, comparison.parent
        );
        println!(
            "stable-patch-id old={} new={} identical={}",
            comparison.old_patch.as_deref().unwrap_or("MISSING"),
            comparison.new_patch.as_deref().unwrap_or("MISSING"),
            comparison.identical()
        );
        println!(
            "range-diff command=git range-diff --no-color --no-ext-diff <old-parent>..{} {}..{}",
            args.old, comparison.parent, args.new
        );
        println!("range-diff begin\n{}range-diff end", comparison.range_diff);
        let (checks, missing) = select_checks(&cwd, &args.new, &comparison);
        let mut passed = comparison.identical() && missing.is_empty();
        for reason in missing {
            println!("MISSING: {reason}");
        }
        for check in checks {
            let result = run_check(&cwd, &args.new, check);
            passed &= result.state == State::Pass;
            print_result(&result);
        }
        println!("old local PASS results counted=0; CI=NOTRUN (local command only)");
        println!("reviewer ACCEPT carried=false; user SHA approval carried=false; independent acceptance=false");
        println!(
            "local comparison result={}",
            if passed {
                "PASS"
            } else {
                "FAIL/MISSING/NOTRUN"
            }
        );
        Ok::<bool, String>(passed)
    })();
    match outcome {
        Ok(true) => ExitCode::SUCCESS,
        Ok(false) => ExitCode::FAILURE,
        Err(error) => {
            eprintln!("{error}");
            ExitCode::FAILURE
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    static FIXTURE_ID: AtomicUsize = AtomicUsize::new(0);

    struct Fixture(PathBuf);
    impl Fixture {
        fn new() -> Self {
            let path = std::env::current_dir()
                .unwrap()
                .join("target/review-carry-fixtures")
                .join(format!(
                    "{}-{}",
                    std::process::id(),
                    FIXTURE_ID.fetch_add(1, Ordering::Relaxed)
                ));
            std::fs::create_dir_all(&path).unwrap();
            git_text(&path, &["init", "--quiet"]).unwrap();
            let fixture = Self(path);
            fixture.write("value.txt", "base\n");
            fixture.commit("base");
            fixture
        }
        fn write(&self, path: &str, value: &str) {
            let path = self.0.join(path);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(path, value).unwrap();
        }
        fn commit(&self, message: &str) -> String {
            git_text(&self.0, &["add", "--all"]).unwrap();
            git_text(
                &self.0,
                &[
                    "-c",
                    "user.name=fixture",
                    "-c",
                    "user.email=fixture@example.invalid",
                    "commit",
                    "--quiet",
                    "--allow-empty",
                    "-m",
                    message,
                ],
            )
            .unwrap();
            git_text(&self.0, &["rev-parse", "HEAD"])
                .unwrap()
                .trim()
                .into()
        }
        fn equivalent(&self) -> Args {
            let base = git_text(&self.0, &["rev-parse", "HEAD"]).unwrap();
            self.write("value.txt", "changed\n");
            let old = self.commit("old change");
            // Only this test-owned repository is branched; production never checks out.
            git_text(&self.0, &["checkout", "--quiet", "-b", "new", base.trim()]).unwrap();
            self.write("value.txt", "changed\n");
            let new = self.commit("new change");
            Args { old, new }
        }
    }
    impl Drop for Fixture {
        fn drop(&mut self) {
            std::fs::remove_dir_all(&self.0).unwrap();
        }
    }

    #[test]
    fn full_sha_args_only_and_no_shell_syntax() {
        let full = "a".repeat(40);
        assert!(parse_args([full.clone().into(), full.clone().into()].into_iter()).is_ok());
        for invalid in [
            "430a260a".into(),
            "A".repeat(40),
            format!("{};touch target/injected", "a".repeat(40)),
            "--help".into(),
        ] {
            assert!(parse_args([invalid.into(), full.clone().into()].into_iter()).is_err());
        }
        assert!(parse_args([full.clone().into()].into_iter()).is_err());
        assert!(
            parse_args([full.clone().into(), full.into(), "extra".into()].into_iter()).is_err()
        );
    }

    #[test]
    fn exact_git_commits_same_patch_different_metadata() {
        let fixture = Fixture::new();
        let args = fixture.equivalent();
        assert_ne!(args.old, args.new);
        validate_checkout(&fixture.0, &args.new).unwrap();
        let comparison = compare(&fixture.0, &args).unwrap();
        assert!(comparison.identical());
        assert!(comparison.range_diff.contains("old change"));
        assert!(comparison.range_diff.contains("new change"));
        assert_eq!(comparison.paths, ["value.txt"]);
        let (checks, missing) = select_checks(&fixture.0, &args.new, &comparison);
        assert!(!missing.is_empty()); // Unknown scope never becomes accepted.
        let result = run_check(&fixture.0, &args.new, checks[0].clone());
        assert_eq!(result.state, State::Pass);
        assert_eq!(result.exit, Some(0));
        assert_eq!(result.sha, args.new);
        assert_eq!(result.count, None);
    }

    #[test]
    fn patch_difference_missing_commit_and_empty_patch_rejected() {
        let fixture = Fixture::new();
        let args = fixture.equivalent();
        fixture.write("value.txt", "different\n");
        let different = fixture.commit("different");
        assert!(!compare(
            &fixture.0,
            &Args {
                old: args.old.clone(),
                new: different.clone()
            }
        )
        .unwrap()
        .identical());
        assert!(compare(
            &fixture.0,
            &Args {
                old: "0".repeat(40),
                new: different
            }
        )
        .is_err());
        let empty = fixture.commit("empty");
        assert!(!compare(
            &fixture.0,
            &Args {
                old: empty.clone(),
                new: empty
            }
        )
        .unwrap()
        .identical());
    }

    #[test]
    fn wrong_checkout_subdirectory_and_dirty_tree_rejected_without_mutation() {
        let fixture = Fixture::new();
        let args = fixture.equivalent();
        assert!(validate_checkout(&fixture.0, &args.old).is_err());
        let child = fixture.0.join("child");
        std::fs::create_dir(&child).unwrap();
        assert!(validate_checkout(&child, &args.new).is_err());
        assert_eq!(
            git_text(&fixture.0, &["rev-parse", "HEAD"]).unwrap().trim(),
            args.new
        );
        fixture.write("untracked.txt", "untracked\n");
        assert!(validate_checkout(&fixture.0, &args.new).is_err());
        std::fs::remove_file(fixture.0.join("untracked.txt")).unwrap();
        fixture.write("value.txt", "dirty\n");
        assert!(validate_checkout(&fixture.0, &args.new).is_err());
    }

    #[test]
    fn actual_new_sha_local_check_failure_is_preserved() {
        let fixture = Fixture::new();
        fixture.write("value.txt", "bad whitespace \n");
        let sha = fixture.commit("whitespace negative control");
        let comparison = compare(
            &fixture.0,
            &Args {
                old: sha.clone(),
                new: sha.clone(),
            },
        )
        .unwrap();
        let (checks, _) = select_checks(&fixture.0, &sha, &comparison);
        let result = run_check(&fixture.0, &sha, checks[0].clone());
        assert_eq!(result.state, State::Fail);
        assert_eq!(result.exit, Some(2));
        assert_eq!(result.sha, sha);
        assert!(result.first_error.unwrap().contains("trailing whitespace"));
    }

    #[test]
    fn required_sources_absent_and_unknown_scope_remain_missing() {
        let fixture = Fixture::new();
        fixture.write("xtask/src/review_carry.rs", "// typed fixture metadata\n");
        let sha = fixture.commit("module fixture");
        let comparison = compare(
            &fixture.0,
            &Args {
                old: sha.clone(),
                new: sha.clone(),
            },
        )
        .unwrap();
        let (checks, missing) = select_checks(&fixture.0, &sha, &comparison);
        assert_eq!(checks.len(), 5);
        assert!(!missing.is_empty());
        for check in checks.into_iter().skip(1) {
            assert!(!check.available);
            let result = run_check(&fixture.0, &sha, check);
            assert_eq!(result.state, State::Missing);
            assert_eq!(result.exit, None);
        }
    }

    #[test]
    fn typed_output_counts_failure_names_and_first_error() {
        assert_eq!(
            test_count(
                "test result: ok. 5 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out\n"
            ),
            Some(5)
        );
        assert_eq!(test_count("Ran 17 tests in 0.123s\n"), Some(17));
        assert_eq!(test_count("compile succeeded\n"), None);
        let fixture = Fixture::new();
        let mut output = git(&fixture.0, &["status", "--porcelain"]).unwrap();
        output.stdout =
            b"test typed_negative ... FAILED\ntest result: FAILED. 0 passed; 1 failed; 0 ignored\n"
                .to_vec();
        output.stderr = b"error: typed first error\n".to_vec();
        let check = Check {
            program: "git",
            args: vec!["status".into()],
            count: Count::Tests,
            source: "typed mock metadata only",
            available: true,
        };
        let result = summarize_output(check.clone(), &"a".repeat(40), output, &fixture.0);
        assert_eq!(result.state, State::Fail);
        assert_eq!(result.count, Some(1));
        assert_eq!(result.failures, ["test typed_negative ... FAILED"]);
        assert_eq!(result.first_error, Some("error: typed first error".into()));
        let mut output = git(&fixture.0, &["status", "--porcelain"]).unwrap();
        output.stdout = b"test result: ok. 0 passed; 0 failed; 0 ignored\n".to_vec();
        assert_eq!(
            summarize_output(check, &"a".repeat(40), output, &fixture.0).state,
            State::Missing
        );
    }

    #[test]
    fn diagnostics_do_not_publish_credentials_or_host_paths() {
        assert_eq!(
            redact("error: token=private", Path::new("/host/worktree")),
            "[credential diagnostic redacted]"
        );
        assert_eq!(
            redact(
                "error at /host/worktree/src/file.rs /other/home/file",
                Path::new("/host/worktree")
            ),
            "error at <worktree>/src/file.rs <absolute-path>"
        );
    }

    #[test]
    fn git_overrides_are_removed_from_all_check_commands() {
        let fixture = Fixture::new();
        let check = command(&fixture.0, "git");
        for name in [
            "GIT_DIR",
            "GIT_WORK_TREE",
            "GIT_INDEX_FILE",
            "GIT_COMMON_DIR",
        ] {
            assert!(check
                .get_envs()
                .any(|(key, value)| key == name && value.is_none()));
        }
    }

    #[test]
    fn selector_only_fixture_passes_real_checks_and_stays_clean() {
        let fixture = Fixture::new();
        let cwd = std::env::current_dir().unwrap();
        let source = PathBuf::from(
            git_text(&cwd, &["rev-parse", "--show-toplevel"])
                .unwrap()
                .trim_end_matches('\n'),
        )
        .canonicalize()
        .unwrap();
        // Materialize existing committed source; do not generate Python or a
        // substitute wrapper. The real registry and its full unit suite run.
        git_text(
            &fixture.0,
            &[
                "fetch",
                "--quiet",
                "--no-tags",
                "--depth=1",
                source.to_str().unwrap(),
                "afba87caa8087eace087d5754ec23c9e2c21597d",
            ],
        )
        .unwrap();
        git_text(
            &fixture.0,
            &["checkout", "--quiet", "--detach", "FETCH_HEAD"],
        )
        .unwrap();
        for path in [
            "scripts/test-ci-selection.sh",
            "scripts/ci_selection.py",
            "scripts/test_ci_selection.py",
        ] {
            assert_eq!(
                std::fs::read(fixture.0.join(path)).unwrap(),
                std::fs::read(source.join(path)).unwrap()
            );
        }
        let base = git_text(&fixture.0, &["rev-parse", "HEAD"]).unwrap();
        let path = "scripts/ci_selection_requirements.txt";
        let original = std::fs::read_to_string(fixture.0.join(path)).unwrap();
        let changed = original.replacen("Pinned parser", "Fixture parser", 1);
        assert_ne!(changed, original);
        fixture.write(path, &changed);
        let old = fixture.commit("old selector requirements comment");
        git_text(
            &fixture.0,
            &["checkout", "--quiet", "-b", "selector-new", base.trim()],
        )
        .unwrap();
        fixture.write(path, &changed);
        let new = fixture.commit("new selector requirements comment");
        let args = Args { old, new };
        validate_checkout(&fixture.0, &args.new).unwrap();
        let comparison = compare(&fixture.0, &args).unwrap();
        assert!(comparison.identical());
        assert_eq!(comparison.paths, [path]);
        let (checks, missing) = select_checks(&fixture.0, &args.new, &comparison);
        assert!(missing.is_empty(), "{missing:?}");
        assert_eq!(checks.len(), 2);
        let mut passed = comparison.identical() && missing.is_empty();
        for check in checks {
            assert!(check.available);
            let result = run_check(&fixture.0, &args.new, check);
            print_result(&result);
            assert_eq!(result.sha, args.new);
            assert_eq!(result.exit, Some(0));
            assert_eq!(result.state, State::Pass, "{result:?}");
            if result.check.count == Count::Tests {
                assert!(result.count.is_some_and(|count| count > 0));
            }
            assert!(result.failures.is_empty());
            assert!(result.first_error.is_none());
            passed &= result.state == State::Pass;
        }
        assert!(passed);
        assert!(!fixture.0.join("scripts/__pycache__").exists());
        assert_eq!(
            git_text(
                &fixture.0,
                &["status", "--porcelain=v1", "--untracked-files=all"]
            )
            .unwrap(),
            ""
        );
        validate_checkout(&fixture.0, &args.new).unwrap();
        println!("selector-only local comparison=PASS; fixture tree clean=true");
    }
}
