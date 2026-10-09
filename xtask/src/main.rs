mod evidence_hash;

use std::ffi::OsString;
use std::fmt;
use std::process::ExitCode;

const HELP: &str = "\
FVOCI development tasks

Usage: cargo xtask <command>

Commands:
  help            Show this help
  evidence-hash   Hash a commit, range, or directive comment

Options:
  -h, --help      Show this help

Run `cargo xtask evidence-hash --help` for evidence-hash options.
";

#[derive(Debug, PartialEq, Eq)]
enum Command {
    Help,
    EvidenceHash(evidence_hash::Parsed),
}

#[derive(Debug, PartialEq, Eq)]
enum CliError {
    MissingCommand,
    UnknownCommand(OsString),
    UnexpectedArgument(OsString),
    Evidence(String),
}

impl fmt::Display for CliError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::MissingCommand => write!(formatter, "missing command"),
            Self::UnknownCommand(command) => write!(formatter, "unknown command {command:?}"),
            Self::UnexpectedArgument(argument) => {
                write!(formatter, "unexpected argument {argument:?}")
            }
            Self::Evidence(message) => write!(formatter, "{message}"),
        }
    }
}

fn parse_args(mut args: impl Iterator<Item = OsString>) -> Result<Command, CliError> {
    let argument = args.next().ok_or(CliError::MissingCommand)?;
    match argument.to_str() {
        Some("help" | "-h" | "--help") => {
            if let Some(argument) = args.next() {
                return Err(CliError::UnexpectedArgument(argument));
            }
            Ok(Command::Help)
        }
        Some("evidence-hash") => match evidence_hash::parse_args(args.collect()) {
            Ok(parsed) => Ok(Command::EvidenceHash(parsed)),
            Err(error) => Err(CliError::Evidence(error.to_string())),
        },
        _ => Err(CliError::UnknownCommand(argument)),
    }
}

fn run(command: Command) -> ExitCode {
    match command {
        Command::Help => {
            print!("{HELP}");
            ExitCode::SUCCESS
        }
        Command::EvidenceHash(parsed) => evidence_hash::execute(&parsed),
    }
}

fn main() -> ExitCode {
    match parse_args(std::env::args_os().skip(1)) {
        Ok(command) => run(command),
        Err(CliError::Evidence(message)) => {
            eprintln!("error: {message}\n\n{}", evidence_hash::help());
            ExitCode::from(2)
        }
        Err(error) => {
            eprintln!("error: {error}\n\n{HELP}");
            ExitCode::from(2)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{parse_args, CliError, Command};
    use std::ffi::OsString;

    #[test]
    fn accepts_help_forms() {
        for argument in ["help", "-h", "--help"] {
            assert_eq!(
                parse_args([OsString::from(argument)].into_iter()),
                Ok(Command::Help)
            );
        }
    }

    #[test]
    fn rejects_missing_command() {
        assert_eq!(
            parse_args(std::iter::empty()),
            Err(CliError::MissingCommand)
        );
    }

    #[test]
    fn rejects_unregistered_commands_and_flags() {
        for argument in [
            "unknown",
            "ff-check",
            "ci-table",
            "review-carry",
            "nextest-proof",
            "sqlite",
            "--unknown",
            "--",
            "",
        ] {
            assert_eq!(
                parse_args([OsString::from(argument)].into_iter()),
                Err(CliError::UnknownCommand(OsString::from(argument)))
            );
        }
    }

    #[test]
    fn rejects_trailing_help_arguments() {
        for help in ["help", "-h", "--help"] {
            for extra in ["unknown", "--help", "--", ""] {
                assert_eq!(
                    parse_args([OsString::from(help), OsString::from(extra)].into_iter()),
                    Err(CliError::UnexpectedArgument(OsString::from(extra)))
                );
            }
        }
    }

    #[test]
    fn accepts_evidence_hash_help() {
        assert_eq!(
            parse_args([OsString::from("evidence-hash"), OsString::from("--help")].into_iter()),
            Ok(Command::EvidenceHash(super::evidence_hash::Parsed::Help))
        );
    }

    #[cfg(unix)]
    #[test]
    fn rejects_non_unicode_command() {
        use std::os::unix::ffi::OsStringExt;

        let argument = OsString::from_vec(vec![0xff]);
        assert_eq!(
            parse_args([argument.clone()].into_iter()),
            Err(CliError::UnknownCommand(argument))
        );
    }
}
