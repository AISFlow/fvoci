//! The time file, `target/xtask-times.jsonl`: one line of JSON for every run of
//! `cargo xtask check` and `cargo xtask test`, with how long each step, each build and
//! each unit of tests took, and which tests failed.
//!
//! Lines are only ever added. A test that failed in one run and passed in a later run
//! of the same source stays failed in the first line and is named flaky in the second.
//! `jq -s . target/xtask-times.jsonl` reads the whole file; `cargo xtask test --part`
//! takes such a file for the weights of its parts.

use std::collections::{BTreeMap, HashMap};
use std::fmt::Write as _;
use std::fs::OpenOptions;
use std::io::{Read as _, Seek as _, SeekFrom, Write as _};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{Instant, SystemTime, UNIX_EPOCH};

use anyhow::{Context as _, Result, bail};
use serde::{Deserialize, Serialize};
use sha2::{Digest as _, Sha256};

use crate::steps::{self, Outcome, Ran};

/// Where the time file is, under the workspace root.
pub(crate) const TIMES_FILE: &str = "target/xtask-times.jsonl";

/// One run of a task.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub(crate) struct Record {
    /// `check` or `test`.
    pub(crate) task: String,
    /// When the run started, in seconds since 1970 (UTC).
    pub(crate) started_unix: u64,
    pub(crate) source: Source,
    /// The arguments the task was given.
    pub(crate) arguments: Vec<String>,
    /// What `FVOCI_TEST_CPUS` said, if it was set.
    pub(crate) test_cpus: Option<usize>,
    /// The `RUST_TEST_THREADS` the tests ran with, the caller's or `FVOCI_TEST_CPUS`'s.
    pub(crate) test_threads: Option<String>,
    pub(crate) seconds: f64,
    pub(crate) verdict: Verdict,
    pub(crate) steps: Vec<StepTime>,
    /// The builds of the test binaries, one for each database the run covered.
    pub(crate) builds: Vec<BuildTime>,
    pub(crate) units: Vec<UnitTime>,
    /// Tests that failed in an earlier run of the same source and passed in this one.
    pub(crate) flaky: Vec<TestName>,
}

/// The code a run ran: the commit, and a digest of what was changed and not committed.
#[derive(Debug, Clone, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(default)]
pub(crate) struct Source {
    pub(crate) commit: String,
    /// Empty when nothing was changed.
    pub(crate) changes: String,
}

/// How a step, a unit or a run ended.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub(crate) enum Verdict {
    #[default]
    Passed,
    Failed,
    Skipped,
    Interrupted,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub(crate) struct StepTime {
    pub(crate) name: String,
    pub(crate) verdict: Verdict,
    pub(crate) seconds: f64,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub(crate) struct BuildTime {
    pub(crate) database: String,
    pub(crate) verdict: Verdict,
    pub(crate) seconds: f64,
}

/// How much of a unit ran.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub(crate) enum Scope {
    /// The unit as a run takes it.
    #[default]
    Whole,
    /// The tests that arguments for its harness chose: the names of a rerun.
    Chosen,
}

/// One unit of tests: one `cargo test` of one database.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub(crate) struct UnitTime {
    pub(crate) database: String,
    pub(crate) unit: String,
    pub(crate) scope: Scope,
    pub(crate) verdict: Verdict,
    pub(crate) seconds: f64,
    pub(crate) passed: u64,
    pub(crate) failed: u64,
    pub(crate) ignored: u64,
    pub(crate) failed_tests: Vec<String>,
    /// Whether `failed_tests` are all its failures: false when a signal stopped it, a
    /// test binary never reported, a failure had no name, or the command failed and
    /// cargo's count of failed binaries was not read (cargo killed), or was more than
    /// the binaries whose result was a failure, or none was a failure (a build).
    pub(crate) every_failure_named: bool,
}

/// A test of a unit of a database; with no name, the whole unit.
#[derive(Debug, Clone, Default, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(default)]
pub(crate) struct TestName {
    pub(crate) database: String,
    pub(crate) unit: String,
    pub(crate) test: Option<String>,
}

/// When a run started, by the calendar and by the clock that measures it, and the
/// source it started on.
#[derive(Debug, Clone)]
pub(crate) struct Clock {
    wall: SystemTime,
    started: Instant,
    root: PathBuf,
    source: Source,
}

impl Clock {
    /// Starts the clock of a run in the checkout at `root`.
    pub(crate) fn start(root: &Path) -> Self {
        Self {
            wall: SystemTime::now(),
            started: Instant::now(),
            root: root.to_owned(),
            source: source(root),
        }
    }

    /// The source the run ran: the one it started on, if it is still the checkout's.
    /// Every unit builds what is there when it starts, so a run during which a file
    /// changed ran no one source, and is of none (its commit is empty).
    fn source_ran(&self) -> Source {
        if source(&self.root) == self.source {
            self.source.clone()
        } else {
            steps::say("    the checkout changed during the run: its record is of no source");
            Source::default()
        }
    }
}

impl Record {
    /// The record of a run of `task` that started at `clock`: its source, arguments
    /// and steps, and its verdict from the steps'.
    pub(crate) fn new(task: &str, clock: &Clock, ran: &Ran) -> Self {
        let steps: Vec<StepTime> = ran
            .results()
            .iter()
            .map(|step| StepTime {
                name: step.name.clone(),
                verdict: match step.outcome {
                    Outcome::Passed => Verdict::Passed,
                    Outcome::Failed(_) => Verdict::Failed,
                    Outcome::Skipped(_) => Verdict::Skipped,
                },
                seconds: step.elapsed.as_secs_f64(),
            })
            .collect();
        let verdict = if ran.interrupted() {
            Verdict::Interrupted
        } else if steps.iter().any(|step| step.verdict == Verdict::Failed) {
            Verdict::Failed
        } else {
            Verdict::Passed
        };
        Self {
            task: task.to_owned(),
            started_unix: unix(clock.wall),
            source: clock.source_ran(),
            arguments: std::env::args_os()
                .skip(1)
                .map(|argument| argument.to_string_lossy().into_owned())
                .collect(),
            test_cpus: crate::cpus::test_cpus().ok().flatten().map(usize::from),
            test_threads: crate::cpus::test_threads()
                .ok()
                .flatten()
                .map(|(_, threads)| threads)
                .or_else(|| std::env::var("RUST_TEST_THREADS").ok()),
            seconds: clock.started.elapsed().as_secs_f64(),
            verdict,
            steps,
            ..Self::default()
        }
    }
}

/// The seconds since 1970 (UTC) of `time`.
fn unix(time: SystemTime) -> u64 {
    time.duration_since(UNIX_EPOCH)
        .map_or(0, |since| since.as_secs())
}

/// The seconds since 1970 (UTC) of now.
pub(crate) fn now_unix() -> u64 {
    unix(SystemTime::now())
}

/// The path of the time file of the workspace at `root`.
pub(crate) fn path(root: &Path) -> PathBuf {
    root.join(TIMES_FILE)
}

/// Adds the record of a run to the time file, and says so. A record that cannot be
/// written is said and otherwise left: it changes nothing of the run's verdict.
pub(crate) fn append(root: &Path, record: &Record) {
    let file = path(root);
    match append_to(&file, record) {
        Ok(()) => steps::say(&format!("times: {TIMES_FILE}")),
        Err(error) => steps::say(&format!("times not written: {error:#}")),
    }
}

fn append_to(file: &Path, record: &Record) -> Result<()> {
    let mut line = serde_json::to_string(record).context("writing the record as JSON")?;
    line.push('\n');
    if let Some(directory) = file.parent() {
        std::fs::create_dir_all(directory)
            .with_context(|| format!("creating {}", directory.display()))?;
    }
    append_line(file, line).with_context(|| format!("appending to {}", file.display()))
}

/// Adds `line` at the end of `file`, on a line of its own: after a last line cut short
/// (a run killed as it wrote), which would otherwise take it along.
fn append_line(file: &Path, mut line: String) -> std::io::Result<()> {
    let mut opened = OpenOptions::new()
        .create(true)
        .read(true)
        .append(true)
        .open(file)?;
    if opened.seek(SeekFrom::End(0))? > 0 {
        let mut last = [0_u8];
        opened.seek(SeekFrom::End(-1))?;
        opened.read_exact(&mut last)?;
        if last != *b"\n" {
            line.insert(0, '\n');
        }
    }
    opened.write_all(line.as_bytes())
}

/// The records of a time file, oldest first. A file that is not there has none; a line
/// that is no record (cut short, or of another shape) is passed over.
///
/// # Errors
///
/// Fails if the file is there and cannot be read.
pub(crate) fn read(file: &Path) -> Result<Vec<Record>> {
    let bytes = match std::fs::read(file) {
        Ok(bytes) => bytes,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(error) => {
            return Err(error).with_context(|| format!("reading {}", file.display()));
        }
    };
    // Line by line: a line cut inside a character spoils itself, not the file.
    Ok(bytes
        .split(|byte| *byte == b'\n')
        .filter_map(|line| serde_json::from_slice(line).ok())
        .collect())
}

/// A time file given on the command line, every line of which must be a record of a
/// run of `check` or `test`. Machines that share a run each read their copy, and a copy
/// cut short or of another kind (a JSON error a download saved) would weigh the units
/// otherwise and share them out otherwise.
///
/// # Errors
///
/// Fails if the file cannot be read, holds no record, or holds a line that is not one.
pub(crate) fn read_given(file: &Path) -> Result<Given> {
    let bytes = std::fs::read(file).with_context(|| format!("reading {}", file.display()))?;
    let text = String::from_utf8(bytes)
        .with_context(|| format!("{} is no time file: not UTF-8", file.display()))?;
    let records = text
        .lines()
        .enumerate()
        .filter(|(_, line)| !line.trim().is_empty())
        .map(|(index, line)| {
            let not_one = || format!("{}:{}: not a record of a run", file.display(), index + 1);
            let record: Record = serde_json::from_str(line).with_context(not_one)?;
            // Every field has a default: any JSON object would read as a record.
            if !matches!(record.task.as_str(), "check" | "test") || record.started_unix == 0 {
                bail!(not_one());
            }
            Ok(record)
        })
        .collect::<Result<Vec<Record>>>()?;
    if records.is_empty() {
        bail!("{} holds no record of a run", file.display());
    }
    Ok(Given {
        records,
        sha256: hexadecimal(&Sha256::digest(text.as_bytes())),
    })
}

/// The records of a time file given on the command line.
#[derive(Debug)]
pub(crate) struct Given {
    pub(crate) records: Vec<Record>,
    /// The SHA-256 of the file, for machines that share a run to compare.
    pub(crate) sha256: String,
}

/// How long each unit of `database` took, from the latest test run that ran it whole
/// and passed: a rerun of some of its tests, a unit cut short or one whose tests
/// failed fast says little of how long it takes.
pub(crate) fn unit_seconds(records: &[Record], database: &str) -> HashMap<String, f64> {
    let mut seconds = HashMap::new();
    for unit in records
        .iter()
        .filter(|record| record.task == "test")
        .flat_map(|record| &record.units)
        .filter(|unit| {
            unit.database == database
                && unit.scope == Scope::Whole
                && unit.verdict == Verdict::Passed
                && unit.seconds.is_finite()
        })
    {
        seconds.insert(unit.unit.clone(), unit.seconds);
    }
    seconds
}

/// The tests that failed in the test runs of `source`, each with when the latest of
/// those runs started. A unit that failed without naming every failure is there whole
/// too, as a test with no name. A unit that was interrupted failed nothing (the units
/// of its run before it did), and without a commit no run is known to be of the same
/// source.
pub(crate) fn failures_of(records: &[Record], source: &Source) -> BTreeMap<TestName, u64> {
    let mut failures = BTreeMap::new();
    if source.commit.is_empty() {
        return failures;
    }
    for record in records
        .iter()
        .filter(|record| record.task == "test" && record.source == *source)
    {
        for unit in record
            .units
            .iter()
            .filter(|unit| unit.verdict == Verdict::Failed)
        {
            let name = |test: Option<&String>| TestName {
                database: unit.database.clone(),
                unit: match test {
                    Some(_) => crate::test::base_unit_name(&unit.unit).to_owned(),
                    None => unit.unit.clone(),
                },
                test: test.cloned(),
            };
            if !unit.every_failure_named || unit.failed_tests.is_empty() {
                failures.insert(name(None), record.started_unix);
            }
            for test in &unit.failed_tests {
                failures.insert(name(Some(test)), record.started_unix);
            }
        }
    }
    failures
}

/// The source of the checkout at `root`. Without git, outside a repository, or if git
/// cannot say what changed, the commit is empty, and no two runs are known to be of
/// one source.
pub(crate) fn source(root: &Path) -> Source {
    let git = |arguments: &[&str]| {
        Command::new("git")
            .args(arguments)
            .current_dir(root)
            .output()
            .ok()
            .filter(|output| output.status.success())
            .map(|output| output.stdout)
    };
    let (Some(commit), Some(status), Some(diff), Some(untracked)) = (
        git(&["rev-parse", "HEAD"]),
        // What a run of the same commit may differ by: the changes to tracked files,
        // and the files git does not track yet (a new test is one), by name and content.
        git(&["status", "--porcelain=v1", "-z", "--untracked-files=all"]),
        git(&["diff", "HEAD", "--binary"]),
        git(&["ls-files", "-z", "--others", "--exclude-standard"]),
    ) else {
        return Source::default();
    };
    let changes = if status.is_empty() {
        String::new()
    } else {
        let mut hasher = Sha256::new().chain_update(&status).chain_update(&diff);
        for file in untracked
            .split(|&byte| byte == 0)
            .filter(|file| !file.is_empty())
        {
            hasher.update(file);
            hasher.update([0]);
            hasher.update(std::fs::read(root.join(file_name(file))).unwrap_or_default());
            hasher.update([0]);
        }
        hexadecimal(&hasher.finalize())
    };
    Source {
        commit: String::from_utf8_lossy(&commit).trim().to_owned(),
        changes,
    }
}

/// A path git gave as bytes (elsewhere than on Unix, read as UTF-8).
fn file_name(bytes: &[u8]) -> PathBuf {
    #[cfg(unix)]
    {
        use std::os::unix::ffi::OsStrExt as _;
        PathBuf::from(std::ffi::OsStr::from_bytes(bytes))
    }
    #[cfg(not(unix))]
    {
        PathBuf::from(String::from_utf8_lossy(bytes).into_owned())
    }
}

/// Bytes in lowercase hexadecimal.
fn hexadecimal(bytes: &[u8]) -> String {
    bytes.iter().fold(String::new(), |mut text, byte| {
        let _ = write!(text, "{byte:02x}");
        text
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn unit(database: &str, name: &str, seconds: f64, failed_tests: &[&str]) -> UnitTime {
        UnitTime {
            database: database.to_owned(),
            unit: name.to_owned(),
            seconds,
            verdict: if failed_tests.is_empty() {
                Verdict::Passed
            } else {
                Verdict::Failed
            },
            failed_tests: failed_tests.iter().map(|&test| test.to_owned()).collect(),
            every_failure_named: true,
            ..UnitTime::default()
        }
    }

    fn run(started_unix: u64, commit: &str, units: Vec<UnitTime>) -> Record {
        Record {
            task: "test".to_owned(),
            started_unix,
            source: Source {
                commit: commit.to_owned(),
                changes: String::new(),
            },
            units,
            ..Record::default()
        }
    }

    #[test]
    fn records_are_added_and_read_back_and_a_broken_line_is_passed_over() {
        let directory = crate::files::testing::scratch("times-append");
        let file = directory.join("times.jsonl");
        assert_eq!(read(&file).expect("no file"), [], "no file, no records");

        let first = run(1, "a", vec![unit("sqlite", "api", 3.5, &["api::x"])]);
        let second = run(2, "a", vec![]);
        let third = run(3, "a", vec![]);
        append_to(&file, &first).expect("written");
        append_to(&file, &second).expect("written");
        let text = std::fs::read_to_string(&file).expect("read as text");
        assert_eq!(text.lines().count(), 2, "a line each, none blank: {text:?}");
        // Cut short inside a character, as a run killed as it wrote would leave it.
        std::fs::OpenOptions::new()
            .append(true)
            .open(&file)
            .and_then(|mut opened| opened.write_all(b"{\"task\": \"\xea\xb0"))
            .expect("a line cut short");
        append_to(&file, &third).expect("written");
        assert_eq!(
            read(&file).expect("read"),
            [first, second, third],
            "every record, in order, without the broken line"
        );
        let _ = std::fs::remove_dir_all(&directory);
    }

    #[test]
    fn a_unit_weighs_what_the_latest_run_of_its_database_took() {
        let records = [
            run(
                1,
                "a",
                vec![
                    unit("sqlite", "api", 10.0, &[]),
                    unit("sqlite", "web", 2.0, &[]),
                ],
            ),
            run(
                2,
                "b",
                vec![
                    unit("sqlite", "api", 12.0, &[]),
                    unit("postgres", "web", 9.0, &[]),
                ],
            ),
            Record {
                task: "check".to_owned(),
                units: vec![unit("sqlite", "web", 99.0, &[])],
                ..Record::default()
            },
            run(
                3,
                "b",
                vec![
                    UnitTime {
                        scope: Scope::Chosen,
                        ..unit("sqlite", "api", 4.0, &[])
                    },
                    unit("sqlite", "web", 0.5, &["web::x"]),
                ],
            ),
            Record {
                verdict: Verdict::Interrupted,
                ..run(
                    4,
                    "b",
                    vec![UnitTime {
                        verdict: Verdict::Interrupted,
                        ..unit("sqlite", "api", 1.0, &[])
                    }],
                )
            },
        ];
        let seconds = unit_seconds(&records, "sqlite");
        assert_eq!(
            seconds.get("api"),
            Some(&12.0),
            "the latest run counts, not a rerun of some tests nor a run cut short"
        );
        assert_eq!(
            seconds.get("web"),
            Some(&2.0),
            "another database or task, or a run where it failed, does not"
        );
        assert_eq!(seconds.len(), 2, "{seconds:?}");
    }

    #[test]
    fn failures_count_only_from_runs_of_the_same_source() {
        let crashed = UnitTime {
            verdict: Verdict::Failed,
            every_failure_named: false,
            ..unit("sqlite", "web", 1.0, &[])
        };
        let partly = UnitTime {
            every_failure_named: false,
            ..unit("sqlite", "libs", 1.0, &["libs::a"])
        };
        let records = [
            run(
                1,
                "a",
                vec![unit("sqlite", "api", 1.0, &["api::x"]), crashed, partly],
            ),
            run(2, "b", vec![unit("sqlite", "api", 1.0, &["api::y"])]),
            Record {
                verdict: Verdict::Interrupted,
                ..run(
                    4,
                    "a",
                    vec![
                        unit("sqlite", "card", 1.0, &["card::q"]),
                        UnitTime {
                            verdict: Verdict::Interrupted,
                            ..unit("sqlite", "store", 1.0, &["store::z"])
                        },
                    ],
                )
            },
            run(
                3,
                "a",
                vec![
                    unit("postgres", "api/1", 1.0, &["api::x"]),
                    unit("sqlite", "doc", 1.0, &[]),
                ],
            ),
        ];
        let source = Source {
            commit: "a".to_owned(),
            changes: String::new(),
        };
        let failures: Vec<(String, u64)> = failures_of(&records, &source)
            .into_iter()
            .map(|(name, started)| {
                (
                    format!(
                        "{}/{}/{}",
                        name.database,
                        name.unit,
                        name.test.unwrap_or_default()
                    ),
                    started,
                )
            })
            .collect();
        assert_eq!(
            failures,
            [
                ("postgres/api/api::x".to_owned(), 3),
                ("sqlite/api/api::x".to_owned(), 1),
                ("sqlite/card/card::q".to_owned(), 4),
                ("sqlite/libs/".to_owned(), 1),
                ("sqlite/libs/libs::a".to_owned(), 1),
                ("sqlite/web/".to_owned(), 1)
            ],
            "commit b's failure is of other code; of the interrupted run, the unit before \
             the interrupt failed and the one it cut short did not; a unit that did not name \
             every failure is there whole too"
        );
    }

    #[test]
    fn a_given_time_file_is_read_whole_or_refused() {
        let directory = crate::files::testing::scratch("times-given");
        let file = directory.join("times.jsonl");
        append_to(&file, &run(1, "a", vec![unit("sqlite", "api", 3.0, &[])])).expect("appends");
        let given = read_given(&file).expect("a file of records");
        assert_eq!(given.records.len(), 1);
        assert_eq!(given.sha256.len(), 64);

        std::fs::write(&file, "").expect("writes");
        assert!(read_given(&file).is_err(), "no record: cut short");
        for other in [
            "<html>not found</html>",
            r#"{"message":"Not Found"}"#,
            "{}",
            "[]",
        ] {
            std::fs::write(&file, format!("{other}\n")).expect("writes");
            let error = read_given(&file).expect_err(other);
            assert!(
                error.to_string().contains("times.jsonl:1"),
                "{other}: {error}"
            );
        }
        assert!(read_given(&directory.join("absent")).is_err());
    }

    #[test]
    fn without_a_commit_no_run_is_of_the_same_source() {
        let records = [run(1, "", vec![unit("sqlite", "api", 1.0, &["api::x"])])];
        assert_eq!(failures_of(&records, &Source::default()).len(), 0);
    }

    #[test]
    fn the_source_of_this_checkout_names_its_commit() {
        let source = source(&crate::workspace_root());
        assert_eq!(source.commit.len(), 40, "a full commit hash: {source:?}");
    }
}
