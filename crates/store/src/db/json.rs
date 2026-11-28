//! The text at a key of a JSON column, as each dialect reads it.

use super::{Dialect, Scope, SqlFragment, is_sql_ident};

impl Scope<'_> {
    /// The text at `key` of the JSON in `column`, `NULL` where it has none: `key` names
    /// a member (`type`), or a member of one, joined by dots (`fields.agentTokenName`).
    /// `column` is a column, or one of a table of the statement (`d.block`).
    ///
    /// # Panics
    ///
    /// Panics if `column` or a name in `key` is no plain name.
    pub(crate) fn json_text(&self, column: &'static str, key: &'static str) -> SqlFragment {
        json_text(self.dialect(), column, key)
    }
}

/// See [`Scope::json_text`].
fn json_text(dialect: Dialect, column: &'static str, key: &'static str) -> SqlFragment {
    assert!(
        column.split('.').all(is_sql_ident),
        "invalid SQL column for JSON text: {column:?}"
    );
    assert!(
        key.split('.').all(is_sql_ident),
        "invalid JSON key: {key:?}"
    );
    let mut text = SqlFragment::new();
    match dialect {
        Dialect::Sqlite => {
            text.push("json_extract(")
                .push(column)
                .push(", '$.")
                .push(key)
                .push("')");
        }
        Dialect::Postgres => {
            let (members, last) = key.rsplit_once('.').unwrap_or(("", key));
            text.push(column);
            for member in members.split('.').filter(|member| !member.is_empty()) {
                text.push("->'").push(member).push("'");
            }
            text.push("->>'").push(last).push("'");
        }
    }
    text
}

#[cfg(test)]
mod tests {
    use super::super::IntoSql as _;
    use super::{Dialect, json_text};

    #[test]
    fn a_member_is_read_as_text_in_each_dialect() {
        assert_eq!(
            (&json_text(Dialect::Sqlite, "d.block", "type")).into_sql_string(),
            "json_extract(d.block, '$.type')"
        );
        assert_eq!(
            (&json_text(Dialect::Postgres, "d.block", "type")).into_sql_string(),
            "d.block->>'type'"
        );
    }

    #[test]
    fn a_member_of_a_member_is_read_through_it() {
        assert_eq!(
            (&json_text(Dialect::Sqlite, "block", "fields.agentTokenName")).into_sql_string(),
            "json_extract(block, '$.fields.agentTokenName')"
        );
        assert_eq!(
            (&json_text(Dialect::Postgres, "block", "fields.agentTokenName")).into_sql_string(),
            "block->'fields'->>'agentTokenName'"
        );
    }

    #[test]
    #[should_panic(expected = "invalid JSON key")]
    fn a_key_that_would_end_the_quote_is_refused() {
        json_text(Dialect::Postgres, "block", "type' OR '1");
    }

    #[test]
    #[should_panic(expected = "invalid SQL column")]
    fn a_column_that_is_no_name_is_refused() {
        json_text(Dialect::Sqlite, "block)", "type");
    }
}
