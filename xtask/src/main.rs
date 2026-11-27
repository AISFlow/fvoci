//! Development tasks of the fvoci workspace, run as `cargo xtask <command>`.
//!
//! * `cargo xtask check` — formatting, lints, unit tests, documentation, names, layers,
//!   lengths, the contract and licenses. Starts no external service.
//! * `cargo xtask test` — every test on SQLite and on PostgreSQL, and the S3 storage
//!   test, in units of which none stops the others. Prepares the containers the tests
//!   need. `--database`, `--part` and `--unit` choose fewer.
//! * `cargo xtask openapi` — fails if `contracts/openapi.json` is not the document the
//!   server prints; `--write` rewrites the file.
//! * `cargo xtask docker` — builds the images of `docker/`, starts them as an
//!   installation, tries it from outside and takes it down; `--push-plan` prints how
//!   the images go to GHCR. Not part of `check` or `test`: it takes minutes.

mod banned;
mod check;
mod containers;
mod cpus;
mod docker;
mod files;
mod layers;
mod lengths;
mod names;
mod openapi;
mod steps;
mod test;
mod times;

use std::path::PathBuf;
use std::process::ExitCode;

use clap::{Parser, Subcommand, ValueEnum};

/// Development tasks of the fvoci workspace.
#[derive(Debug, Parser)]
#[command(name = "cargo xtask", bin_name = "cargo xtask")]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Debug, Subcommand)]
enum Command {
    /// Formatting, lints, unit tests, documentation, names, layers, lengths, the
    /// contract and licenses. Starts no external service.
    Check {
        /// Leaves out the dependency check (`cargo deny`), which reads the advisory
        /// database from the network.
        #[arg(long)]
        skip_licenses: bool,
    },
    /// Runs every test on SQLite and on PostgreSQL, and the S3 storage test. Starts the
    /// test containers when they are not running and stops those it started.
    ///
    /// The tests run in units: `libs` (the unit tests of the libraries and binaries),
    /// `doc` (the documentation tests) and each integration test target by its name. A
    /// failed unit stops nothing; the summary names the failed tests and the command
    /// that runs them again, and `target/xtask-times.jsonl` keeps each unit's time.
    Test {
        /// The databases to run the tests on.
        #[arg(long, value_enum, default_value_t = Databases::All)]
        database: Databases,
        /// Runs one part of the units of one database, `2/4` the second of four: for
        /// machines that share the run. The same units and times always make the
        /// same parts.
        #[arg(long, conflicts_with = "unit")]
        part: Option<test::Part>,
        /// A time file (as `target/xtask-times.jsonl`) whose latest times of units run
        /// whole and passed weigh the units of `--part`; one that is not there, holds
        /// no record or a line that is not one fails the run. Without it every unit
        /// weighs the same.
        #[arg(long, requires = "part")]
        times: Option<PathBuf>,
        /// Runs this unit of one database only; may be given more than once.
        #[arg(long)]
        unit: Vec<String>,
        /// Arguments for the test harness of the units of `--unit` instead of their
        /// filter: `-- --exact name`.
        #[arg(last = true, requires = "unit", value_name = "HARNESS_ARGUMENTS")]
        arguments: Vec<String>,
    },
    /// Fails if `contracts/openapi.json` is not the OpenAPI document the server prints
    /// from its route definitions.
    Openapi {
        /// Rewrites the file instead of failing.
        #[arg(long)]
        write: bool,
    },
    /// Builds the server's and the search server's images for this machine, and the
    /// server's once more from a source older than the build cache, starts them with
    /// docker/compose.yaml, signs up, writes a card and finds it by search, checks what
    /// the installation must not do, and takes it down with its volumes.
    Docker {
        /// Prints the commands that put the images of this commit on GHCR instead,
        /// and runs nothing.
        #[arg(long)]
        push_plan: bool,
    },
}

/// Which databases `cargo xtask test` covers.
#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
enum Databases {
    /// SQLite, PostgreSQL and the S3 storage test.
    All,
    /// SQLite only; needs no container.
    Sqlite,
    /// PostgreSQL only.
    Postgres,
    /// The S3 storage tests only; they need PostgreSQL too.
    S3,
}

impl Databases {
    /// The one database it names, or `None` for all of them.
    fn one(self) -> Option<test::Database> {
        match self {
            Self::All => None,
            Self::Sqlite => Some(test::Database::Sqlite),
            Self::Postgres => Some(test::Database::Postgres),
            Self::S3 => Some(test::Database::S3),
        }
    }
}

/// The workspace root: the directory above this crate's manifest.
fn workspace_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("the xtask crate sits in the workspace root")
        .to_owned()
}

fn main() -> ExitCode {
    let cli = Cli::parse();
    let root = workspace_root();
    if let Err(error) = steps::catch_interrupts() {
        steps::say(&format!("error: {error:#}"));
        return ExitCode::FAILURE;
    }
    let result = match cli.command {
        Command::Check { skip_licenses } => {
            let licenses = if skip_licenses {
                check::Licenses::Skip
            } else {
                check::Licenses::Check
            };
            check::run(&root, licenses)
        }
        Command::Test {
            database,
            part,
            times,
            unit,
            arguments,
        } => {
            let choice = match (part, unit.is_empty()) {
                (Some(part), _) => test::Choice::Part { part, times },
                (None, false) => test::Choice::Units {
                    names: unit,
                    arguments,
                },
                (None, true) => test::Choice::Every,
            };
            test::run(&root, database.one(), &choice)
        }
        Command::Openapi { write } => {
            let mode = if write {
                openapi::Mode::Write
            } else {
                openapi::Mode::Check
            };
            openapi::run(&root, mode)
        }
        Command::Docker { push_plan: true } => docker::print_push_plan(&root),
        Command::Docker { push_plan: false } => docker::run(&root),
    };
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            steps::say(&format!("error: {error:#}"));
            ExitCode::FAILURE
        }
    }
}

#[cfg(test)]
mod tests {
    use clap::CommandFactory as _;

    use super::*;

    #[test]
    fn the_command_line_definition_is_consistent() {
        Cli::command().debug_assert();
    }

    #[test]
    fn the_workspace_root_holds_the_workspace_manifest() {
        let manifest = workspace_root().join("Cargo.toml");
        let text = std::fs::read_to_string(&manifest).expect("the workspace manifest is readable");
        assert!(
            text.contains("[workspace]"),
            "{} is the workspace manifest",
            manifest.display()
        );
    }
}
