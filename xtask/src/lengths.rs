//! `cargo xtask check`, the step "lengths": how long a Rust file and a function may be.
//!
//! The numbers are the constants of this file, and every file and function is held to
//! them: there is no list of what may be longer. Code that would go over is split.

mod measure;

use std::path::Path;

use anyhow::{Context as _, Result, bail};

use crate::files;
use crate::steps;

/// The most lines a file may have.
pub(crate) const MAX_FILE_LINES: usize = 1000;

/// The most lines a function may have, from the line of its `fn` to the line of its
/// closing brace, comments and empty lines included. (clippy's `too_many_lines`, which
/// the workspace has on as well, counts 100 lines of code alone; this is the stricter.)
pub(crate) const MAX_FUNCTION_LINES: usize = 100;

/// The most lines a file of tests may have (see [`is_test_file`]). Tests get twice the
/// limits of the code: a test file is a list of cases that are read one at a time, and
/// a test is often one long scene.
pub(crate) const MAX_TEST_FILE_LINES: usize = 2 * MAX_FILE_LINES;

/// The most lines a function of tests may have: one in a file of tests, or in a module
/// named `tests`. The step refuses a module of that name that is not `#[cfg(test)]`,
/// and a function, a type or a trait of that name, so that the name says it.
pub(crate) const MAX_TEST_FUNCTION_LINES: usize = 2 * MAX_FUNCTION_LINES;

/// The name of the module that holds tests.
pub(crate) const TEST_MODULE: &str = "tests";

/// The directories whose `.rs` files are measured.
pub(crate) const DIRECTORIES: &[&str] = &["crates", "xtask"];

/// What a length is the length of.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) enum Subject {
    /// A file, by its path from the workspace root.
    File(String),
    /// A function, by its file and its name.
    Function {
        /// Its file, by the path from the workspace root.
        file: String,
        /// Its name with the modules, the type or the trait and the functions around
        /// it: `tests::Server::start`, `<AppError as From<StoreError>>::from`. Two
        /// functions of a file may still have one name (two `impl` blocks of a type,
        /// two `cfg` variants); those are told apart by their lengths alone.
        name: String,
    },
}

/// Whether a file is one of tests: a file under the directory `tests` of a crate, next
/// to its `src`, or the file `tests.rs` of a test module.
pub(crate) fn is_test_file(file: &str) -> bool {
    let mut parts = file.split('/');
    let beside_src = parts
        .by_ref()
        .take_while(|part| *part != "src")
        .any(|part| part == TEST_MODULE);
    beside_src || file.rsplit('/').next() == Some("tests.rs")
}

impl Subject {
    /// The file the subject is, or is in.
    fn file(&self) -> &str {
        match self {
            Self::File(file) | Self::Function { file, .. } => file,
        }
    }

    /// The most lines it may have without being listed.
    fn limit(&self) -> usize {
        match self {
            Self::File(file) if is_test_file(file) => MAX_TEST_FILE_LINES,
            Self::File(_) => MAX_FILE_LINES,
            Self::Function { file, name } if is_test_file(file) || is_in_test_module(name) => {
                MAX_TEST_FUNCTION_LINES
            }
            Self::Function { .. } => MAX_FUNCTION_LINES,
        }
    }
}

/// Whether the name of a function has the test module among what is around it. What
/// is written inside `<…>` is the type and the trait of an `impl`, not around it.
fn is_in_test_module(name: &str) -> bool {
    let mut depth = 0_usize;
    let outside: String = name
        .chars()
        .filter(|character| {
            match character {
                '<' => depth += 1,
                '>' => depth = depth.saturating_sub(1),
                _ => return depth == 0,
            }
            false
        })
        .collect();
    outside.split("::").any(|part| part == TEST_MODULE)
}

impl std::fmt::Display for Subject {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::File(file) => write!(formatter, "{file}"),
            Self::Function { file, name } => write!(formatter, "{file}: fn {name}"),
        }
    }
}

/// A length that was measured.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Length {
    pub(crate) subject: Subject,
    pub(crate) lines: usize,
    /// The line the subject starts on, from 1.
    pub(crate) at: usize,
}

/// What the measuring of the sources gave.
#[derive(Debug, Default)]
pub(crate) struct Measured {
    pub(crate) lengths: Vec<Length>,
    /// What is refused whatever its length, because it would make the measure say
    /// less than there is: each a line for the reader.
    pub(crate) refused: Vec<String>,
}

/// The step: measures the files and says what is over its limit.
///
/// # Errors
///
/// Fails if a file cannot be read, if the sources hold something the step refuses
/// whatever its length, or if any file or function is over its limit.
pub(crate) fn run(root: &Path) -> Result<()> {
    let measured = measure_all(root)?;
    let mut problems = measured.refused;
    problems.extend(judge(&measured.lengths));
    if problems.is_empty() {
        steps::say(&format!(
            "    every file is within {MAX_FILE_LINES} lines and every function within \
             {MAX_FUNCTION_LINES} (tests: {MAX_TEST_FILE_LINES} and {MAX_TEST_FUNCTION_LINES})"
        ));
        return Ok(());
    }
    for problem in &problems {
        steps::say(&format!("    {problem}"));
    }
    steps::say("    over a limit: shorten the code or split it");
    bail!("{} files or functions are over their limit", problems.len())
}

/// The length of every `.rs` file under [`DIRECTORIES`] and of every function in them.
fn measure_all(root: &Path) -> Result<Measured> {
    let mut measured = Measured::default();
    for directory in DIRECTORIES {
        for file in files::rust_files(&root.join(directory))? {
            let source = std::fs::read_to_string(&file)
                .with_context(|| format!("reading {}", file.display()))?;
            let shown = files::shown_path(root, &file);
            let source = measure::measure_source(&shown, &source)
                .with_context(|| format!("reading {shown}"))?;
            measured.lengths.extend(source.lengths);
            measured.refused.extend(source.refused);
        }
    }
    Ok(measured)
}

/// Says each file and function that is over its limit, with where it starts: the files
/// first, then the functions, each in the order of their paths.
fn judge(measured: &[Length]) -> Vec<String> {
    let mut over: Vec<&Length> = measured
        .iter()
        .filter(|length| length.lines > length.subject.limit())
        .collect();
    over.sort_by(|one, other| one.subject.cmp(&other.subject));
    over.into_iter()
        .map(|length| {
            format!(
                "{}:{}: {} has {} lines, the limit is {}",
                length.subject.file(),
                length.at,
                length.subject,
                length.lines,
                length.subject.limit(),
            )
        })
        .collect()
}

#[cfg(test)]
mod tests;
