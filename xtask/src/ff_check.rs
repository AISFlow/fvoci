//! Read-only replacement for manual remote-head, parent, diff digest, patch-id,
//! approval-path and previous-head CI queries. A successful observation is not
//! permission to push, fast-forward, merge, or carry a review.

use std::collections::{BTreeMap, BTreeSet};
use std::ffi::OsString;
use std::io::Write;
use std::path::Path;
use std::process::{Command, ExitCode, Stdio};
use std::time::{SystemTime, UNIX_EPOCH};

pub const HELP: &str = "\
Usage: cargo xtask ff-check --remote <name> --ref refs/heads/<branch>
       --repo <owner/repo> --head <40-hex> --expected-remote-head <40-hex>
       [--require-check <additional GitHub Actions check name> ...]

Queries only; never pushes, fast-forwards, merges, or grants approval.
The candidate must have exactly one parent equal to the expected remote head.
CI is queried only for that expected previous head. The five repository CI gates
are always required; --require-check adds requirements without replacing them.
Missing data, unfinished CI and failed CI produce a nonzero exit status.
";

const GATES: [&str; 5] = [
    "rust-ci-gate",
    "web-ci-gate",
    "documents-ci-gate",
    "collab-engine-ci-gate",
    "install-ci-gate",
];

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
struct Sha(String);

impl Sha {
    fn parse(value: &str) -> Result<Self, String> {
        if value.len() == 40 && value.bytes().all(|byte| byte.is_ascii_hexdigit()) {
            Ok(Self(value.to_ascii_lowercase()))
        } else {
            Err("SHA must contain exactly 40 hexadecimal characters".into())
        }
    }
}

#[derive(Debug, PartialEq, Eq)]
pub struct Options {
    remote: String,
    reference: String,
    repository: String,
    head: Sha,
    expected: Sha,
    required: BTreeSet<String>,
}

#[derive(Debug, PartialEq, Eq)]
pub enum Args {
    Help,
    Check(Options),
}

fn safe_text(value: &str) -> bool {
    !value.is_empty() && !value.chars().any(char::is_control) && !value.contains('\\')
}

fn identifier(value: &str) -> bool {
    !value.is_empty()
        && !value.starts_with('-')
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || b"-_.".contains(&byte))
}

pub fn parse_args(args: impl Iterator<Item = OsString>) -> Result<Args, String> {
    let mut args = args.peekable();
    if matches!(
        args.peek().and_then(|arg| arg.to_str()),
        Some("-h" | "--help")
    ) {
        args.next();
        return if args.next().is_none() {
            Ok(Args::Help)
        } else {
            Err("unexpected argument after ff-check help".into())
        };
    }
    let mut values = BTreeMap::new();
    let mut required: BTreeSet<String> = GATES.iter().map(|name| (*name).into()).collect();
    while let Some(flag) = args.next() {
        let flag = flag.to_str().ok_or("ff-check arguments must be Unicode")?;
        if !matches!(
            flag,
            "--remote"
                | "--ref"
                | "--repo"
                | "--head"
                | "--expected-remote-head"
                | "--require-check"
        ) {
            return Err("unknown ff-check argument (see --help)".into());
        }
        let value = args
            .next()
            .ok_or_else(|| format!("missing value for {flag}"))?;
        let value = value
            .into_string()
            .map_err(|_| "ff-check arguments must be Unicode")?;
        if flag == "--require-check" {
            if !safe_text(&value) || value.starts_with('-') {
                return Err("invalid required check name".into());
            }
            required.insert(value);
        } else if values.insert(flag.to_owned(), value).is_some() {
            return Err(format!("duplicate argument {flag}"));
        }
    }
    let mut take = |name: &str| values.remove(name).ok_or_else(|| format!("missing {name}"));
    let remote = take("--remote")?;
    let reference = take("--ref")?;
    let repository = take("--repo")?;
    let head = Sha::parse(&take("--head")?)?;
    let expected = Sha::parse(&take("--expected-remote-head")?)?;
    if !identifier(&remote) {
        return Err("--remote must be a configured remote name, not a URL or path".into());
    }
    if !reference.starts_with("refs/heads/")
        || !reference
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || b"/-_.".contains(&byte))
    {
        return Err("--ref must be an explicit refs/heads/<branch> reference".into());
    }
    let repository_parts: Vec<_> = repository.split('/').collect();
    if repository_parts.len() != 2 || !repository_parts.iter().all(|part| identifier(part)) {
        return Err("--repo must be owner/repo without a host or URL".into());
    }
    Ok(Args::Check(Options {
        remote,
        reference,
        repository,
        head,
        expected,
        required,
    }))
}

// Do not echo child stderr: it may include remote credentials or host paths.
// Preserve the tool, operation and exact exit status instead.
fn execute(
    directory: &Path,
    tool: &str,
    args: &[&str],
    input: Option<&[u8]>,
    operation: &str,
) -> Result<Vec<u8>, String> {
    let mut child = Command::new(tool)
        .args(args)
        .current_dir(directory)
        .env("GIT_TERMINAL_PROMPT", "0")
        .env("GIT_NO_REPLACE_OBJECTS", "1")
        .stdin(if input.is_some() {
            Stdio::piped()
        } else {
            Stdio::null()
        })
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|error| format!("MISSING {tool}: {operation}; spawn={:?}", error.kind()))?;
    if let Some(input) = input {
        let result = child.stdin.take().expect("piped stdin").write_all(input);
        if let Err(error) = result {
            let _ = child.kill();
            let _ = child.wait();
            return Err(format!(
                "FAIL {tool}: {operation}; stdin={:?}",
                error.kind()
            ));
        }
    }
    let output = child
        .wait_with_output()
        .map_err(|error| format!("FAIL {tool}: {operation}; wait={:?}", error.kind()))?;
    if !output.status.success() {
        return Err(format!("FAIL {tool}: {operation}; exit={}", output.status));
    }
    Ok(output.stdout)
}

fn text(bytes: Vec<u8>, operation: &str) -> Result<String, String> {
    String::from_utf8(bytes).map_err(|_| format!("MISSING {operation}: non-Unicode output"))
}

fn git(directory: &Path, args: &[&str], operation: &str) -> Result<Vec<u8>, String> {
    execute(directory, "git", args, None, operation)
}

fn remote_head(directory: &Path, options: &Options) -> Result<Sha, String> {
    git(
        directory,
        &["check-ref-format", &options.reference],
        "validate remote ref",
    )?;
    // Require a configured remote, so ls-remote cannot interpret a path/URL operand.
    git(
        directory,
        &["remote", "get-url", &options.remote],
        "resolve configured remote",
    )?;
    let result = text(
        git(
            directory,
            &[
                "ls-remote",
                "--exit-code",
                "--refs",
                &options.remote,
                &options.reference,
            ],
            "read remote head",
        )?,
        "remote head",
    )?;
    let lines: Vec<_> = result.lines().collect();
    if lines.len() != 1 {
        return Err("MISSING remote head: expected exactly one ref".into());
    }
    let (sha, reference) = lines[0]
        .split_once('\t')
        .ok_or("MISSING remote head: malformed row")?;
    if reference != options.reference {
        return Err("FAIL remote head: returned a different ref".into());
    }
    Sha::parse(sha)
}

fn approval_path(path: &str) -> bool {
    matches!(
        path,
        "AGENTS.md" | "scripts/ci_selection.py" | "docs/testing-turso.md"
    ) || path.starts_with(".agents/")
        || path.starts_with(".github/workflows/")
        || path.starts_with("xtask/")
}

#[derive(Debug)]
struct GitEvidence {
    parent: Sha,
    digest: String,
    patch_id: String,
    files: Vec<String>,
}

fn candidate(directory: &Path, head: &Sha) -> Result<GitEvidence, String> {
    let object_type = text(
        git(
            directory,
            &["cat-file", "-t", &head.0],
            "candidate commit exists",
        )?,
        "object type",
    )?;
    if object_type.trim() != "commit" {
        return Err("FAIL candidate: SHA is not a commit".into());
    }
    let parents = text(
        git(
            directory,
            &["show", "-s", "--format=%P", &head.0],
            "candidate parents",
        )?,
        "candidate parents",
    )?;
    let parents: Vec<_> = parents.split_whitespace().collect();
    if parents.len() != 1 {
        return Err(format!(
            "FAIL candidate: requires exactly one parent; parent_count={}",
            parents.len()
        ));
    }
    let parent = Sha::parse(parents[0])?;
    // A shallow boundary must not be treated as an ordinary parent/diff.
    let parent_type = text(
        git(
            directory,
            &["cat-file", "-t", &parent.0],
            "parent commit exists",
        )?,
        "parent type",
    )?;
    if parent_type.trim() != "commit" {
        return Err("MISSING candidate parent commit".into());
    }
    let patch = git(
        directory,
        &["diff", "--binary", "--full-index", &parent.0, &head.0],
        "canonical binary full-index diff",
    )?;
    let digest = text(
        execute(directory, "sha256sum", &[], Some(&patch), "diff SHA-256")?,
        "diff SHA-256",
    )?;
    let digest = digest
        .split_whitespace()
        .next()
        .ok_or("MISSING diff SHA-256")?;
    if digest.len() != 64 || !digest.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return Err("MISSING diff SHA-256: malformed digest".into());
    }
    let patch_id = text(
        execute(
            directory,
            "git",
            &["patch-id", "--stable"],
            Some(&patch),
            "stable patch-id",
        )?,
        "stable patch-id",
    )?;
    let rows: Vec<_> = patch_id.lines().collect();
    if rows.len() != 1 {
        return Err("MISSING stable patch-id: empty or ambiguous diff".into());
    }
    let columns: Vec<_> = rows[0].split_whitespace().collect();
    if columns.len() != 2 {
        return Err("MISSING stable patch-id: malformed row".into());
    }
    let patch_id = Sha::parse(columns[0])?.0;
    // --no-renames lists both old and new paths, including moves out of approval paths.
    let files = git(
        directory,
        &[
            "diff",
            "--name-only",
            "-z",
            "--no-renames",
            &parent.0,
            &head.0,
        ],
        "changed paths",
    )?;
    let mut paths = Vec::new();
    for file in files
        .split(|byte| *byte == 0)
        .filter(|file| !file.is_empty())
    {
        let file =
            std::str::from_utf8(file).map_err(|_| "MISSING changed path: non-Unicode name")?;
        if !safe_text(file) {
            return Err("MISSING changed path: unsafe display characters".into());
        }
        paths.push(file.to_owned());
    }
    Ok(GitEvidence {
        parent,
        digest: digest.to_owned(),
        patch_id,
        files: paths,
    })
}

// JSON decoding is delegated to gh's supported jq formatter. This module only
// consumes its fixed field projection, rejects escaped/control data, and checks
// pagination counts. API errors never become an empty successful observation.
const RUNS_JQ: &str = r#"(["count", .total_count] | @tsv), (.workflow_runs[] | ["run", .id, .workflow_id, .run_attempt, .head_sha, .check_suite_id, .event, .head_branch, .status, (.conclusion // "-")] | @tsv)"#;
const CHECKS_JQ: &str = r#"(["count", .total_count] | @tsv), (.check_runs[] | ["check", .id, .head_sha, .check_suite.id, .app.slug, .name, .status, (.conclusion // "-")] | @tsv)"#;
const STATUS_JQ: &str = r#"(["sha", .sha] | @tsv), (["count", .total_count] | @tsv), (.statuses[] | ["status", .id, .context, .state] | @tsv)"#;

fn api(directory: &Path, endpoint: &str, projection: &str) -> Result<String, String> {
    text(
        execute(
            directory,
            "gh",
            &[
                "api",
                "--hostname",
                "github.com",
                "--method",
                "GET",
                "--paginate",
                endpoint,
                "--jq",
                projection,
            ],
            None,
            "GitHub read-only metadata query (stderr withheld)",
        )?,
        "GitHub metadata",
    )
}

fn rows<'a>(input: &'a str, tag: &str, width: usize) -> Result<Vec<Vec<&'a str>>, String> {
    let mut expected = None;
    let mut result = Vec::new();
    let mut ids = BTreeSet::new();
    for row in input.lines() {
        let fields: Vec<_> = row.split('\t').collect();
        if fields.len() == 2 && fields[0] == "count" {
            let count = number(fields[1])? as usize;
            if expected.is_some_and(|previous| previous != count) {
                return Err("MISSING CI: pagination changed during observation".into());
            }
            expected = Some(count);
        } else if fields.len() == width
            && fields[0] == tag
            && fields.iter().all(|field| safe_text(field))
        {
            if !ids.insert(fields[1]) {
                return Err("MISSING CI: duplicate record during pagination".into());
            }
            result.push(fields);
        } else {
            return Err("MISSING CI: malformed projected metadata".into());
        }
    }
    if expected != Some(result.len()) {
        return Err("MISSING CI: absent or incomplete paginated metadata".into());
    }
    Ok(result)
}

fn number(value: &str) -> Result<u64, String> {
    value
        .parse()
        .map_err(|_| "MISSING CI: invalid numeric field".into())
}

#[derive(Clone, Debug)]
struct Workflow {
    id: u64,
    workflow: u64,
    attempt: u64,
    sha: Sha,
    suite: u64,
    event: String,
    branch: String,
    status: String,
    conclusion: String,
}

#[derive(Clone, Debug)]
struct Check {
    id: u64,
    sha: Sha,
    suite: u64,
    app: String,
    name: String,
    status: String,
    conclusion: String,
}

#[derive(Debug)]
struct Status {
    id: u64,
    name: String,
    state: String,
}

#[derive(Default)]
struct Ci {
    workflows: Vec<Workflow>,
    checks: Vec<Check>,
    statuses: Vec<Status>,
}

fn ci_metadata(directory: &Path, options: &Options) -> Result<Ci, String> {
    git(
        directory,
        &["check-ref-format", &options.reference],
        "validate GitHub ref before API query",
    )?;
    let reference = options
        .reference
        .strip_prefix("refs/")
        .expect("validated ref");
    let api_ref = api(
        directory,
        &format!("repos/{}/git/ref/{reference}", options.repository),
        "[.ref, .object.sha] | @tsv",
    )?;
    let fields: Vec<_> = api_ref.trim_end().split('\t').collect();
    if fields.len() != 2
        || fields[0] != options.reference
        || Sha::parse(fields[1])? != options.expected
    {
        return Err("FAIL GitHub repo/ref: head differs from expected remote head".into());
    }
    let runs = api(
        directory,
        &format!(
            "repos/{}/actions/runs?head_sha={}&per_page=100",
            options.repository, options.expected.0
        ),
        RUNS_JQ,
    )?;
    let checks = api(
        directory,
        &format!(
            "repos/{}/commits/{}/check-runs?filter=all&per_page=100",
            options.repository, options.expected.0
        ),
        CHECKS_JQ,
    )?;
    let statuses = api(
        directory,
        &format!(
            "repos/{}/commits/{}/status?per_page=100",
            options.repository, options.expected.0
        ),
        STATUS_JQ,
    )?;
    parse_ci(&runs, &checks, &statuses, &options.expected)
}

fn parse_ci(runs: &str, checks: &str, statuses: &str, expected: &Sha) -> Result<Ci, String> {
    let mut ci = Ci::default();
    for fields in rows(runs, "run", 10)? {
        ci.workflows.push(Workflow {
            id: number(fields[1])?,
            workflow: number(fields[2])?,
            attempt: number(fields[3])?,
            sha: Sha::parse(fields[4])?,
            suite: number(fields[5])?,
            event: fields[6].into(),
            branch: fields[7].into(),
            status: fields[8].into(),
            conclusion: fields[9].into(),
        });
    }
    for fields in rows(checks, "check", 8)? {
        ci.checks.push(Check {
            id: number(fields[1])?,
            sha: Sha::parse(fields[2])?,
            suite: number(fields[3])?,
            app: fields[4].into(),
            name: fields[5].into(),
            status: fields[6].into(),
            conclusion: fields[7].into(),
        });
    }
    let mut status_rows = String::new();
    let mut sha_seen = false;
    for row in statuses.lines() {
        if let Some(sha) = row.strip_prefix("sha\t") {
            if Sha::parse(sha)? != *expected {
                return Err("FAIL CI: commit statuses belong to a different SHA".into());
            }
            sha_seen = true;
        } else {
            status_rows.push_str(row);
            status_rows.push('\n');
        }
    }
    if !sha_seen {
        return Err("MISSING CI: commit status SHA".into());
    }
    for fields in rows(&status_rows, "status", 4)? {
        ci.statuses.push(Status {
            id: number(fields[1])?,
            name: fields[2].into(),
            state: fields[3].into(),
        });
    }
    if ci.workflows.iter().any(|run| run.sha != *expected)
        || ci.checks.iter().any(|check| check.sha != *expected)
    {
        return Err("FAIL CI: workflow/check metadata belongs to a different SHA".into());
    }
    Ok(ci)
}

fn state(status: &str, conclusion: &str) -> &'static str {
    match (status, conclusion) {
        ("completed", "success") => "PASS",
        ("completed", "skipped" | "neutral") => "NOTRUN",
        ("completed", "-") => "MISSING",
        ("completed", _) => "FAIL",
        _ => "UNFINISHED",
    }
}

fn evaluate_ci(
    ci: &Ci,
    expected: &Sha,
    required: &BTreeSet<String>,
) -> Result<Vec<String>, String> {
    if ci.workflows.iter().any(|run| run.sha != *expected)
        || ci.checks.iter().any(|check| check.sha != *expected)
    {
        return Err("FAIL CI: metadata belongs to a different SHA".into());
    }
    let mut latest_runs = BTreeMap::new();
    for run in &ci.workflows {
        let key = (run.workflow, &run.event, &run.branch);
        let previous: &mut &Workflow = latest_runs.entry(key).or_insert(run);
        if (run.id, run.attempt) > (previous.id, previous.attempt) {
            *previous = run;
        }
    }
    if latest_runs.is_empty() {
        return Err("MISSING CI: no workflow runs on expected previous head".into());
    }
    let suites: BTreeSet<_> = latest_runs.values().map(|run| run.suite).collect();
    let mut lines = Vec::new();
    let mut blocked = false;
    for run in latest_runs.values() {
        let result = state(&run.status, &run.conclusion);
        lines.push(format!(
            "ci_workflow={} id={} attempt={} status={} conclusion={} result={result}",
            run.workflow, run.id, run.attempt, run.status, run.conclusion
        ));
        blocked |= result != "PASS";
    }
    let mut latest_checks = BTreeMap::new();
    for check in &ci.checks {
        if check.app == "github-actions" && !suites.contains(&check.suite) {
            continue;
        }
        let key = (&check.app, check.suite, &check.name);
        let previous: &mut &Check = latest_checks.entry(key).or_insert(check);
        if check.id > previous.id {
            *previous = check;
        }
    }
    for check in latest_checks.values() {
        let result = state(&check.status, &check.conclusion);
        let required_check = check.app == "github-actions" && required.contains(&check.name);
        lines.push(format!("ci_check={:?} id={} suite={} app={} status={} conclusion={} required={required_check} result={result}", check.name, check.id, check.suite, check.app, check.status, check.conclusion));
        // Optional skipped/neutral jobs are completed, but never count as PASS.
        blocked |= matches!(result, "FAIL" | "MISSING" | "UNFINISHED")
            || (required_check && result != "PASS");
    }
    for name in required {
        if !latest_checks
            .values()
            .any(|check| check.app == "github-actions" && check.name == *name)
        {
            lines.push(format!("ci_required={name:?} result=MISSING"));
            blocked = true;
        }
    }
    let mut latest_statuses = BTreeMap::new();
    for status in &ci.statuses {
        let previous: &mut &Status = latest_statuses.entry(&status.name).or_insert(status);
        if status.id > previous.id {
            *previous = status;
        }
    }
    for status in latest_statuses.values() {
        let result = match status.state.as_str() {
            "success" => "PASS",
            "pending" => "UNFINISHED",
            _ => "FAIL",
        };
        lines.push(format!(
            "ci_commit_status={:?} id={} state={} result={result}",
            status.name, status.id, status.state
        ));
        blocked |= result != "PASS";
    }
    lines.push(format!(
        "ci_previous_head={} result={}",
        expected.0,
        if blocked { "FAIL" } else { "PASS" }
    ));
    // Preserve every failure row for the CLI rather than dropping partial evidence.
    Ok(lines)
}

pub fn run(args: Args) -> ExitCode {
    let Args::Check(options) = args else {
        print!("{HELP}");
        return ExitCode::SUCCESS;
    };
    let directory = Path::new(".");
    let timestamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|time| time.as_secs())
        .unwrap_or_default();
    println!(
        "observed_at_unix={timestamp} mode=read-only self-check-is-not-independent-acceptance"
    );
    println!(
        "remote={} ref={} repo={} candidate_head={} expected_remote_head={}",
        options.remote, options.reference, options.repository, options.head.0, options.expected.0
    );
    let mut failed = false;
    let mut first_remote = None;
    match remote_head(directory, &options) {
        Ok(head) => {
            println!(
                "remote_head={} matches_expected={}",
                head.0,
                head == options.expected
            );
            failed |= head != options.expected;
            first_remote = Some(head);
        }
        Err(error) => {
            println!("remote_head result={error}");
            failed = true;
        }
    }
    match candidate(directory, &options.head) {
        Ok(evidence) => {
            println!(
                "candidate_parent={} matches_expected_remote={}",
                evidence.parent.0,
                evidence.parent == options.expected
            );
            failed |= evidence.parent != options.expected;
            println!(
                "patch_sha256={} stable_patch_id={}",
                evidence.digest, evidence.patch_id
            );
            let approvals: Vec<_> = evidence
                .files
                .iter()
                .filter(|file| approval_path(file))
                .collect();
            println!(
                "approval_paths_present={} changed_files={}",
                !approvals.is_empty(),
                evidence.files.len()
            );
            for file in &evidence.files {
                println!("changed_file={file:?}");
            }
            for file in approvals {
                println!("approval_path={file:?}");
            }
            println!(
                "approval=NOT_GRANTED Rust-review-and-final-SHA-owner-approval-remain-required"
            );
        }
        Err(error) => {
            println!("candidate result={error}");
            failed = true;
        }
    }
    match ci_metadata(directory, &options)
        .and_then(|ci| evaluate_ci(&ci, &options.expected, &options.required))
    {
        Ok(lines) => {
            failed |= lines
                .last()
                .is_none_or(|line| !line.ends_with("result=PASS"));
            for line in lines {
                println!("{line}");
            }
        }
        Err(error) => {
            println!("ci_previous_head={} result={error}", options.expected.0);
            failed = true;
        }
    }
    // Detect movement during observation; this is still an observation, not a lock.
    match remote_head(directory, &options) {
        Ok(head) => {
            let stable = first_remote.as_ref() == Some(&head) && head == options.expected;
            println!(
                "remote_head_recheck={} unchanged_and_expected={stable}",
                head.0
            );
            failed |= !stable;
        }
        Err(error) => {
            println!("remote_head_recheck result={error}");
            failed = true;
        }
    }
    println!(
        "ff_check_observation={} authorization=NOT_GRANTED",
        if failed { "FAIL" } else { "PASS" }
    );
    if failed {
        ExitCode::FAILURE
    } else {
        ExitCode::SUCCESS
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::path::PathBuf;
    use std::sync::atomic::{AtomicUsize, Ordering};

    const A: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
    const B: &str = "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb";

    fn options(head: &str, parent: &str) -> Options {
        let Args::Check(options) = parse_args(
            [
                "--remote",
                "origin",
                "--ref",
                "refs/heads/fixture",
                "--repo",
                "AISFlow/fvoci",
                "--head",
                head,
                "--expected-remote-head",
                parent,
            ]
            .into_iter()
            .map(OsString::from),
        )
        .unwrap() else {
            panic!("check options");
        };
        options
    }

    fn good_ci(sha: &str) -> Ci {
        let sha = Sha::parse(sha).unwrap();
        Ci {
            workflows: vec![Workflow {
                id: 10,
                workflow: 1,
                attempt: 1,
                sha: sha.clone(),
                suite: 100,
                event: "push".into(),
                branch: "fixture".into(),
                status: "completed".into(),
                conclusion: "success".into(),
            }],
            checks: GATES
                .iter()
                .enumerate()
                .map(|(index, name)| Check {
                    id: index as u64 + 1000,
                    sha: sha.clone(),
                    suite: 100,
                    app: "github-actions".into(),
                    name: (*name).into(),
                    status: "completed".into(),
                    conclusion: "success".into(),
                })
                .collect(),
            statuses: Vec::new(),
        }
    }

    fn passed(ci: &Ci, sha: &str) -> bool {
        evaluate_ci(ci, &Sha::parse(sha).unwrap(), &options(B, A).required)
            .is_ok_and(|lines| lines.last().unwrap().ends_with("result=PASS"))
    }

    #[test]
    fn sha_accepts_full_hex_and_rejects_short_invalid_or_revision_syntax() {
        assert_eq!(Sha::parse(&A.to_uppercase()).unwrap().0, A);
        for invalid in [
            "",
            "93534e28",
            "HEAD",
            &"g".repeat(40),
            &"a".repeat(39),
            &"a".repeat(41),
            &format!("{A}^"),
            &format!(" {A}"),
        ] {
            assert!(Sha::parse(invalid).is_err());
        }
    }

    #[test]
    fn parser_requires_explicit_query_arguments_and_cannot_weaken_default_gates() {
        assert_eq!(options(B, A).required.len(), 5);
        assert_eq!(
            parse_args(["--help"].into_iter().map(OsString::from)).unwrap(),
            Args::Help
        );
        for input in [
            vec![],
            vec!["--head", B],
            vec!["--help", "extra"],
            vec!["--push"],
            vec!["--head", A, "--head", B],
        ] {
            assert!(parse_args(input.into_iter().map(OsString::from)).is_err());
        }
        let mut args = vec![
            "--remote",
            "origin",
            "--ref",
            "refs/heads/fixture",
            "--repo",
            "AISFlow/fvoci",
            "--head",
            B,
            "--expected-remote-head",
            A,
        ];
        args.extend(["--require-check", "additional-gate"]);
        let Args::Check(parsed) = parse_args(args.into_iter().map(OsString::from)).unwrap() else {
            panic!("check options");
        };
        assert_eq!(parsed.required.len(), 6);
    }

    #[test]
    fn parser_rejects_paths_urls_injection_and_invalid_sha_in_both_positions() {
        let valid = [
            "--remote",
            "origin",
            "--ref",
            "refs/heads/fixture",
            "--repo",
            "AISFlow/fvoci",
            "--head",
            B,
            "--expected-remote-head",
            A,
        ];
        for (index, value) in [
            (1, "/private/path"),
            (1, "https://example.invalid/repo"),
            (1, "--upload-pack=bad"),
            (3, "main"),
            (3, "refs/heads/x?token=bad"),
            (5, "https://github.com/AISFlow/fvoci"),
            (7, "short"),
            (9, "short"),
        ] {
            let mut input = valid;
            input[index] = value;
            assert!(parse_args(input.into_iter().map(OsString::from)).is_err());
        }
    }

    #[cfg(unix)]
    #[test]
    fn parser_rejects_non_unicode_without_echoing_the_input() {
        use std::os::unix::ffi::OsStringExt;
        assert_eq!(
            parse_args([OsString::from_vec(vec![0xff])].into_iter()).unwrap_err(),
            "ff-check arguments must be Unicode"
        );
    }

    #[test]
    fn approval_paths_are_component_bounded_and_include_current_user_policy() {
        for path in [
            "AGENTS.md",
            ".agents/rules.md",
            ".github/workflows/rust.yml",
            "scripts/ci_selection.py",
            "xtask/src/ff_check.rs",
            "docs/testing-turso.md",
        ] {
            assert!(approval_path(path));
        }
        for path in [
            "AGENTS.md.old",
            ".agents-old/rules.md",
            ".github/workflows-old/rust.yml",
            "xtask-old/file",
            "docs/other.md",
        ] {
            assert!(!approval_path(path));
        }
    }

    #[test]
    fn ci_accepts_exact_sha_and_rejects_wrong_sha_or_missing_gate() {
        let mut ci = good_ci(A);
        assert!(passed(&ci, A));
        assert!(!passed(&ci, B));
        ci.checks.pop();
        assert!(!passed(&ci, A));
        assert!(!passed(&Ci::default(), A));
    }

    #[test]
    fn ci_rejects_latest_failed_unfinished_cancelled_skipped_and_null_required_check() {
        for (status, conclusion) in [
            ("completed", "failure"),
            ("in_progress", "-"),
            ("queued", "-"),
            ("completed", "cancelled"),
            ("completed", "skipped"),
            ("completed", "neutral"),
            ("completed", "-"),
        ] {
            let mut ci = good_ci(A);
            let mut latest = ci.checks[0].clone();
            latest.id += 10000;
            latest.status = status.into();
            latest.conclusion = conclusion.into();
            ci.checks.push(latest);
            assert!(!passed(&ci, A), "{status}/{conclusion}");
        }
    }

    #[test]
    fn ci_uses_latest_workflow_and_attempt_and_never_borrows_superseded_checks() {
        let mut ci = good_ci(A);
        let mut newest = ci.workflows[0].clone();
        newest.id += 1;
        newest.suite += 1;
        ci.workflows.push(newest);
        assert!(!passed(&ci, A)); // New run succeeded, but its required checks are missing.
        ci.workflows[1].suite = 100;
        ci.workflows[1].attempt = 2;
        ci.workflows[1].status = "queued".into();
        ci.workflows[1].conclusion = "-".into();
        assert!(!passed(&ci, A)); // Old checks cannot mask a queued rerun.
        ci.workflows[1].status = "completed".into();
        ci.workflows[1].conclusion = "success".into();
        ci.workflows[0].conclusion = "cancelled".into();
        assert!(passed(&ci, A)); // Only the newer completed run is used.
    }

    #[test]
    fn ci_ignores_old_check_attempt_and_other_app_cannot_supply_required_gate() {
        let mut ci = good_ci(A);
        let mut old = ci.checks[0].clone();
        old.id = 1;
        old.conclusion = "failure".into();
        ci.checks.push(old);
        assert!(passed(&ci, A));
        ci.checks[0].app = "another-app".into();
        assert!(!passed(&ci, A));
    }

    #[test]
    fn ci_optional_skip_is_not_pass_and_latest_commit_status_failure_blocks() {
        let mut ci = good_ci(A);
        let mut optional = ci.checks[0].clone();
        optional.name = "optional-witness".into();
        optional.conclusion = "skipped".into();
        ci.checks.push(optional);
        assert!(passed(&ci, A));
        assert!(
            evaluate_ci(&ci, &Sha::parse(A).unwrap(), &options(B, A).required)
                .unwrap()
                .iter()
                .any(|line| line.contains("result=NOTRUN"))
        );
        ci.statuses.push(Status {
            id: 1,
            name: "external".into(),
            state: "success".into(),
        });
        ci.statuses.push(Status {
            id: 2,
            name: "external".into(),
            state: "pending".into(),
        });
        assert!(!passed(&ci, A));
    }

    #[test]
    fn projected_metadata_checks_counts_pagination_sha_and_rejects_escaped_names() {
        let runs =
            format!("count\t1\nrun\t10\t1\t1\t{A}\t100\tpush\tfixture\tcompleted\tsuccess\n");
        let checks = format!(
            "count\t1\ncheck\t1000\t{A}\t100\tgithub-actions\trust-ci-gate\tcompleted\tsuccess\n"
        );
        let statuses = format!("sha\t{A}\ncount\t0\n");
        assert!(parse_ci(&runs, &checks, &statuses, &Sha::parse(A).unwrap()).is_ok());
        assert!(parse_ci(&runs, &checks, &statuses, &Sha::parse(B).unwrap()).is_err());
        assert!(rows("", "check", 8).is_err());
        assert!(rows("count\t1\n", "check", 8).is_err());
        assert!(rows("count\t0\ncount\t1\n", "check", 8).is_err());
        let duplicate = format!(
            "count\t2\n{}\n{}\n",
            checks.lines().nth(1).unwrap(),
            checks.lines().nth(1).unwrap()
        );
        assert!(rows(&duplicate, "check", 8).is_err());
        assert!(parse_ci(
            &runs,
            &checks.replace("rust-ci-gate", "bad\\tname"),
            &statuses,
            &Sha::parse(A).unwrap()
        )
        .is_err());
        let pages = format!(
            "count\t2\n{}count\t2\n{}",
            checks.lines().nth(1).unwrap().to_owned() + "\n",
            checks.lines().nth(1).unwrap().replace("1000", "1001") + "\n"
        );
        assert_eq!(rows(&pages, "check", 8).unwrap().len(), 2);
    }

    struct Fixture(PathBuf);

    impl Fixture {
        fn new() -> Self {
            static NEXT: AtomicUsize = AtomicUsize::new(0);
            // Cargo runs package tests from xtask/, while the standalone static
            // test binary is listed from the worktree root. Keep both modes in
            // the same task-owned, root-ignored evidence directory.
            let cwd = std::env::current_dir().unwrap();
            let root = text(
                git(
                    &cwd,
                    &["rev-parse", "--show-toplevel"],
                    "fixture worktree root",
                )
                .unwrap(),
                "fixture worktree root",
            )
            .unwrap();
            let path = PathBuf::from(root.trim_end())
                .join("target/xtask-ff-check-1-evidence")
                .join(format!(
                    "git-fixture-{}-{}",
                    std::process::id(),
                    NEXT.fetch_add(1, Ordering::Relaxed)
                ));
            fs::create_dir_all(path.parent().unwrap()).unwrap();
            fs::create_dir(&path).unwrap();
            let fixture = Self(path);
            fixture.git(&["init", "--quiet", "--initial-branch=fixture"]);
            fixture.git(&["config", "user.name", "FVOCI fixture"]);
            fixture.git(&["config", "user.email", "fixture@example.invalid"]);
            fixture.git(&["config", "commit.gpgsign", "false"]);
            fixture.git(&["remote", "add", "origin", "."]);
            fixture
        }

        fn git(&self, args: &[&str]) -> String {
            text(
                git(&self.0, args, "local fixture git command").unwrap(),
                "fixture output",
            )
            .unwrap()
        }

        fn commit(&self, filename: &str, bytes: &[u8]) -> Sha {
            let file = self.0.join(filename);
            fs::create_dir_all(file.parent().unwrap()).unwrap();
            fs::write(file, bytes).unwrap();
            self.git(&["add", "--", filename]);
            self.git(&["commit", "--quiet", "-m", "typed ff-check fixture"]);
            Sha::parse(self.git(&["rev-parse", "HEAD"]).trim()).unwrap()
        }
    }

    impl Drop for Fixture {
        fn drop(&mut self) {
            fs::remove_dir_all(&self.0).unwrap();
        }
    }

    #[test]
    fn actual_git_fixture_matches_manual_binary_digest_patch_id_parent_and_remote() {
        let fixture = Fixture::new();
        let parent = fixture.commit("ordinary.txt", b"parent\n");
        fixture.git(&["branch", "remote-target", &parent.0]);
        let head = fixture.commit("xtask/data.bin", &[0, 1, 255, 0, 128]);
        let mut options = options(&head.0, &parent.0);
        options.reference = "refs/heads/remote-target".into();
        assert_eq!(remote_head(&fixture.0, &options).unwrap(), parent);
        let evidence = candidate(&fixture.0, &head).unwrap();
        assert_eq!(evidence.parent, parent);
        assert_eq!(evidence.files, ["xtask/data.bin"]);
        assert!(approval_path(&evidence.files[0]));
        let patch = git(
            &fixture.0,
            &["diff", "--binary", "--full-index", &parent.0, &head.0],
            "manual fixture diff",
        )
        .unwrap();
        assert!(String::from_utf8_lossy(&patch).contains("GIT binary patch"));
        let manual_digest =
            execute(&fixture.0, "sha256sum", &[], Some(&patch), "manual digest").unwrap();
        assert_eq!(
            evidence.digest,
            String::from_utf8(manual_digest)
                .unwrap()
                .split_whitespace()
                .next()
                .unwrap()
        );
        let manual_id = execute(
            &fixture.0,
            "git",
            &["patch-id", "--stable"],
            Some(&patch),
            "manual patch-id",
        )
        .unwrap();
        assert_eq!(
            evidence.patch_id,
            String::from_utf8(manual_id)
                .unwrap()
                .split_whitespace()
                .next()
                .unwrap()
        );
        assert!(passed(&good_ci(&evidence.parent.0), &parent.0));
        assert!(!passed(&good_ci(&head.0), &parent.0)); // Expected wrong-SHA negative control.
        fixture.git(&["update-ref", &options.reference, &head.0]);
        assert_ne!(remote_head(&fixture.0, &options).unwrap(), options.expected);
    }

    #[test]
    fn actual_git_rejects_missing_commit_ref_root_merge_and_empty_patch() {
        let fixture = Fixture::new();
        let root = fixture.commit("root.txt", b"root\n");
        assert!(candidate(&fixture.0, &root)
            .unwrap_err()
            .contains("parent_count=0"));
        assert!(candidate(&fixture.0, &Sha::parse(A).unwrap()).is_err());
        let mut missing_ref = options(B, A);
        missing_ref.reference = "refs/heads/missing".into();
        assert!(remote_head(&fixture.0, &missing_ref).is_err());
        missing_ref.reference = "refs/heads/../unsafe".into();
        assert!(remote_head(&fixture.0, &missing_ref).is_err());
        fixture.git(&["commit", "--quiet", "--allow-empty", "-m", "empty fixture"]);
        let empty = Sha::parse(fixture.git(&["rev-parse", "HEAD"]).trim()).unwrap();
        assert!(candidate(&fixture.0, &empty)
            .unwrap_err()
            .contains("MISSING stable patch-id"));
        let tree = fixture.git(&["rev-parse", &format!("{}^{{tree}}", root.0)]);
        let merge = fixture.git(&[
            "commit-tree",
            tree.trim(),
            "-p",
            &root.0,
            "-p",
            &empty.0,
            "-m",
            "merge fixture",
        ]);
        assert!(candidate(&fixture.0, &Sha::parse(merge.trim()).unwrap())
            .unwrap_err()
            .contains("parent_count=2"));
        assert!(candidate(&fixture.0, &Sha::parse(tree.trim()).unwrap())
            .unwrap_err()
            .contains("not a commit"));
    }

    #[test]
    fn actual_git_rename_out_of_approval_path_preserves_old_path_and_parent_mismatch() {
        let fixture = Fixture::new();
        let parent = fixture.commit("xtask/old.txt", b"approval fixture\n");
        fixture.git(&["mv", "xtask/old.txt", "ordinary.txt"]);
        fixture.git(&["commit", "--quiet", "-m", "move fixture"]);
        let head = Sha::parse(fixture.git(&["rev-parse", "HEAD"]).trim()).unwrap();
        let evidence = candidate(&fixture.0, &head).unwrap();
        assert_eq!(evidence.parent, parent);
        assert!(evidence.files.iter().any(|path| path == "xtask/old.txt"));
        assert!(evidence.files.iter().any(|path| approval_path(path)));
        assert_ne!(evidence.parent, Sha::parse(A).unwrap());
    }

    #[test]
    fn unavailable_cli_is_missing_and_child_error_never_leaks_arguments_or_paths() {
        let fixture = Fixture::new();
        let error = execute(
            &fixture.0,
            "fvoci-ff-check-nonexistent-tool",
            &[],
            None,
            "negative control",
        )
        .unwrap_err();
        assert!(error.contains("MISSING"));
        let error = execute(
            &fixture.0,
            "git",
            &["definitely-not-a-command", "sensitive-marker"],
            None,
            "negative control",
        )
        .unwrap_err();
        assert!(error.contains("exit="));
        assert!(!error.contains("sensitive-marker"));
        assert!(!error.contains(&fixture.0.display().to_string()));
    }
}
