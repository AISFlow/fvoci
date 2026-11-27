//! The pinned recordings in `golden/` and the comparison against them.
//!
//! A recording that differs from its golden file fails the test. When an answer is
//! changed on purpose, run the tests once with `FVOCI_REGRESSION_UPDATE=1` to rewrite the
//! golden files, and commit them with the change.

use std::fmt::Write as _;
use std::path::PathBuf;

use serde_json::Value;

use crate::harness::Step;

/// Set to `1` to rewrite the golden files instead of comparing.
const UPDATE_ENV: &str = "FVOCI_REGRESSION_UPDATE";

/// How many differing places of one body are listed.
const BODY_DIFF_LINES: usize = 12;

/// How much of a differing value is shown.
const SHOWN_VALUE_LENGTH: usize = 160;

fn golden_path(file_name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("regression")
        .join("golden")
        .join(file_name)
}

/// Whether this run records instead of compares: only if the variable is exactly `1`.
///
/// # Panics
///
/// Panics on any other non-empty value, and when recording is asked for under `CI`. A
/// run that records passes whatever the server answers, so it must never be entered by
/// a value that was meant as "off" (`0`, `false`) or in a place that is there to verify.
fn updating() -> bool {
    match std::env::var(UPDATE_ENV).as_deref() {
        Err(_) | Ok("") => false,
        Ok("1") => {
            assert!(
                std::env::var_os("CI").is_none(),
                "{UPDATE_ENV}=1 under CI: a run that records verifies nothing"
            );
            true
        }
        Ok(other) => panic!(
            "{UPDATE_ENV}={other:?} is neither unset nor 1; unset it to compare, set it to 1 to record"
        ),
    }
}

/// Compares `actual` with the golden file, or rewrites the file when updating.
/// `differences` describes where the two differ, given the golden text.
#[track_caller]
fn check(file_name: &str, actual: &str, differences: impl FnOnce(&str) -> String) {
    let path = golden_path(file_name);
    if updating() {
        std::fs::create_dir_all(path.parent().expect("a golden file has a directory"))
            .unwrap_or_else(|error| {
                panic!("creating the directory of {}: {error}", path.display())
            });
        let before = std::fs::read_to_string(&path).ok();
        std::fs::write(&path, actual)
            .unwrap_or_else(|error| panic!("writing {}: {error}", path.display()));
        // Said out loud, so that a run that recorded does not read like one that passed.
        let verdict = match before.as_deref() {
            Some(before) if before == actual => "unchanged",
            Some(_) => "CHANGED",
            None => "NEW",
        };
        #[expect(
            clippy::print_stderr,
            reason = "the one line a recording run has to say; the test harness shows it"
        )]
        {
            eprintln!("recorded {file_name}: {verdict} (not compared)");
        }
        return;
    }
    let expected = std::fs::read_to_string(&path).unwrap_or_else(|error| {
        panic!(
            "reading {}: {error}\nrun the test with {UPDATE_ENV}=1 to record it",
            path.display()
        )
    });
    assert!(
        expected == actual,
        "the answers differ from {}:\n{}\nIf the change is intended, rerun with {UPDATE_ENV}=1 \
         and commit the golden file with it.",
        path.display(),
        differences(&expected),
    );
}

/// Checks a scenario's recording against `golden/<scenario>.json`.
#[track_caller]
pub(crate) fn check_steps(scenario: &str, steps: &[Step]) {
    let mut actual = serde_json::to_string_pretty(steps).expect("steps serialize to JSON");
    actual.push('\n');
    check(
        &format!("{scenario}.json"),
        &actual,
        |expected| match serde_json::from_str::<Vec<Step>>(expected) {
            Ok(expected) => step_differences(&expected, steps),
            Err(error) => format!("  the golden file does not parse: {error}"),
        },
    );
}

/// Checks a plain-text table against `golden/<name>.txt`. `differences` gets the golden
/// text and describes where the table differs from it.
#[track_caller]
pub(crate) fn check_table(name: &str, table: &str, differences: impl FnOnce(&str) -> String) {
    check(&format!("{name}.txt"), table, differences);
}

fn step_differences(expected: &[Step], actual: &[Step]) -> String {
    let mut report = String::new();
    if expected.len() != actual.len() {
        let _ = writeln!(
            report,
            "  the golden file has {} steps, this run recorded {}",
            expected.len(),
            actual.len()
        );
    }
    for (want, got) in expected.iter().zip(actual) {
        if want.name != got.name {
            let _ = writeln!(
                report,
                "  step order differs: expected {:?}, got {:?}",
                want.name, got.name
            );
            continue;
        }
        let mut parts = Vec::new();
        if want.request != got.request {
            parts.push(format!("request {:?} vs {:?}", want.request, got.request));
        }
        if want.status != got.status {
            parts.push(format!("status {} vs {}", want.status, got.status));
        }
        if want.content_type != got.content_type {
            parts.push(format!(
                "content type {:?} vs {:?}",
                want.content_type, got.content_type
            ));
        }
        if want.headers != got.headers {
            parts.push(format!("headers {:?} vs {:?}", want.headers, got.headers));
        }
        if want.body != got.body {
            parts.push(format!(
                "body:\n{}",
                body_differences(&want.body, &got.body)
            ));
        }
        if !parts.is_empty() {
            let _ = writeln!(
                report,
                "  {} [{}] (expected vs got)\n    {}",
                got.name,
                got.request,
                parts.join("; ")
            );
        }
    }
    report
}

/// A compact description of where two normalized JSON values differ.
fn body_differences(expected: &Value, actual: &Value) -> String {
    fn walk(expected: &Value, actual: &Value, place: &str, lines: &mut Vec<String>) {
        if lines.len() >= BODY_DIFF_LINES {
            return;
        }
        match (expected, actual) {
            (Value::Object(want), Value::Object(got)) => {
                let mut names: Vec<&String> = want.keys().chain(got.keys()).collect();
                names.sort();
                names.dedup();
                for name in names {
                    match (want.get(name), got.get(name)) {
                        (Some(a), Some(b)) => walk(a, b, &format!("{place}.{name}"), lines),
                        (Some(a), None) => {
                            lines
                                .push(format!("      {place}.{name}: only expected: {}", short(a)));
                        }
                        (None, Some(b)) => {
                            lines.push(format!("      {place}.{name}: only got: {}", short(b)));
                        }
                        (None, None) => {}
                    }
                }
            }
            (Value::Array(want), Value::Array(got)) => {
                if want.len() != got.len() {
                    lines.push(format!(
                        "      {place}: {} items vs {} items",
                        want.len(),
                        got.len()
                    ));
                }
                for (index, (a, b)) in want.iter().zip(got).enumerate() {
                    walk(a, b, &format!("{place}[{index}]"), lines);
                }
            }
            (a, b) if a != b => lines.push(format!("      {place}: {} vs {}", short(a), short(b))),
            _ => {}
        }
    }

    let mut lines = Vec::new();
    walk(expected, actual, "$", &mut lines);
    lines.join("\n")
}

fn short(value: &Value) -> String {
    let text = value.to_string();
    if text.chars().count() <= SHOWN_VALUE_LENGTH {
        return text;
    }
    let cut: String = text.chars().take(SHOWN_VALUE_LENGTH - 3).collect();
    format!("{cut}...")
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    fn step(name: &str, status: u16, body: Value) -> Step {
        Step {
            name: name.to_owned(),
            request: format!("GET /{name} as anonymous"),
            status,
            content_type: "application/json".to_owned(),
            headers: std::collections::BTreeMap::new(),
            body,
        }
    }

    #[test]
    fn equal_recordings_have_no_differences() {
        let steps = [step("a", 200, json!({"x": 1}))];
        assert_eq!(
            step_differences(&steps, &steps),
            "",
            "a recording equals itself"
        );
    }

    #[test]
    fn a_changed_status_and_body_are_reported_with_their_place() {
        let expected = [step("a", 200, json!({"x": [1, 2], "y": "old"}))];
        let actual = [step("a", 404, json!({"x": [1, 3], "z": true}))];
        let report = step_differences(&expected, &actual);
        for part in [
            "status 200 vs 404",
            "$.x[1]: 2 vs 3",
            "$.y: only expected: \"old\"",
            "$.z: only got: true",
        ] {
            assert!(report.contains(part), "{part:?} is missing from:\n{report}");
        }
    }

    #[test]
    fn a_missing_step_is_reported() {
        let expected = [step("a", 200, json!(null)), step("b", 200, json!(null))];
        let report = step_differences(&expected, &expected[..1]);
        assert!(
            report.contains("the golden file has 2 steps, this run recorded 1"),
            "the step counts are missing from:\n{report}"
        );
    }
}
