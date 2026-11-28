//! The reads of a [`Scope`] that decode what they find: a row, the one row, every
//! row, or whether there is any.
//!
//! Each runs through [`Scope::fetch_all`], so a statement is taken for a read or a
//! write, and a value bound as an identifier that is none matches no row, as there.

use std::fmt::Display;

use super::{FromRow, Scope, Sql};
use crate::error::StoreError;

impl Scope<'_> {
    /// The first row `sql` finds, if any.
    ///
    /// # Errors
    ///
    /// Returns [`StoreError::Database`] if the query fails, and what
    /// [`FromRow::from_row`] returns for the row.
    pub(crate) async fn first<T: FromRow>(&mut self, sql: Sql) -> Result<Option<T>, StoreError> {
        self.fetch_optional(sql)
            .await?
            .as_ref()
            .map(T::from_row)
            .transpose()
    }

    /// The row `sql` finds.
    ///
    /// `what` is [`Send`], so that the future of a caller is: a [`format_args!`] in the
    /// statement that awaits this would be held across the query. A formatted name is
    /// given as a [`format!`].
    ///
    /// # Errors
    ///
    /// Returns [`StoreError::NotFound`] naming `what` if `sql` finds no row, and what
    /// [`Scope::first`] returns.
    pub(crate) async fn one<T: FromRow>(
        &mut self,
        sql: Sql,
        what: impl Display + Send,
    ) -> Result<T, StoreError> {
        self.first(sql)
            .await?
            .ok_or_else(|| StoreError::not_found(what))
    }

    /// Every row `sql` finds, in its order, collected as [`Iterator::collect`] collects:
    /// into a [`Vec`], a set, or a map of pairs.
    ///
    /// # Errors
    ///
    /// Returns [`StoreError::Database`] if the query fails, and what
    /// [`FromRow::from_row`] returns for a row.
    pub(crate) async fn all<C: FromIterator<T>, T: FromRow>(
        &mut self,
        sql: Sql,
    ) -> Result<C, StoreError> {
        self.fetch_all(sql).await?.iter().map(T::from_row).collect()
    }

    /// Whether `sql` finds a row.
    ///
    /// # Errors
    ///
    /// Returns [`StoreError::Database`] if the query fails.
    pub(crate) async fn exists(&mut self, sql: Sql) -> Result<bool, StoreError> {
        Ok(self.fetch_optional(sql).await?.is_some())
    }
}
