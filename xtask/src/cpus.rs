//! `FVOCI_TEST_CPUS`: how many processors the tests may use, said in one place.
//!
//! Set, it is the number of processors every test tool behaves as if the machine had:
//! each `cargo test` the tasks start runs that many test threads, as libtest does on a
//! machine of that size, and the web app's Playwright and Vitest pick their workers from
//! it the same way (`apps/web/tools/test-cpus.ts`). A variable of one tool that is set
//! too (`RUST_TEST_THREADS`) wins over it, as does a tool's own flag (`--workers`).
//! Unset, every tool picks from the machine as it always did.

use std::ffi::OsString;
use std::num::NonZeroUsize;

use anyhow::{Result, bail};

/// The variable that gives the tests their processors.
pub(crate) const TEST_CPUS_ENV: &str = "FVOCI_TEST_CPUS";

/// libtest's own variable for the number of its threads.
const TEST_THREADS_ENV: &str = "RUST_TEST_THREADS";

/// The processors `FVOCI_TEST_CPUS` gives the tests, if it is set.
///
/// # Errors
///
/// Fails if it is set to something that is not a whole number of 1 or more.
pub(crate) fn test_cpus() -> Result<Option<NonZeroUsize>> {
    parse(std::env::var_os(TEST_CPUS_ENV))
}

/// What a `cargo test` started by a task adds to its environment: `RUST_TEST_THREADS`
/// from `FVOCI_TEST_CPUS`, unless the caller set `RUST_TEST_THREADS` itself.
///
/// # Errors
///
/// Fails if `FVOCI_TEST_CPUS` is not a whole number of 1 or more.
pub(crate) fn test_threads() -> Result<Option<(&'static str, String)>> {
    Ok(threads_for(
        test_cpus()?,
        std::env::var_os(TEST_THREADS_ENV).as_ref(),
    ))
}

fn parse(value: Option<OsString>) -> Result<Option<NonZeroUsize>> {
    let Some(value) = value else {
        return Ok(None);
    };
    let text = value.to_string_lossy();
    let text = text.trim();
    if text.is_empty() {
        return Ok(None);
    }
    // Digits only, as the web app's tools take it: `+4` is no number of processors there.
    let digits = text.bytes().all(|byte| byte.is_ascii_digit());
    match text.parse::<NonZeroUsize>().ok().filter(|_| digits) {
        Some(cpus) => Ok(Some(cpus)),
        None => {
            bail!("{TEST_CPUS_ENV} is `{text}`: give a whole number of processors, 1 or more")
        }
    }
}

fn threads_for(
    cpus: Option<NonZeroUsize>,
    test_threads: Option<&OsString>,
) -> Option<(&'static str, String)> {
    if test_threads.is_some() {
        return None;
    }
    cpus.map(|cpus| (TEST_THREADS_ENV, cpus.to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_whole_number_of_one_or_more_is_taken_and_nothing_else() {
        let four = NonZeroUsize::new(4);
        assert_eq!(parse(Some(" 4 ".into())).ok(), Some(four), "spaces around");
        assert_eq!(parse(None).ok(), Some(None), "unset");
        assert_eq!(parse(Some(String::new().into())).ok(), Some(None), "empty");
        for wrong in ["0", "-2", "+4", "1.5", "four", "4x"] {
            let error = parse(Some(wrong.into())).expect_err(wrong);
            assert!(
                error.to_string().contains(TEST_CPUS_ENV),
                "the error names the variable: {error}"
            );
        }
    }

    #[test]
    fn the_processors_become_test_threads_unless_libtest_was_told_itself() {
        let six = NonZeroUsize::new(6);
        assert_eq!(
            threads_for(six, None),
            Some(("RUST_TEST_THREADS", "6".to_owned())),
            "the test threads follow the processors"
        );
        assert_eq!(
            threads_for(six, Some(&"8".into())),
            None,
            "RUST_TEST_THREADS set by the caller stays as it is"
        );
        assert_eq!(threads_for(None, None), None, "unset, nothing changes");
    }
}
