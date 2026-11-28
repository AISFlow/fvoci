//! The kind of a board (`containers.kind`) as queries read it and ask for it.
//!
//! A list of boards is of the kinds its caller names: there is no list that reads
//! boards and wikis without saying so.

use fvoci_model::{ContainerKind, ContainerKinds};

use crate::db::{IntoSql, Sql};
use crate::error::StoreError;

/// Pushes ` AND <column> IN (<the kinds>)`, and nothing for every kind. `column` names
/// the kind column of the boards read, with its alias.
pub(crate) fn push_kinds(sql: &mut Sql, column: impl IntoSql, kinds: ContainerKinds) {
    if kinds == ContainerKinds::EVERY {
        return;
    }
    if kinds.is_empty() {
        sql.push(" AND 1 = 0");
        return;
    }
    sql.push(" AND ")
        .push(column)
        .push(" IN (")
        .bind_list(kinds.iter().map(ContainerKind::as_str))
        .push(")");
}

/// The kind a stored text names.
pub(crate) fn kind_from_text(text: &str) -> Result<ContainerKind, StoreError> {
    text.parse().map_err(|error| StoreError::Corrupt {
        column: "kind",
        reason: format!("{error}"),
    })
}
