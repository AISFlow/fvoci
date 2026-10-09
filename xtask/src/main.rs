//! Development-task dispatcher.
//!
//! `help` (and no arguments) print usage and perform no work. Harness commands
//! are intentionally absent until a later batch moves them here.

use std::io::{self, Write};
use std::process::ExitCode;

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match dispatch(&args) {
        Ok(text) => {
            if emit(&mut io::stdout(), &text).is_err() {
                return ExitCode::from(1);
            }
            ExitCode::SUCCESS
        }
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

/// Run one subcommand.
///
/// An empty argument list and `help` are no-ops that return usage text.
/// Any other name is rejected.
fn dispatch(args: &[String]) -> Result<String, String> {
    match args {
        [] => Ok(help_text()),
        [command] if command == "help" => Ok(help_text()),
        [command, ..] => Err(format!("unknown subcommand: {command}\n{}", help_text())),
    }
}

fn help_text() -> String {
    "\
xtask

Usage:
    cargo xtask <subcommand>

Subcommands:
    help    Print this help and do nothing
"
    .to_string()
}

#[cfg(test)]
mod tests {
    use super::dispatch;

    #[test]
    fn help_and_no_args_are_a_noop() {
        let usage = dispatch(&[]).expect("no arguments");
        assert_eq!(dispatch(&["help".to_string()]).expect("help"), usage);
        assert!(usage.contains("help"));
        assert!(dispatch(&["run".to_string()]).is_err());
    }
}
