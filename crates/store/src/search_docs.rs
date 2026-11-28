//! What the search server is to hold of the blocks: a document for each card and each
//! page on a board or in a wiki that is no template, but none for a block whose
//! `isTemplate` is `true` ([`fvoci_model::Block::is_template`]: for a card, a template
//! of its board's cards). One read says so, for the worker that sends the documents
//! and for the search that checks each of the search server's answers against it:
//! what is not read here is not found.

use fvoci_model::{ContainerKind, SearchKind};

use crate::Store;
use crate::db::{Dialect, FromRow, Id, Row, Scope, Sql};
use crate::error::StoreError;

/// Bounded parameters however many identifiers are asked for.
const BATCH: usize = 300;

/// A card or a page as the search server is to hold it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SearchDocument {
    /// The identifier of the card or the page.
    pub id: String,
    /// The organization of its workspace.
    pub org_id: String,
    /// The workspace of its container.
    pub workspace_id: String,
    /// The board it is on, or the wiki it is in.
    pub container_id: String,
    /// What its container is. No document says so: the search asks the database.
    pub container_kind: ContainerKind,
    /// The title of its container; empty if untitled. No document says so: read from the database.
    pub container_title: String,
    /// What it is.
    pub kind: SearchKind,
    /// Its title.
    pub title: String,
    /// The plain text of its body; empty without one.
    pub text: String,
    /// A card's number on its board; `None` for a page, entry or unit.
    pub number: Option<u32>,
}

/// The columns of a document, of the blocks `b`, their boards `bo` and the boards'
/// workspaces `w`.
const DOCUMENT_COLUMNS: &str = "b.id, w.org_id, bo.workspace_id, b.container_id, b.title, COALESCE(b.body_text, ''), b.type, bo.kind, b.number, bo.title";

/// Starts the statement that reads `columns` of the blocks that have a document, with
/// their boards `bo` and workspaces `w`: the cards and the pages of the boards and
/// wikis that are no templates, but no block whose `isTemplate` is `true` (for a card,
/// a template of its board's cards). What is in the trash is no block, and has none. More `AND` conditions on `b` may follow.
pub(crate) fn select_documents(scope: &Scope<'_>, columns: &'static str) -> Sql {
    // Only the JSON `true`, as `block_derived` reads it: not `1`, nor `"true"`.
    let no_template = match scope.dialect() {
        Dialect::Sqlite => "json_type(b.fields, '$.isTemplate') IS NOT 'true'",
        Dialect::Postgres => "(b.fields->'isTemplate')::text IS DISTINCT FROM 'true'",
    };
    let mut sql = scope.sql([
        "SELECT ",
        columns,
        " FROM blocks AS b JOIN containers AS bo ON bo.id = b.container_id",
    ]);
    sql.push(&scope.same_organization(&[("bo", "b")]))
        .push(" JOIN workspaces AS w ON w.id = bo.workspace_id")
        .push(&scope.same_organization(&[("w", "bo")]))
        .push([
            " WHERE bo.is_template = false AND bo.is_builtin = false AND ",
            no_template,
            " AND b.type IN (",
        ])
        .bind_list(
            SearchKind::ALL
                .iter()
                .map(|kind| kind.block_type().as_str().to_owned()),
        )
        .push(")");
    if let Some(org_id) = scope.org_id() {
        sql.push_bind(" AND w.org_id = ", Id(&org_id));
    }
    sql
}

impl Store {
    /// The documents of those of `ids` that have one, among the rows this handle
    /// reaches, by identifier. What is no identifier has none.
    ///
    /// # Errors
    ///
    /// Returns [`StoreError::Database`] if the query fails.
    pub async fn search_documents(
        &self,
        ids: &[impl AsRef<str>],
    ) -> Result<Vec<SearchDocument>, StoreError> {
        let mut ids: Vec<&str> = ids
            .iter()
            .map(AsRef::as_ref)
            .filter(|id| fvoci_model::is_id(id))
            .collect();
        ids.sort_unstable();
        ids.dedup();
        self.with_conn(async |scope| {
            let mut documents = Vec::with_capacity(ids.len());
            for batch in ids.chunks(BATCH) {
                let mut sql = select_documents(scope, DOCUMENT_COLUMNS);
                sql.push(" AND b.id IN (")
                    .bind_list(batch.iter().map(|id| Id(id)))
                    .push(") ORDER BY b.id");
                for row in scope.fetch_all(sql).await? {
                    documents.push(SearchDocument::from_row(&row)?);
                }
            }
            Ok(documents)
        })
        .await
    }

    /// Up to `limit` documents of the blocks of a board or a wiki, by identifier, those after
    /// the identifier `after` (`None` for the first): for sending a board whole, a page
    /// at a time. A board that is a template, or is not there, has none.
    ///
    /// # Errors
    ///
    /// Returns [`StoreError::Database`] if the query fails.
    pub async fn search_documents_of_container(
        &self,
        container_id: &str,
        after: Option<&str>,
        limit: i64,
    ) -> Result<Vec<SearchDocument>, StoreError> {
        self.with_conn(async |scope| {
            let mut sql = select_documents(scope, DOCUMENT_COLUMNS);
            sql.push_bind(" AND b.container_id = ", Id(container_id));
            if let Some(after) = after {
                sql.push_bind(" AND b.id > ", Id(after));
            }
            sql.push_bind(" ORDER BY b.id LIMIT ", limit);
            scope.all(sql).await
        })
        .await
    }
}

impl FromRow for SearchDocument {
    fn from_row(row: &Row) -> Result<Self, StoreError> {
        let block_type = crate::blocks::block_type_column(row, 6, "type")?;
        let kind = SearchKind::of_block(&block_type).ok_or_else(|| StoreError::Corrupt {
            column: "type",
            reason: format!("a document of a block of type {block_type}"),
        })?;
        let number = if kind == SearchKind::Card {
            crate::card_numbers::number_from_row(row, 8)?
        } else {
            None
        };
        Ok(SearchDocument {
            id: row.id(0)?,
            org_id: row.id(1)?,
            workspace_id: row.id(2)?,
            container_id: row.id(3)?,
            container_kind: crate::container_kinds::kind_from_text(&row.text(7)?)?,
            container_title: row.text(9)?,
            kind,
            title: row.text(4)?,
            text: row.text(5)?,
            number,
        })
    }
}
