mod ci_table;
mod ff_check;
mod nextest_proof;
mod review_carry;

use std::ffi::OsString;
use std::fmt;
use std::process::ExitCode;

const HELP: &str = "\
FVOCI development tasks

Usage: cargo xtask <command>

Commands:
  help                         Show this help
  review-carry <old> <new>     Compare exact commits and run fresh local checks
  ci-table <sha>               Show exact-SHA CI jobs and timeouts
  nextest-proof <options>      Run an explicitly allocated archive proof
  ff-check <options>           Query candidate and previous-head requirements

Options:
  -h, --help    Show this help

Use <command> --help for command arguments.
";

const REVIEW_CARRY_HELP: &str = "Usage: cargo xtask review-carry <old-40-hex> <new-40-hex>\n\
Runs fresh local checks in the clean worktree at the exact new SHA.\n\
This report does not carry an independent review or user approval.\n";
const CI_TABLE_HELP: &str = "Usage: cargo xtask ci-table <40-lowercase-hex>\n\
Queries exact-SHA CI jobs, checks and timeouts without writing to GitHub.\n\
Incomplete observations return a nonzero exit status.\n";

#[derive(Debug, PartialEq, Eq)]
enum Command {
    Help,
    TaskHelp(&'static str),
    ReviewCarry(review_carry::Args),
    CiTable(String),
    NextestProof(Vec<OsString>),
    FfCheck(ff_check::Args),
}

#[derive(Debug, PartialEq, Eq)]
enum CliError {
    MissingCommand,
    UnknownCommand(OsString),
    UnexpectedArgument(OsString),
    InvalidArguments(String),
}

impl fmt::Display for CliError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::MissingCommand => write!(formatter, "missing command"),
            Self::UnknownCommand(command) => write!(formatter, "unknown command {command:?}"),
            Self::UnexpectedArgument(argument) => {
                write!(formatter, "unexpected argument {argument:?}")
            }
            Self::InvalidArguments(error) => formatter.write_str(error),
        }
    }
}

fn parse_args(mut args: impl Iterator<Item = OsString>) -> Result<Command, CliError> {
    let argument = args.next().ok_or(CliError::MissingCommand)?;
    let command = match argument.to_str() {
        Some("help" | "-h" | "--help") => Command::Help,
        Some("review-carry") => {
            let args: Vec<_> = args.collect();
            if args == [OsString::from("--help")] || args == [OsString::from("-h")] {
                return Ok(Command::TaskHelp(REVIEW_CARRY_HELP));
            }
            return review_carry::parse_args(args.into_iter())
                .map(Command::ReviewCarry)
                .map_err(CliError::InvalidArguments);
        }
        Some("ci-table") => {
            let args: Vec<_> = args.collect();
            if args == [OsString::from("--help")] || args == [OsString::from("-h")] {
                return Ok(Command::TaskHelp(CI_TABLE_HELP));
            }
            let sha = match args.as_slice() {
                [sha] => sha.to_str().ok_or_else(|| {
                    CliError::InvalidArguments("ci-table SHA must be Unicode".into())
                })?,
                _ => {
                    return Err(CliError::InvalidArguments(
                        "ci-table requires exactly one full commit SHA".into(),
                    ));
                }
            };
            ci_table::validate_sha(sha).map_err(CliError::InvalidArguments)?;
            return Ok(Command::CiTable(sha.into()));
        }
        Some("nextest-proof") => {
            let args: Vec<_> = args.collect();
            if args != [OsString::from("--help")] && args != [OsString::from("-h")] {
                nextest_proof::parse(args.iter().cloned())
                    .map_err(|error| CliError::InvalidArguments(error.to_string()))?;
            }
            return Ok(Command::NextestProof(args));
        }
        Some("ff-check") => {
            return ff_check::parse_args(args)
                .map(Command::FfCheck)
                .map_err(CliError::InvalidArguments);
        }
        _ => return Err(CliError::UnknownCommand(argument)),
    };
    if let Some(argument) = args.next() {
        return Err(CliError::UnexpectedArgument(argument));
    }
    Ok(command)
}

fn run(command: Command) -> ExitCode {
    match command {
        Command::Help => {
            print!("{HELP}");
            ExitCode::SUCCESS
        }
        Command::TaskHelp(help) => {
            print!("{help}");
            ExitCode::SUCCESS
        }
        Command::ReviewCarry(args) => review_carry::run(args),
        Command::FfCheck(args) => ff_check::run(args),
        Command::CiTable(sha) => match ci_table::run(&sha) {
            Ok(true) => ExitCode::SUCCESS,
            Ok(false) => ExitCode::FAILURE,
            Err(error) => {
                eprintln!("error: {error}");
                ExitCode::FAILURE
            }
        },
        Command::NextestProof(args) => match nextest_proof::run(args) {
            Ok(()) => ExitCode::SUCCESS,
            Err(error) => {
                eprintln!("error: {error}");
                ExitCode::FAILURE
            }
        },
    }
}

fn main() -> ExitCode {
    match parse_args(std::env::args_os().skip(1)) {
        Ok(command) => run(command),
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
            let expected = match argument {
                "review-carry" => CliError::InvalidArguments(
                    "review-carry requires two lowercase 40-character commit SHAs".into(),
                ),
                "ci-table" => CliError::InvalidArguments(
                    "ci-table requires exactly one full commit SHA".into(),
                ),
                "nextest-proof" => CliError::InvalidArguments("FAIL: missing required flag".into()),
                "ff-check" => CliError::InvalidArguments("missing --remote".into()),
                _ => CliError::UnknownCommand(OsString::from(argument)),
            };
            assert_eq!(
                parse_args([OsString::from(argument)].into_iter()),
                Err(expected)
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
