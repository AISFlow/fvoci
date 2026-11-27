//! Named steps that all run, with a summary at the end, and the commands they start.

use std::ffi::OsString;
use std::io::Write as _;
use std::path::Path;
use std::process::Command;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use anyhow::{Context as _, Result, bail};

/// One named part of a task.
pub(crate) struct Step<'a> {
    name: String,
    work: Work<'a>,
}

/// What a step does when its turn comes.
enum Work<'a> {
    Run(Box<dyn FnOnce() -> Result<()> + 'a>),
    /// Left out on request, with the reason. It shows in the summary, so that a run
    /// that checked less does not read like one that checked everything.
    Skip(String),
}

impl<'a> Step<'a> {
    pub(crate) fn new(name: &str, action: impl FnOnce() -> Result<()> + 'a) -> Self {
        Self {
            name: name.to_owned(),
            work: Work::Run(Box::new(action)),
        }
    }

    /// A step that is not run, and says so in the summary.
    pub(crate) fn skipped(name: &str, reason: &str) -> Self {
        Self {
            name: name.to_owned(),
            work: Work::Skip(reason.to_owned()),
        }
    }
}

impl std::fmt::Debug for Step<'_> {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("Step")
            .field("name", &self.name)
            .finish_non_exhaustive()
    }
}

/// How a step ended.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Outcome {
    Passed,
    Failed(String),
    Skipped(String),
}

/// A step that had its turn: how it ended and how long it took.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct StepResult {
    pub(crate) name: String,
    pub(crate) outcome: Outcome,
    pub(crate) elapsed: Duration,
}

/// The steps of a task after their turns, and whether the task was interrupted.
#[derive(Debug)]
#[must_use = "the verdict of the task is in `into_result`"]
pub(crate) struct Ran {
    task: String,
    results: Vec<StepResult>,
    interrupted: bool,
}

impl Ran {
    pub(crate) fn results(&self) -> &[StepResult] {
        &self.results
    }

    pub(crate) fn interrupted(&self) -> bool {
        self.interrupted
    }

    /// The task's verdict.
    ///
    /// # Errors
    ///
    /// Fails if the task was interrupted or a step failed; the error names the failed steps.
    pub(crate) fn into_result(self) -> Result<()> {
        let failed: Vec<&str> = self
            .results
            .iter()
            .filter(|result| matches!(result.outcome, Outcome::Failed(_)))
            .map(|result| result.name.as_str())
            .collect();
        if self.interrupted {
            bail!("{}: interrupted", self.task)
        } else if failed.is_empty() {
            Ok(())
        } else {
            bail!(
                "{}: {} failed: {}",
                self.task,
                failed.len(),
                failed.join(", ")
            )
        }
    }
}

/// Writes a line to standard error, where the output of the started commands goes too.
pub(crate) fn say(line: &str) {
    // A closed standard error leaves nobody to tell.
    let _ = writeln!(std::io::stderr().lock(), "{line}");
}

/// Set once the task is asked to stop (Ctrl-C, or `SIGTERM` or `SIGHUP`).
static INTERRUPTED: AtomicBool = AtomicBool::new(false);

/// Lets the task outlive Ctrl-C long enough to end properly: the step that runs ends
/// (the commands it started get Ctrl-C from the terminal too), the steps after it are
/// skipped, and what the task set up is put back, its containers stopped. A second
/// Ctrl-C changes nothing; a task that must end at once is killed.
///
/// # Errors
///
/// Fails if the handler cannot be installed.
pub(crate) fn catch_interrupts() -> Result<()> {
    ctrlc::set_handler(|| {
        INTERRUPTED.store(true, Ordering::SeqCst);
        say("\ninterrupted: the step that runs ends, the rest are skipped");
    })
    .context("installing the handler of Ctrl-C")
}

/// Whether the task was asked to stop.
pub(crate) fn interrupted() -> bool {
    INTERRUPTED.load(Ordering::SeqCst)
}

/// Runs every step, also after one failed, and prints a summary. Once the task is
/// interrupted the steps not yet started are skipped. The verdict is the returned
/// [`Ran::into_result`].
pub(crate) fn run_all(task: &str, steps: Vec<Step<'_>>) -> Ran {
    run_all_reporting(task, steps, &mut say, &interrupted)
}

/// [`run_all`] with the progress lines going to `report`.
fn run_all_reporting(
    task: &str,
    steps: Vec<Step<'_>>,
    report: &mut dyn FnMut(&str),
    stop: &dyn Fn() -> bool,
) -> Ran {
    let mut results: Vec<StepResult> = Vec::with_capacity(steps.len());
    for step in steps {
        if stop() {
            results.push(StepResult {
                name: step.name,
                outcome: Outcome::Skipped("interrupted".to_owned()),
                elapsed: Duration::ZERO,
            });
            continue;
        }
        report(&format!("\n==> {}", step.name));
        let started = Instant::now();
        let outcome = match step.work {
            Work::Skip(reason) => {
                report(&format!("    skipped: {reason}"));
                Outcome::Skipped(reason)
            }
            Work::Run(action) => match action() {
                Ok(()) => Outcome::Passed,
                Err(error) => {
                    report(&format!("    failed: {error:#}"));
                    Outcome::Failed(format!("{error:#}"))
                }
            },
        };
        results.push(StepResult {
            name: step.name,
            outcome,
            elapsed: started.elapsed(),
        });
    }

    report(&format!("\n{}", summary(task, &results)));
    Ran {
        task: task.to_owned(),
        results,
        interrupted: stop(),
    }
}

fn summary(task: &str, results: &[StepResult]) -> String {
    let mut lines = vec![format!("{task} summary")];
    for StepResult {
        name,
        outcome,
        elapsed,
    } in results
    {
        lines.push(match outcome {
            Outcome::Passed => format!("  PASS  {name}  ({:.1}s)", elapsed.as_secs_f64()),
            Outcome::Failed(_) => format!("  FAIL  {name}  ({:.1}s)", elapsed.as_secs_f64()),
            Outcome::Skipped(reason) => format!("  SKIP  {name}  ({reason})"),
        });
    }
    lines.join("\n")
}

/// The `cargo` that runs this task.
pub(crate) fn cargo() -> OsString {
    std::env::var_os("CARGO").unwrap_or_else(|| "cargo".into())
}

/// Runs a command in `directory` with extra environment variables, its output going to
/// the terminal.
///
/// # Errors
///
/// Fails if the program cannot be started or exits with a failure.
pub(crate) fn run(
    directory: &Path,
    program: impl AsRef<std::ffi::OsStr>,
    arguments: &[&str],
    environment: &[(&str, &str)],
) -> Result<()> {
    let program = program.as_ref();
    let shown = format!("{} {}", program.to_string_lossy(), arguments.join(" "));
    // A Ctrl-C while the step prepared it reached no command that runs now.
    if interrupted() {
        bail!("interrupted before `{shown}`");
    }
    say(&format!("    $ {shown}"));
    let status = Command::new(program)
        .args(arguments)
        .envs(environment.iter().copied())
        .current_dir(directory)
        .status()
        .with_context(|| format!("starting `{shown}`"))?;
    if !status.success() {
        bail!("`{shown}` ended with {status}");
    }
    Ok(())
}

/// What a command that ran to its end wrote, and whether it succeeded.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Captured {
    pub(crate) succeeded: bool,
    /// Standard output, trimmed.
    pub(crate) output: String,
    /// Standard error, trimmed: why the command failed, if it did.
    pub(crate) error: String,
}

/// Runs a command and returns what it wrote, whether it succeeded or not.
///
/// # Errors
///
/// Fails if the program cannot be started.
pub(crate) fn capture(program: &str, arguments: &[&str]) -> Result<Captured> {
    let output = Command::new(program)
        .args(arguments)
        .output()
        .with_context(|| format!("starting `{program} {}`", arguments.join(" ")))?;
    Ok(Captured {
        succeeded: output.status.success(),
        output: String::from_utf8_lossy(&output.stdout).trim().to_owned(),
        error: String::from_utf8_lossy(&output.stderr).trim().to_owned(),
    })
}

#[cfg(test)]
mod tests {
    use std::cell::Cell;

    use super::*;

    #[test]
    fn every_step_runs_even_after_a_failure() {
        let ran = Cell::new(0);
        let steps = vec![
            Step::new("first", || {
                ran.set(ran.get() + 1);
                bail!("broken")
            }),
            Step::new("second", || {
                ran.set(ran.get() + 1);
                Ok(())
            }),
        ];
        let mut lines = Vec::new();
        let error = run_all_reporting(
            "task",
            steps,
            &mut |line| lines.push(line.to_owned()),
            &|| false,
        )
        .into_result()
        .expect_err("a failed step fails the task");
        assert_eq!(ran.get(), 2, "both steps ran");
        assert_eq!(
            error.to_string(),
            "task: 1 failed: first",
            "the error names the failed step"
        );
        assert_eq!(
            lines[..3],
            ["\n==> first", "    failed: broken", "\n==> second"],
            "each step is announced, and a failure with its reason"
        );
    }

    #[test]
    fn the_summary_lists_every_step_with_its_verdict() {
        let results = [
            StepResult {
                name: "fmt".to_owned(),
                outcome: Outcome::Passed,
                elapsed: Duration::from_millis(1500),
            },
            StepResult {
                name: "clippy".to_owned(),
                outcome: Outcome::Failed("x".to_owned()),
                elapsed: Duration::from_secs(2),
            },
        ];
        assert_eq!(
            summary("check", &results),
            "check summary\n  PASS  fmt  (1.5s)\n  FAIL  clippy  (2.0s)",
            "one line per step"
        );
    }

    #[cfg(unix)]
    #[test]
    fn a_failing_command_is_an_error_that_names_it() {
        let error = run(Path::new("."), "false", &["an-argument"], &[])
            .expect_err("`false` exits with a failure");
        assert!(
            error.to_string().contains("`false an-argument` ended with"),
            "the error names the command: {error}"
        );
    }

    #[test]
    fn a_program_that_does_not_exist_is_an_error_that_names_it() {
        let error = run(Path::new("."), "no-such-program-of-xtask", &[], &[])
            .expect_err("a missing program cannot be started");
        assert!(
            error
                .to_string()
                .contains("starting `no-such-program-of-xtask"),
            "the error names the program: {error}"
        );
    }

    #[cfg(unix)]
    #[test]
    fn captured_output_is_trimmed_and_a_failure_keeps_its_reason() {
        let captured = capture("sh", &["-c", "echo ' true '"]).expect("sh starts");
        assert!(captured.succeeded, "{captured:?}");
        assert_eq!(
            captured.output, "true",
            "the output without surrounding space"
        );

        let captured =
            capture("sh", &["-c", "echo no; echo ' why ' >&2; exit 3"]).expect("sh starts");
        assert!(!captured.succeeded, "{captured:?}");
        assert_eq!(captured.error, "why", "what it said about the failure");
    }

    #[test]
    fn a_skipped_step_is_not_run_and_shows_in_the_summary() {
        let mut lines = Vec::new();
        let steps = vec![
            Step::new("ran", || Ok(())),
            Step::skipped("left out", "--skip-it"),
        ];
        run_all_reporting(
            "task",
            steps,
            &mut |line| lines.push(line.to_owned()),
            &|| false,
        )
        .into_result()
        .expect("a skipped step fails nothing");
        let summary = lines.last().expect("a summary");
        assert!(
            summary.contains("SKIP  left out  (--skip-it)"),
            "the summary says what did not run: {summary}"
        );
    }

    #[test]
    fn after_an_interruption_the_rest_is_skipped_and_the_task_fails() {
        let stopped = std::cell::Cell::new(false);
        let ran = std::cell::Cell::new(0);
        let steps = vec![
            Step::new("first", || {
                ran.set(ran.get() + 1);
                stopped.set(true);
                Ok(())
            }),
            Step::new("second", || {
                ran.set(ran.get() + 1);
                Ok(())
            }),
        ];
        let mut lines = Vec::new();
        let error = run_all_reporting(
            "task",
            steps,
            &mut |line| lines.push(line.to_owned()),
            &|| stopped.get(),
        )
        .into_result()
        .expect_err("an interrupted task fails");
        assert_eq!(ran.get(), 1, "the second step did not run");
        assert_eq!(error.to_string(), "task: interrupted");
        let summary = lines.last().expect("a summary");
        assert!(
            summary.contains("SKIP  second  (interrupted)"),
            "the summary says why: {summary}"
        );
    }
}
