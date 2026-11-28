//! `cargo xtask check`, in the step "layers": where the store may write a row lock.
//!
//! A statement of the store holds rows through `Scope::row_lock`, `Scope::hold` and
//! `Scope::first_held` (`crates/store/src/db/locks.rs`), which record each lock at its
//! rank as they take it, so that a debug build sees the order a transaction takes its
//! locks in. A locking clause written anywhere else would take a lock that it does not
//! see. The step refuses one ([`holds_locking_clause`]) in the string literals of every
//! source of [`STORE_SOURCES`], their tests included, but the database layer itself
//! ([`is_the_database_layer`]). Comments and documentation say what they like. A debug
//! build of the store looks for the same words, past SQL comments, in each statement it
//! runs, however its text was put together (`check_clause`, beside `Sql::push_lock`).

use std::path::Path;

use anyhow::{Context as _, Result, anyhow, bail};
use proc_macro2::{Delimiter, Group, Literal, TokenStream, TokenTree};

use crate::{files, steps};

/// The sources of the store, from the workspace root.
const STORE_SOURCES: &str = "crates/store/src";

/// The words after `FOR` that make a locking clause, in lowercase.
const LOCKS: [&[&str]; 4] = [
    &["update"],
    &["no", "key", "update"],
    &["share"],
    &["key", "share"],
];

/// Fails if a source of the store outside its database layer writes a locking clause.
///
/// # Errors
///
/// Fails on the places it finds, and if a source cannot be read or is no Rust.
pub(crate) fn run(root: &Path) -> Result<()> {
    let sources = root.join(STORE_SOURCES);
    let mut read = 0;
    let mut places = Vec::new();
    for file in files::rust_files(&sources)? {
        let relative = file.strip_prefix(&sources).unwrap_or(&file);
        if is_the_database_layer(relative) {
            continue;
        }
        let shown = files::shown_path(root, &file);
        let text = std::fs::read_to_string(&file).with_context(|| format!("reading {shown}"))?;
        read += 1;
        let lines = locking_lines(&text).with_context(|| format!("reading {shown}"))?;
        places.extend(lines.into_iter().map(|line| format!("{shown}:{line}")));
    }
    if places.is_empty() {
        steps::say(&format!(
            "    no locking clause in the {read} sources of the store outside its database layer"
        ));
        return Ok(());
    }
    for place in &places {
        steps::say(&format!("    {place}"));
    }
    steps::say(
        "    these write a locking clause: hold the rows through Scope::row_lock, hold or \
         first_held (crates/store/src/db/locks.rs), which record the lock at its rank",
    );
    bail!("{} places write a locking clause", places.len())
}

/// Whether a source, by its path under [`STORE_SOURCES`], is of the database layer,
/// which writes the clauses: `db.rs` and what is under `db/`.
fn is_the_database_layer(relative: &Path) -> bool {
    relative == Path::new("db.rs") || relative.starts_with("db")
}

/// The lines, from 1, of the string literals of `source` that hold a locking clause.
///
/// # Errors
///
/// Fails if `source` is no Rust that can be read into tokens.
fn locking_lines(source: &str) -> Result<Vec<usize>> {
    let tokens: TokenStream = source.parse().map_err(|error| anyhow!("{error}"))?;
    let mut lines = Vec::new();
    collect_lines(tokens, &mut lines);
    lines.dedup();
    Ok(lines)
}

/// Adds to `lines` those of the string literals of `tokens` and of the groups in them
/// that hold a locking clause, past the documentation (`#[doc = "…"]`, as the reader
/// gives `///`; it drops the other comments).
fn collect_lines(tokens: TokenStream, lines: &mut Vec<usize>) {
    let mut tokens = tokens.into_iter().peekable();
    while let Some(token) = tokens.next() {
        match token {
            TokenTree::Punct(punct) if punct.as_char() == '#' => {
                if matches!(tokens.peek(), Some(TokenTree::Punct(bang)) if bang.as_char() == '!') {
                    tokens.next();
                }
                if matches!(tokens.peek(), Some(TokenTree::Group(group)) if is_documentation(group))
                {
                    tokens.next();
                }
            }
            TokenTree::Group(group) => collect_lines(group.stream(), lines),
            TokenTree::Literal(literal) => {
                if string_value(&literal).is_some_and(|value| holds_locking_clause(&value)) {
                    lines.push(literal.span().start().line);
                }
            }
            TokenTree::Punct(_) | TokenTree::Ident(_) => {}
        }
    }
}

/// Whether `group` is the inside of a documentation attribute: `[doc = "…"]`.
fn is_documentation(group: &Group) -> bool {
    group.delimiter() == Delimiter::Bracket
        && matches!(group.stream().into_iter().next(), Some(TokenTree::Ident(name)) if name == "doc")
}

/// The text of a string literal, raw or not; `None` for any other literal.
fn string_value(literal: &Literal) -> Option<String> {
    syn::parse_str::<syn::LitStr>(&literal.to_string())
        .ok()
        .map(|text| text.value())
}

/// Whether `text` holds a locking clause: `FOR` and then `UPDATE`, `NO KEY UPDATE`,
/// `SHARE` or `KEY SHARE`, as words in any case with any space between.
fn holds_locking_clause(text: &str) -> bool {
    let words: Vec<String> = text
        .split(|c: char| !(c.is_alphanumeric() || c == '_'))
        .filter(|word| !word.is_empty())
        .map(str::to_lowercase)
        .collect();
    words.iter().enumerate().any(|(at, word)| {
        let after = &words[at + 1..];
        word == "for"
            && LOCKS.iter().any(|lock| {
                lock.len() <= after.len() && after.iter().zip(lock.iter()).all(|(a, b)| a == b)
            })
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::files::testing::{scratch, write};

    #[test]
    fn each_lock_is_a_clause_in_any_case_and_spacing() {
        for text in [
            "SELECT id FROM users WHERE id = $1 FOR UPDATE",
            " for no key\n   update",
            " FOR  SHARE OF w",
            " For Key Share SKIP LOCKED",
            ") FOR UPDATE)",
        ] {
            assert!(holds_locking_clause(text), "{text:?}");
        }
    }

    #[test]
    fn words_that_only_look_alike_are_no_clause() {
        for text in [
            "waiting for updates",
            "FORUPDATE",
            "for sharing",
            "a key share",
            "FOR KEY",
            "UPDATE users SET name = $1",
            "for",
        ] {
            assert!(!holds_locking_clause(text), "{text:?}");
        }
    }

    #[test]
    fn literals_are_read_and_comments_are_not() {
        let source = concat!(
            "//! A module FOR UPDATE.\n",
            "/// Holds FOR SHARE.\n",
            "/** FOR KEY SHARE */\n",
            "fn f() {\n",
            "    // FOR UPDATE\n",
            "    sql.push(\" FOR UPDATE\");\n",
            "    let raw = r#\"SELECT 1 FOR NO KEY UPDATE\"#;\n",
            "    let not = b\" FOR SHARE\";\n",
            "    #[cfg(test)]\n",
            "    let x = [\" FOR SHARE OF c\", \"no lock\"];\n",
            "}\n",
        );
        assert_eq!(locking_lines(source).expect("the source reads"), [6, 7, 10]);
    }

    #[test]
    fn source_that_is_no_rust_fails() {
        assert!(locking_lines("fn f( {").is_err());
    }

    #[test]
    fn the_database_layer_writes_clauses_and_the_rest_of_the_store_does_not() {
        let root = scratch("locks-run");
        let clause = "fn f() { sql.push(\" FOR UPDATE\"); }\n";
        for file in ["db.rs", "db/locks.rs", "db/locks/tests.rs"] {
            write(&root, &format!("{STORE_SOURCES}/{file}"), clause);
        }
        write(&root, &format!("{STORE_SOURCES}/users.rs"), "fn f() {}\n");
        assert!(run(&root).is_ok());
        for file in ["dbx.rs", "users/tests.rs", "tests.rs"] {
            write(&root, &format!("{STORE_SOURCES}/{file}"), clause);
        }
        let error = run(&root).expect_err("the clauses are refused").to_string();
        assert_eq!(error, "3 places write a locking clause");
    }
}
