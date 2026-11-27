//! What the test harness (libtest) of a `cargo test` says: how many tests passed, failed
//! and were ignored, and which by name.
//!
//! A unit failed if its command failed, if its harness named a failed test, or if one of
//! its test binaries started and never reported its result (it aborted, was killed, or
//! a test ended its process). Cargo's count of the binaries that failed, on its standard
//! error, tells whether a binary failed that named nothing (one that died after its
//! result); without that count a failed command has not named every failure. Only when
//! every failure was named are the names run again, and never those of documentation
//! tests nor of a unit a stop signal ended ([`super::report`]); else the whole unit is.

use std::borrow::Cow;
use std::io::{BufRead as _, BufReader, IsTerminal as _, Read, Write};
use std::path::Path;
use std::process::{Command, ExitStatus, Stdio};
use std::sync::mpsc;
use std::time::{Duration, Instant};

use anyhow::{Context as _, Result, bail};

/// The tests of a unit, as its harness reported them.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct Report {
    /// The test binaries that said they were running (`running 3 tests`).
    pub(crate) started: u64,
    /// The test binaries that reported their result (`test result:`).
    pub(crate) finished: u64,
    /// The test binaries whose result was a failure (`test result: FAILED.`).
    pub(crate) failed_binaries: u64,
    pub(crate) passed: u64,
    pub(crate) failed: u64,
    pub(crate) ignored: u64,
    /// The tests it named as failed, each once, in the order it named them.
    pub(crate) failed_tests: Vec<String>,
    /// The tests it named as passed.
    pub(crate) passed_tests: Vec<String>,
    /// Every test it named in a line of its own (`test x ... `), whatever followed: a
    /// test that prints with its output not captured has its output there.
    pub(crate) seen_tests: Vec<String>,
}

impl Report {
    /// Whether every test binary that started reported its result.
    pub(crate) fn every_binary_finished(&self) -> bool {
        self.started == self.finished
    }

    /// Whether it named every test that failed, so that running those names again runs
    /// every failure again: every binary that started reported, and named as many
    /// failures as it counted.
    pub(crate) fn names_every_failure(&self) -> bool {
        self.every_binary_finished()
            && u64::try_from(self.failed_tests.len()).is_ok_and(|named| named >= self.failed)
    }
}

/// What libtest adds to a test's name in the line of its verdict, and not in its list
/// of failures nor in a filter: `test x - should panic ... ok`.
const MODES: [&str; 3] = [" - should panic", " - compile fail", " - compile"];

/// A test's name without the mode libtest adds to it.
fn bare(name: &str) -> &str {
    MODES
        .iter()
        .find_map(|mode| name.strip_suffix(mode))
        .unwrap_or(name)
}

/// Reads the harness's output a line at a time.
#[derive(Debug, Default)]
pub(crate) struct Reading {
    report: Report,
    /// The names under the last `failures:` heading, until something else than a name
    /// comes: only the list just before a `test result:` line is the list of failures,
    /// and not a line of a failed test's output that says `failures:`.
    listing: Option<Vec<String>>,
}

impl Reading {
    pub(crate) fn line(&mut self, line: &str) {
        let line = line.trim_end_matches(['\r', '\n']);
        if let Some(result) = line.strip_prefix("test result: ") {
            self.report.finished = self.report.finished.saturating_add(1);
            if result.starts_with("FAILED") {
                self.report.failed_binaries = self.report.failed_binaries.saturating_add(1);
            }
            self.counts(result);
            for name in self.listing.take().unwrap_or_default() {
                self.failed(name);
            }
        } else if line == "failures:" {
            self.listing = Some(Vec::new());
        } else if is_running(line) {
            self.listing = None;
            self.report.started = self.report.started.saturating_add(1);
        } else if let Some(names) = &mut self.listing
            && (line.is_empty() || line.starts_with("    "))
        {
            if !line.trim().is_empty() {
                names.push(line.trim_start().to_owned());
            }
        } else {
            self.listing = None;
            let rest = line.strip_prefix("test ");
            if let Some((name, _)) = rest.and_then(|rest| rest.split_once(" ... ")) {
                self.report.seen_tests.push(bare(name).to_owned());
            }
            if let Some((name, verdict)) = rest.and_then(|rest| rest.rsplit_once(" ... ")) {
                match verdict {
                    "FAILED" => self.failed(bare(name).to_owned()),
                    "ok" => self.report.passed_tests.push(bare(name).to_owned()),
                    _ => {}
                }
            }
        }
    }

    pub(crate) fn finish(self) -> Report {
        self.report
    }

    fn failed(&mut self, name: String) {
        if !self.report.failed_tests.contains(&name) {
            self.report.failed_tests.push(name);
        }
    }

    /// Adds the counts of a line such as `ok. 3 passed; 1 failed; 2 ignored; 0 measured;
    /// 0 filtered out; finished in 0.10s`.
    fn counts(&mut self, result: &str) {
        let counts = result.split_once(". ").map_or(result, |(_, counts)| counts);
        for part in counts.split("; ") {
            let Some((number, what)) = part.split_once(' ') else {
                continue;
            };
            let Ok(number) = number.parse::<u64>() else {
                continue;
            };
            // The numbers come from any line that looks like a result, a test's own too.
            let count = match what {
                "passed" => &mut self.report.passed,
                "failed" => &mut self.report.failed,
                "ignored" => &mut self.report.ignored,
                _ => continue,
            };
            *count = count.saturating_add(number);
        }
    }
}

/// Whether `line` is a test binary's first: `running 1 test`, `running 12 tests`.
fn is_running(line: &str) -> bool {
    line.strip_prefix("running ")
        .and_then(|rest| {
            rest.strip_suffix(" tests")
                .or_else(|| rest.strip_suffix(" test"))
        })
        .is_some_and(|count| !count.is_empty() && count.bytes().all(|byte| byte.is_ascii_digit()))
}

/// A command of tests, after it ended.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct UnitRun {
    /// Whether it exited with success.
    pub(crate) succeeded: bool,
    /// Whether it was asked to stop (Ctrl-C reaches it as it reaches this task): then
    /// its end says nothing of its tests.
    pub(crate) interrupted: bool,
    /// How many test binaries cargo said failed (`error: 2 targets failed:`), if it said.
    pub(crate) failed_targets: Option<u64>,
    pub(crate) report: Report,
    pub(crate) elapsed: Duration,
}

/// How long the standard error of a command that ended may stay open: a process it
/// left behind may hold it. Cargo's words are all in it by then.
const ERRORS_AFTER_THE_END: Duration = Duration::from_secs(30);

/// What the reader of a command's standard error tells.
enum Errors {
    /// Cargo counted this many failed test binaries.
    Counted(u64),
    /// The stream ended.
    Ended,
}

/// What cargo says on its standard error at the end of a `cargo test --no-fail-fast`
/// whose test binaries failed: `error: 2 targets failed:`, `error: 1 target failed:`,
/// in colour or not.
fn failed_targets(line: &str) -> Option<u64> {
    let line = without_colour(line);
    let (count, what) = line.trim_end().strip_prefix("error: ")?.split_once(' ')?;
    if matches!(what, "targets failed:" | "target failed:") {
        count.parse().ok()
    } else {
        None
    }
}

/// `line` without the escape sequences that colour a terminal's text (`ESC [ … m`).
fn without_colour(line: &str) -> Cow<'_, str> {
    if !line.contains('\u{1b}') {
        return Cow::Borrowed(line);
    }
    let mut plain = String::with_capacity(line.len());
    let mut characters = line.chars();
    while let Some(character) = characters.next() {
        if character == '\u{1b}' {
            // An escape ends with its first letter, `m` for a colour.
            for escaped in characters.by_ref() {
                if escaped.is_ascii_alphabetic() {
                    break;
                }
            }
        } else {
            plain.push(character);
        }
    }
    Cow::Owned(plain)
}

/// Passes `errors` on to `echo` a line at a time, and tells `told` each count of failed
/// test binaries cargo gives in it as it comes, then that the stream ended.
fn read_errors(errors: impl Read, mut echo: impl Write, told: &mpsc::Sender<Errors>) {
    let mut errors = BufReader::new(errors);
    let mut line = Vec::new();
    // An error reading the stream ends it: what cargo said before still counts. Nobody
    // to tell is a task that waits no longer.
    while errors
        .read_until(b'\n', &mut line)
        .is_ok_and(|read| read > 0)
    {
        let _ = echo.write_all(&line);
        if let Some(count) = failed_targets(&String::from_utf8_lossy(&line)) {
            let _ = told.send(Errors::Counted(count));
        }
        line.clear();
    }
    let _ = told.send(Errors::Ended);
}

/// The last count of failed test binaries `told` gives until the stream it reads ends,
/// or until `wait` after the command's end, when a process it left holds the stream.
fn last_count(told: &mpsc::Receiver<Errors>, wait: Duration, shown: &str) -> Option<u64> {
    let deadline = Instant::now() + wait;
    let mut count = None;
    loop {
        match told.recv_timeout(deadline.saturating_duration_since(Instant::now())) {
            Ok(Errors::Counted(counted)) => count = Some(counted),
            Ok(Errors::Ended) | Err(mpsc::RecvTimeoutError::Disconnected) => return count,
            Err(mpsc::RecvTimeoutError::Timeout) => {
                crate::steps::say(&format!(
                    "    the standard error of `{shown}` is still open {:.1}s after its end: \
                     a process it started holds it",
                    wait.as_secs_f64()
                ));
                return count;
            }
        }
    }
}

/// Whether a command that ended with `status` was asked to stop: on Unix, ended by
/// `SIGHUP`, `SIGINT` or `SIGTERM`, which this task's terminal or caller sends to every
/// command of it; elsewhere, whether this task was asked to stop.
fn asked_to_stop(status: ExitStatus) -> bool {
    #[cfg(unix)]
    {
        use std::os::unix::process::ExitStatusExt as _;
        // SIGHUP, SIGINT and SIGTERM have these numbers on every Unix.
        matches!(status.signal(), Some(1 | 2 | 15))
    }
    #[cfg(not(unix))]
    {
        let _ = status;
        crate::steps::interrupted()
    }
}

/// Runs a command of tests in `directory`, its standard output read as the harness's
/// and its standard error as cargo's, each passed on to this task's.
///
/// # Errors
///
/// Fails if the program cannot be started or its output cannot be read.
pub(crate) fn run(
    directory: &Path,
    program: &std::ffi::OsStr,
    arguments: &[String],
    environment: &[(String, String)],
) -> Result<UnitRun> {
    let shown = format!("{} {}", program.to_string_lossy(), arguments.join(" "));
    if crate::steps::interrupted() {
        bail!("interrupted before `{shown}`");
    }
    crate::steps::say(&format!("    $ {shown}"));
    let mut command = Command::new(program);
    command
        .args(arguments)
        .envs(environment.iter().map(|(name, value)| (name, value)))
        .current_dir(directory);
    // Cargo writes to a pipe here: in colour still, if it is a terminal it writes to in
    // the end and the environment does not say otherwise (cargo's own config can).
    let unset = |name: &str| std::env::var_os(name).is_none_or(|value| value.is_empty());
    if std::io::stderr().is_terminal()
        && unset("CARGO_TERM_COLOR")
        && unset("NO_COLOR")
        && std::env::var_os("TERM").is_none_or(|term| term != "dumb")
    {
        command.env("CARGO_TERM_COLOR", "always");
    }
    run_echoing(
        command,
        &shown,
        &mut std::io::stdout(),
        std::io::stderr(),
        ERRORS_AFTER_THE_END,
    )
}

/// Runs `command`, its standard output read as the harness's and written to `echo`, its
/// standard error read as cargo's and written to `echo_errors`, for no longer than
/// `errors_wait` after the command's end.
fn run_echoing(
    mut command: Command,
    shown: &str,
    echo: &mut dyn Write,
    echo_errors: impl Write + Send + 'static,
    errors_wait: Duration,
) -> Result<UnitRun> {
    let started = Instant::now();
    let mut child = command
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .with_context(|| format!("starting `{shown}`"))?;
    // Read on a thread of its own, so that neither stream fills while the other is read.
    let (tell, told) = mpsc::channel();
    let errors = child
        .stderr
        .take()
        .context("the standard error of a command that pipes it")?;
    std::thread::spawn(move || read_errors(errors, echo_errors, &tell));
    let mut reading = Reading::default();
    if let Some(output) = child.stdout.take() {
        let mut output = BufReader::new(output);
        let mut line = Vec::new();
        loop {
            line.clear();
            let read = output
                .read_until(b'\n', &mut line)
                .with_context(|| format!("reading the output of `{shown}`"))?;
            if read == 0 {
                break;
            }
            // A closed standard output leaves nobody to show the line to; it is still read.
            let _ = echo.write_all(&line);
            reading.line(&String::from_utf8_lossy(&line));
        }
    }
    let status = child
        .wait()
        .with_context(|| format!("waiting for `{shown}`"))?;
    let failed_targets = last_count(&told, errors_wait, shown);
    Ok(UnitRun {
        succeeded: status.success(),
        interrupted: asked_to_stop(status),
        failed_targets,
        report: reading.finish(),
        elapsed: started.elapsed(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// How long the tests wait for a command's standard error after its end.
    const WAIT: Duration = Duration::from_secs(5);

    fn read(text: &str) -> Report {
        let mut reading = Reading::default();
        text.lines().for_each(|line| reading.line(line));
        reading.finish()
    }

    /// The output of two binaries, one with failures, as libtest writes it: a failed
    /// test's own output, which says `failures:` and names a test, comes first.
    const TWO_BINARIES: &str = "
running 2 tests
test a::passes ... ok
test a::slow ... ignored, takes a minute

test result: ok. 1 passed; 0 failed; 1 ignored; 0 measured; 0 filtered out; finished in 0.01s


running 4 tests
test b::fine ... ok
test b::broken ... FAILED
test b::fine_too ... ok
test b::also_broken ... FAILED

failures:

---- b::broken stdout ----
failures:
    b::not_a_failure

thread 'b::broken' panicked at src/lib.rs:3:5:
no

failures:
    b::also_broken
    b::broken

test result: FAILED. 2 passed; 2 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.02s
";

    #[test]
    fn counts_add_up_and_the_failures_are_named_once() {
        let report = read(TWO_BINARIES);
        assert_eq!((report.passed, report.failed, report.ignored), (3, 2, 1));
        assert_eq!((report.started, report.finished), (2, 2));
        assert_eq!(report.failed_binaries, 1);
        assert_eq!(
            report.failed_tests,
            ["b::broken", "b::also_broken"],
            "each once, and not the name in a test's own output"
        );
        assert_eq!(report.passed_tests, ["a::passes", "b::fine", "b::fine_too"]);
        assert!(report.names_every_failure());
    }

    #[test]
    fn a_failure_is_named_by_the_list_when_its_line_was_cut() {
        let report = read(
            "running 1 test\ntest c::one ... some output of a child processFAILED\n\n\
             failures:\n\n---- c::one stdout ----\nx\n\nfailures:\n    c::one\n\n\
             test result: FAILED. 0 passed; 1 failed; 0 ignored; 0 measured; 0 filtered out; \
             finished in 0.00s\n",
        );
        assert_eq!(report.failed_tests, ["c::one"]);
    }

    #[test]
    fn documentation_tests_are_named_with_their_spaces() {
        let report = read(
            "running 1 test\ntest crates/x/src/lib.rs - f (line 3) ... FAILED\n\n\
             test result: FAILED. 0 passed; 1 failed; 0 ignored; 0 measured; 0 filtered out; \
             finished in 0.00s\n",
        );
        assert_eq!(report.failed_tests, ["crates/x/src/lib.rs - f (line 3)"]);
    }

    #[test]
    fn a_harness_that_ended_early_has_not_named_every_failure() {
        let report = read("running 3 tests\ntest d::a ... ok\n");
        assert_eq!(report.failed, 0, "no result line, no counts");
        assert!(!report.every_binary_finished(), "{report:?}");
        assert!(!report.names_every_failure(), "{report:?}");
        let counted =
            read("running 2 tests\ntest result: FAILED. 0 passed; 2 failed; 0 ignored;\n");
        assert!(
            !counted.names_every_failure(),
            "two failures counted, none named"
        );
    }

    #[test]
    fn a_binary_that_died_is_seen_beside_one_that_named_its_failure() {
        let report = read(
            "running 1 test\ntest a ... FAILED\n\nfailures:\n    a\n\n\
             test result: FAILED. 0 passed; 1 failed; 0 ignored; 0 measured; 0 filtered out; \
             finished in 0.00s\n\nrunning 3 tests\ntest b::one ... ok\n",
        );
        assert_eq!(report.failed_tests, ["a"]);
        assert_eq!((report.started, report.finished), (2, 1));
        assert!(
            !report.names_every_failure(),
            "the second binary's failure has no name: the unit runs again whole"
        );
    }

    #[test]
    fn the_mode_libtest_adds_to_a_name_is_left_out() {
        let report = read(
            "running 2 tests\ntest p::panics - should panic ... FAILED\n\
             test q::fine - should panic ... ok\n\nfailures:\n    p::panics\n\n\
             test result: FAILED. 1 passed; 1 failed; 0 ignored; 0 measured; 0 filtered out; \
             finished in 0.00s\n",
        );
        assert_eq!(
            report.failed_tests,
            ["p::panics"],
            "once, as a filter takes it"
        );
        assert_eq!(report.passed_tests, ["q::fine"]);
    }

    #[test]
    fn only_a_binarys_own_first_line_counts_as_one_running() {
        for line in ["running 1 test", "running 12 tests", "running 0 tests"] {
            assert!(is_running(line), "{line}");
        }
        for line in [
            "running tests",
            "running 3 testsuites",
            "    running 3 tests",
            "running x tests",
        ] {
            assert!(!is_running(line), "{line}");
        }
    }

    #[cfg(unix)]
    #[test]
    fn a_command_is_run_and_read_and_its_status_kept() {
        // Its output goes to `echo`, not to this test's: the harness of a run of these
        // tests would read it as its own.
        let script = "echo 'test e::x ... FAILED'; \
                      echo 'test result: FAILED. 0 passed; 1 failed; 0 ignored'; exit 101";
        let mut command = Command::new("sh");
        command.args(["-c", script]);
        let mut echo = Vec::new();
        let ran = run_echoing(command, "sh", &mut echo, std::io::sink(), WAIT).expect("sh starts");
        assert!(!ran.succeeded, "the exit status is kept");
        assert!(!ran.interrupted);
        assert_eq!(ran.report.failed_tests, ["e::x"]);
        assert!(
            String::from_utf8_lossy(&echo).starts_with("test e::x ... FAILED\n"),
            "every line is passed on"
        );

        let mut command = Command::new("sh");
        command
            .args(["-c", "test \"$THE_VALUE\" = given"])
            .env("THE_VALUE", "given");
        let ran =
            run_echoing(command, "sh", &mut Vec::new(), std::io::sink(), WAIT).expect("sh starts");
        assert!(ran.succeeded, "a command that succeeds");
        assert_eq!(ran.failed_targets, None);
    }

    #[cfg(unix)]
    #[test]
    fn cargos_count_of_failed_binaries_is_read_from_its_standard_error() {
        // As cargo ends a run of which two binaries failed; the error stream goes to
        // the sink, not to this test's, which the task of a run of these tests reads.
        let script = "echo 'error: test failed, to rerun pass `--lib`' >&2; \
                      echo 'error: 2 targets failed:' >&2; echo '    `--lib`' >&2; exit 101";
        let mut command = Command::new("sh");
        command.args(["-c", script]);
        let ran =
            run_echoing(command, "sh", &mut Vec::new(), std::io::sink(), WAIT).expect("sh starts");
        assert_eq!(ran.failed_targets, Some(2));
        assert_eq!(failed_targets("error: 1 target failed:\n"), Some(1));
        assert_eq!(failed_targets("error: could not compile `x`"), None);
    }

    #[cfg(unix)]
    #[test]
    fn a_process_left_holding_the_standard_error_holds_the_task_no_longer_than_the_wait() {
        // It does not hold the standard output, which the task reads to its end.
        let script = "sleep 30 >/dev/null & echo 'error: 1 target failed:' >&2; exit 101";
        let mut command = Command::new("sh");
        command.args(["-c", script]);
        let started = Instant::now();
        let wait = Duration::from_secs(2);
        let ran =
            run_echoing(command, "sh", &mut Vec::new(), std::io::sink(), wait).expect("sh starts");
        // The process left behind holds the stream for 30 s; the wait is much shorter.
        assert!(
            started.elapsed() < Duration::from_secs(15),
            "{:?}",
            started.elapsed()
        );
        assert_eq!(
            ran.failed_targets,
            Some(1),
            "what cargo said before still counts"
        );
    }

    #[cfg(unix)]
    #[test]
    fn a_command_ended_by_a_stop_signal_was_asked_to_stop() {
        use std::os::unix::process::ExitStatusExt as _;
        // A wait status: the signal that ended the process, or its exit code times 256.
        for (status, asked) in [
            (1, true),
            (2, true),
            (15, true),
            (9, false),
            (101 << 8, false),
        ] {
            assert_eq!(
                asked_to_stop(ExitStatus::from_raw(status)),
                asked,
                "{status}"
            );
        }
    }

    #[test]
    fn a_count_in_colour_is_read() {
        let line = "\u{1b}[1m\u{1b}[91merror\u{1b}[0m: 3 targets failed:\n";
        assert_eq!(failed_targets(line), Some(3));
        assert_eq!(without_colour("plain"), "plain");
    }

    #[test]
    fn a_test_whose_output_follows_its_name_is_seen() {
        let report = read("running 1 test\ntest api::talks ... it says hello\nok\n");
        assert_eq!(report.seen_tests, ["api::talks"]);
        assert!(report.passed_tests.is_empty(), "{report:?}");
    }
}
