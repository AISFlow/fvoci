use std::ffi::OsString;
use std::fmt;
use std::process::ExitCode;

const HELP: &str = "\
FVOCI development tasks

Usage: cargo xtask <command>

Commands:
  help          Show this help

Options:
  -h, --help    Show this help

No task commands are registered yet.
";

#[derive(Debug, PartialEq, Eq)]
enum Command {
    Help,
}

#[derive(Debug, PartialEq, Eq)]
enum CliError {
    MissingCommand,
    UnknownCommand(OsString),
    UnexpectedArgument(OsString),
}

impl fmt::Display for CliError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::MissingCommand => write!(formatter, "missing command"),
            Self::UnknownCommand(command) => write!(formatter, "unknown command {command:?}"),
            Self::UnexpectedArgument(argument) => {
                write!(formatter, "unexpected argument {argument:?}")
            }
        }
    }
}

fn parse_args(mut args: impl Iterator<Item = OsString>) -> Result<Command, CliError> {
    let argument = args.next().ok_or(CliError::MissingCommand)?;
    let command = match argument.to_str() {
        Some("help" | "-h" | "--help") => Command::Help,
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
