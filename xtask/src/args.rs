//! Minimal long-option parser with the argparse behaviour the SQLite entry
//! points relied on: `--name value`, `--name=value`, boolean flags, required
//! options, `-h/--help`, and an optional trailing command (argparse
//! `REMAINDER`) that starts at the first positional or after `--`.
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

pub fn parse(argv: Vec<OsString>, specs: &[Spec], remainder: bool) -> Outcome {
    let mut parsed = Parsed::default();
    let mut args = argv.into_iter();
    while let Some(arg) = args.next() {
        let Some(text) = arg.to_str() else {
            if remainder {
                parsed.command.push(arg);
                parsed.command.extend(args);
                break;
            }
            return Outcome::Usage(format!("unrecognized arguments: {}", arg.to_string_lossy()));
        };
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
            continue;
        }
        if let Some(body) = text.strip_prefix("--") {
            let (name, inline) = match body.split_once('=') {
                Some((name, value)) => (name, Some(value.to_owned())),
                None => (body, None),
            };
            let Some(spec) = specs.iter().find(|spec| spec.name == name) else {
                return Outcome::Usage(format!("unrecognized arguments: {text}"));
            };
            if spec.takes_value {
                let value = match inline {
                    Some(value) => value,
                    None => match args.next().and_then(|v| v.into_string().ok()) {
                        Some(value) if !value.starts_with("--") || value == "--" => value,
                        _ => {
                            return Outcome::Usage(format!(
                                "argument --{name}: expected one argument"
                            ))
                        }
                    },
                };
                parsed.values.insert(spec.name, value);
            } else if inline.is_some() {
                return Outcome::Usage(format!("argument --{name}: ignored explicit argument"));
            } else if !parsed.flags.contains(&spec.name) {
                parsed.flags.push(spec.name);
            }
            continue;
        }
        if remainder && !text.starts_with('-') {
            parsed.command.push(arg);
            parsed.command.extend(args);
            break;
        }
        return Outcome::Usage(format!("unrecognized arguments: {text}"));
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
