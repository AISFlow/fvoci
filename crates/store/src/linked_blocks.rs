//! Blocks as a reference shows them: what an identifier names, what names a block, and
//! what there is to name, each without its body.
//!
//! Nothing here asks who reads. A caller gives the boards it has asked about, or asks
//! of each block's board what it is given: the rows of [`crate::body_links`] say what
//! a body says, of blocks its writer was never asked about.

use std::collections::HashSet;

use fvoci_model::{Block, BlockType};

use crate::Store;
use crate::blocks::Paging;
use crate::db::{Dialect, Id, Row, Scope, Sql};
use crate::error::StoreError;

/// The columns of a block as a reference shows it ([`shown_block`]): without its body.
const SHOWN_COLUMNS: &str = "id, container_id, parent_id, type, title, fields, updated_at, number";

/// How many identifiers one statement is asked about: well under the parameters a
/// statement of either database takes.
const ASKED_AT_ONCE: usize = 500;

/// The block of a row read with [`SHOWN_COLUMNS`]: what it is, where it is and what it
/// is called, and no body.
fn shown_block(row: &Row) -> Result<Block, StoreError> {
    Ok(Block {
        id: row.id(0)?,
        container_id: row.id(1)?,
        parent_id: row.id(2)?,
        block_type: crate::blocks::block_type_column(row, 3, "type")?,
        title: row.text(4)?,
        fields: row.json_map(5, "fields")?,
        updated_at: row.int(6)?,
        number: crate::card_numbers::number_from_row(row, 7)?,
        ..Block::default()
    })
}

/// Starts the query for the cards, pages and units on the containers of `container_ids`,
/// of which there is at least one, without their bodies. Uses literal `type IN (...)`
/// so both databases can match the partial index `blocks_container_titled_updated_at_idx`.
fn select_named_on(scope: &Scope<'_>, columns: &'static str, container_ids: &[&str]) -> Sql {
    let mut sql = scope.sql(["SELECT ", columns, " FROM blocks WHERE container_id IN ("]);
    sql.bind_list(container_ids.iter().map(|id| Id(id)))
        .push(") AND type IN ('card', 'page', 'unit')");
    sql
}

/// Appends the SQL condition excluding blocks with JSON `fields.isTemplate == true`
/// (not `1` nor `"true"`).
fn push_not_template(sql: &mut Sql, dialect: Dialect) {
    sql.push(match dialect {
        Dialect::Sqlite => " AND json_type(fields, '$.isTemplate') IS NOT 'true'",
        Dialect::Postgres => " AND (fields->'isTemplate')::text IS DISTINCT FROM 'true'",
    });
}

/// Appends an ASCII case-insensitive literal substring filter (`instr`/`strpos`, never
/// `LIKE`, so `%` and `_` are ordinary characters) on `title`.
fn push_ascii_title_holds(sql: &mut Sql, dialect: Dialect, word: &str) {
    let prefix = match dialect {
        Dialect::Sqlite => " AND instr(lower(title), ",
        Dialect::Postgres => " AND strpos(lower(title COLLATE \"C\"), ",
    };
    sql.push_bind(prefix, word.to_ascii_lowercase())
        .push(") > 0");
}

/// Whether the title has every word of `words` in it, which are in lower case, whatever
/// the case of its letters.
fn title_has(title: &str, words: &[String]) -> bool {
    let title = title.to_lowercase();
    words.iter().all(|word| title.contains(word.as_str()))
}

impl Store {
    /// The blocks the identifiers name that are on a board now, of whatever type, each
    /// without its body and in no order: what a reference is resolved from. An
    /// identifier that names none (never one, in the trash, of another organization)
    /// has no block here, and nothing says which. Asked some at a time, as a statement
    /// binds only so many.
    ///
    /// # Errors
    ///
    /// Returns [`StoreError::Database`] if a query fails.
    pub async fn named_blocks(&self, ids: &[&str]) -> Result<Vec<Block>, StoreError> {
        if ids.is_empty() {
            return Ok(Vec::new());
        }
        self.with_conn(async |scope| {
            let mut found = Vec::new();
            for some in ids.chunks(ASKED_AT_ONCE) {
                let mut sql = scope.sql(["SELECT ", SHOWN_COLUMNS, " FROM blocks WHERE id IN ("]);
                sql.bind_list(some.iter().map(|id| Id(id))).push(")");
                for row in scope.fetch_all(sql).await? {
                    found.push(shown_block(&row)?);
                }
            }
            Ok(found)
        })
        .await
    }

    /// Which of the identifiers name a block of the type that is on the board now (in
    /// the trash a block is not among them): a card of a board, or a page of a wiki,
    /// what a body may name to whoever reads the board or the wiki alone, by its link.
    /// Asked some at a time, as bodies may name more than one statement binds.
    ///
    /// # Errors
    ///
    /// Returns [`StoreError::Database`] if a query fails.
    pub async fn blocks_on_container_among(
        &self,
        container_id: &str,
        block_type: BlockType,
        ids: &[&str],
    ) -> Result<HashSet<String>, StoreError> {
        if ids.is_empty() {
            return Ok(HashSet::new());
        }
        self.with_conn(async |scope| {
            let mut cards = HashSet::new();
            for some in ids.chunks(ASKED_AT_ONCE) {
                let mut sql = scope.sql("SELECT id FROM blocks WHERE container_id = ");
                sql.bind(Id(container_id))
                    .push_bind(" AND type = ", block_type.as_str())
                    .push(" AND id IN (")
                    .bind_list(some.iter().map(|id| Id(id)))
                    .push(")");
                for row in scope.fetch_all(sql).await? {
                    cards.insert(row.id(0)?);
                }
            }
            Ok(cards)
        })
        .await
    }

    /// One page of the cards and pages on the boards of `container_ids` whose body names
    /// the block `target_id`, or which have a comment whose body does: each once,
    /// without its body, the oldest first. The block itself is not among them, whatever
    /// its own body or its comments name.
    ///
    /// What names from a board that is not in `container_ids`, and what is in the trash,
    /// is not counted and takes no place in a page.
    ///
    /// # Errors
    ///
    /// Returns [`StoreError::Database`] if the query fails.
    pub async fn blocks_naming(
        &self,
        target_id: &str,
        container_ids: &[&str],
        paging: Paging,
    ) -> Result<Vec<Block>, StoreError> {
        if container_ids.is_empty() {
            return Ok(Vec::new());
        }
        self.with_conn(async |scope| {
            let mut sql = select_named_on(scope, SHOWN_COLUMNS, container_ids);
            // What a comment names, the card or the page it is on names. The rows of
            // the boards asked about are the only ones looked at; what is found is held
            // to the boards its block is on now all the same.
            sql.push_bind(" AND id <> ", Id(target_id))
                .push_bind(
                    " AND id IN (SELECT CASE WHEN s.type = ",
                    BlockType::Comment.as_str(),
                )
                .push(
                    " THEN s.parent_id ELSE s.id END \
                     FROM body_links l JOIN blocks s ON s.id = l.source_id WHERE l.target_id = ",
                )
                .bind(Id(target_id))
                .push(" AND l.container_id IN (")
                .bind_list(container_ids.iter().map(|id| Id(id)))
                .push(")) ORDER BY created_at, id")
                .push_page(paging);
            scope
                .fetch_all(sql)
                .await?
                .iter()
                .map(shown_block)
                .collect()
        })
        .await
    }

    /// The cards, pages and units on `container_ids` whose title has every word of
    /// `term` in it, as spaces divide it and whatever the case of the letters: at most
    /// `limit`, the one changed last first, without their bodies. An empty term finds
    /// every one. A block whose `isTemplate` is `true` ([`Block::is_template`]) is never
    /// found, and nothing in the trash is.
    ///
    /// # Errors
    ///
    /// Returns [`StoreError::Database`] if a query fails.
    pub async fn titled_blocks(
        &self,
        container_ids: &[&str],
        term: &str,
        limit: usize,
    ) -> Result<Vec<Block>, StoreError> {
        if container_ids.is_empty() || limit == 0 {
            return Ok(Vec::new());
        }
        let raw_words: Vec<&str> = term.split_whitespace().collect();
        if raw_words.iter().any(|word| word.contains('\0')) {
            return Ok(Vec::new());
        }
        let mut sql_words = Vec::new();
        let mut unicode_words = Vec::new();
        for word in raw_words {
            if word.chars().all(|ch| !ch.is_alphabetic() || ch.is_ascii()) {
                sql_words.push(word);
            } else {
                unicode_words.push(word.to_lowercase());
            }
        }
        let sql_limit = i64::try_from(limit).unwrap_or(i64::MAX);
        self.with_conn(async |scope| {
            let dialect = scope.dialect();
            if unicode_words.is_empty() {
                let mut sql = select_named_on(scope, SHOWN_COLUMNS, container_ids);
                push_not_template(&mut sql, dialect);
                for word in &sql_words {
                    push_ascii_title_holds(&mut sql, dialect, word);
                }
                sql.push(" ORDER BY updated_at DESC, id")
                    .push_bind(" LIMIT ", sql_limit);
                return scope
                    .fetch_all(sql)
                    .await?
                    .iter()
                    .map(shown_block)
                    .collect();
            }

            // Fallback when the query includes non-ASCII cased letters (which SQLite's
            // built-in `lower()` does not fold): still push `isTemplate` and ASCII/caseless
            // words into SQL and scan only `(id, title)` without deserializing 64 KB
            // `fields` on non-matching rows, then load `SHOWN_COLUMNS` for matched IDs.
            let mut candidates = select_named_on(scope, "id, title", container_ids);
            push_not_template(&mut candidates, dialect);
            for word in &sql_words {
                push_ascii_title_holds(&mut candidates, dialect, word);
            }
            candidates.push(" ORDER BY updated_at DESC, id");
            let mut matched_ids = Vec::with_capacity(limit);
            for row in &scope.fetch_all(candidates).await? {
                let title = row.text(1)?;
                if title_has(&title, &unicode_words) {
                    matched_ids.push(row.id(0)?);
                    if matched_ids.len() == limit {
                        break;
                    }
                }
            }
            if matched_ids.is_empty() {
                return Ok(Vec::new());
            }
            let mut found = Vec::with_capacity(matched_ids.len());
            for chunk in matched_ids.chunks(ASKED_AT_ONCE) {
                let mut sql = scope.sql(["SELECT ", SHOWN_COLUMNS, " FROM blocks WHERE id IN ("]);
                sql.bind_list(chunk.iter().map(|id| Id(id)))
                    .push(") ORDER BY updated_at DESC, id");
                for row in &scope.fetch_all(sql).await? {
                    found.push(shown_block(row)?);
                }
            }
            found.sort_by(|a, b| {
                b.updated_at
                    .cmp(&a.updated_at)
                    .then_with(|| a.id.cmp(&b.id))
            });
            Ok(found)
        })
        .await
    }
}
