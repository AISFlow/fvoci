//! The end of `cargo xtask test`'s summary: the time of every unit, the tests that
//! failed with the command that runs them again, and the tests that were flaky.

use std::borrow::Cow;
use std::collections::BTreeMap;
use std::fmt::Write as _;
use std::time::Duration;

use super::harness::UnitRun;
use super::shards;
use super::units::DOC;
use crate::times::{Scope, TestName, UnitTime, Verdict};

/// One unit after its turn.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct UnitResult {
    /// The database's name as `--database` takes it.
    pub(crate) database: &'static str,
    pub(crate) unit: String,
    pub(crate) scope: Scope,
    /// Expected `passed + failed + ignored` count when this unit is a `--part` subunit.
    pub(crate) expected_tests: Option<usize>,
    pub(crate) run: UnitRun,
}

impl UnitResult {
    /// Returns `(counted, expected)` when a whole, non-interrupted subunit run reported a
    /// different total test count (`passed + failed + ignored`) than its `--list` slice.
    pub(crate) fn wrong_test_count(&self) -> Option<(u64, u64)> {
        if self.run.interrupted || self.scope != Scope::Whole {
            return None;
        }
        let want = u64::try_from(self.expected_tests?).ok()?;
        let report = &self.run.report;
        let counted = report
            .passed
            .saturating_add(report.failed)
            .saturating_add(report.ignored);
        (counted != want).then_some((counted, want))
    }

    /// Whether it failed: its command failed, its harness named a failure, a test binary
    /// of it never reported, or a subunit ran fewer/more tests than its slice listed.
    pub(crate) fn failed(&self) -> bool {
        let report = &self.run.report;
        !self.run.succeeded
            || !report.failed_tests.is_empty()
            || !report.every_binary_finished()
            || self.wrong_test_count().is_some()
    }

    /// Passed, failed, or interrupted: a signal that asks to stop (Ctrl-C) ended its
    /// command.
    pub(crate) fn verdict(&self) -> Verdict {
        if self.run.interrupted {
            Verdict::Interrupted
        } else if self.failed() {
            Verdict::Failed
        } else {
            Verdict::Passed
        }
    }

    /// Whether every test binary of it is accounted for: no signal stopped it (the
    /// binaries after the one it stopped in did not start), every binary that started
    /// reported, and a command that failed had cargo count its failed binaries, no more
    /// than those whose result was a failure (none that died after its result, no build
    /// that failed). Without the count (cargo killed) they are not known.
    pub(crate) fn every_binary_accounted_for(&self) -> bool {
        let report = &self.run.report;
        !self.run.interrupted
            && report.every_binary_finished()
            && (self.run.succeeded
                || self
                    .run
                    .failed_targets
                    .is_some_and(|targets| targets > 0 && targets <= report.failed_binaries))
    }

    /// Whether the names of its failed tests are all its failures: every binary is
    /// accounted for, any subunit test-count invariant matched, and the harness named as
    /// many failures as it counted.
    fn every_failure_named(&self) -> bool {
        self.wrong_test_count().is_none()
            && self.every_binary_accounted_for()
            && self.run.report.names_every_failure()
    }

    /// Whether its failures run again by name. Documentation tests do not: rustdoc
    /// cuts the arguments for their harness at spaces, and every name of one has some.
    fn reruns_by_name(&self) -> bool {
        self.unit != DOC && !self.run.report.failed_tests.is_empty() && self.every_failure_named()
    }

    pub(crate) fn time(&self) -> UnitTime {
        let report = &self.run.report;
        UnitTime {
            database: self.database.to_owned(),
            unit: self.unit.clone(),
            scope: self.scope,
            verdict: self.verdict(),
            seconds: self.run.elapsed.as_secs_f64(),
            passed: report.passed,
            failed: report.failed,
            ignored: report.ignored,
            failed_tests: report.failed_tests.clone(),
            every_failure_named: self.every_failure_named(),
        }
    }
}

/// The commands that run the failures of `results` again: for each database, one for
/// the tests named (every unit given runs every name, so a name also runs in a unit
/// that has a test of that name and passed it), and one for the units whose failures
/// were not all named, or are documentation tests, which run whole.
pub(crate) fn reruns(results: &[UnitResult]) -> Vec<String> {
    let mut databases: Vec<&str> = Vec::new();
    for result in results.iter().filter(|result| result.failed()) {
        if !databases.contains(&result.database) {
            databases.push(result.database);
        }
    }
    let mut commands = Vec::new();
    for database in databases {
        let failed = results
            .iter()
            .filter(|result| result.database == database && result.failed());
        let (named, whole): (Vec<&UnitResult>, Vec<&UnitResult>) =
            failed.partition(|result| result.reruns_by_name());
        let command = |units: &[&UnitResult]| {
            let mut command = format!("cargo xtask test --database {database}");
            let mut seen: Vec<&str> = Vec::new();
            for result in units {
                let unit = shards::base_name(&result.unit);
                if !seen.contains(&unit) {
                    seen.push(unit);
                    command.push_str(" --unit ");
                    command.push_str(unit);
                }
            }
            command
        };
        if !named.is_empty() {
            let mut line = command(&named);
            line.push_str(" -- --exact");
            let mut given: Vec<&str> = Vec::new();
            for name in named
                .iter()
                .flat_map(|result| &result.run.report.failed_tests)
            {
                if !given.contains(&name.as_str()) {
                    given.push(name);
                    line.push(' ');
                    line.push_str(&shell_word(name));
                }
            }
            commands.push(line);
        }
        if !whole.is_empty() {
            commands.push(command(&whole));
        }
    }
    commands
}

/// The tests that passed in `results` and failed before, in `earlier`, with when the
/// run they failed in started; and the units that failed before without naming every
/// failure and passed whole now.
pub(crate) fn flaky(
    results: &[UnitResult],
    earlier: &BTreeMap<TestName, u64>,
) -> Vec<(TestName, u64)> {
    let mut flaky = Vec::new();
    for result in results {
        let name = |test: Option<&String>| TestName {
            database: result.database.to_owned(),
            unit: match test {
                Some(_) => shards::base_name(&result.unit).to_owned(),
                None => result.unit.clone(),
            },
            test: test.cloned(),
        };
        let whole = name(None);
        if result.scope == Scope::Whole && !result.failed() {
            if let Some(&started) = earlier.get(&whole) {
                flaky.push((whole, started));
            }
            if !result.unit.contains('/') {
                for (earlier_name, &started) in earlier {
                    if earlier_name.database == result.database
                        && earlier_name.test.is_none()
                        && earlier_name.unit != result.unit
                        && shards::base_name(&earlier_name.unit) == result.unit
                    {
                        flaky.push((earlier_name.clone(), started));
                    }
                }
            }
        }
        let report = &result.run.report;
        // A name can be in two binaries of a unit and fail in one, or not be reached
        // in one that died: a pass of it in the other is no pass of the test.
        let passes = report
            .passed_tests
            .iter()
            .filter(|test| report.every_binary_finished() && !report.failed_tests.contains(test));
        for test in passes {
            let named = name(Some(test));
            if let Some(&started) = earlier.get(&named) {
                flaky.push((named, started));
            }
        }
    }
    flaky
}

/// The lines after the steps' summary.
pub(crate) fn lines(results: &[UnitResult], flaky: &[(TestName, u64)], now_unix: u64) -> String {
    let mut text = String::new();
    if !results.is_empty() {
        text.push_str("\ntime by unit, slowest first\n");
        let mut slowest: Vec<&UnitResult> = results.iter().collect();
        slowest.sort_by_key(|result| std::cmp::Reverse(result.run.elapsed));
        for result in slowest {
            let report = &result.run.report;
            let _ = writeln!(
                text,
                "  {:>9}  {}  {:<8}  {}  ({} passed, {} failed, {} ignored)",
                clock(result.run.elapsed),
                match result.verdict() {
                    Verdict::Passed => "PASS",
                    Verdict::Interrupted => "STOP",
                    Verdict::Failed | Verdict::Skipped => "FAIL",
                },
                result.database,
                result.unit,
                report.passed,
                report.failed,
                report.ignored
            );
        }
    }
    let failed: Vec<&UnitResult> = results.iter().filter(|result| result.failed()).collect();
    if !failed.is_empty() {
        text.push_str("\nfailed tests\n");
        for result in failed {
            let _ = writeln!(text, "  {} {}:", result.database, result.unit);
            for name in &result.run.report.failed_tests {
                let _ = writeln!(text, "    {name}");
            }
            if result.run.interrupted {
                text.push_str("    (stopped before its end: it runs again whole)\n");
            } else if let Some((counted, want)) = result.wrong_test_count() {
                let _ = writeln!(
                    text,
                    "    (expected {want} tests in this shard, harness counted {counted}: \
                     it runs again whole)"
                );
            } else if result.unit == DOC {
                text.push_str(
                    "    (rustdoc cuts a name at its spaces: the documentation tests run \
                     again whole)\n",
                );
            } else if !result.reruns_by_name() {
                text.push_str(
                    "    (a failure has no name, a test binary never reported, or cargo's \
                     count of failed binaries was not read: see the output above; it runs \
                     again whole)\n",
                );
            }
        }
        text.push_str("run them again:\n");
        for command in reruns(results) {
            let _ = writeln!(text, "  {command}");
        }
    }
    if !flaky.is_empty() {
        text.push_str(
            "\nflaky: failed in an earlier run of this same source, passed in this one \
             (the earlier run stays failed)\n",
        );
        for (name, started) in flaky {
            let _ = writeln!(
                text,
                "  {} {}: {}  (failed {} ago)",
                name.database,
                name.unit,
                name.test.as_deref().unwrap_or("(the whole unit)"),
                clock(Duration::from_secs(now_unix.saturating_sub(*started)))
            );
        }
    }
    text
}

/// A span as `1h02m`, `3m07.2s` or `4.1s`.
fn clock(span: Duration) -> String {
    let seconds = span.as_secs();
    if seconds >= 3600 {
        format!("{}h{:02}m", seconds / 3600, seconds % 3600 / 60)
    } else if seconds >= 60 {
        format!("{}m{:04.1}s", seconds / 60, span.as_secs_f64() % 60.0)
    } else {
        format!("{:.1}s", span.as_secs_f64())
    }
}

/// `word` as one word of a POSIX shell's command line.
fn shell_word(word: &str) -> Cow<'_, str> {
    let plain = !word.is_empty()
        && word
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || "_-:./=+,@%".contains(c));
    if plain {
        Cow::Borrowed(word)
    } else {
        Cow::Owned(format!("'{}'", word.replace('\'', r"'\''")))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test::harness::Report;

    /// How the command of a unit exited.
    #[derive(Clone, Copy)]
    enum Exit {
        Success,
        Failure,
    }

    fn result(database: &'static str, unit: &str, exit: Exit, report: Report) -> UnitResult {
        UnitResult {
            database,
            unit: unit.to_owned(),
            scope: Scope::Whole,
            expected_tests: None,
            run: UnitRun {
                succeeded: matches!(exit, Exit::Success),
                interrupted: false,
                // As cargo counts them: the binaries that failed, one at least.
                failed_targets: match exit {
                    Exit::Success => None,
                    Exit::Failure => Some(report.failed_binaries.max(1)),
                },
                report,
                elapsed: Duration::from_millis(83_400),
            },
        }
    }

    /// A report of one binary that ran to its end.
    fn finished(passed: &[&str], failed: &[&str]) -> Report {
        let count = |names: &[&str]| u64::try_from(names.len()).expect("a few names");
        Report {
            started: 1,
            finished: 1,
            passed: count(passed),
            failed: count(failed),
            failed_binaries: u64::from(!failed.is_empty()),
            failed_tests: failed.iter().map(|&name| name.to_owned()).collect(),
            passed_tests: passed.iter().map(|&name| name.to_owned()).collect(),
            ..Report::default()
        }
    }

    fn test_name(database: &str, unit: &str, test: Option<&str>) -> TestName {
        TestName {
            database: database.to_owned(),
            unit: unit.to_owned(),
            test: test.map(str::to_owned),
        }
    }

    #[test]
    fn one_command_per_database_runs_every_named_failure_again() {
        let results = [
            result(
                "postgres",
                "rooms",
                Exit::Failure,
                finished(&[], &["rooms::a", "rooms::b"]),
            ),
            result(
                "postgres",
                "web",
                Exit::Failure,
                finished(&["web::c"], &["web::d"]),
            ),
            result("postgres", "api", Exit::Success, finished(&["api::e"], &[])),
            result(
                "sqlite",
                "doc",
                Exit::Failure,
                finished(&[], &["src/lib.rs - f (line 3)"]),
            ),
        ];
        assert_eq!(
            reruns(&results),
            [
                "cargo xtask test --database postgres --unit rooms --unit web -- --exact \
                 rooms::a rooms::b web::d",
                "cargo xtask test --database sqlite --unit doc",
            ],
            "a unit that passed is left out; documentation tests run again whole"
        );
        let text = lines(&results, &[], 0);
        assert!(
            text.contains("the documentation tests run again whole"),
            "{text}"
        );
    }

    #[test]
    fn a_name_with_spaces_is_one_word_of_the_command() {
        let results = [result(
            "sqlite",
            "regression",
            Exit::Failure,
            finished(&[], &["case it's a"]),
        )];
        assert_eq!(
            reruns(&results),
            [r"cargo xtask test --database sqlite --unit regression -- --exact 'case it'\''s a'"]
        );
    }

    #[test]
    fn a_unit_whose_failures_are_not_all_named_runs_again_whole() {
        let crashed = result("sqlite", "api", Exit::Failure, Report::default());
        let short = result(
            "sqlite",
            "web",
            Exit::Failure,
            Report {
                failed: 2,
                ..finished(&[], &["web::a"])
            },
        );
        let died = result(
            "sqlite",
            "libs",
            Exit::Failure,
            Report {
                started: 2,
                ..finished(&[], &["libs::a"])
            },
        );
        // Every test passed and the command failed: a binary died after its result.
        let after = result("sqlite", "card", Exit::Failure, finished(&["card::a"], &[]));
        let named = result(
            "sqlite",
            "store",
            Exit::Failure,
            finished(&[], &["store::x"]),
        );
        let results = [crashed, short, died, after, named];
        assert_eq!(
            reruns(&results),
            [
                "cargo xtask test --database sqlite --unit store -- --exact store::x",
                "cargo xtask test --database sqlite --unit api --unit web --unit libs --unit card",
            ]
        );
        let text = lines(&results, &[], 0);
        assert_eq!(text.matches("it runs again whole").count(), 4, "{text}");
        let unnamed: Vec<String> = results
            .iter()
            .map(UnitResult::time)
            .filter(|time| !time.every_failure_named)
            .map(|time| time.unit)
            .collect();
        assert_eq!(
            unnamed,
            ["api", "web", "libs", "card"],
            "recorded whole too"
        );
    }

    #[test]
    fn a_unit_fails_on_a_named_failure_a_binary_that_never_reported_or_a_rerun_of_nothing() {
        assert!(result("sqlite", "x", Exit::Success, finished(&[], &["x::a"])).failed());
        let silent = Report {
            started: 1,
            ..Report::default()
        };
        assert!(
            result("sqlite", "x", Exit::Success, silent).failed(),
            "a test ended the process before its binary reported"
        );
        assert!(!result("sqlite", "x", Exit::Success, finished(&["x::a"], &[])).failed());
    }

    #[test]
    fn a_unit_that_was_asked_to_stop_is_interrupted_and_runs_again_whole() {
        let mut stopped = result("sqlite", "x", Exit::Failure, finished(&["x::a"], &["x::b"]));
        stopped.run.interrupted = true;
        assert_eq!(stopped.time().verdict, Verdict::Interrupted);
        let text = lines(std::slice::from_ref(&stopped), &[], 0);
        assert!(text.contains("STOP  sqlite    x"), "{text}");
        assert!(
            text.contains("(stopped before its end: it runs again whole)"),
            "{text}"
        );
        assert_eq!(
            reruns(&[stopped]),
            ["cargo xtask test --database sqlite --unit x"],
            "the binaries after the one it stopped in did not start"
        );
    }

    #[test]
    fn a_binary_cargo_counts_as_failed_beside_one_that_named_its_failure_is_unnamed() {
        // Binary A named `x::b`; binary B said `test result: ok` and then died.
        let mut died = result(
            "sqlite",
            "x",
            Exit::Failure,
            Report {
                started: 2,
                finished: 2,
                ..finished(&["x::a"], &["x::b"])
            },
        );
        died.run.failed_targets = Some(2);
        assert!(!died.time().every_failure_named);
        assert_eq!(
            reruns(std::slice::from_ref(&died)),
            ["cargo xtask test --database sqlite --unit x"]
        );
        died.run.failed_targets = Some(1);
        assert!(
            died.time().every_failure_named,
            "cargo counts the one that named it"
        );
        died.run.failed_targets = None;
        assert!(
            !died.time().every_failure_named,
            "a failed command whose count was not read: cargo killed, or its words changed"
        );
    }

    #[test]
    fn the_summary_lists_units_slowest_first_then_failures_then_flaky_tests() {
        let mut fast = result("sqlite", "libs", Exit::Success, finished(&[], &[]));
        fast.run.elapsed = Duration::from_secs(2);
        let slow = result("sqlite", "api", Exit::Failure, finished(&[], &["api::x"]));
        let flaky = [
            (test_name("sqlite", "web", Some("web::y")), 1000),
            (test_name("sqlite", "doc", None), 1535),
        ];
        let text = lines(&[fast, slow], &flaky, 1600);
        let api = text.find("FAIL  sqlite    api").expect(&text);
        let libs = text.find("PASS  sqlite    libs").expect(&text);
        assert!(api < libs, "slowest first: {text}");
        assert!(text.contains("  1m23.4s  FAIL"), "{text}");
        assert!(
            text.contains("\n  cargo xtask test --database sqlite --unit api -- --exact api::x\n"),
            "{text}"
        );
        assert!(
            text.contains("  sqlite web: web::y  (failed 10m00.0s ago)"),
            "{text}"
        );
        assert!(
            text.contains("  sqlite doc: (the whole unit)  (failed 1m05.0s ago)"),
            "{text}"
        );
    }

    #[test]
    fn a_test_that_passes_now_and_failed_before_is_flaky() {
        let results = [result(
            "postgres",
            "api",
            Exit::Success,
            finished(&["api::x", "api::y"], &[]),
        )];
        let earlier = BTreeMap::from([
            (test_name("postgres", "api", Some("api::y")), 7),
            (test_name("sqlite", "api", Some("api::x")), 8),
        ]);
        let flaky = flaky(&results, &earlier);
        assert_eq!(
            flaky,
            [(test_name("postgres", "api", Some("api::y")), 7)],
            "another database's failure is another test"
        );
    }

    #[test]
    fn a_pass_beside_a_binary_that_died_is_not_flaky() {
        let died = result(
            "sqlite",
            "libs",
            Exit::Failure,
            Report {
                started: 2,
                ..finished(&["tests::a"], &[])
            },
        );
        let earlier = BTreeMap::from([(test_name("sqlite", "libs", Some("tests::a")), 3)]);
        assert_eq!(
            flaky(&[died], &earlier),
            [],
            "the binary that died may have had it too"
        );
    }

    #[test]
    fn a_name_that_passed_in_one_binary_and_failed_in_another_is_not_flaky() {
        let both = result(
            "sqlite",
            "libs",
            Exit::Failure,
            Report {
                started: 2,
                finished: 2,
                ..finished(&["tests::a"], &["tests::a"])
            },
        );
        let earlier = BTreeMap::from([(test_name("sqlite", "libs", Some("tests::a")), 3)]);
        assert_eq!(flaky(&[both], &earlier), []);
    }

    #[test]
    fn a_unit_that_failed_unnamed_is_flaky_only_when_it_passes_whole() {
        let earlier = BTreeMap::from([(test_name("sqlite", "api", None), 5)]);
        let whole = result("sqlite", "api", Exit::Success, finished(&["api::x"], &[]));
        assert_eq!(
            flaky(std::slice::from_ref(&whole), &earlier),
            [(test_name("sqlite", "api", None), 5)]
        );
        let chosen = UnitResult {
            scope: Scope::Chosen,
            ..whole
        };
        assert_eq!(
            flaky(&[chosen], &earlier),
            [],
            "some of its tests passing says nothing of the whole"
        );
    }

    #[test]
    fn failed_subunits_rerun_under_their_target_unit_name_once() {
        let results = [
            result(
                "postgres",
                "api/1",
                Exit::Failure,
                finished(&[], &["boards::a"]),
            ),
            result(
                "postgres",
                "api/2",
                Exit::Failure,
                finished(&[], &["users::b"]),
            ),
        ];
        assert_eq!(
            reruns(&results),
            ["cargo xtask test --database postgres --unit api -- --exact boards::a users::b"]
        );
    }

    #[test]
    fn subunit_named_and_whole_failures_are_flaky_when_rerun_under_base_unit() {
        use crate::times::{self, Record, Source, UnitTime};

        let source = Source {
            commit: "abc123".to_owned(),
            changes: String::new(),
        };
        let shard_records = [
            Record {
                task: "test".to_owned(),
                started_unix: 100,
                source: source.clone(),
                units: vec![UnitTime {
                    database: "postgres".to_owned(),
                    unit: "api/1".to_owned(),
                    verdict: Verdict::Failed,
                    failed_tests: vec!["boards::archive".to_owned()],
                    every_failure_named: true,
                    ..UnitTime::default()
                }],
                ..Record::default()
            },
            Record {
                task: "test".to_owned(),
                started_unix: 200,
                source: source.clone(),
                units: vec![UnitTime {
                    database: "postgres".to_owned(),
                    unit: "api/1".to_owned(),
                    verdict: Verdict::Failed,
                    every_failure_named: false,
                    ..UnitTime::default()
                }],
                ..Record::default()
            },
        ];
        let earlier = times::failures_of(&shard_records, &source);

        // 1) Rerunning `--unit api -- --exact boards::archive` (`Scope::Chosen`) catches
        // the named failure from `api/1`, without clearing the unnamed whole `api/1` failure.
        let named_rerun = UnitResult {
            scope: Scope::Chosen,
            ..result(
                "postgres",
                "api",
                Exit::Success,
                finished(&["boards::archive"], &[]),
            )
        };
        assert_eq!(
            flaky(&[named_rerun], &earlier),
            [(test_name("postgres", "api", Some("boards::archive")), 100)],
            "named rerun on `--unit api` matches failure recorded in `api/1`"
        );

        // 2) `api/2` passing whole does NOT clear `api/1`'s unnamed whole failure.
        let other_shard = result(
            "postgres",
            "api/2",
            Exit::Success,
            finished(&["users::x"], &[]),
        );
        assert_eq!(
            flaky(&[other_shard], &earlier),
            [],
            "another shard passing whole says nothing of `api/1`"
        );

        // 3) `--unit api` passing whole catches `api/1`'s unnamed whole failure.
        let whole_api = result(
            "postgres",
            "api",
            Exit::Success,
            finished(&["boards::y"], &[]),
        );
        assert_eq!(
            flaky(&[whole_api], &earlier),
            [(test_name("postgres", "api/1", None), 200)],
            "whole `--unit api` pass catches unnamed `api/1` failure"
        );
    }

    #[test]
    fn subunit_whose_counted_tests_disagree_with_expected_slice_fails() {
        let mut mismatch = result(
            "postgres",
            "api/1",
            Exit::Success,
            finished(&["boards::a", "boards::b"], &[]),
        );
        mismatch.expected_tests = Some(3);
        assert!(
            mismatch.failed(),
            "2 counted != 3 expected fails the subunit"
        );
        assert_eq!(mismatch.verdict(), Verdict::Failed);
        assert!(!mismatch.time().every_failure_named);
        assert_eq!(
            reruns(std::slice::from_ref(&mismatch)),
            ["cargo xtask test --database postgres --unit api"],
            "mismatched subunit reruns whole under its base unit"
        );
        let summary = lines(std::slice::from_ref(&mismatch), &[], 0);
        assert!(
            summary.contains("expected 3 tests in this shard, harness counted 2"),
            "{summary}"
        );

        // Matching count passes; interrupted run stays Interrupted; Chosen rerun ignores slice count.
        let mut matching = mismatch.clone();
        matching.expected_tests = Some(2);
        assert!(!matching.failed(), "matching count passes");

        let mut stopped = mismatch.clone();
        stopped.run.interrupted = true;
        assert_eq!(stopped.verdict(), Verdict::Interrupted);

        let mut chosen = mismatch;
        chosen.scope = Scope::Chosen;
        assert!(
            !chosen.failed(),
            "Chosen rerun is not held to whole slice count"
        );
    }
}
