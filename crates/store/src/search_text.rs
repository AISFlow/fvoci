//! The search of the cards and pages of a workspace by the letters of their title and
//! body (`blocks.body_text`), for a server without a search server: every word asked
//! for is in the title or in the body, as it is written there, ASCII letters in either
//! case. A word is looked for as text, by position, never as a pattern of `LIKE`: each
//! comparison of a row is one step SQLite cannot stop in, and `LIKE` of a long word that
//! almost matches everywhere in a large body takes many times longer than `instr`.
//! It reads the blocks that have a document ([`crate::search_docs`]), the same
//! ones a search server holds, and answers their identifiers alone, the best first:
//! the caller checks each against what the user reads, as it checks what a search
//! server answers.

use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Instant;

use fvoci_model::SearchKind;

use crate::Store;
use crate::db::{Dialect, Id, Scope, Sql};
use crate::error::StoreError;
use crate::search_docs::select_documents;

/// What a search by the letters asks for.
#[derive(Debug, Clone, Copy)]
pub struct TextSearch<'a> {
    /// The workspace searched.
    pub workspace_id: &'a str,
    /// The boards and wikis of the workspace searched; `None` for all of them.
    pub containers: Option<&'a [String]>,
    /// The one kind asked for, if one is.
    pub kind: Option<SearchKind>,
    /// The words, none empty: each is in the title or the body of what is found.
    pub words: &'a [String],
    /// Set once the statement is sent with time left: a search late after that was
    /// late in the database, and one late before it, for a connection.
    pub begun: &'a AtomicBool,
}

/// What a search for cards by their board-local number (`#12`) asks for.
#[derive(Debug, Clone, Copy)]
pub struct NumberSearch<'a> {
    /// The workspace searched.
    pub workspace_id: &'a str,
    /// The containers of the workspace searched; `None` for all of them.
    pub containers: Option<&'a [String]>,
    /// The card number to match exactly (1 or more).
    pub number: u32,
    /// Set once the statement is sent with time left.
    pub begun: &'a AtomicBool,
}

/// What came of a search by the letters.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TextFound {
    /// The identifiers of the cards and the pages found, the best first.
    Ids(Vec<String>),
    /// The database did not finish in the time the search had, and was stopped.
    OutOfTime,
}

/// A word as it is looked for: its ASCII letters in lower case, and nothing else
/// changed. It is looked for as text, by position (`instr`, `strpos`), never as a
/// pattern: no character of it means anything but itself.
fn needle(word: &str) -> String {
    word.to_ascii_lowercase()
}

/// Adds the condition that the column `column` of `b`, with its ASCII letters in lower
/// case and no other letter changed, holds `word`. SQLite's `lower` changes ASCII
/// letters alone; PostgreSQL's does under the collation `C`.
fn push_holds(sql: &mut Sql, dialect: Dialect, column: &'static str, word: &str) {
    match dialect {
        Dialect::Sqlite => sql.push_bind(["instr(lower(", column, "), "], needle(word)),
        Dialect::Postgres => {
            sql.push_bind(["strpos(lower(", column, " COLLATE \"C\"), "], needle(word))
        }
    };
    sql.push(") > 0");
}

/// The body of `b`, empty where it has none.
const BODY: &str = "COALESCE(b.body_text, '')";

/// Adds the condition that every word is in the title or in the body.
fn push_every_word(sql: &mut Sql, dialect: Dialect, words: &[String]) {
    for (index, word) in words.iter().enumerate() {
        if index > 0 {
            sql.push(" AND ");
        }
        sql.push("(");
        push_holds(sql, dialect, "b.title", word);
        sql.push(" OR ");
        push_holds(sql, dialect, BODY, word);
        sql.push(")");
    }
}

impl Store {
    /// Up to `limit` cards and pages that `search` finds, past the `offset` best: the
    /// ones whose title holds every word first, then the most recently changed, and
    /// then by identifier, so that a page follows the one before. Stopped at `by`.
    /// Words of which one holds a NUL character find nothing: no text stored holds one.
    /// Words that are empty are none: every text holds one.
    ///
    /// # Errors
    ///
    /// Returns [`StoreError::Database`] if the query fails otherwise than by the time.
    pub async fn search_text(
        &self,
        search: &TextSearch<'_>,
        offset: i64,
        limit: i64,
        by: Instant,
    ) -> Result<TextFound, StoreError> {
        let words: Vec<String> = search
            .words
            .iter()
            .filter(|word| !word.is_empty())
            .cloned()
            .collect();
        if words.is_empty() || words.iter().any(|word| word.contains('\0')) {
            return Ok(TextFound::Ids(Vec::new()));
        }
        let search = TextSearch {
            words: &words,
            ..*search
        };
        self.with_conn(async |scope| {
            let sql = statement(scope, &search, offset, limit);
            fetch_ids_by(scope, sql, search.begun, by).await
        })
        .await
    }

    /// Up to `limit` non-template cards of `search.workspace_id` whose `number` equals
    /// `search.number`, past `offset`, most recently changed first then by identifier.
    /// Stopped at `by`.
    ///
    /// # Errors
    ///
    /// Returns [`StoreError::Database`] if the query fails otherwise than by the time.
    pub async fn search_cards_by_number(
        &self,
        search: &NumberSearch<'_>,
        offset: i64,
        limit: i64,
        by: Instant,
    ) -> Result<TextFound, StoreError> {
        if search.number == 0 {
            return Ok(TextFound::Ids(Vec::new()));
        }
        self.with_conn(async |scope| {
            let sql = number_statement(scope, search, offset, limit);
            fetch_ids_by(scope, sql, search.begun, by).await
        })
        .await
    }
}

/// Runs an identifier-selecting search statement under the deadline `by`, marking
/// `begun` once sent with time remaining.
async fn fetch_ids_by(
    scope: &mut Scope<'_>,
    sql: Sql,
    begun: &AtomicBool,
    by: Instant,
) -> Result<TextFound, StoreError> {
    if Instant::now() < by {
        begun.store(true, Ordering::Release);
    }
    let Some(rows) = scope.fetch_all_by(sql, by).await? else {
        return Ok(TextFound::OutOfTime);
    };
    rows.iter()
        .map(|row| row.id(0))
        .collect::<Result<Vec<String>, StoreError>>()
        .map(TextFound::Ids)
}

/// The statement of [`Store::search_cards_by_number`].
fn number_statement(scope: &Scope<'_>, search: &NumberSearch<'_>, offset: i64, limit: i64) -> Sql {
    let mut sql = select_documents(scope, "b.id");
    sql.push_bind(" AND bo.workspace_id = ", Id(search.workspace_id));
    if let Some(containers) = search.containers {
        sql.push(" AND b.container_id IN (")
            .bind_list(containers.iter().map(|id| Id(id)))
            .push(")");
    }
    sql.push_bind(" AND b.type = ", SearchKind::Card.block_type().as_str())
        .push_bind(" AND b.number = ", i64::from(search.number))
        .push(" ORDER BY b.updated_at DESC, b.id")
        .push_bind(" LIMIT ", limit)
        .push_bind(" OFFSET ", offset);
    sql
}

/// The statement of [`Store::search_text`].
fn statement(scope: &Scope<'_>, search: &TextSearch<'_>, offset: i64, limit: i64) -> Sql {
    let dialect = scope.dialect();
    let mut sql = select_documents(scope, "b.id");
    sql.push_bind(" AND bo.workspace_id = ", Id(search.workspace_id));
    if let Some(containers) = search.containers {
        sql.push(" AND b.container_id IN (")
            .bind_list(containers.iter().map(|id| Id(id)))
            .push(")");
    }
    if let Some(kind) = search.kind {
        sql.push_bind(" AND b.type = ", kind.block_type().as_str().to_owned());
    }
    sql.push(" AND ");
    push_every_word(&mut sql, dialect, search.words);
    sql.push(" ORDER BY CASE WHEN ");
    push_title_holds_every_word(&mut sql, dialect, search.words);
    sql.push(" THEN 0 ELSE 1 END, b.updated_at DESC, b.id")
        .push_bind(" LIMIT ", limit)
        .push_bind(" OFFSET ", offset);
    sql
}

/// Adds the condition that the title holds every word.
fn push_title_holds_every_word(sql: &mut Sql, dialect: Dialect, words: &[String]) {
    for (index, word) in words.iter().enumerate() {
        if index > 0 {
            sql.push(" AND ");
        }
        push_holds(sql, dialect, "b.title", word);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_word_is_looked_for_as_it_is_with_its_ascii_letters_in_lower_case() {
        assert_eq!(needle("환불"), "환불");
        assert_eq!(needle("100%"), "100%");
        assert_eq!(needle(r"A_b\C"), r"a_b\c");
        assert_eq!(needle("ÉTÉ API"), "ÉtÉ api");
    }
}
