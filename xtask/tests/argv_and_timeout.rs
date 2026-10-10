//! Regressions against the argparse and `subprocess.run(timeout=...)`
//! contracts of the former Python entry points: `--` ends option parsing
//! without a trailing command, an option-like token is never consumed as a
//! separate option value, and the timeout covers pipe EOF as well as the
//! direct child's exit.

use std::ffi::OsString;
use std::process::Command;
use std::time::{Duration, Instant};
use xtask::args::{self, Outcome};
use xtask::process::{self, RunError};

const BUILD: &[args::Spec] = &[
    args::required("archive"),
    args::required("prefix"),
    args::required("target"),
    args::value("cc"),
    args::value("ar"),
    args::flag("identity-only"),
];

const CI: &[args::Spec] = &[
    args::value("parent"),
    args::value("env-file"),
    args::value("github-env"),
    args::value("github-output"),
    args::flag("identity-only"),
    args::flag("cache-fallback"),
    args::value("expected-cache-identity"),
];

fn os(args: &[&str]) -> Vec<OsString> {
    args.iter().map(OsString::from).collect()
}

fn usage(argv: &[&str], specs: &[args::Spec], remainder: bool) -> String {
    match args::parse(os(argv), specs, remainder) {
        Outcome::Usage(message) => message,
        other => panic!("{argv:?} parsed as {other:?}"),
    }
}

const T: &str = "--target=x86_64-unknown-linux-gnu";

#[test]
fn double_dash_ends_build_options() {
    assert_eq!(
        usage(
            &["--archive=a", "--prefix=p", T, "--", "--identity-only"],
            BUILD,
            false
        ),
        "unrecognized arguments: -- --identity-only"
    );
    // Required options after `--` are positionals, so they are missing.
    assert_eq!(
        usage(&["--", "--archive=a", "--prefix=p", T], BUILD, false),
        "the following arguments are required: --archive, --prefix, --target"
    );
    // `-h` after `--` is a positional, not help.
    assert_eq!(
        usage(&["--archive=a", "--prefix=p", T, "--", "-h"], BUILD, false),
        "unrecognized arguments: -- -h"
    );
}

#[test]
fn option_like_tokens_are_missing_values() {
    for value in [
        "-h",
        "--help",
        "--",
        "--cc",
        "--arch",
        "-x",
        "--unknown",
        "-hx",
        // Not Unicode Nd after `-` / `-.`: argparse classifies these as options.
        "-..5",
        "-²",
        "-½",
        "-Ⅻ",
        "-٫5",
        "-x1",
    ] {
        assert_eq!(
            usage(
                &["--archive", "a", "--prefix", "p", T, "--cc", value],
                BUILD,
                false
            ),
            "argument --cc: expected one argument",
            "{value}"
        );
        assert_eq!(
            usage(&["--parent", value], CI, true),
            "argument --parent: expected one argument",
            "{value}"
        );
    }
    // `--arch=x y` is an abbreviated option with a value, still option-like.
    assert_eq!(
        usage(
            &["--archive=a", "--prefix=p", T, "--cc", "--arch=x y"],
            BUILD,
            false
        ),
        "argument --cc: expected one argument"
    );
}

#[test]
fn argument_like_values_and_inline_values_are_kept() {
    // argparse `-\.?\d` is a prefix match over Unicode decimal digits (Nd).
    for value in [
        "-",
        "-5",
        "-1.5",
        "-.5",
        "-x y",
        "--not an option",
        "",
        "-1.zip",
        "-1tool",
        "-2.",
        "-５",
        "-١",
        "-1.５",
        "-.٥x",
        "-𝟎",
    ] {
        let Outcome::Parsed(parsed) = args::parse(
            os(&["--archive", "a", "--prefix", "p", T, "--cc", value]),
            BUILD,
            false,
        ) else {
            panic!("{value:?}")
        };
        assert_eq!(parsed.get("cc"), Some(value));
    }
    let Outcome::Parsed(parsed) =
        args::parse(os(&["--archive=-h", "--prefix=--", T]), BUILD, false)
    else {
        panic!()
    };
    assert_eq!(parsed.get("archive"), Some("-h"));
    assert_eq!(parsed.get("prefix"), Some("--"));
}

#[test]
fn trailing_command_starts_at_argument_like_tokens() {
    for first in ["-5", "-", "-x y", "consumer", "-1.zip", "-５", "-2."] {
        let Outcome::Parsed(parsed) = args::parse(os(&["--parent", "p", first, "-h"]), CI, true)
        else {
            panic!("{first:?}")
        };
        assert_eq!(parsed.command, os(&[first, "-h"]));
    }
}

#[test]
fn unrecognized_tokens_are_reported_after_required_and_help() {
    assert_eq!(
        usage(&["--unknown", "--archive", "a"], BUILD, false),
        "the following arguments are required: --prefix, --target"
    );
    assert_eq!(
        args::parse(os(&["--unknown", "-h"]), BUILD, false),
        Outcome::Help
    );
    assert_eq!(
        usage(
            &["--archive=a", "--prefix=p", T, "x", "--unknown=1"],
            BUILD,
            false
        ),
        "unrecognized arguments: x --unknown=1"
    );
}

#[test]
fn timeout_covers_pipe_eof_after_direct_child_exit() {
    let started = Instant::now();
    let error = process::run(
        Command::new("sh").args(["-c", "sleep 3 & printf out; printf err >&2; exit 0"]),
        Duration::from_millis(200),
        true,
    )
    .unwrap_err();
    assert!(matches!(error, RunError::Timeout(_)), "{error}");
    assert!(
        started.elapsed() < Duration::from_millis(2500),
        "{:?}",
        started.elapsed()
    );
}

#[test]
fn inherited_output_ignores_descendant_pipes() {
    // Without capture there are no pipes to wait for, as with subprocess.run.
    let done = process::run(
        Command::new("sh").args(["-c", "sleep 3 >/dev/null 2>&1 & exit 4"]),
        Duration::from_secs(2),
        false,
    )
    .unwrap();
    assert_eq!(process::returncode(done.status), 4);
}
