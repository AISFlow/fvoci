//! POSIX shell quoting compatible with Python's `shlex.quote`/`shlex.join`,
//! and the narrow `export NAME=VALUE` reader the SQLite wrapper accepts.

/// Same output as Python 3 `shlex.quote`.
pub fn quote(value: &str) -> String {
    if value.is_empty() {
        return "''".to_owned();
    }
    let safe = value
        .bytes()
        .all(|b| b.is_ascii_alphanumeric() || b"_@%+=:,./-".contains(&b));
    if safe {
        value.to_owned()
    } else {
        format!("'{}'", value.replace('\'', "'\"'\"'"))
    }
}

/// Same output as Python 3 `shlex.join`.
pub fn join<I, S>(argv: I) -> String
where
    I: IntoIterator<Item = S>,
    S: AsRef<str>,
{
    argv.into_iter()
        .map(|arg| quote(arg.as_ref()))
        .collect::<Vec<_>>()
        .join(" ")
}

/// Python `shlex.split` (POSIX mode, no comments in the forms used here) for
/// one line. `None` for an unterminated quote or trailing escape, where
/// Python raised ValueError.
pub fn split_quoted(line: &str) -> Option<Vec<String>> {
    let mut words = Vec::new();
    let mut current = String::new();
    let mut in_word = false;
    let mut chars = line.chars();
    while let Some(c) = chars.next() {
        match c {
            ' ' | '\t' => {
                if in_word {
                    words.push(std::mem::take(&mut current));
                    in_word = false;
                }
            }
            '\'' => {
                in_word = true;
                loop {
                    match chars.next()? {
                        '\'' => break,
                        other => current.push(other),
                    }
                }
            }
            '"' => {
                in_word = true;
                loop {
                    match chars.next()? {
                        '"' => break,
                        '\\' => match chars.next()? {
                            escaped @ ('"' | '\\') => current.push(escaped),
                            other => {
                                current.push('\\');
                                current.push(other);
                            }
                        },
                        other => current.push(other),
                    }
                }
            }
            '\\' => {
                in_word = true;
                current.push(chars.next()?);
            }
            other => {
                in_word = true;
                current.push(other);
            }
        }
    }
    if in_word {
        words.push(current);
    }
    Some(words)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn quote_matches_python_shlex() {
        // Expected values from Python 3.14 `shlex.quote`.
        assert_eq!(quote(""), "''");
        assert_eq!(
            quote("/tmp/a-b_c.d/e:f,g@h%i+j=k"),
            "/tmp/a-b_c.d/e:f,g@h%i+j=k"
        );
        assert_eq!(quote("/tmp/owned parent/x"), "'/tmp/owned parent/x'");
        assert_eq!(quote("it's"), "'it'\"'\"'s'");
        assert_eq!(quote("caf\u{e9}"), "'caf\u{e9}'");
        assert_eq!(join(["cc", "-O2", "a b"]), "cc -O2 'a b'");
    }

    #[test]
    fn split_round_trips_quote() {
        for value in ["plain", "/tmp/owned parent/lib", "it's", "", "a'b'c d"] {
            let line = format!("export K={}", quote(value));
            assert_eq!(
                split_quoted(&line).unwrap(),
                vec!["export".to_owned(), format!("K={value}")]
            );
        }
    }

    #[test]
    fn split_matches_python_shlex_posix() {
        // Expected values from Python 3.14 `shlex.split`.
        let pair = |v: &str| Some(vec!["export".to_owned(), format!("K={v}")]);
        assert_eq!(split_quoted("export K=\"x\""), pair("x"));
        assert_eq!(split_quoted("export K=a\\ b"), pair("a b"));
        assert_eq!(split_quoted("export K=\"a\\$b\\\"c\""), pair("a\\$b\"c"));
        assert_eq!(split_quoted("export K='open"), None);
        assert_eq!(split_quoted("export K=\"open"), None);
        assert_eq!(split_quoted("export K=\\"), None);
    }
}
