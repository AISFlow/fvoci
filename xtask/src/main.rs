use std::ffi::OsString;
use std::fmt;
use std::process::ExitCode;

const HELP: &str = "\
FVOCI development tasks

Usage: cargo xtask <command> [arguments...]

Commands:
  help          Show this help
  rust-binaries Rust workflow executable hand-off (build, pack, unpack, run)
  selected-library
                Selected SQLite library controls (one build, exact filters)
  sqlite-build  Authenticated native SQLite static build
                (entry point: scripts/prepare-sqlite-build.sh)
  sqlite-ci     Pinned SQLite prerequisite and root Cargo entry
                (entry point: scripts/prepare-sqlite-ci.sh)

Options:
  -h, --help    Show this help
";

#[derive(Debug, PartialEq, Eq)]
enum Command {
    Help,
    RustBinaries(Vec<OsString>),
    SelectedLibrary(Vec<OsString>),
    SqliteBuild(Vec<OsString>),
    SqliteCi(Vec<OsString>),
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
        // Task commands own their remaining arguments.
        Some("rust-binaries") => return Ok(Command::RustBinaries(args.collect())),
        Some("selected-library") => return Ok(Command::SelectedLibrary(args.collect())),
        Some("sqlite-build") => return Ok(Command::SqliteBuild(args.collect())),
        Some("sqlite-ci") => return Ok(Command::SqliteCi(args.collect())),
        _ => return Err(CliError::UnknownCommand(argument)),
    };
    if let Some(argument) = args.next() {
        return Err(CliError::UnexpectedArgument(argument));
    }
    Ok(command)
}

/// Exit with a Python-compatible status: `-N` (consumer killed by signal N)
/// becomes `256 - N`, exactly as `sys.exit(-N)` did.
fn exit_status(code: i32) -> ExitCode {
    ExitCode::from((code & 0xff) as u8)
}

fn run(command: Command) -> ExitCode {
    match command {
        Command::Help => {
            print!("{HELP}");
            ExitCode::SUCCESS
        }
        Command::RustBinaries(args) => exit_status(xtask::rust_binaries::main(args)),
        Command::SelectedLibrary(args) => exit_status(xtask::selected_library::main(args)),
        Command::SqliteBuild(args) => exit_status(xtask::sqlite_build::main(args)),
        Command::SqliteCi(args) => exit_status(xtask::sqlite_ci::main(args)),
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
    use super::{exit_status, parse_args, CliError, Command};
    use std::ffi::OsString;
    use std::process::ExitCode;

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
    fn task_commands_keep_their_arguments() {
        let args = ["sqlite-ci", "--parent", "p", "--", "cargo", "--help"].map(OsString::from);
        assert_eq!(
            parse_args(args.clone().into_iter()),
            Ok(Command::SqliteCi(args[1..].to_vec()))
        );
        let args = ["rust-binaries", "run", "--test", "a"].map(OsString::from);
        assert_eq!(
            parse_args(args.clone().into_iter()),
            Ok(Command::RustBinaries(args[1..].to_vec()))
        );
        let args = ["selected-library", "--target-dir", "t"].map(OsString::from);
        assert_eq!(
            parse_args(args.clone().into_iter()),
            Ok(Command::SelectedLibrary(args[1..].to_vec()))
        );
        let args = ["sqlite-build", "--help"].map(OsString::from);
        assert_eq!(
            parse_args(args.clone().into_iter()),
            Ok(Command::SqliteBuild(args[1..].to_vec()))
        );
    }

    #[test]
    fn signal_return_codes_match_python_sys_exit() {
        assert_eq!(exit_status(-9), ExitCode::from(247));
        assert_eq!(exit_status(23), ExitCode::from(23));
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
