//! Commit evidence hashes for the recorder.
//!
//! The diff itself stays `git`'s. This command hashes that stdout, sums
//! numstat, and checks a directive comment. Tests build a temporary repository,
//! so they do not read this clone's history and still pass in a depth-1 checkout.
//!
//! | Intent | Old shell | New | Why |
//! | --- | --- | --- | --- |
//! | Patch byte identity | `git diff --binary --full-index "$parent" "$sha"` piped to `sha256sum` | `patch-sha256` | Digest the exact diff bytes the recorder compared by hand. Parent is `sha^` unless `--base` is set. |
//! | Stable patch id | `git show "$sha"` piped to `git patch-id --stable` | `patch-id` from `git diff --binary --full-index "$parent" "$sha"` piped to `git patch-id --stable` | Without `--full-index`, index abbreviations change with how many objects the clone has. The same command covers a `--base` range. |
//! | Diff size | `git diff --numstat "$parent" "$sha"`, then sum | `files`, `insertions`, `deletions` | Totals. A binary `-` counts as one file and zero lines. |
//! | Directive self-hash | Drop the first two lines and the last byte, then `sha256sum`, and compare with the first 64-hex token anywhere | `directive-sha256` and `directive` | Hash the bytes after the two-line header as-is. The expected token is the 64-hex value on the first line (`지시문 sha256`), which is where #368 comments put it. The second line is the blank separator and is not searched. |
//! | Expected value | Visual compare of a full hex or `prefix…suffix` | `--expect field=pattern` | Non-zero exit on mismatch. Empty sides, extra ellipses, non-hex, the wrong length, or an overlapping prefix and suffix that disagree are an ambiguous expect. |
//!
//! Raw directive bytes are `gh api repos/OWNER/REPO/issues/comments/<id> | jq -j .body`.
//! A body that ends in `\n` or contains CR is rejected. `directive: MISMATCH` exits 1.
//! An empty commit exits 1 because that diff has no patch id. A merge is hashed
//! with the same full-index diff against its parent, not with `git show`.
//!
//! Fail closed (non-zero, message on stderr, no partial stdout) for an unknown
//! commit, an ambiguous short SHA, a root commit, a missing parent (including a
//! shallow clone that does not contain `sha^`), an unreadable directive file, a
//! directive body that cannot be checked, or an ambiguous `--expect`.

use std::ffi::OsString;
use std::fmt;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, ExitCode, ExitStatus, Stdio};

const DIRECTIVE_GH: &str = "gh api repos/OWNER/REPO/issues/comments/<id> | jq -j .body";

const HELP: &str = "\
Hash a commit the way the evidence recorder checks it

Usage: cargo xtask evidence-hash [options] --sha <sha>
       cargo xtask evidence-hash [options] --sha <sha> --base <parent>

The diff parent of a commit is <sha>^ unless --base is set. patch-sha256 and
patch-id both come from
`git diff --binary --full-index <parent> <sha> | git patch-id --stable`
(sha256sum of that same diff for patch-sha256). Numstat totals come from
`git diff --numstat` of that pair. An empty commit exits 1.

Save a GitHub issue comment with:
  gh api repos/OWNER/REPO/issues/comments/<id> | jq -j .body
The checker drops the first two lines and hashes the remaining bytes as-is.
The expected hash is the 64-hex token on the first line. A trailing newline
(typical of `gh api --jq`) or any CR is an error. MISMATCH exits 1.

Options:
  --repo <path>               Repository (default: .)
  --sha <rev>                 Commit to hash
  --base <rev>                Diff parent instead of <sha>^
  --directive-file <path>     Raw GitHub issue comment body
  --expect <field>=<pattern>  Compare a field to hex or prefix…suffix
  --json                      Print one JSON object
  -h, --help                  Show this help

Fields for --expect: patch-sha256, patch-id, directive-sha256
The prefix…suffix form accepts '...' or '…'.
";

#[derive(Debug, PartialEq, Eq)]
pub(crate) enum Parsed {
    Help,
    Run(Options),
}

#[derive(Debug, PartialEq, Eq)]
pub(crate) struct Options {
    repo: PathBuf,
    target: Target,
    directive_file: Option<PathBuf>,
    expects: Vec<Expect>,
    json: bool,
}

#[derive(Debug, PartialEq, Eq)]
struct Target {
    sha: String,
    base: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Field {
    PatchSha256,
    PatchId,
    DirectiveSha256,
}

impl Field {
    fn parse(name: &str) -> Option<Self> {
        match name {
            "patch-sha256" => Some(Self::PatchSha256),
            "patch-id" => Some(Self::PatchId),
            "directive-sha256" => Some(Self::DirectiveSha256),
            _ => None,
        }
    }

    fn width(self) -> usize {
        match self {
            Self::PatchId => 40,
            Self::PatchSha256 | Self::DirectiveSha256 => 64,
        }
    }
}

impl fmt::Display for Field {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::PatchSha256 => "patch-sha256",
            Self::PatchId => "patch-id",
            Self::DirectiveSha256 => "directive-sha256",
        })
    }
}

#[derive(Debug, PartialEq, Eq)]
struct Expect {
    field: Field,
    pattern: CompiledPattern,
    raw: String,
}

#[derive(Debug, PartialEq, Eq)]
enum CompiledPattern {
    Exact(String),
    Abbrev { prefix: String, suffix: String },
}

impl CompiledPattern {
    fn matches(&self, actual: &str) -> bool {
        let actual = actual.to_ascii_lowercase();
        match self {
            Self::Exact(hex) => actual == *hex,
            Self::Abbrev { prefix, suffix } => {
                actual.starts_with(prefix.as_str()) && actual.ends_with(suffix.as_str())
            }
        }
    }
}

#[derive(Debug, PartialEq, Eq)]
pub(crate) enum EvidenceError {
    Usage(String),
    Ambiguous(String),
    Failed(String),
}

impl EvidenceError {
    fn exit_code(&self) -> u8 {
        match self {
            Self::Usage(_) | Self::Ambiguous(_) => 2,
            Self::Failed(_) => 1,
        }
    }
}

impl fmt::Display for EvidenceError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Usage(message) | Self::Ambiguous(message) | Self::Failed(message) => {
                formatter.write_str(message)
            }
        }
    }
}

#[derive(Debug, PartialEq, Eq)]
struct Evidence {
    commit: String,
    parent: String,
    patch_sha256: String,
    patch_id: String,
    files: u64,
    insertions: u64,
    deletions: u64,
    directive: Option<Directive>,
}

#[derive(Debug, PartialEq, Eq)]
struct Directive {
    sha256: String,
    verdict: Verdict,
}

#[derive(Debug, PartialEq, Eq)]
enum Verdict {
    Match,
    Mismatch,
}

impl fmt::Display for Verdict {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::Match => "MATCH",
            Self::Mismatch => "MISMATCH",
        })
    }
}

struct Outcome {
    stdout: String,
    stderr: String,
    exit: u8,
}

pub(crate) fn help() -> &'static str {
    debug_assert!(HELP.contains(DIRECTIVE_GH));
    HELP
}

pub(crate) fn parse_args(args: Vec<OsString>) -> Result<Parsed, EvidenceError> {
    if args.iter().any(|arg| arg == "--help" || arg == "-h") {
        return Ok(Parsed::Help);
    }

    let mut json = false;
    let mut sha = None;
    let mut base = None;
    let mut directive_file = None;
    let mut repo = PathBuf::from(".");
    let mut expects = Vec::new();
    let mut positional = Vec::new();
    let mut iter = args.into_iter();
    while let Some(arg) = iter.next() {
        let Some(text) = arg.to_str() else {
            return Err(usage(format!("unexpected argument {arg:?}")));
        };
        match text {
            "--json" => json = true,
            "--sha" => sha = Some(need_value(&mut iter, "--sha")?),
            "--base" => base = Some(need_value(&mut iter, "--base")?),
            "--directive-file" => {
                directive_file = Some(PathBuf::from(need_value(&mut iter, "--directive-file")?));
            }
            "--repo" => repo = PathBuf::from(need_value(&mut iter, "--repo")?),
            "--expect" => expects.push(parse_expect(&need_value(&mut iter, "--expect")?)?),
            other if other.starts_with('-') => {
                return Err(usage(format!("unexpected argument {other}")));
            }
            other => positional.push(other.to_string()),
        }
    }

    if expects
        .iter()
        .any(|expect| expect.field == Field::DirectiveSha256)
        && directive_file.is_none()
    {
        return Err(usage(
            "--expect directive-sha256 requires --directive-file".to_string(),
        ));
    }
    reject_duplicate_expects(&expects)?;
    let target = resolve_target(sha, base, &positional)?;
    Ok(Parsed::Run(Options {
        repo,
        target,
        directive_file,
        expects,
        json,
    }))
}

pub(crate) fn execute(parsed: &Parsed) -> ExitCode {
    match parsed {
        Parsed::Help => {
            print!("{HELP}");
            ExitCode::SUCCESS
        }
        Parsed::Run(options) => {
            let outcome = outcome(options);
            print!("{}", outcome.stdout);
            if !outcome.stderr.is_empty() {
                eprint!("{}", outcome.stderr);
            }
            ExitCode::from(outcome.exit)
        }
    }
}

fn outcome(options: &Options) -> Outcome {
    let evidence = match compute(options) {
        Ok(evidence) => evidence,
        Err(error) => {
            return Outcome {
                stdout: String::new(),
                stderr: format!("error: {error}\n"),
                exit: error.exit_code(),
            };
        }
    };
    let stdout = render(&evidence, options.json);
    if let Err(error) = check_expects(&evidence, &options.expects) {
        return Outcome {
            stdout,
            stderr: format!("error: {error}\n"),
            exit: error.exit_code(),
        };
    }
    if matches!(
        evidence.directive,
        Some(Directive {
            verdict: Verdict::Mismatch,
            ..
        })
    ) {
        return Outcome {
            stdout,
            stderr: "error: directive hash mismatch\n".to_string(),
            exit: 1,
        };
    }
    Outcome {
        stdout,
        stderr: String::new(),
        exit: 0,
    }
}

fn compute(options: &Options) -> Result<Evidence, EvidenceError> {
    let (commit, parent) = if let Some(base) = &options.target.base {
        // Range endpoints are not required to be commits.
        let parent = resolve_rev(&options.repo, base)?;
        let commit = resolve_rev(&options.repo, &options.target.sha)?;
        (commit, parent)
    } else {
        let commit = resolve_commit(&options.repo, &options.target.sha)?;
        let parent = resolve_parent(&options.repo, &commit)?;
        (commit, parent)
    };
    let diff = git_stdout(
        &options.repo,
        &[
            "diff",
            "--no-ext-diff",
            "--binary",
            "--full-index",
            &parent,
            &commit,
        ],
    )?;
    let patch_sha256 = sha256_hex(&diff)?;
    let patch_id = patch_id_of_diff(&options.repo, &diff)?;
    let totals = numstat(&options.repo, &parent, &commit)?;
    let directive = match &options.directive_file {
        Some(path) => Some(directive_from_file(path)?),
        None => None,
    };
    Ok(Evidence {
        commit,
        parent,
        patch_sha256,
        patch_id,
        files: totals.files,
        insertions: totals.insertions,
        deletions: totals.deletions,
        directive,
    })
}

fn render(evidence: &Evidence, json: bool) -> String {
    if json {
        render_json(evidence)
    } else {
        render_text(evidence)
    }
}

fn render_text(evidence: &Evidence) -> String {
    let mut out = format!(
        "commit: {commit}\nparent: {parent}\npatch-sha256: {sha}\npatch-id: {id}\nfiles: {files}\ninsertions: {insertions}\ndeletions: {deletions}\n",
        commit = evidence.commit,
        parent = evidence.parent,
        sha = evidence.patch_sha256,
        id = evidence.patch_id,
        files = evidence.files,
        insertions = evidence.insertions,
        deletions = evidence.deletions,
    );
    if let Some(directive) = &evidence.directive {
        out.push_str(&format!(
            "directive-sha256: {sha}\ndirective: {verdict}\n",
            sha = directive.sha256,
            verdict = directive.verdict,
        ));
    }
    out
}

fn render_json(evidence: &Evidence) -> String {
    let mut out = format!(
        "{{\"commit\":\"{commit}\",\"parent\":\"{parent}\",\"patch_sha256\":\"{sha}\",\"patch_id\":\"{id}\",\"files\":{files},\"insertions\":{insertions},\"deletions\":{deletions}",
        commit = evidence.commit,
        parent = evidence.parent,
        sha = evidence.patch_sha256,
        id = evidence.patch_id,
        files = evidence.files,
        insertions = evidence.insertions,
        deletions = evidence.deletions,
    );
    if let Some(directive) = &evidence.directive {
        out.push_str(&format!(
            ",\"directive_sha256\":\"{sha}\",\"directive\":\"{verdict}\"",
            sha = directive.sha256,
            verdict = directive.verdict,
        ));
    }
    out.push_str("}\n");
    out
}

fn check_expects(evidence: &Evidence, expects: &[Expect]) -> Result<(), EvidenceError> {
    for expect in expects {
        let actual = match expect.field {
            Field::PatchSha256 => evidence.patch_sha256.as_str(),
            Field::PatchId => evidence.patch_id.as_str(),
            Field::DirectiveSha256 => evidence
                .directive
                .as_ref()
                .ok_or_else(|| {
                    usage("--expect directive-sha256 requires --directive-file".to_string())
                })?
                .sha256
                .as_str(),
        };
        if !expect.pattern.matches(actual) {
            return Err(EvidenceError::Failed(format!(
                "expect mismatch: {field} is {actual}, pattern was {raw}",
                field = expect.field,
                raw = expect.raw,
            )));
        }
    }
    Ok(())
}

fn resolve_target(
    sha: Option<String>,
    base: Option<String>,
    positional: &[String],
) -> Result<Target, EvidenceError> {
    if let Some(extra) = positional.first() {
        return Err(usage(format!(
            "unexpected revision argument '{extra}'; use --sha <sha> or --sha <sha> --base <parent>"
        )));
    }
    let Some(sha) = sha else {
        return Err(usage(
            "missing --sha; use --sha <sha> or --sha <sha> --base <parent>".to_string(),
        ));
    };
    if sha.contains("..") {
        return Err(usage(
            "use --sha <sha> --base <parent> for a range".to_string(),
        ));
    }
    Ok(Target { sha, base })
}

fn need_value(
    iter: &mut impl Iterator<Item = OsString>,
    flag: &str,
) -> Result<String, EvidenceError> {
    let Some(value) = iter.next() else {
        return Err(usage(format!("missing value for {flag}")));
    };
    value
        .to_str()
        .map(str::to_string)
        .ok_or_else(|| usage(format!("value for {flag} is not unicode: {value:?}")))
}

fn parse_expect(raw: &str) -> Result<Expect, EvidenceError> {
    let Some((field_name, pattern)) = raw.split_once('=') else {
        return Err(ambiguous(raw, "expected field=hex or field=prefix…suffix"));
    };
    let Some(field) = Field::parse(field_name) else {
        return Err(ambiguous(
            raw,
            "field must be patch-sha256, patch-id, or directive-sha256",
        ));
    };
    if pattern.contains('=') {
        return Err(ambiguous(raw, "pattern must not contain '='"));
    }
    let compiled = compile_pattern(pattern, field.width(), raw)?;
    Ok(Expect {
        field,
        pattern: compiled,
        raw: raw.to_string(),
    })
}

fn reject_duplicate_expects(expects: &[Expect]) -> Result<(), EvidenceError> {
    for (index, expect) in expects.iter().enumerate() {
        if expects[..index]
            .iter()
            .any(|earlier| earlier.field == expect.field)
        {
            return Err(ambiguous(&expect.raw, "duplicate field"));
        }
    }
    Ok(())
}

fn compile_pattern(
    pattern: &str,
    width: usize,
    raw: &str,
) -> Result<CompiledPattern, EvidenceError> {
    if pattern.is_empty() {
        return Err(ambiguous(raw, "pattern is empty"));
    }
    if let Some((prefix, suffix)) = split_ellipsis(pattern, raw)? {
        let prefix = hex_bytes(prefix, raw)?;
        let suffix = hex_bytes(suffix, raw)?;
        if prefix.is_empty() || suffix.is_empty() {
            return Err(ambiguous(raw, "prefix and suffix must both be non-empty"));
        }
        if prefix.len() > width || suffix.len() > width {
            return Err(ambiguous(raw, "prefix or suffix is longer than the hash"));
        }
        if prefix.len() + suffix.len() > width {
            let overlap = prefix.len() + suffix.len() - width;
            let prefix_tail = &prefix[prefix.len() - overlap..];
            let suffix_head = &suffix[..overlap];
            if prefix_tail != suffix_head {
                return Err(ambiguous(
                    raw,
                    "prefix and suffix disagree where they overlap",
                ));
            }
        }
        return Ok(CompiledPattern::Abbrev { prefix, suffix });
    }
    if is_hex(pattern) && pattern.len() == width {
        return Ok(CompiledPattern::Exact(pattern.to_ascii_lowercase()));
    }
    Err(ambiguous(
        raw,
        &format!("expected {width} hex digits or prefix…suffix"),
    ))
}

fn split_ellipsis<'a>(
    pattern: &'a str,
    raw: &str,
) -> Result<Option<(&'a str, &'a str)>, EvidenceError> {
    let unicode = pattern.matches('…').count();
    let ascii = pattern.matches("...").count();
    if unicode + ascii == 0 {
        return Ok(None);
    }
    if unicode + ascii != 1 {
        return Err(ambiguous(raw, "use a single '...' or '…' separator"));
    }
    if unicode == 1 {
        Ok(pattern.split_once('…'))
    } else {
        Ok(pattern.split_once("..."))
    }
}

fn hex_bytes(value: &str, raw: &str) -> Result<String, EvidenceError> {
    if value.is_empty() {
        return Ok(String::new());
    }
    if !is_hex(value) {
        return Err(ambiguous(raw, "prefix and suffix must be hex"));
    }
    Ok(value.to_ascii_lowercase())
}

fn is_hex(value: &str) -> bool {
    !value.is_empty() && value.bytes().all(|byte| byte.is_ascii_hexdigit())
}

fn usage(message: String) -> EvidenceError {
    EvidenceError::Usage(message)
}

fn ambiguous(raw: &str, reason: &str) -> EvidenceError {
    EvidenceError::Ambiguous(format!("ambiguous --expect '{raw}': {reason}"))
}

fn resolve_commit(repo: &Path, rev: &str) -> Result<String, EvidenceError> {
    let spec = format!("{rev}^{{commit}}");
    let output = git(repo)
        .args(["rev-parse", "--verify", "--end-of-options", &spec])
        .output()
        .map_err(|err| spawn_error("git", err))?;
    if !output.status.success() {
        return Err(missing_rev(repo, rev, &output.stderr, true));
    }
    parse_rev_stdout(&output.stdout)
}

fn resolve_rev(repo: &Path, rev: &str) -> Result<String, EvidenceError> {
    let output = git(repo)
        .args(["rev-parse", "--verify", "--end-of-options", rev])
        .output()
        .map_err(|err| spawn_error("git", err))?;
    if !output.status.success() {
        return Err(missing_rev(repo, rev, &output.stderr, false));
    }
    parse_rev_stdout(&output.stdout)
}

fn resolve_parent(repo: &Path, sha: &str) -> Result<String, EvidenceError> {
    let spec = format!("{sha}^");
    let output = git(repo)
        .args(["rev-parse", "--verify", "--end-of-options", &spec])
        .output()
        .map_err(|err| spawn_error("git", err))?;
    if output.status.success() {
        return parse_rev_stdout(&output.stdout);
    }
    if not_a_repository(&output.stderr) {
        return Err(missing_rev(repo, sha, &output.stderr, true));
    }
    if !commit_records_parent(repo, sha)? {
        return Err(EvidenceError::Failed(format!(
            "root commit {sha} has no parent"
        )));
    }
    let why = if is_shallow(repo) {
        "this repository is shallow and does not contain the parent"
    } else {
        "the parent commit is not in this repository"
    };
    Err(EvidenceError::Failed(format!(
        "parent of {sha} is missing ({sha}^); {why}; pass --base when that tree is already available"
    )))
}

fn commit_records_parent(repo: &Path, sha: &str) -> Result<bool, EvidenceError> {
    let stdout = git_stdout(repo, &["cat-file", "-p", sha])?;
    let text = std::str::from_utf8(&stdout)
        .map_err(|_| EvidenceError::Failed("git cat-file output is not utf-8".to_string()))?;
    Ok(text.lines().any(|line| line.starts_with("parent ")))
}

fn missing_rev(repo: &Path, rev: &str, stderr: &[u8], commit: bool) -> EvidenceError {
    if not_a_repository(stderr) {
        return EvidenceError::Failed(format!("not a git repository: {}", repo.display()));
    }
    if is_short_hex(rev) && matching_objects(repo, rev) > 1 {
        return EvidenceError::Failed(format!("ambiguous short SHA '{rev}'"));
    }
    let kind = if commit { "commit" } else { "revision" };
    EvidenceError::Failed(format!("unknown {kind} '{rev}'"))
}

fn is_short_hex(rev: &str) -> bool {
    is_hex(rev) && rev.len() < 40
}

fn matching_objects(repo: &Path, prefix: &str) -> usize {
    let flag = format!("--disambiguate={prefix}");
    let Ok(output) = git(repo).args(["rev-parse", &flag]).output() else {
        return 0;
    };
    if !output.status.success() {
        return 0;
    }
    String::from_utf8_lossy(&output.stdout)
        .lines()
        .filter(|line| !line.is_empty())
        .count()
}

fn not_a_repository(stderr: &[u8]) -> bool {
    String::from_utf8_lossy(stderr).contains("not a git repository")
}

fn parse_rev_stdout(stdout: &[u8]) -> Result<String, EvidenceError> {
    let text = std::str::from_utf8(stdout)
        .map_err(|_| EvidenceError::Failed("git rev-parse output is not utf-8".to_string()))?;
    let sha = text.trim();
    if is_hex(sha) && (sha.len() == 40 || sha.len() == 64) {
        Ok(sha.to_ascii_lowercase())
    } else {
        Err(EvidenceError::Failed(format!(
            "git rev-parse output is not a commit id: {text:?}"
        )))
    }
}

fn is_shallow(repo: &Path) -> bool {
    git(repo)
        .args(["rev-parse", "--is-shallow-repository"])
        .output()
        .ok()
        .filter(|output| output.status.success())
        .and_then(|output| String::from_utf8(output.stdout).ok())
        .is_some_and(|text| text.trim() == "true")
}

fn numstat(repo: &Path, from: &str, to: &str) -> Result<Totals, EvidenceError> {
    let stdout = git_stdout(repo, &["diff", "--no-ext-diff", "--numstat", from, to])?;
    parse_numstat(&stdout)
}

#[derive(Debug, Default, PartialEq, Eq)]
struct Totals {
    files: u64,
    insertions: u64,
    deletions: u64,
}

fn parse_numstat(bytes: &[u8]) -> Result<Totals, EvidenceError> {
    let text = std::str::from_utf8(bytes).map_err(|_| numstat_error())?;
    let mut totals = Totals::default();
    for line in text.split('\n') {
        if line.is_empty() {
            continue;
        }
        let mut parts = line.splitn(3, '\t');
        let Some(added) = parts.next() else {
            return Err(numstat_error());
        };
        let Some(deleted) = parts.next() else {
            return Err(numstat_error());
        };
        let Some(path) = parts.next() else {
            return Err(numstat_error());
        };
        if path.is_empty() {
            return Err(numstat_error());
        }
        totals.files = totals
            .files
            .checked_add(1)
            .ok_or_else(|| EvidenceError::Failed("numstat file count overflow".to_string()))?;
        totals.insertions = totals
            .insertions
            .checked_add(parse_hunk(added)?)
            .ok_or_else(|| EvidenceError::Failed("numstat insertion count overflow".to_string()))?;
        totals.deletions = totals
            .deletions
            .checked_add(parse_hunk(deleted)?)
            .ok_or_else(|| EvidenceError::Failed("numstat deletion count overflow".to_string()))?;
    }
    Ok(totals)
}

fn parse_hunk(field: &str) -> Result<u64, EvidenceError> {
    if field == "-" {
        return Ok(0);
    }
    field.parse::<u64>().map_err(|_| numstat_error())
}

fn numstat_error() -> EvidenceError {
    EvidenceError::Failed("git diff --numstat produced an unrecognized line".to_string())
}

fn patch_id_of_diff(repo: &Path, diff: &[u8]) -> Result<String, EvidenceError> {
    let mut command = git(repo);
    command
        .args(["patch-id", "--stable"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let mut child = command.spawn().map_err(|err| spawn_error("git", err))?;
    let write_result = child
        .stdin
        .take()
        .ok_or_else(|| EvidenceError::Failed("git patch-id has no stdin".to_string()))
        .and_then(|mut stdin| {
            stdin
                .write_all(diff)
                .map_err(|err| EvidenceError::Failed(format!("git patch-id write failed: {err}")))
        });
    if let Err(error) = write_result {
        let _ = child.kill();
        let _ = child.wait();
        return Err(error);
    }
    let output = child
        .wait_with_output()
        .map_err(|err| spawn_error("git", err))?;
    if !output.status.success() {
        return Err(command_failure(
            "git",
            "patch-id --stable",
            output.status,
            &output.stderr,
        ));
    }
    parse_patch_id(&output.stdout)
}

fn parse_patch_id(stdout: &[u8]) -> Result<String, EvidenceError> {
    let text = std::str::from_utf8(stdout)
        .map_err(|_| EvidenceError::Failed("git patch-id output is not utf-8".to_string()))?;
    let mut ids = Vec::new();
    for line in text.lines() {
        if line.is_empty() {
            continue;
        }
        let Some(id) = line.split_whitespace().next() else {
            continue;
        };
        if id.len() != 40 || !is_hex(id) {
            return Err(EvidenceError::Failed(format!(
                "git patch-id --stable produced unrecognized output: {text:?}"
            )));
        }
        ids.push(id.to_ascii_lowercase());
    }
    match ids.as_slice() {
        [id] => Ok(id.clone()),
        [] => Err(EvidenceError::Failed(
            "git patch-id --stable produced no patch id; an empty commit has an empty diff and exits 1"
                .to_string(),
        )),
        _ => Err(EvidenceError::Failed(
            "git patch-id --stable produced multiple patch ids".to_string(),
        )),
    }
}

fn git_stdout(repo: &Path, args: &[&str]) -> Result<Vec<u8>, EvidenceError> {
    let output = git(repo)
        .args(args)
        .output()
        .map_err(|err| spawn_error("git", err))?;
    if !output.status.success() {
        return Err(command_failure(
            "git",
            &args.join(" "),
            output.status,
            &output.stderr,
        ));
    }
    Ok(output.stdout)
}

fn git(repo: &Path) -> Command {
    let mut command = Command::new("git");
    command
        .args([
            "-c",
            "diff.noprefix=false",
            "-c",
            "diff.renames=true",
            "-c",
            "diff.mnemonicPrefix=false",
        ])
        .arg("-C")
        .arg(repo)
        .arg("--no-pager")
        .stdin(Stdio::null());
    scrub_git_env(&mut command);
    command
}

fn scrub_git_env(command: &mut Command) {
    command
        .env("LC_ALL", "C")
        .env("GIT_TERMINAL_PROMPT", "0")
        .env_remove("GIT_DIR")
        .env_remove("GIT_WORK_TREE")
        .env_remove("GIT_INDEX_FILE")
        .env_remove("GIT_OBJECT_DIRECTORY")
        .env_remove("GIT_ALTERNATE_OBJECT_DIRECTORIES")
        .env_remove("GIT_NAMESPACE")
        .env_remove("GIT_COMMON_DIR");
}

fn sha256_hex(bytes: &[u8]) -> Result<String, EvidenceError> {
    let mut command = Command::new("sha256sum");
    command
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .env("LC_ALL", "C");
    let mut child = command
        .spawn()
        .map_err(|err| spawn_error("sha256sum", err))?;
    let write_result = child
        .stdin
        .take()
        .ok_or_else(|| EvidenceError::Failed("sha256sum has no stdin".to_string()))
        .and_then(|mut stdin| {
            stdin
                .write_all(bytes)
                .map_err(|err| EvidenceError::Failed(format!("sha256sum write failed: {err}")))
        });
    if let Err(error) = write_result {
        let _ = child.kill();
        let _ = child.wait();
        return Err(error);
    }
    let output = child
        .wait_with_output()
        .map_err(|err| spawn_error("sha256sum", err))?;
    if !output.status.success() {
        return Err(command_failure(
            "sha256sum",
            "",
            output.status,
            &output.stderr,
        ));
    }
    parse_sha256(&output.stdout)
}

fn parse_sha256(stdout: &[u8]) -> Result<String, EvidenceError> {
    let text = std::str::from_utf8(stdout)
        .map_err(|_| EvidenceError::Failed("sha256sum output is not utf-8".to_string()))?;
    let hex = text.split_whitespace().next().unwrap_or("");
    if hex.len() == 64 && is_hex(hex) {
        Ok(hex.to_ascii_lowercase())
    } else {
        Err(EvidenceError::Failed(format!(
            "sha256sum output is not a sha256 digest: {text:?}"
        )))
    }
}

fn directive_from_file(path: &Path) -> Result<Directive, EvidenceError> {
    let bytes = std::fs::read(path).map_err(|err| {
        EvidenceError::Failed(format!(
            "unreadable directive file {}: {err}",
            path.display()
        ))
    })?;
    directive_from_bytes(&bytes)
}

fn directive_from_bytes(bytes: &[u8]) -> Result<Directive, EvidenceError> {
    if bytes.contains(&b'\r') {
        return Err(EvidenceError::Failed(
            "directive body contains CR; save it with `gh api repos/OWNER/REPO/issues/comments/<id> | jq -j .body`"
                .to_string(),
        ));
    }
    if bytes.ends_with(b"\n") {
        return Err(EvidenceError::Failed(
            "directive body ends with a newline; this looks like `gh api --jq .body`, not `jq -j .body`"
                .to_string(),
        ));
    }
    let preimage = drop_first_two_lines(bytes)?;
    let embedded = hash_on_first_line(bytes)?;
    let sha256 = sha256_hex(preimage)?;
    let verdict = if sha256 == embedded {
        Verdict::Match
    } else {
        Verdict::Mismatch
    };
    Ok(Directive { sha256, verdict })
}

/// Header is two newline-terminated lines. The remainder is hashed as-is.
fn drop_first_two_lines(bytes: &[u8]) -> Result<&[u8], EvidenceError> {
    let mut seen = 0;
    for (index, byte) in bytes.iter().enumerate() {
        if *byte == b'\n' {
            seen += 1;
            if seen == 2 {
                return Ok(&bytes[index + 1..]);
            }
        }
    }
    Err(EvidenceError::Failed(
        "directive body has fewer than 2 lines".to_string(),
    ))
}

/// #368 comments put `지시문 sha256 \`<64-hex>\`` on the first line.
/// The second line is the blank separator. Later lines are not searched.
fn hash_on_first_line(bytes: &[u8]) -> Result<String, EvidenceError> {
    let end = bytes
        .iter()
        .position(|byte| *byte == b'\n')
        .unwrap_or(bytes.len());
    let line = &bytes[..end];
    let mut found = None;
    let mut index = 0;
    while index < line.len() {
        if line[index].is_ascii_hexdigit() {
            let start = index;
            while index < line.len() && line[index].is_ascii_hexdigit() {
                index += 1;
            }
            if index - start == 64 {
                if found.is_some() {
                    return Err(EvidenceError::Failed(
                        "first line has more than one 64-hex value".to_string(),
                    ));
                }
                found = Some(
                    std::str::from_utf8(&line[start..index])
                        .map_err(|_| {
                            EvidenceError::Failed("directive hash is not utf-8".to_string())
                        })?
                        .to_ascii_lowercase(),
                );
            }
        } else {
            index += 1;
        }
    }
    found
        .ok_or_else(|| EvidenceError::Failed("first line has no 64-hex directive hash".to_string()))
}

fn spawn_error(program: &str, err: std::io::Error) -> EvidenceError {
    EvidenceError::Failed(format!("failed to start {program}: {err}"))
}

fn command_failure(program: &str, args: &str, status: ExitStatus, stderr: &[u8]) -> EvidenceError {
    let detail = String::from_utf8_lossy(stderr);
    let detail = detail.trim();
    if args.is_empty() {
        if detail.is_empty() {
            EvidenceError::Failed(format!("{program} failed ({status})"))
        } else {
            EvidenceError::Failed(format!("{program} failed ({status}): {detail}"))
        }
    } else if detail.is_empty() {
        EvidenceError::Failed(format!("{program} {args} failed ({status})"))
    } else {
        EvidenceError::Failed(format!("{program} {args} failed ({status}): {detail}"))
    }
}

#[cfg(test)]
mod tests {
    use super::{
        compute, help, outcome, parse_args, CompiledPattern, EvidenceError, Field, Options, Parsed,
        Target,
    };
    use std::ffi::OsString;
    use std::io::Write;
    use std::path::{Path, PathBuf};
    use std::process::Command;
    use std::sync::atomic::{AtomicU64, Ordering};

    struct Repo {
        root: PathBuf,
        origin: PathBuf,
    }

    impl Repo {
        fn new() -> Self {
            static NEXT: AtomicU64 = AtomicU64::new(0);
            let n = NEXT.fetch_add(1, Ordering::Relaxed);
            let root = std::env::temp_dir()
                .join(format!("fvoci-evidence-hash-{}-{n}", std::process::id()));
            let origin = root.join("origin");
            std::fs::create_dir_all(&origin).unwrap();
            let repo = Self { root, origin };
            repo.git(&["init", "-b", "main"]);
            repo.git(&["config", "user.email", "evidence@example.com"]);
            repo.git(&["config", "user.name", "Evidence"]);
            repo.git(&["config", "commit.gpgsign", "false"]);
            repo.git(&["config", "core.autocrlf", "false"]);
            repo
        }

        fn commit(&self, rel: &str, bytes: &[u8], message: &str) -> String {
            let path = self.origin.join(rel);
            if let Some(parent) = path.parent() {
                std::fs::create_dir_all(parent).unwrap();
            }
            std::fs::write(&path, bytes).unwrap();
            self.git(&["add", "--", rel]);
            self.git(&["commit", "-m", message]);
            self.rev_parse("HEAD")
        }

        fn rev_parse(&self, rev: &str) -> String {
            self.sh(&format!("git rev-parse {rev}")).trim().to_string()
        }

        fn clone_depth(&self, depth: u32) -> PathBuf {
            let dest = self.root.join(format!("shallow-{depth}"));
            let url = format!("file://{}", self.origin.display());
            let output = git_command()
                .args([
                    "clone",
                    "--depth",
                    &depth.to_string(),
                    "--no-local",
                    &url,
                    dest.to_str().unwrap(),
                ])
                .output()
                .unwrap();
            assert!(
                output.status.success(),
                "clone --depth {depth}: {}",
                String::from_utf8_lossy(&output.stderr)
            );
            dest
        }

        fn git(&self, args: &[&str]) {
            let output = git_command()
                .arg("-C")
                .arg(&self.origin)
                .args(args)
                .output()
                .unwrap();
            assert!(
                output.status.success(),
                "git {args:?}\n{}",
                String::from_utf8_lossy(&output.stderr)
            );
        }

        fn sh(&self, script: &str) -> String {
            sh(&self.origin, script)
        }
    }

    impl Drop for Repo {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.root);
        }
    }

    fn git_command() -> Command {
        let mut command = Command::new("git");
        command
            .env("LC_ALL", "C")
            .env("GIT_TERMINAL_PROMPT", "0")
            .env_remove("GIT_DIR")
            .env_remove("GIT_WORK_TREE")
            .stdin(std::process::Stdio::null());
        command
    }

    fn sh(dir: &Path, script: &str) -> String {
        let output = Command::new("bash")
            .arg("-c")
            .arg(format!("set -o pipefail; {script}"))
            .current_dir(dir)
            .env("LC_ALL", "C")
            .env("GIT_TERMINAL_PROMPT", "0")
            .env_remove("GIT_DIR")
            .env_remove("GIT_WORK_TREE")
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{script}\n{}",
            String::from_utf8_lossy(&output.stderr)
        );
        String::from_utf8(output.stdout).unwrap()
    }

    fn first_field(text: &str) -> String {
        text.split_whitespace().next().unwrap().to_string()
    }

    fn oracle_sha256(bytes: &[u8]) -> String {
        let mut child = Command::new("sha256sum")
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .spawn()
            .unwrap();
        child.stdin.take().unwrap().write_all(bytes).unwrap();
        let output = child.wait_with_output().unwrap();
        assert!(output.status.success());
        first_field(&String::from_utf8(output.stdout).unwrap())
    }

    fn sum_numstat(text: &str) -> (u64, u64, u64) {
        let mut files = 0u64;
        let mut insertions = 0u64;
        let mut deletions = 0u64;
        for line in text.lines() {
            if line.is_empty() {
                continue;
            }
            let mut parts = line.split('\t');
            let added = parts.next().unwrap();
            let deleted = parts.next().unwrap();
            files += 1;
            insertions += if added == "-" {
                0
            } else {
                added.parse::<u64>().unwrap()
            };
            deletions += if deleted == "-" {
                0
            } else {
                deleted.parse::<u64>().unwrap()
            };
        }
        (files, insertions, deletions)
    }

    const PINNED_DIFF: &str = "git -c diff.noprefix=false -c diff.renames=true -c diff.mnemonicPrefix=false diff --no-ext-diff";

    const COMMENT_6076767394: &[u8] = include_bytes!("../fixtures/6076767394.body");
    const COMMENT_6077088942: &[u8] = include_bytes!("../fixtures/6077088942.body");

    fn commit_options(repo: &Path, sha: &str) -> Options {
        Options {
            repo: repo.to_path_buf(),
            target: Target {
                sha: sha.to_string(),
                base: None,
            },
            directive_file: None,
            expects: Vec::new(),
            json: false,
        }
    }

    fn with_base(mut options: Options, base: &str) -> Options {
        options.target.base = Some(base.to_string());
        options
    }

    fn os(args: &[&str]) -> Vec<OsString> {
        args.iter().map(OsString::from).collect()
    }

    fn full_index_sha256(dir: &Path, parent: &str, sha: &str) -> String {
        first_field(&sh(
            dir,
            &format!("{PINNED_DIFF} --binary --full-index {parent} {sha} | sha256sum"),
        ))
    }

    fn full_index_patch_id(dir: &Path, parent: &str, sha: &str) -> String {
        first_field(&sh(
            dir,
            &format!("{PINNED_DIFF} --binary --full-index {parent} {sha} | git patch-id --stable"),
        ))
    }

    fn show_patch_id(dir: &Path, sha: &str) -> String {
        first_field(&sh(dir, &format!("git show {sha} | git patch-id --stable")))
    }

    fn second_line_end(bytes: &[u8]) -> usize {
        let mut seen = 0;
        for (index, byte) in bytes.iter().enumerate() {
            if *byte == b'\n' {
                seen += 1;
                if seen == 2 {
                    return index + 1;
                }
            }
        }
        panic!("fixture has fewer than 2 lines");
    }

    fn first_line_hash(bytes: &[u8]) -> String {
        let end = bytes.iter().position(|byte| *byte == b'\n').unwrap();
        let line = std::str::from_utf8(&bytes[..end]).unwrap();
        let mut found = None;
        let chars: Vec<char> = line.chars().collect();
        let mut index = 0;
        while index < chars.len() {
            if chars[index].is_ascii_hexdigit() {
                let start = index;
                while index < chars.len() && chars[index].is_ascii_hexdigit() {
                    index += 1;
                }
                if index - start == 64 {
                    assert!(found.is_none(), "more than one 64-hex token on line 1");
                    found = Some(chars[start..index].iter().collect::<String>());
                }
            } else {
                index += 1;
            }
        }
        found.expect("line 1 hash").to_ascii_lowercase()
    }

    fn assert_raw_comment(bytes: &[u8]) {
        assert!(!bytes.ends_with(b"\n"));
        assert!(!bytes.contains(&b'\r'));
    }

    #[test]
    fn commit_hashes_match_full_index_diff_not_git_show() {
        let repo = Repo::new();
        repo.commit("a.txt", b"one\ntwo\nthree\n", "root");
        let head = repo.commit("a.txt", b"one\ntwo\nfour\nfive\n", "edit");
        let evidence = compute(&commit_options(&repo.origin, &head)).unwrap();

        let patch = full_index_sha256(&repo.origin, "HEAD^", "HEAD");
        let patch_id = full_index_patch_id(&repo.origin, "HEAD^", "HEAD");
        let (files, insertions, deletions) = sum_numstat(&sh(
            &repo.origin,
            &format!("{PINNED_DIFF} --numstat HEAD^ HEAD"),
        ));

        assert_eq!(evidence.commit, head);
        assert_eq!(evidence.parent, repo.rev_parse("HEAD^"));
        assert_eq!(evidence.patch_sha256, patch);
        assert_eq!(evidence.patch_id, patch_id);
        assert_eq!(
            (evidence.files, evidence.insertions, evidence.deletions),
            (files, insertions, deletions)
        );
        assert_eq!((files, insertions, deletions), (1, 2, 1));
    }

    #[test]
    fn base_uses_the_same_full_index_diff_for_patch_id() {
        let repo = Repo::new();
        let root = repo.commit("a.txt", b"a\n", "root");
        repo.commit("a.txt", b"b\n", "mid");
        let head = repo.commit("a.txt", b"c\n", "head");
        let evidence = compute(&with_base(commit_options(&repo.origin, &head), &root)).unwrap();

        let patch = full_index_sha256(&repo.origin, &root, &head);
        let patch_id = full_index_patch_id(&repo.origin, &root, &head);
        let parent_only = full_index_patch_id(&repo.origin, "HEAD^", "HEAD");
        let shown = show_patch_id(&repo.origin, &head);
        assert_eq!(evidence.parent, root);
        assert_eq!(evidence.commit, head);
        assert_eq!(evidence.patch_sha256, patch);
        assert_eq!(evidence.patch_id, patch_id);
        assert_ne!(evidence.patch_id, parent_only);
        assert_ne!(evidence.patch_id, shown);
    }

    #[test]
    fn pinned_diff_config_ignores_repo_noprefix() {
        let repo = Repo::new();
        repo.commit("a.txt", b"a\n", "root");
        let head = repo.commit("a.txt", b"b\n", "edit");
        repo.git(&["config", "diff.noprefix", "true"]);
        let evidence = compute(&commit_options(&repo.origin, &head)).unwrap();
        let pinned = full_index_sha256(&repo.origin, "HEAD^", "HEAD");
        let unpinned = first_field(&sh(
            &repo.origin,
            "git diff --binary --full-index HEAD^ HEAD | sha256sum",
        ));
        assert_eq!(evidence.patch_sha256, pinned);
        assert_ne!(pinned, unpinned);
        assert_eq!(evidence.commit, head);
    }

    #[test]
    fn binary_patch_id_is_stable_across_clone_object_counts() {
        let repo = Repo::new();
        repo.commit("b.bin", b"a", "root");
        let head = repo.commit("b.bin", b"b\0\xff", "binary");
        pack_distinct_blobs(&repo.origin, 40_000);
        let shallow = repo.clone_depth(2);

        let fat_short = first_field(&sh(
            &repo.origin,
            "git diff HEAD^ HEAD | git patch-id --stable",
        ));
        let shallow_short =
            first_field(&sh(&shallow, "git diff HEAD^ HEAD | git patch-id --stable"));
        let stable = full_index_patch_id(&repo.origin, "HEAD^", "HEAD");
        let shallow_stable = full_index_patch_id(&shallow, "HEAD^", "HEAD");
        assert_ne!(
            fat_short, shallow_short,
            "abbreviations should depend on object count"
        );
        assert_eq!(stable, shallow_stable);
        assert_ne!(stable, fat_short);

        let fat = compute(&commit_options(&repo.origin, &head)).unwrap();
        let thin = compute(&commit_options(&shallow, &head)).unwrap();
        assert_eq!(fat.patch_id, stable);
        assert_eq!(thin.patch_id, stable);
        assert_eq!(
            fat.patch_sha256,
            full_index_sha256(&repo.origin, "HEAD^", "HEAD")
        );
        assert_eq!(fat.patch_sha256, thin.patch_sha256);
    }

    #[test]
    fn binary_numstat_counts_the_file_and_zero_lines() {
        let repo = Repo::new();
        repo.commit("a.txt", b"text\n", "root");
        let head = repo.commit("b.bin", &[0, 1, 2, 255], "binary");
        let evidence = compute(&commit_options(&repo.origin, &head)).unwrap();
        let numstat = sh(&repo.origin, &format!("{PINNED_DIFF} --numstat HEAD^ HEAD"));
        assert!(numstat.contains('-'), "{numstat}");
        let (files, insertions, deletions) = sum_numstat(&numstat);
        assert_eq!(
            (evidence.files, evidence.insertions, evidence.deletions),
            (files, insertions, deletions)
        );
        assert_eq!((files, insertions, deletions), (1, 0, 0));
    }

    #[test]
    fn unknown_sha_root_commit_and_ambiguous_prefix_are_distinct() {
        let repo = Repo::new();
        let root = repo.commit("a.txt", b"one\n", "root");
        let unknown =
            compute(&commit_options(&repo.origin, "definitely-not-a-commit")).unwrap_err();
        assert!(unknown.to_string().contains("unknown commit"), "{unknown}");
        assert!(!unknown.to_string().contains("ambiguous"), "{unknown}");

        let missing = compute(&commit_options(&repo.origin, &root)).unwrap_err();
        let message = missing.to_string();
        assert!(message.contains("root commit"), "{message}");
        assert!(!message.contains("shallow"), "{message}");
        assert!(!message.contains("unknown commit"), "{message}");

        pack_distinct_blobs(&repo.origin, 2_000);
        let listed = sh(
            &repo.origin,
            "git cat-file --batch-check --batch-all-objects",
        );
        let objects: Vec<String> = listed
            .lines()
            .filter_map(|line| line.split_whitespace().next().map(str::to_string))
            .collect();
        let prefix = shared_prefix(&objects).expect("two objects share a 4-hex prefix");
        let ambiguous = compute(&commit_options(&repo.origin, &prefix)).unwrap_err();
        assert!(
            ambiguous.to_string().contains("ambiguous short SHA"),
            "{ambiguous}"
        );
        assert!(
            !ambiguous.to_string().contains("unknown commit"),
            "{ambiguous}"
        );

        let empty = repo.root.join("empty");
        std::fs::create_dir_all(&empty).unwrap();
        let not_repo = compute(&commit_options(&empty, "HEAD")).unwrap_err();
        assert!(
            not_repo.to_string().contains("not a git repository"),
            "{not_repo}"
        );
    }

    fn shared_prefix(shas: &[String]) -> Option<String> {
        // `git rev-parse --disambiguate` ignores prefixes shorter than 4.
        for length in (4..8).rev() {
            let mut seen = std::collections::BTreeMap::<&str, usize>::new();
            for sha in shas {
                if sha.len() < length {
                    continue;
                }
                *seen.entry(&sha[..length]).or_insert(0) += 1;
            }
            if let Some((prefix, _)) = seen.into_iter().find(|(_, count)| *count > 1) {
                return Some(prefix.to_string());
            }
        }
        None
    }

    #[test]
    fn shallow_depth_one_fails_and_depth_two_matches_full_index() {
        let repo = Repo::new();
        repo.commit("a.txt", b"a\n", "root");
        let mid = repo.commit("a.txt", b"b\n", "mid");
        let head = repo.commit("a.txt", b"c\n", "head");

        let depth_one = repo.clone_depth(1);
        let err = compute(&commit_options(&depth_one, "HEAD")).unwrap_err();
        let message = err.to_string();
        assert!(message.contains("shallow"), "{message}");
        assert!(message.contains("parent"), "{message}");
        assert!(!message.contains("root commit"), "{message}");

        let depth_two = repo.clone_depth(2);
        assert_eq!(
            sh(&depth_two, "git rev-parse --is-shallow-repository").trim(),
            "true"
        );
        let evidence = compute(&commit_options(&depth_two, &head)).unwrap();
        assert_eq!(
            evidence.patch_sha256,
            full_index_sha256(&repo.origin, &mid, &head)
        );
        assert_eq!(
            evidence.patch_id,
            full_index_patch_id(&repo.origin, &mid, &head)
        );
        assert_eq!(evidence.parent, mid);
    }

    #[test]
    fn empty_commit_exits_1_and_merge_uses_full_index_diff() {
        let repo = Repo::new();
        repo.commit("a.txt", b"a\n", "root");
        repo.git(&["commit", "--allow-empty", "-m", "empty"]);
        let err = compute(&commit_options(&repo.origin, "HEAD")).unwrap_err();
        assert!(err.to_string().contains("empty commit"), "{err}");
        assert_eq!(err.exit_code(), 1);

        repo.git(&["checkout", "-b", "side"]);
        let side = repo.commit("s.txt", b"side\n", "side");
        repo.git(&["checkout", "main"]);
        repo.git(&["merge", "--no-ff", "side", "-m", "merge"]);
        let merge = repo.rev_parse("HEAD");
        let evidence = compute(&commit_options(&repo.origin, &merge)).unwrap();
        let parent = repo.rev_parse("HEAD^");
        assert_ne!(parent, side);
        assert_eq!(
            evidence.patch_id,
            full_index_patch_id(&repo.origin, &parent, &merge)
        );
        let shown = sh(
            &repo.origin,
            &format!("git show {merge} | git patch-id --stable"),
        );
        assert!(
            shown.trim().is_empty() || first_field(&shown) != evidence.patch_id,
            "git show of a merge is not the patch-id input: {shown}"
        );
    }

    #[test]
    fn issue_368_comments_match_and_bad_bodies_exit_1() {
        for bytes in [COMMENT_6076767394, COMMENT_6077088942] {
            assert_raw_comment(bytes);
            let expected = first_line_hash(bytes);
            let preimage = &bytes[second_line_end(bytes)..];
            assert_eq!(oracle_sha256(preimage), expected);
            let result = directive_outcome(bytes);
            assert_eq!(result.exit, 0, "{}", result.stderr);
            assert!(result
                .stdout
                .contains(&format!("directive-sha256: {expected}")));
            assert!(result.stdout.contains("directive: MATCH"));
        }

        let mut trailed = COMMENT_6076767394.to_vec();
        trailed.push(b'\n');
        let result = directive_outcome(&trailed);
        assert_eq!(result.exit, 1);
        assert!(result.stderr.contains("newline"), "{}", result.stderr);
        assert!(result.stderr.contains("jq -j"), "{}", result.stderr);
        assert!(result.stdout.is_empty(), "{}", result.stdout);

        let crlf = COMMENT_6077088942
            .iter()
            .flat_map(|byte| {
                if *byte == b'\n' {
                    vec![b'\r', b'\n']
                } else {
                    vec![*byte]
                }
            })
            .collect::<Vec<_>>();
        let result = directive_outcome(&crlf);
        assert_eq!(result.exit, 1);
        assert!(result.stderr.contains("CR"), "{}", result.stderr);

        let mut mismatched = COMMENT_6076767394.to_vec();
        let flip = second_line_end(&mismatched);
        mismatched[flip] ^= 0x01;
        let result = directive_outcome(&mismatched);
        assert_eq!(result.exit, 1);
        assert!(
            result.stdout.contains("directive: MISMATCH"),
            "{}",
            result.stdout
        );
        assert!(
            result.stderr.contains("directive hash mismatch"),
            "{}",
            result.stderr
        );

        let mut buried = b"header without a long hash\n\n".to_vec();
        buried.extend(b"ab".repeat(32));
        let result = directive_outcome(&buried);
        assert_eq!(result.exit, 1);
        assert!(result.stderr.contains("first line"), "{}", result.stderr);
    }

    fn directive_outcome(bytes: &[u8]) -> super::Outcome {
        let repo = Repo::new();
        repo.commit("a.txt", b"a\n", "root");
        let head = repo.commit("a.txt", b"b\n", "edit");
        let path = repo.root.join("directive.body");
        std::fs::write(&path, bytes).unwrap();
        let mut options = commit_options(&repo.origin, &head);
        options.directive_file = Some(path);
        let result = outcome(&options);
        // Keep the repo alive until outcome has finished reading it.
        drop(repo);
        result
    }

    #[test]
    fn unreadable_directive_fails_closed() {
        let repo = Repo::new();
        repo.commit("a.txt", b"a\n", "root");
        let head = repo.commit("a.txt", b"b\n", "edit");
        let mut options = commit_options(&repo.origin, &head);
        options.directive_file = Some(repo.root.join("missing-directive"));
        let result = outcome(&options);
        assert_eq!(result.exit, 1);
        assert!(
            result.stderr.contains("unreadable directive file"),
            "{}",
            result.stderr
        );
        assert!(result.stdout.is_empty(), "{}", result.stdout);
    }

    #[test]
    fn ambiguous_expect_is_rejected_before_git() {
        let overlap = format!("patch-sha256={}…{}", "a".repeat(50), "b".repeat(20));
        let patterns = [
            "patch-sha256=abcd",
            "patch-sha256=…abcdef",
            "patch-sha256=abcdef…",
            "patch-sha256=abc…def…123",
            "patch-sha256=qq…aa",
            "nope=abcd",
            overlap.as_str(),
        ];
        for pattern in patterns {
            let err = parse_args(os(&["--expect", pattern])).unwrap_err();
            assert!(
                matches!(err, EvidenceError::Ambiguous(_)),
                "{pattern} -> {err}"
            );
            assert_eq!(err.exit_code(), 2);
        }
        let duplicate = parse_args(os(&[
            "--sha",
            "HEAD",
            "--expect",
            &format!("patch-id={}", "ab".repeat(20)),
            "--expect",
            &format!("patch-id={}", "cd".repeat(20)),
        ]))
        .unwrap_err();
        assert!(duplicate.to_string().contains("duplicate"), "{duplicate}");
        let positional = parse_args(os(&["HEAD...main"])).unwrap_err();
        assert!(
            positional.to_string().contains("unexpected revision"),
            "{positional}"
        );
        assert_eq!(positional.exit_code(), 2);
    }

    #[test]
    fn expect_prefix_suffix_matches_and_mismatch_exits_nonzero() {
        let repo = Repo::new();
        repo.commit("a.txt", b"a\n", "root");
        let head = repo.commit("a.txt", b"b\n", "edit");
        let evidence = compute(&commit_options(&repo.origin, &head)).unwrap();
        let prefix = &evidence.patch_sha256[..8];
        let suffix = &evidence.patch_sha256[evidence.patch_sha256.len() - 8..];
        let ascii = format!("patch-sha256={prefix}...{suffix}");
        let unicode = format!("patch-sha256={prefix}…{suffix}");
        let upper_id = format!("patch-id={}", evidence.patch_id.to_ascii_uppercase());
        let origin = repo.origin.to_str().unwrap();
        for pattern in [&ascii, &unicode, &upper_id] {
            let parsed =
                parse_args(os(&["--repo", origin, "--sha", &head, "--expect", pattern])).unwrap();
            let Parsed::Run(options) = parsed else {
                panic!("help")
            };
            let result = outcome(&options);
            assert_eq!(result.exit, 0, "{pattern}: {}", result.stderr);
            assert!(result.stdout.contains(&format!("commit: {head}")));
        }
        let wrong = format!("patch-sha256={prefix}...00000000");
        let parsed = parse_args(os(&[
            "--repo", origin, "--sha", &head, "--expect", &wrong, "--json",
        ]))
        .unwrap();
        let Parsed::Run(options) = parsed else {
            panic!("help")
        };
        let result = outcome(&options);
        assert_eq!(result.exit, 1);
        assert!(
            result.stderr.contains("expect mismatch"),
            "{}",
            result.stderr
        );
        assert!(result
            .stdout
            .contains(&format!("\"patch_sha256\":\"{}\"", evidence.patch_sha256)));
        assert_eq!(result.stdout.lines().count(), 1);
    }

    #[test]
    fn text_output_is_one_line_per_value() {
        let repo = Repo::new();
        repo.commit("a.txt", b"one\ntwo\nthree\n", "root");
        let head = repo.commit("a.txt", b"one\ntwo\nfour\nfive\n", "edit");
        let parent = repo.rev_parse("HEAD^");
        let result = outcome(&commit_options(&repo.origin, &head));
        assert_eq!(result.exit, 0, "{}", result.stderr);
        let lines: Vec<_> = result.stdout.lines().collect();
        assert_eq!(lines[0], format!("commit: {head}"));
        assert_eq!(lines[1], format!("parent: {parent}"));
        assert!(lines[2].starts_with("patch-sha256: "));
        assert_eq!(lines[2].split_whitespace().nth(1).unwrap().len(), 64);
        assert!(lines[3].starts_with("patch-id: "));
        assert_eq!(lines[3].split_whitespace().nth(1).unwrap().len(), 40);
        assert_eq!(&lines[4..], ["files: 1", "insertions: 2", "deletions: 1"]);
    }

    #[test]
    fn help_states_the_raw_gh_command_and_two_usages() {
        let text = help();
        assert!(text.contains(super::DIRECTIVE_GH));
        assert!(text.contains("--sha <sha> --base <parent>"));
        assert!(!text.contains("--head"));
        assert_eq!(parse_args(os(&["--help"])), Ok(Parsed::Help));
        let missing = parse_args(os(&["--json"])).unwrap_err();
        assert!(missing.to_string().contains("missing --sha"), "{missing}");
        assert_eq!(missing.exit_code(), 2);
    }

    #[test]
    fn abbrev_pattern_stores_lowercase_hex() {
        let parsed = parse_args(os(&["--sha", "HEAD", "--expect", "patch-id=ABCD…0123"])).unwrap();
        let Parsed::Run(options) = parsed else {
            panic!("help")
        };
        assert_eq!(options.expects.len(), 1);
        assert_eq!(options.expects[0].field, Field::PatchId);
        assert_eq!(
            options.expects[0].pattern,
            CompiledPattern::Abbrev {
                prefix: "abcd".to_string(),
                suffix: "0123".to_string(),
            }
        );
    }

    fn pack_distinct_blobs(repo: &Path, count: usize) {
        let mut child = git_command()
            .arg("-C")
            .arg(repo)
            .args(["fast-import", "--quiet"])
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::piped())
            .spawn()
            .unwrap();
        {
            let mut stdin = child.stdin.take().unwrap();
            for index in 0..count {
                let data = format!("u{index:06}");
                write!(stdin, "blob\ndata {}\n{data}", data.len()).unwrap();
            }
        }
        let output = child.wait_with_output().unwrap();
        assert!(
            output.status.success(),
            "fast-import: {}",
            String::from_utf8_lossy(&output.stderr)
        );
    }
}
