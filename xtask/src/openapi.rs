//! `cargo xtask openapi`: the contract file, and whether it still says what the server
//! does.
//!
//! The contract is `contracts/openapi.json`: the OpenAPI document the server builds from
//! its own route definitions and prints with `--print-openapi`. The file is committed, so
//! that a change of the API shows as a change of that file, and so that the web app can
//! generate its types from it without building the server. This task fails when the file
//! is not what the server prints now; `--write` makes it so.

use std::path::Path;
use std::process::{Command, Stdio};

use anyhow::{Context as _, Result, bail};

use crate::steps;

/// The contract file, relative to the workspace root.
pub(crate) const CONTRACT: &str = "contracts/openapi.json";

/// What the task does with a contract file that is stale.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Mode {
    /// Fails, and says where the file and the server part.
    Check,
    /// Rewrites the file.
    Write,
}

/// Compares the contract file with what the server prints, or rewrites it.
///
/// # Errors
///
/// Fails if the server cannot be built or run, if the file cannot be read or written,
/// or, when checking, if the file is not what the server prints.
pub(crate) fn run(root: &Path, mode: Mode) -> Result<()> {
    let printed = printed_by_the_server(root)?;
    let path = root.join(CONTRACT);
    match mode {
        Mode::Write => {
            if std::fs::read_to_string(&path).is_ok_and(|committed| committed == printed) {
                steps::say(&format!("    {CONTRACT} is up to date"));
                return Ok(());
            }
            if let Some(directory) = path.parent() {
                std::fs::create_dir_all(directory)
                    .with_context(|| format!("creating {}", directory.display()))?;
            }
            std::fs::write(&path, &printed)
                .with_context(|| format!("writing {}", path.display()))?;
            steps::say(&format!("    wrote {CONTRACT}"));
            Ok(())
        }
        Mode::Check => {
            let committed = std::fs::read_to_string(&path).with_context(|| {
                format!("reading {CONTRACT}; create it with `cargo xtask openapi --write`")
            })?;
            match first_difference(&committed, &printed) {
                None => {
                    steps::say(&format!("    {CONTRACT} is what the server prints"));
                    Ok(())
                }
                Some(difference) => bail!(
                    "{CONTRACT} is stale: {difference}; \
                     update it with `cargo xtask openapi --write` and commit the change"
                ),
            }
        }
    }
}

/// Builds the server and returns the document it prints.
fn printed_by_the_server(root: &Path) -> Result<String> {
    let arguments = [
        "run",
        "--quiet",
        "--locked",
        "--package",
        "fvoci-server",
        "--",
        "--print-openapi",
    ];
    let shown = format!("cargo {}", arguments.join(" "));
    steps::say(&format!("    $ {shown}"));
    let output = Command::new(steps::cargo())
        .args(arguments)
        .current_dir(root)
        .stderr(Stdio::inherit())
        .output()
        .with_context(|| format!("starting `{shown}`"))?;
    if !output.status.success() {
        bail!("`{shown}` ended with {}", output.status);
    }
    String::from_utf8(output.stdout).context("the printed document is not UTF-8")
}

/// Where two texts first differ, as a sentence, or `None` if they are the same.
fn first_difference(committed: &str, printed: &str) -> Option<String> {
    if committed == printed {
        return None;
    }
    let mut committed_lines = committed.lines();
    let mut printed_lines = printed.lines();
    let mut number = 0_usize;
    loop {
        number += 1;
        match (committed_lines.next(), printed_lines.next()) {
            (Some(file), Some(server)) if file == server => {}
            (Some(file), Some(server)) => {
                // As they are, with what stands before and after the text: lines that
                // differ in nothing but that would otherwise be shown as the same.
                return Some(format!(
                    "line {number} is {file:?} in the file and {server:?} from the server"
                ));
            }
            (Some(_), None) => {
                return Some(format!(
                    "the file goes on after line {}, where the server's document ends",
                    number - 1
                ));
            }
            (None, Some(_)) => {
                return Some(format!(
                    "the server's document goes on after line {}, where the file ends",
                    number - 1
                ));
            }
            // The same lines: the texts differ in their line ends only.
            (None, None) => return Some("the line ends differ".to_owned()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_same_text_has_no_difference() {
        assert_eq!(first_difference("{\n}\n", "{\n}\n"), None);
    }

    #[test]
    fn a_changed_line_is_named_with_both_sides() {
        let difference =
            first_difference("{\n  \"a\": 1\n}\n", "{\n  \"a\": 2\n}\n").expect("the texts differ");
        assert_eq!(
            difference,
            r#"line 2 is "  \"a\": 1" in the file and "  \"a\": 2" from the server"#
        );
    }

    #[test]
    fn lines_that_differ_in_their_indentation_are_shown_to_differ() {
        let difference = first_difference("{\n  \"a\": 1\n}\n", "{\n    \"a\": 1\n}\n")
            .expect("the texts differ");
        assert_eq!(
            difference,
            r#"line 2 is "  \"a\": 1" in the file and "    \"a\": 1" from the server"#
        );
    }

    #[test]
    fn a_longer_side_is_named() {
        assert_eq!(
            first_difference("a\nb\n", "a\n").as_deref(),
            Some("the file goes on after line 1, where the server's document ends")
        );
        assert_eq!(
            first_difference("a\n", "a\nb\n").as_deref(),
            Some("the server's document goes on after line 1, where the file ends")
        );
    }

    #[test]
    fn line_ends_alone_are_a_difference() {
        assert_eq!(
            first_difference("a\n", "a").as_deref(),
            Some("the line ends differ")
        );
    }
}
