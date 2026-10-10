//! Minimal long-option parser with the argparse behaviour the SQLite entry
//! points relied on: `--name value`, `--name=value`, boolean flags, required
//! options, `-h/--help`, and an optional trailing command (argparse
//! `REMAINDER`) that starts at the first positional or after `--`.
//! Without a trailing command, a standalone `--` ends option parsing and the
//! tokens after it are unrecognized positionals. A separate option value that
//! looks like an option (`-h`, `--`, `--x`) is a missing value, as in argparse.
//! Usage errors exit 2 like argparse; option abbreviations are not accepted.

use std::collections::BTreeMap;
use std::ffi::OsString;

#[derive(Clone, Copy)]
pub struct Spec {
    pub name: &'static str,
    pub takes_value: bool,
    pub required: bool,
}

pub const fn value(name: &'static str) -> Spec {
    Spec {
        name,
        takes_value: true,
        required: false,
    }
}

pub const fn required(name: &'static str) -> Spec {
    Spec {
        name,
        takes_value: true,
        required: true,
    }
}

pub const fn flag(name: &'static str) -> Spec {
    Spec {
        name,
        takes_value: false,
        required: false,
    }
}

#[derive(Debug, Default, PartialEq)]
pub struct Parsed {
    pub values: BTreeMap<&'static str, String>,
    pub flags: Vec<&'static str>,
    /// Trailing command; `None` when the spec takes no command.
    pub command: Vec<OsString>,
}

impl Parsed {
    pub fn get(&self, name: &str) -> Option<&str> {
        self.values.get(name).map(String::as_str)
    }

    pub fn has(&self, name: &str) -> bool {
        self.flags.contains(&name)
    }
}

#[derive(Debug, PartialEq)]
pub enum Outcome {
    Parsed(Parsed),
    Help,
    /// argparse-style usage error message; the caller exits 2.
    Usage(String),
}

/// `^-\d+$|^-\d*\.\d+$`, argparse's `_negative_number_matcher`.
fn negative_number(text: &str) -> bool {
    let Some(body) = text.strip_prefix('-') else {
        return false;
    };
    let digits = |s: &str| s.bytes().all(|b| b.is_ascii_digit());
    match body.split_once('.') {
        None => !body.is_empty() && digits(body),
        Some((whole, fraction)) => digits(whole) && !fraction.is_empty() && digits(fraction),
    }
}

/// argparse `_parse_optional`: whether `text` is an option token ("O") rather
/// than an argument ("A"). Known options and their prefixes (argparse's
/// abbreviation match, which these entries do not accept but still classify)
/// are options; a lone `-`, a negative number, or a token containing a space
/// is an argument; any other token starting with `-` is an (unknown) option.
/// `--` itself is handled by the caller.
fn option_like(text: &str, specs: &[Spec]) -> bool {
    if !text.starts_with('-') || text.len() == 1 {
        return false;
    }
    if let Some(body) = text.strip_prefix("--") {
        let prefix = body.split_once('=').map_or(body, |(prefix, _)| prefix);
        if "help".starts_with(prefix) || specs.iter().any(|spec| spec.name.starts_with(prefix)) {
            return true;
        }
    } else if text.starts_with("-h") {
        return true;
    }
    !(negative_number(text) || text.contains(' '))
}

pub fn parse(argv: Vec<OsString>, specs: &[Spec], remainder: bool) -> Outcome {
    let mut parsed = Parsed::default();
    // argparse collects unrecognized tokens and reports them after the
    // required-option check; everything after a standalone `--` is positional.
    let mut extras: Vec<String> = Vec::new();
    let mut options_ended = false;
    let mut args = argv.into_iter();
    while let Some(arg) = args.next() {
        let Some(text) = arg.to_str() else {
            if remainder {
                parsed.command.push(arg);
                parsed.command.extend(args);
                break;
            }
            extras.push(arg.to_string_lossy().into_owned());
            continue;
        };
        if options_ended {
            extras.push(text.to_owned());
            continue;
        }
        if text == "-h" || text == "--help" {
            return Outcome::Help;
        }
        if text == "--" {
            if remainder {
                // Callers strip one leading `--`, so keep it as argparse did.
                parsed.command.push(arg);
                parsed.command.extend(args);
                break;
            }
            options_ended = true;
            extras.push(text.to_owned());
            continue;
        }
        if !option_like(text, specs) {
            if remainder {
                parsed.command.push(arg);
                parsed.command.extend(args);
                break;
            }
            extras.push(text.to_owned());
            continue;
        }
        let Some(body) = text.strip_prefix("--") else {
            extras.push(text.to_owned());
            continue;
        };
        let (name, inline) = match body.split_once('=') {
            Some((name, value)) => (name, Some(value.to_owned())),
            None => (body, None),
        };
        let Some(spec) = specs.iter().find(|spec| spec.name == name) else {
            extras.push(text.to_owned());
            continue;
        };
        if spec.takes_value {
            let value = match inline {
                Some(value) => value,
                None => match args.next().and_then(|v| v.into_string().ok()) {
                    Some(value) if value != "--" && !option_like(&value, specs) => value,
                    _ => {
                        return Outcome::Usage(format!("argument --{name}: expected one argument"))
                    }
                },
            };
            parsed.values.insert(spec.name, value);
        } else if inline.is_some() {
            return Outcome::Usage(format!("argument --{name}: ignored explicit argument"));
        } else if !parsed.flags.contains(&spec.name) {
            parsed.flags.push(spec.name);
        }
    }
    let missing: Vec<String> = specs
        .iter()
        .filter(|spec| spec.required && !parsed.values.contains_key(spec.name))
        .map(|spec| format!("--{}", spec.name))
        .collect();
    if !missing.is_empty() {
        return Outcome::Usage(format!(
            "the following arguments are required: {}",
            missing.join(", ")
        ));
    }
    if !extras.is_empty() {
        return Outcome::Usage(format!("unrecognized arguments: {}", extras.join(" ")));
    }
    Outcome::Parsed(parsed)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn os(args: &[&str]) -> Vec<OsString> {
        args.iter().map(OsString::from).collect()
    }

    const SPECS: &[Spec] = &[required("archive"), value("cc"), flag("identity-only")];

    #[test]
    fn accepts_both_value_forms_and_flags() {
        let Outcome::Parsed(parsed) = parse(
            os(&["--archive", "a b", "--cc=gcc", "--identity-only"]),
            SPECS,
            false,
        ) else {
            panic!()
        };
        assert_eq!(parsed.get("archive"), Some("a b"));
        assert_eq!(parsed.get("cc"), Some("gcc"));
        assert!(parsed.has("identity-only"));
    }

    #[test]
    fn usage_errors() {
        for argv in [
            &["--cc", "x"][..],
            &["--archive"],
            &["--archive", "a", "--unknown"],
            &["--arch", "a"],
            &["--archive", "a", "positional"],
            &["--archive", "a", "--identity-only=1"],
        ] {
            assert!(
                matches!(parse(os(argv), SPECS, false), Outcome::Usage(_)),
                "{argv:?}"
            );
        }
        assert_eq!(parse(os(&["--help"]), SPECS, false), Outcome::Help);
    }

    #[test]
    fn remainder_starts_at_first_positional_or_double_dash() {
        let specs = &[value("parent"), flag("identity-only")];
        let Outcome::Parsed(parsed) = parse(
            os(&[
                "--parent",
                "p",
                "consumer",
                "--fixture-argument",
                "--parent",
            ]),
            specs,
            true,
        ) else {
            panic!()
        };
        assert_eq!(
            parsed.command,
            os(&["consumer", "--fixture-argument", "--parent"])
        );
        let Outcome::Parsed(parsed) = parse(os(&["--", "cargo", "--help"]), specs, true) else {
            panic!()
        };
        assert_eq!(parsed.command, os(&["--", "cargo", "--help"]));
    }
}
