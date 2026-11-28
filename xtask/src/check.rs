//! `cargo xtask check`: everything that needs no external service.

use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{Result, bail};

use crate::steps::{self, Step};
use crate::times;

/// Whether the dependency check runs.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Licenses {
    Check,
    Skip,
}

/// Where `cargo install --root target/tools` puts the tools this workspace uses.
const LOCAL_TOOLS: &str = "target/tools/bin";

/// Runs the checks. Every check runs, also after one failed.
///
/// # Errors
///
/// Fails if a check failed.
pub(crate) fn run(root: &Path, licenses: Licenses) -> Result<()> {
    let cargo = steps::cargo();
    let test_threads: Vec<(&str, String)> = crate::cpus::test_threads()?.into_iter().collect();
    let mut checks = vec![
        Step::new("format", || {
            steps::run(root, &cargo, &["fmt", "--all", "--check"], &[])
        }),
        Step::new("clippy", || {
            steps::run(
                root,
                &cargo,
                &[
                    "clippy",
                    "--workspace",
                    "--all-targets",
                    "--all-features",
                    "--locked",
                    "--",
                    "-D",
                    "warnings",
                ],
                &[],
            )
        }),
        Step::new("unit tests", || {
            let environment: Vec<(&str, &str)> = test_threads
                .iter()
                .map(|(name, value)| (*name, value.as_str()))
                .collect();
            steps::run(
                root,
                &cargo,
                &["test", "--workspace", "--lib", "--bins", "--locked"],
                &environment,
            )
        }),
        Step::new("documentation", || {
            steps::run(
                root,
                &cargo,
                &["doc", "--workspace", "--no-deps", "--locked"],
                &[("RUSTDOCFLAGS", "-D warnings")],
            )
        }),
        Step::new("names", || {
            // Both run, so that one's failure does not hide the other's.
            let banned = crate::banned::run(root);
            let workspaces = crate::names::run(root);
            banned.and(workspaces)
        }),
        Step::new("layers", || {
            // Both run, so that one's failure does not hide the other's.
            let layers = crate::layers::run(root);
            let locks = crate::locks::run(root);
            layers.and(locks)
        }),
        Step::new("lengths", || crate::lengths::run(root)),
        Step::new("contract", || {
            crate::openapi::run(root, crate::openapi::Mode::Check)
        }),
    ];
    checks.push(match licenses {
        Licenses::Check => Step::new("licenses and advisories", || {
            let cargo_deny = tool(root, "cargo-deny");
            // The hint is for a tool that is not there; a licence or an advisory that
            // fails the check is reported as what it is.
            if Command::new(&cargo_deny).arg("--version").output().is_err() {
                bail!(
                    "cargo-deny is needed; install it with \
                     `cargo install cargo-deny --locked --root target/tools`"
                );
            }
            steps::run(root, cargo_deny, &["check"], &[])
        }),
        Licenses::Skip => Step::skipped("licenses and advisories", "--skip-licenses"),
    });
    let clock = times::Clock::start(root);
    let ran = steps::run_all("check", checks);
    times::append(root, &times::Record::new("check", &clock, &ran));
    ran.into_result()
}

/// The path of a tool: the copy under `target/tools` if there is one, else the name, for
/// the search path to resolve.
fn tool(root: &Path, name: &str) -> PathBuf {
    let local = root.join(LOCAL_TOOLS).join(name);
    if local.is_file() {
        local
    } else {
        PathBuf::from(name)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A directory for one test's files, under the workspace's `target/`.
    fn scratch(name: &str) -> PathBuf {
        crate::workspace_root()
            .join("target")
            .join("tmp")
            .join("xtask")
            .join(name)
    }

    #[test]
    fn a_tool_under_target_tools_is_preferred() {
        let root = scratch("tool-lookup");
        let _ = std::fs::remove_dir_all(&root);
        assert_eq!(
            tool(&root, "cargo-deny"),
            PathBuf::from("cargo-deny"),
            "without a local copy the search path decides"
        );
        std::fs::create_dir_all(root.join(LOCAL_TOOLS)).expect("the directory is created");
        std::fs::write(root.join(LOCAL_TOOLS).join("cargo-deny"), "").expect("the file is written");
        assert_eq!(
            tool(&root, "cargo-deny"),
            root.join(LOCAL_TOOLS).join("cargo-deny"),
            "a local copy wins"
        );
        let _ = std::fs::remove_dir_all(&root);
    }
}
