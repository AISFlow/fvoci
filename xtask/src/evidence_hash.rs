//! Commit evidence hashes for the recorder.
//!
//! The diff itself stays `git`'s. This command only hashes that stdout, sums
//! numstat, and checks a directive comment. Tests build a temporary repository,
//! so they do not read this clone's history and still pass in a depth-1 checkout.
//!
//! | Intent | Old shell | New | Why |
//! | --- | --- | --- | --- |
//! | Patch byte identity | `git diff --binary --full-index "$parent" "$sha"` piped to `sha256sum` | `patch-sha256` | Digest the exact diff bytes the recorder compared by hand. Parent is `sha^` unless `--base` is set. |
//! | Stable patch id | `git show "$sha"` piped to `git patch-id --stable` | `patch-id` | One stable id. A `base..head` range uses `git diff "$base" "$head"` instead of `git show`. |
//! | Diff size | `git diff --numstat "$parent" "$sha"`, then sum | `files`, `insertions`, `deletions` | Totals. A binary `-` counts as one file and zero lines. |
//! | Directive self-hash | Drop the first two lines and the last byte, then `sha256sum`, and compare with the first 64-hex token | `directive-sha256` and `directive` | `MATCH` or `MISMATCH` against the hash embedded in the comment body. |
//! | Expected value | Visual compare of a full hex or `prefix…suffix` | `--expect field=pattern` | Non-zero exit on mismatch. Empty sides, extra ellipses, non-hex, the wrong length, or an overlapping prefix and suffix that disagree are an ambiguous expect. |
//!
//! Fail closed (non-zero, message on stderr, no partial stdout) for an unknown
//! SHA, a missing parent (including a shallow clone that does not contain
//! `sha^`), an unreadable directive file, a directive body that cannot be
//! checked, or an ambiguous `--expect`. A directive `MISMATCH` is printed and
//! does not by itself change the exit code; pass `--expect` to gate on a hex
//! value.

use std::ffi::OsString;
use std::fmt;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, ExitCode, ExitStatus, Stdio};

const HELP: &str = "\
Hash a commit or a base..head range the way the evidence recorder checks it

Usage: cargo xtask evidence-hash [options] <sha>
       cargo xtask evidence-hash [options] <base>..<head>
       cargo xtask evidence-hash [options] --sha <sha> [--base <parent>]
       cargo xtask evidence-hash [options] --base <base> --head <head>

The diff parent of a commit is <sha>^ unless --base is set. patch-sha256 is the
sha256 of `git diff --binary --full-index <parent> <sha>`. patch-id is
`git patch-id --stable` of `git show <sha>`, or of `git diff <base> <head>` for
a range. Numstat totals come from `git diff --numstat` of that same pair.

Options:
  --repo <path>               Repository (default: .)
  --sha <rev>                 Commit to hash
  --base <rev>                Diff parent, or the start of a range with --head
  --head <rev>                End of a range
  --directive-file <path>     GitHub issue comment body
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
enum Target {
    Commit { sha: String, base: Option<String> },
    Range { base: String, head: String },
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
    HELP
}

pub(crate) fn parse_args(args: Vec<OsString>) -> Result<Parsed, EvidenceError> {
    if args.iter().any(|arg| arg == "--help" || arg == "-h") {
        return Ok(Parsed::Help);
    }

    let mut json = false;
    let mut sha = None;
    let mut base = None;
    let mut head = None;
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
            "--head" => head = Some(need_value(&mut iter, "--head")?),
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
    let target = resolve_target(sha, base, head, &positional)?;
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
    Outcome {
        stdout,
        stderr: String::new(),
        exit: 0,
    }
}

fn compute(options: &Options) -> Result<Evidence, EvidenceError> {
    let (from, to, patch_id_args) = match &options.target {
        Target::Commit { sha, base } => {
            let commit = resolve_commit(&options.repo, sha)?;
            let parent = match base {
                Some(base) => resolve_rev(&options.repo, base)?,
                None => resolve_parent(&options.repo, &commit)?,
            };
            (parent, commit.clone(), vec!["show".to_string(), commit])
        }
        Target::Range { base, head } => {
            let from = resolve_rev(&options.repo, base)?;
            let to = resolve_rev(&options.repo, head)?;
            (from.clone(), to.clone(), vec!["diff".to_string(), from, to])
        }
    };
    let patch_sha256 = diff_sha256(&options.repo, &from, &to)?;
    let patch_id = run_patch_id(
        &options.repo,
        &patch_id_args.iter().map(String::as_str).collect::<Vec<_>>(),
    )?;
    let totals = numstat(&options.repo, &from, &to)?;
    let directive = match &options.directive_file {
        Some(path) => Some(directive_from_file(path)?),
        None => None,
    };
    Ok(Evidence {
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
        "patch-sha256: {sha}\npatch-id: {id}\nfiles: {files}\ninsertions: {insertions}\ndeletions: {deletions}\n",
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
        "{{\"patch_sha256\":\"{sha}\",\"patch_id\":\"{id}\",\"files\":{files},\"insertions\":{insertions},\"deletions\":{deletions}",
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
    head: Option<String>,
    positional: &[String],
) -> Result<Target, EvidenceError> {
    match positional {
        [] => target_from_flags(sha, base, head),
        [one] => positional_target(one, sha, base, head),
        _ => Err(usage("too many revision arguments".to_string())),
    }
}

fn target_from_flags(
    sha: Option<String>,
    base: Option<String>,
    head: Option<String>,
) -> Result<Target, EvidenceError> {
    match (sha, base, head) {
        (Some(sha), base, None) => {
            if sha.contains("..") {
                return Err(usage("use BASE..HEAD for a range, not --sha".to_string()));
            }
            Ok(Target::Commit { sha, base })
        }
        (None, Some(base), Some(head)) => Ok(Target::Range { base, head }),
        (Some(_), _, Some(_)) => Err(usage(
            "--sha and --head together are ambiguous; use --base and --head for a range"
                .to_string(),
        )),
        (None, Some(_), None) => Err(usage("--base requires --sha or --head".to_string())),
        (None, None, Some(_)) => Err(usage("--head requires --base".to_string())),
        (None, None, None) => Err(usage("missing commit SHA or BASE..HEAD range".to_string())),
    }
}

fn positional_target(
    spec: &str,
    sha: Option<String>,
    base: Option<String>,
    head: Option<String>,
) -> Result<Target, EvidenceError> {
    if sha.is_some() || head.is_some() {
        return Err(usage(
            "pass either a revision argument or --sha/--head".to_string(),
        ));
    }
    if let Some((left, right)) = split_range(spec)? {
        if base.is_some() {
            return Err(usage("pass either BASE..HEAD or --base".to_string()));
        }
        return Ok(Target::Range {
            base: left,
            head: right,
        });
    }
    Ok(Target::Commit {
        sha: spec.to_string(),
        base,
    })
}

fn split_range(spec: &str) -> Result<Option<(String, String)>, EvidenceError> {
    if spec.contains("...") {
        return Err(usage(
            "three-dot ranges are not supported; use BASE..HEAD".to_string(),
        ));
    }
    match spec.split_once("..") {
        Some((left, right)) if !left.is_empty() && !right.is_empty() && !right.contains("..") => {
            Ok(Some((left.to_string(), right.to_string())))
        }
        Some(_) => Err(usage(format!("ambiguous range '{spec}'"))),
        None => Ok(None),
    }
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
        return Err(rev_failure(repo, rev, &output.stderr, true));
    }
    parse_rev_stdout(&output.stdout)
}

fn resolve_rev(repo: &Path, rev: &str) -> Result<String, EvidenceError> {
    let output = git(repo)
        .args(["rev-parse", "--verify", "--end-of-options", rev])
        .output()
        .map_err(|err| spawn_error("git", err))?;
    if !output.status.success() {
        return Err(rev_failure(repo, rev, &output.stderr, false));
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
        return Err(rev_failure(repo, sha, &output.stderr, true));
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

fn rev_failure(repo: &Path, rev: &str, stderr: &[u8], commit: bool) -> EvidenceError {
    if not_a_repository(stderr) {
        return EvidenceError::Failed(format!("not a git repository: {}", repo.display()));
    }
    let kind = if commit { "commit" } else { "revision" };
    EvidenceError::Failed(format!("unknown {kind} '{rev}'"))
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

fn diff_sha256(repo: &Path, from: &str, to: &str) -> Result<String, EvidenceError> {
    let stdout = git_stdout(repo, &["diff", "--binary", "--full-index", from, to])?;
    sha256_hex(&stdout)
}

fn numstat(repo: &Path, from: &str, to: &str) -> Result<Totals, EvidenceError> {
    let stdout = git_stdout(repo, &["diff", "--numstat", from, to])?;
    parse_numstat(&stdout)
}

#[derive(Debug, Default, PartialEq, Eq)]
struct Totals {
    files: u64,
    insertions: u64,
    deletions: u64,
}

fn parse_numstat(bytes: &[u8]) -> Result<Totals, EvidenceError> {
    let mut totals = Totals::default();
    if bytes.is_empty() {
        return Ok(totals);
    }
    let mut start = 0;
    for (index, byte) in bytes.iter().enumerate() {
        if *byte == b'\n' {
            accumulate_numstat(&bytes[start..index], &mut totals)?;
            start = index + 1;
        }
    }
    if start != bytes.len() {
        accumulate_numstat(&bytes[start..], &mut totals)?;
    }
    Ok(totals)
}

fn accumulate_numstat(line: &[u8], totals: &mut Totals) -> Result<(), EvidenceError> {
    if line.is_empty() {
        return Ok(());
    }
    let mut parts = line.splitn(3, |byte| *byte == b'\t');
    let added = parts.next().unwrap_or_default();
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
    Ok(())
}

fn parse_hunk(field: &[u8]) -> Result<u64, EvidenceError> {
    if field == b"-" {
        return Ok(0);
    }
    let text = std::str::from_utf8(field).map_err(|_| numstat_error())?;
    text.parse::<u64>().map_err(|_| numstat_error())
}

fn numstat_error() -> EvidenceError {
    EvidenceError::Failed("git diff --numstat produced an unrecognized line".to_string())
}

fn run_patch_id(repo: &Path, producer_args: &[&str]) -> Result<String, EvidenceError> {
    let mut producer = git(repo);
    producer
        .args(producer_args)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let mut producer = producer.spawn().map_err(|err| spawn_error("git", err))?;
    let Some(stdout) = producer.stdout.take() else {
        let _ = producer.kill();
        let _ = producer.wait();
        return Err(EvidenceError::Failed(
            "git producer has no stdout".to_string(),
        ));
    };
    let producer_stderr = producer.stderr.take();

    let mut consumer = git(repo);
    consumer
        .args(["patch-id", "--stable"])
        .stdin(stdout)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let consumer = match consumer.spawn() {
        Ok(child) => child,
        Err(err) => {
            let _ = producer.kill();
            let _ = producer.wait();
            return Err(spawn_error("git", err));
        }
    };

    let stderr_thread = std::thread::spawn(move || {
        let mut buf = Vec::new();
        if let Some(mut stderr) = producer_stderr {
            let _ = stderr.read_to_end(&mut buf);
        }
        buf
    });
    let consumer_output = consumer.wait_with_output();
    let producer_status = producer.wait();
    let producer_stderr = stderr_thread.join().unwrap_or_default();

    let consumer_output = consumer_output.map_err(|err| spawn_error("git", err))?;
    let producer_status = producer_status.map_err(|err| spawn_error("git", err))?;
    if !producer_status.success() {
        return Err(command_failure(
            "git",
            &producer_args.join(" "),
            producer_status,
            &producer_stderr,
        ));
    }
    if !consumer_output.status.success() {
        return Err(command_failure(
            "git",
            "patch-id --stable",
            consumer_output.status,
            &consumer_output.stderr,
        ));
    }
    parse_patch_id(&consumer_output.stdout)
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
            "git patch-id --stable produced no patch id".to_string(),
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
    let preimage = directive_preimage(bytes)?;
    let Some(embedded) = first_hex_run(bytes, 64) else {
        return Err(EvidenceError::Failed(
            "directive body has no 64-hex value".to_string(),
        ));
    };
    let sha256 = sha256_hex(preimage)?;
    let verdict = if sha256 == embedded {
        Verdict::Match
    } else {
        Verdict::Mismatch
    };
    Ok(Directive { sha256, verdict })
}

/// Drop the first two newline-terminated lines, then one trailing byte.
fn directive_preimage(bytes: &[u8]) -> Result<&[u8], EvidenceError> {
    let mut seen = 0;
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] == b'\n' {
            seen += 1;
            index += 1;
            if seen == 2 {
                break;
            }
        } else {
            index += 1;
        }
    }
    if seen < 2 {
        return Err(EvidenceError::Failed(
            "directive body has fewer than 2 lines".to_string(),
        ));
    }
    let rest = &bytes[index..];
    if rest.is_empty() {
        return Err(EvidenceError::Failed(
            "directive body is missing the trailing byte".to_string(),
        ));
    }
    Ok(&rest[..rest.len() - 1])
}

fn first_hex_run(bytes: &[u8], len: usize) -> Option<String> {
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index].is_ascii_hexdigit() {
            let start = index;
            while index < bytes.len() && bytes[index].is_ascii_hexdigit() {
                index += 1;
            }
            if index - start == len {
                return Some(
                    std::str::from_utf8(&bytes[start..index])
                        .unwrap_or("")
                        .to_ascii_lowercase(),
                );
            }
        } else {
            index += 1;
        }
    }
    None
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
        compute, outcome, parse_args, CompiledPattern, EvidenceError, Field, Options, Parsed,
        Target, Verdict,
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

    fn commit_options(repo: &Path, sha: &str) -> Options {
        Options {
            repo: repo.to_path_buf(),
            target: Target::Commit {
                sha: sha.to_string(),
                base: None,
            },
            directive_file: None,
            expects: Vec::new(),
            json: false,
        }
    }

    fn os(args: &[&str]) -> Vec<OsString> {
        args.iter().map(OsString::from).collect()
    }

    #[test]
    fn commit_hashes_match_real_git_commands() {
        let repo = Repo::new();
        repo.commit("a.txt", b"one\ntwo\nthree\n", "root");
        let head = repo.commit("a.txt", b"one\ntwo\nfour\nfive\n", "edit");
        let evidence = compute(&commit_options(&repo.origin, &head)).unwrap();

        let patch = first_field(&repo.sh("git diff --binary --full-index HEAD^ HEAD | sha256sum"));
        let patch_id = first_field(&repo.sh("git show HEAD | git patch-id --stable"));
        let numstat = repo.sh("git diff --numstat HEAD^ HEAD");
        let (files, insertions, deletions) = sum_numstat(&numstat);

        assert_eq!(evidence.patch_sha256, patch);
        assert_eq!(evidence.patch_id, patch_id);
        assert_eq!(
            (evidence.files, evidence.insertions, evidence.deletions),
            (files, insertions, deletions)
        );
        assert_eq!((files, insertions, deletions), (1, 2, 1));
        assert!(evidence.directive.is_none());
    }

    #[test]
    fn base_override_changes_patch_bytes_and_keeps_show_patch_id() {
        let repo = Repo::new();
        let root = repo.commit("a.txt", b"a\n", "root");
        repo.commit("a.txt", b"b\n", "mid");
        let head = repo.commit("a.txt", b"c\n", "head");
        let mut options = commit_options(&repo.origin, &head);
        options.target = Target::Commit {
            sha: head.clone(),
            base: Some(root.clone()),
        };
        let evidence = compute(&options).unwrap();

        let ranged = first_field(&repo.sh(&format!(
            "git diff --binary --full-index {root} {head} | sha256sum"
        )));
        let parent_only =
            first_field(&repo.sh("git diff --binary --full-index HEAD^ HEAD | sha256sum"));
        let show_id = first_field(&repo.sh(&format!("git show {head} | git patch-id --stable")));
        assert_eq!(evidence.patch_sha256, ranged);
        assert_ne!(evidence.patch_sha256, parent_only);
        assert_eq!(evidence.patch_id, show_id);
    }

    #[test]
    fn range_patch_id_matches_git_diff_not_show() {
        let repo = Repo::new();
        let root = repo.commit("a.txt", b"a\n", "root");
        repo.commit("a.txt", b"b\n", "mid");
        let head = repo.commit("a.txt", b"c\n", "head");
        let mut options = commit_options(&repo.origin, &head);
        options.target = Target::Range {
            base: root.clone(),
            head: head.clone(),
        };
        let evidence = compute(&options).unwrap();

        let diff_id =
            first_field(&repo.sh(&format!("git diff {root} {head} | git patch-id --stable")));
        let show_id = first_field(&repo.sh(&format!("git show {head} | git patch-id --stable")));
        let patch = first_field(&repo.sh(&format!(
            "git diff --binary --full-index {root} {head} | sha256sum"
        )));
        let (files, insertions, deletions) =
            sum_numstat(&repo.sh(&format!("git diff --numstat {root} {head}")));
        assert_eq!(evidence.patch_id, diff_id);
        assert_ne!(evidence.patch_id, show_id);
        assert_eq!(evidence.patch_sha256, patch);
        assert_eq!(
            (evidence.files, evidence.insertions, evidence.deletions),
            (files, insertions, deletions)
        );
    }

    #[test]
    fn binary_numstat_counts_the_file_and_zero_lines() {
        let repo = Repo::new();
        repo.commit("a.txt", b"text\n", "root");
        let head = repo.commit("b.bin", &[0, 1, 2, 255], "binary");
        let evidence = compute(&commit_options(&repo.origin, &head)).unwrap();
        let numstat = repo.sh("git diff --numstat HEAD^ HEAD");
        assert!(numstat.contains('-'), "{numstat}");
        let (files, insertions, deletions) = sum_numstat(&numstat);
        assert_eq!(
            (evidence.files, evidence.insertions, evidence.deletions),
            (files, insertions, deletions)
        );
        assert_eq!((files, insertions, deletions), (1, 0, 0));
    }

    #[test]
    fn unknown_sha_and_missing_root_parent_fail_closed() {
        let repo = Repo::new();
        let root = repo.commit("a.txt", b"one\n", "root");
        let unknown =
            compute(&commit_options(&repo.origin, "definitely-not-a-commit")).unwrap_err();
        assert!(unknown.to_string().contains("unknown commit"), "{unknown}");
        assert_eq!(unknown.exit_code(), 1);

        let missing = compute(&commit_options(&repo.origin, &root)).unwrap_err();
        let message = missing.to_string();
        assert!(message.contains("parent"), "{message}");
        assert!(message.contains("missing"), "{message}");
        assert!(!message.contains("shallow"), "{message}");
        assert_eq!(missing.exit_code(), 1);

        let empty = repo.root.join("empty");
        std::fs::create_dir_all(&empty).unwrap();
        let not_repo = compute(&commit_options(&empty, "HEAD")).unwrap_err();
        assert!(
            not_repo.to_string().contains("not a git repository"),
            "{not_repo}"
        );
    }

    #[test]
    fn shallow_depth_one_fails_and_depth_two_matches_git() {
        let repo = Repo::new();
        repo.commit("a.txt", b"a\n", "root");
        let mid = repo.commit("a.txt", b"b\n", "mid");
        let head = repo.commit("a.txt", b"c\n", "head");

        let depth_one = repo.clone_depth(1);
        let err = compute(&commit_options(&depth_one, "HEAD")).unwrap_err();
        let message = err.to_string();
        assert!(message.contains("shallow"), "{message}");
        assert!(message.contains("parent"), "{message}");
        assert_eq!(err.exit_code(), 1);

        let depth_two = repo.clone_depth(2);
        assert_eq!(
            sh(&depth_two, "git rev-parse --is-shallow-repository").trim(),
            "true"
        );
        let evidence = compute(&commit_options(&depth_two, &head)).unwrap();
        let patch = first_field(&repo.sh(&format!(
            "git diff --binary --full-index {mid} {head} | sha256sum"
        )));
        let patch_id = first_field(&repo.sh(&format!("git show {head} | git patch-id --stable")));
        assert_eq!(evidence.patch_sha256, patch);
        assert_eq!(evidence.patch_id, patch_id);
    }

    #[test]
    fn empty_commit_has_no_patch_id() {
        let repo = Repo::new();
        repo.commit("a.txt", b"a\n", "root");
        repo.git(&["commit", "--allow-empty", "-m", "empty"]);
        let err = compute(&commit_options(&repo.origin, "HEAD")).unwrap_err();
        assert!(err.to_string().contains("no patch id"), "{err}");
        assert_eq!(err.exit_code(), 1);
    }

    #[test]
    fn directive_match_ignores_header_and_trailing_byte() {
        let repo = Repo::new();
        repo.commit("a.txt", b"a\n", "root");
        let head = repo.commit("a.txt", b"b\n", "edit");
        let preimage = b"alpha\nbeta";
        let expected = oracle_sha256(preimage);
        let mut body = Vec::new();
        body.extend(b"evidence directive\n");
        body.extend(format!("hash: {expected}\n").into_bytes());
        body.extend(preimage);
        body.push(b'\n');
        let path = repo.root.join("directive.txt");
        std::fs::write(&path, &body).unwrap();

        let mut options = commit_options(&repo.origin, &head);
        options.directive_file = Some(path);
        let evidence = compute(&options).unwrap();
        let directive = evidence.directive.unwrap();
        assert_eq!(directive.sha256, expected);
        assert_eq!(directive.verdict, Verdict::Match);

        let flipped = body.len() - 2;
        body[flipped] ^= 0x01;
        let mismatch_path = repo.root.join("mismatch.txt");
        std::fs::write(&mismatch_path, &body).unwrap();
        options.directive_file = Some(mismatch_path);
        let mismatch = compute(&options).unwrap().directive.unwrap();
        assert_eq!(mismatch.verdict, Verdict::Mismatch);
        assert_ne!(mismatch.sha256, expected);

        let result = outcome(&options);
        assert_eq!(result.exit, 0, "{}", result.stderr);
        assert!(result.stdout.contains("directive: MISMATCH"));
    }

    #[test]
    fn unreadable_or_incomplete_directive_fails_closed() {
        let repo = Repo::new();
        repo.commit("a.txt", b"a\n", "root");
        let head = repo.commit("a.txt", b"b\n", "edit");
        let missing = repo.root.join("no-such-directive");
        let mut options = commit_options(&repo.origin, &head);
        options.directive_file = Some(missing);
        let err = compute(&options).unwrap_err();
        assert!(
            err.to_string().contains("unreadable directive file"),
            "{err}"
        );
        assert_eq!(err.exit_code(), 1);

        let short = repo.root.join("short.txt");
        std::fs::write(&short, b"only one line\n").unwrap();
        options.directive_file = Some(short);
        let err = compute(&options).unwrap_err();
        assert!(err.to_string().contains("fewer than 2 lines"), "{err}");

        let no_hex = repo.root.join("no-hex.txt");
        std::fs::write(&no_hex, b"title\nbody without a hash\nrest\n").unwrap();
        options.directive_file = Some(no_hex);
        let err = compute(&options).unwrap_err();
        assert!(err.to_string().contains("no 64-hex value"), "{err}");
        let result = outcome(&options);
        assert_eq!(result.exit, 1);
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
            assert!(err.to_string().contains("ambiguous --expect"), "{err}");
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

        let three_dot = parse_args(os(&["HEAD...main"])).unwrap_err();
        assert!(three_dot.to_string().contains("three-dot"), "{three_dot}");
        assert_eq!(three_dot.exit_code(), 2);
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

        for pattern in [&ascii, &unicode, &upper_id] {
            let parsed = parse_args(os(&[
                "--repo",
                repo.origin.to_str().unwrap(),
                "--sha",
                &head,
                "--expect",
                pattern,
            ]))
            .unwrap();
            let Parsed::Run(options) = parsed else {
                panic!("help");
            };
            let result = outcome(&options);
            assert_eq!(result.exit, 0, "{pattern}: {}", result.stderr);
            assert!(result.stdout.contains(&evidence.patch_sha256));
        }

        let wrong = format!("patch-sha256={prefix}...00000000");
        let parsed = parse_args(os(&[
            "--repo",
            repo.origin.to_str().unwrap(),
            "--sha",
            &head,
            "--expect",
            &wrong,
            "--json",
        ]))
        .unwrap();
        let Parsed::Run(options) = parsed else {
            panic!("help");
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
        assert!(result.stdout.ends_with('\n'));
        assert_eq!(result.stdout.lines().count(), 1);
    }

    #[test]
    fn text_output_is_one_line_per_value() {
        let repo = Repo::new();
        repo.commit("a.txt", b"one\ntwo\nthree\n", "root");
        let head = repo.commit("a.txt", b"one\ntwo\nfour\nfive\n", "edit");
        let result = outcome(&commit_options(&repo.origin, &head));
        assert_eq!(result.exit, 0, "{}", result.stderr);
        let lines: Vec<_> = result.stdout.lines().collect();
        assert_eq!(
            lines,
            vec![
                lines[0],
                lines[1],
                "files: 1",
                "insertions: 2",
                "deletions: 1",
            ]
        );
        assert!(lines[0].starts_with("patch-sha256: "));
        assert_eq!(lines[0].split_whitespace().nth(1).unwrap().len(), 64);
        assert!(lines[1].starts_with("patch-id: "));
        assert_eq!(lines[1].split_whitespace().nth(1).unwrap().len(), 40);
    }

    #[test]
    fn help_and_missing_revision_are_usage() {
        assert_eq!(parse_args(os(&["--help"])), Ok(Parsed::Help));
        assert_eq!(parse_args(os(&["-h"])), Ok(Parsed::Help));
        let missing = parse_args(os(&["--json"])).unwrap_err();
        assert!(missing.to_string().contains("missing commit"), "{missing}");
        assert_eq!(missing.exit_code(), 2);
        let both = parse_args(os(&["--sha", "HEAD", "--head", "HEAD"])).unwrap_err();
        assert!(both.to_string().contains("ambiguous"), "{both}");
    }

    #[test]
    fn abbrev_pattern_stores_lowercase_hex() {
        let parsed = parse_args(os(&["--sha", "HEAD", "--expect", "patch-id=ABCD…0123"])).unwrap();
        let Parsed::Run(options) = parsed else {
            panic!("help");
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
}
