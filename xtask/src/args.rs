//! Minimal long-option parser with the argparse behaviour the SQLite entry
//! points relied on: `--name value`, `--name=value`, boolean flags, required
//! options, repeatable values (argparse `append`), `-h/--help`, and an
//! optional trailing command (argparse `REMAINDER`) that starts at the first
//! positional or after `--`.
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
    /// Every occurrence is kept in order (argparse `action="append"`).
    pub repeated: bool,
}

pub const fn value(name: &'static str) -> Spec {
    Spec {
        name,
        takes_value: true,
        required: false,
        repeated: false,
    }
}

pub const fn required(name: &'static str) -> Spec {
    Spec {
        name,
        takes_value: true,
        required: true,
        repeated: false,
    }
}

pub const fn repeated(name: &'static str) -> Spec {
    Spec {
        name,
        takes_value: true,
        required: false,
        repeated: true,
    }
}

pub const fn flag(name: &'static str) -> Spec {
    Spec {
        name,
        takes_value: false,
        required: false,
        repeated: false,
    }
}

#[derive(Debug, Default, PartialEq)]
pub struct Parsed {
    pub values: BTreeMap<&'static str, String>,
    /// Values of `repeated` options, in command-line order.
    pub lists: BTreeMap<&'static str, Vec<String>>,
    pub flags: Vec<&'static str>,
    /// Trailing command; `None` when the spec takes no command.
    pub command: Vec<OsString>,
}

impl Parsed {
    pub fn get(&self, name: &str) -> Option<&str> {
        self.values.get(name).map(String::as_str)
    }

    pub fn all(&self, name: &str) -> &[String] {
        self.lists.get(name).map_or(&[], Vec::as_slice)
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

/// Python `re` `\d` for str patterns: Unicode general category Nd, as
/// inclusive ranges from the oracle's own Python 3.14 (Unicode 16.0.0).
/// Not `char::is_numeric`, which also accepts Nl/No.
const DECIMAL_DIGITS: [(u32, u32); 71] = [
    (0x30, 0x39),
    (0x660, 0x669),
    (0x6F0, 0x6F9),
    (0x7C0, 0x7C9),
    (0x966, 0x96F),
    (0x9E6, 0x9EF),
    (0xA66, 0xA6F),
    (0xAE6, 0xAEF),
    (0xB66, 0xB6F),
    (0xBE6, 0xBEF),
    (0xC66, 0xC6F),
    (0xCE6, 0xCEF),
    (0xD66, 0xD6F),
    (0xDE6, 0xDEF),
    (0xE50, 0xE59),
    (0xED0, 0xED9),
    (0xF20, 0xF29),
    (0x1040, 0x1049),
    (0x1090, 0x1099),
    (0x17E0, 0x17E9),
    (0x1810, 0x1819),
    (0x1946, 0x194F),
    (0x19D0, 0x19D9),
    (0x1A80, 0x1A89),
    (0x1A90, 0x1A99),
    (0x1B50, 0x1B59),
    (0x1BB0, 0x1BB9),
    (0x1C40, 0x1C49),
    (0x1C50, 0x1C59),
    (0xA620, 0xA629),
    (0xA8D0, 0xA8D9),
    (0xA900, 0xA909),
    (0xA9D0, 0xA9D9),
    (0xA9F0, 0xA9F9),
    (0xAA50, 0xAA59),
    (0xABF0, 0xABF9),
    (0xFF10, 0xFF19),
    (0x104A0, 0x104A9),
    (0x10D30, 0x10D39),
    (0x10D40, 0x10D49),
    (0x11066, 0x1106F),
    (0x110F0, 0x110F9),
    (0x11136, 0x1113F),
    (0x111D0, 0x111D9),
    (0x112F0, 0x112F9),
    (0x11450, 0x11459),
    (0x114D0, 0x114D9),
    (0x11650, 0x11659),
    (0x116C0, 0x116C9),
    (0x116D0, 0x116E3),
    (0x11730, 0x11739),
    (0x118E0, 0x118E9),
    (0x11950, 0x11959),
    (0x11BF0, 0x11BF9),
    (0x11C50, 0x11C59),
    (0x11D50, 0x11D59),
    (0x11DA0, 0x11DA9),
    (0x11F50, 0x11F59),
    (0x16130, 0x16139),
    (0x16A60, 0x16A69),
    (0x16AC0, 0x16AC9),
    (0x16B50, 0x16B59),
    (0x16D70, 0x16D79),
    (0x1CCF0, 0x1CCF9),
    (0x1D7CE, 0x1D7FF),
    (0x1E140, 0x1E149),
    (0x1E2F0, 0x1E2F9),
    (0x1E4F0, 0x1E4F9),
    (0x1E5F1, 0x1E5FA),
    (0x1E950, 0x1E959),
    (0x1FBF0, 0x1FBF9),
];

fn decimal_digit(c: char) -> bool {
    let c = u32::from(c);
    DECIMAL_DIGITS
        .binary_search_by(|&(start, end)| {
            if end < c {
                std::cmp::Ordering::Less
            } else if start > c {
                std::cmp::Ordering::Greater
            } else {
                std::cmp::Ordering::Equal
            }
        })
        .is_ok()
}

/// argparse's `_negative_number_matcher`, `re.compile(r'-\.?\d').match`: a
/// prefix test, so `-1`, `-1.zip`, `-2.`, `-.5` and `-５` all match. argparse
/// treats a match as an argument only while no defined option itself looks
/// like a negative number (`_has_negative_number_optionals`); every option
/// here is `--name` or `-h`, which never match, so a match is always an
/// argument.
fn negative_number(text: &str) -> bool {
    let Some(body) = text.strip_prefix('-') else {
        return false;
    };
    let body = body.strip_prefix('.').unwrap_or(body);
    body.chars().next().is_some_and(decimal_digit)
}

/// argparse `_parse_optional`: whether `text` is an option token ("O") rather
/// than an argument ("A"). Known options and their prefixes (argparse's
/// abbreviation match, which these entries do not accept but still classify)
/// are options; a lone `-`, a token that starts like a negative number
/// (`negative_number`), or a token containing a space
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
            if spec.repeated {
                parsed.lists.entry(spec.name).or_default().push(value);
            } else {
                parsed.values.insert(spec.name, value);
            }
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
    fn repeated_values_keep_every_occurrence_in_order() {
        let specs = &[repeated("test"), flag("nocapture")];
        let Outcome::Parsed(parsed) = parse(
            os(&["--test", "b", "--nocapture", "--test=a", "--test", "b"]),
            specs,
            false,
        ) else {
            panic!()
        };
        assert_eq!(parsed.all("test"), ["b", "a", "b"]);
        assert!(parsed.values.is_empty());
        assert_eq!(
            parse(os(&[]), specs, false),
            Outcome::Parsed(Parsed::default())
        );
        assert!(matches!(
            parse(os(&["--test", "--nocapture"]), specs, false),
            Outcome::Usage(_)
        ));
    }

    #[test]
    fn decimal_digit_table_is_sorted_and_matches_nd_samples() {
        assert!(DECIMAL_DIGITS.windows(2).all(|w| w[0].1 < w[1].0));
        assert!(DECIMAL_DIGITS.iter().all(|(s, e)| s <= e));
        // 760 Nd code points in Unicode 16.0.0.
        let total: u32 = DECIMAL_DIGITS.iter().map(|(s, e)| e - s + 1).sum();
        assert_eq!(total, 760);
        for c in ['0', '9', '５', '١', '𝟎', '\u{1FBF9}'] {
            assert!(decimal_digit(c), "{c:?}");
        }
        for c in ['/', ':', '²', '½', 'Ⅻ', '٫', 'a', '\u{1FBFA}'] {
            assert!(!decimal_digit(c), "{c:?}");
        }
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
