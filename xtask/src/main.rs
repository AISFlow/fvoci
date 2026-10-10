//! Development-task dispatcher.
//!
//! `help` (and no arguments) print usage and perform no work. `start-test`
//! starts one isolated test service, runs a command, and removes the service.

mod start_test;

use std::io::{self, Write};
use std::process::ExitCode;

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match dispatch(&args) {
        Ok(Action::Show(text)) => {
            if emit(&mut io::stdout(), &text).is_err() {
                return ExitCode::from(1);
            }
            ExitCode::SUCCESS
        }
        Ok(Action::StartTest(args)) => start_test::run(&args),
        Err(text) => {
            if emit(&mut io::stderr(), &text).is_err() {
                return ExitCode::from(1);
            }
            ExitCode::from(2)
        }
    }
}

fn emit(sink: &mut dyn Write, text: &str) -> io::Result<()> {
    sink.write_all(text.as_bytes())?;
    sink.flush()
}

#[derive(Debug)]
enum Action {
    Show(String),
    StartTest(Vec<String>),
}

/// Run one subcommand.
///
/// An empty argument list and `help` are no-ops that return usage text.
/// `start-test` delegates to the service launcher. Any other name is rejected.
fn dispatch(args: &[String]) -> Result<Action, String> {
    match args {
        [] => Ok(Action::Show(help_text())),
        [command] if command == "help" => Ok(Action::Show(help_text())),
        [command, rest @ ..] if command == "start-test" => Ok(Action::StartTest(rest.to_vec())),
        [command, ..] => Err(format!("unknown subcommand: {command}\n{}", help_text())),
    }
}

fn help_text() -> String {
    "\
xtask

Usage:
    cargo xtask <subcommand>

Subcommands:
    help
            Print this help and do nothing
    start-test <postgres|minio|meili> [command...]
            Start one isolated test service, run the command, then remove it
"
    .to_string()
}

#[cfg(test)]
mod tests {
    use super::{dispatch, Action};

    fn shown(args: &[&str]) -> Result<String, String> {
        let owned: Vec<String> = args.iter().map(|s| (*s).to_string()).collect();
        match dispatch(&owned) {
            Ok(Action::Show(text)) => Ok(text),
            Ok(Action::StartTest(_)) => panic!("expected help text"),
            Err(text) => Err(text),
        }
    }

    #[test]
    fn help_and_no_args_are_a_noop() {
        let usage = shown(&[]).expect("no arguments");
        assert_eq!(shown(&["help"]).expect("help"), usage);
        assert!(usage.contains("help"));
        assert!(usage.contains("start-test"));
        assert!(shown(&["run"]).is_err());
    }

    #[test]
    fn start_test_is_a_known_subcommand() {
        let owned = vec!["start-test".to_string(), "postgres".to_string()];
        match dispatch(&owned) {
            Ok(Action::StartTest(args)) => assert_eq!(args, vec!["postgres".to_string()]),
            other => panic!("expected start-test, got {other:?}"),
        }
    }
}
