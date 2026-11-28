//! Result-row decoding across SQLite and PostgreSQL.

use serde_json::Value;
use sqlx::postgres::{PgRow, Postgres};
use sqlx::sqlite::SqliteRow;
use sqlx::types::Uuid;
use sqlx::{Column as _, Row as _};

use super::sqlite_id;
use crate::error::StoreError;

/// A result row from either database, read by column position.
///
/// The getters treat SQL `NULL` as the empty value of their type. The columns that may
/// be `NULL` are references that may be absent (a block's parent, a creator), and the
/// model spells "none" as the empty string.
#[derive(Debug)]
pub(crate) enum Row {
    Sqlite(SqliteRow),
    Postgres(PgRow),
}

impl Row {
    /// Reads a text column.
    pub(crate) fn text(&self, index: usize) -> Result<String, StoreError> {
        let value: Option<String> = match self {
            Self::Sqlite(row) => row.try_get(index)?,
            Self::Postgres(row) => row.try_get(index)?,
        };
        Ok(value.unwrap_or_default())
    }

    /// Reads a column of text or an identifier column, as the model holds both: text as
    /// it is, an identifier (`uuid` on PostgreSQL) in its canonical form. `NULL` reads
    /// as the empty string.
    fn text_or_id(&self, index: usize) -> Result<String, StoreError> {
        match self {
            Self::Postgres(row)
                if <Uuid as sqlx::Type<Postgres>>::compatible(
                    row.try_column(index)?.type_info(),
                ) =>
            {
                self.id(index)
            }
            _ => self.text(index),
        }
    }

    /// Reads a text column that may be `NULL`, as `None`: for where no value and an
    /// empty one are told apart.
    pub(crate) fn opt_text(&self, index: usize) -> Result<Option<String>, StoreError> {
        Ok(match self {
            Self::Sqlite(row) => row.try_get(index)?,
            Self::Postgres(row) => row.try_get(index)?,
        })
    }

    /// Reads a 64-bit integer column that may be `NULL`, as `None`.
    pub(crate) fn opt_int(&self, index: usize) -> Result<Option<i64>, StoreError> {
        Ok(match self {
            Self::Sqlite(row) => row.try_get(index)?,
            Self::Postgres(row) => row.try_get(index)?,
        })
    }

    /// Reads an identifier column: `uuid` on PostgreSQL, text on SQLite. `NULL` reads
    /// as the empty string, which is how the model says "none".
    pub(crate) fn id(&self, index: usize) -> Result<String, StoreError> {
        match self {
            Self::Sqlite(row) => {
                let value: Option<String> = row.try_get(index)?;
                Ok(value.unwrap_or_default())
            }
            Self::Postgres(row) => {
                let value: Option<Uuid> = row.try_get(index)?;
                Ok(value.map_or_default(sqlite_id))
            }
        }
    }

    /// Reads a 64-bit integer column. Narrower PostgreSQL integer columns must be cast to
    /// `BIGINT` in the query.
    pub(crate) fn int(&self, index: usize) -> Result<i64, StoreError> {
        let value: Option<i64> = match self {
            Self::Sqlite(row) => row.try_get(index)?,
            Self::Postgres(row) => row.try_get(index)?,
        };
        Ok(value.unwrap_or_default())
    }

    /// Reads a 64-bit floating-point column (`DOUBLE PRECISION` on PostgreSQL, `REAL` on
    /// SQLite).
    pub(crate) fn float(&self, index: usize) -> Result<f64, StoreError> {
        let value: Option<f64> = match self {
            Self::Sqlite(row) => row.try_get(index)?,
            Self::Postgres(row) => row.try_get(index)?,
        };
        Ok(value.unwrap_or_default())
    }

    /// Reads a 64-bit floating-point column that may be `NULL`, as `None`.
    pub(crate) fn opt_float(&self, index: usize) -> Result<Option<f64>, StoreError> {
        Ok(match self {
            Self::Sqlite(row) => row.try_get(index)?,
            Self::Postgres(row) => row.try_get(index)?,
        })
    }

    /// Reads a column of bytes: `bytea` on PostgreSQL, `BLOB` on SQLite. `NULL` reads
    /// as no bytes.
    pub(crate) fn bytes(&self, index: usize) -> Result<Vec<u8>, StoreError> {
        let value: Option<Vec<u8>> = match self {
            Self::Sqlite(row) => row.try_get(index)?,
            Self::Postgres(row) => row.try_get(index)?,
        };
        Ok(value.unwrap_or_default())
    }

    /// Reads a boolean column.
    pub(crate) fn bool(&self, index: usize) -> Result<bool, StoreError> {
        let value: Option<bool> = match self {
            Self::Sqlite(row) => row.try_get(index)?,
            Self::Postgres(row) => row.try_get(index)?,
        };
        Ok(value.unwrap_or_default())
    }

    /// Reads a JSON column: `json` or `jsonb` on PostgreSQL, JSON as text or as a blob on
    /// SQLite.
    ///
    /// `NULL` and empty text read as JSON `null`.
    pub(crate) fn json(&self, index: usize, column: &'static str) -> Result<Value, StoreError> {
        match self {
            Self::Sqlite(row) => {
                // Read as bytes, which accepts both storage classes: this server writes
                // JSON as text, and SQLite keeps what is bound as bytes as a BLOB even
                // in a text column.
                let bytes: Option<Vec<u8>> = row.try_get(index)?;
                match bytes.as_deref() {
                    None | Some([]) => Ok(Value::Null),
                    Some(bytes) => {
                        serde_json::from_slice(bytes).map_err(|error| StoreError::Corrupt {
                            column,
                            reason: error.to_string(),
                        })
                    }
                }
            }
            Self::Postgres(row) => {
                let value: Option<Value> = row.try_get(index)?;
                Ok(value.unwrap_or(Value::Null))
            }
        }
    }

    /// Reads a JSON column that holds an object. Anything else, including `null`, reads
    /// as an empty object.
    pub(crate) fn json_map(
        &self,
        index: usize,
        column: &'static str,
    ) -> Result<fvoci_model::JsonMap, StoreError> {
        match self.json(index, column)? {
            Value::Object(map) => Ok(map),
            _ => Ok(fvoci_model::JsonMap::new()),
        }
    }
}

/// A value read from a result row: a model type from the columns its query selects, or
/// a [`Column`] or a tuple of them from the first columns in their order.
pub(crate) trait FromRow: Sized {
    /// Reads the value.
    ///
    /// # Errors
    ///
    /// Returns [`StoreError::Database`] if a column is missing or of another type, and
    /// [`StoreError::Corrupt`] if what it holds is no value of `Self`.
    fn from_row(row: &Row) -> Result<Self, StoreError>;
}

/// A value of one column, read as [`Row`]'s getters read it: `NULL` as the empty value.
pub(crate) trait Column: Sized {
    /// Reads column `index`.
    ///
    /// # Errors
    ///
    /// Returns [`StoreError::Database`] if there is no such column or it is of another
    /// type.
    fn column(row: &Row, index: usize) -> Result<Self, StoreError>;
}

/// Text, or an identifier in its canonical form.
impl Column for String {
    fn column(row: &Row, index: usize) -> Result<Self, StoreError> {
        row.text_or_id(index)
    }
}

impl Column for i64 {
    fn column(row: &Row, index: usize) -> Result<Self, StoreError> {
        row.int(index)
    }
}

impl Column for f64 {
    fn column(row: &Row, index: usize) -> Result<Self, StoreError> {
        row.float(index)
    }
}

impl Column for bool {
    fn column(row: &Row, index: usize) -> Result<Self, StoreError> {
        row.bool(index)
    }
}

impl<T: Column> FromRow for T {
    fn from_row(row: &Row) -> Result<Self, StoreError> {
        T::column(row, 0)
    }
}

impl<A: Column, B: Column> FromRow for (A, B) {
    fn from_row(row: &Row) -> Result<Self, StoreError> {
        Ok((A::column(row, 0)?, B::column(row, 1)?))
    }
}

impl<A: Column, B: Column, C: Column> FromRow for (A, B, C) {
    fn from_row(row: &Row) -> Result<Self, StoreError> {
        Ok((A::column(row, 0)?, B::column(row, 1)?, C::column(row, 2)?))
    }
}

#[cfg(test)]
mod tests {
    use sqlx::Connection as _;

    use super::{Column, FromRow, Row};

    /// The one row of `select`, on SQLite.
    async fn row_of(select: &'static str) -> Row {
        let mut conn = sqlx::SqliteConnection::connect("sqlite::memory:")
            .await
            .expect("a database");
        Row::Sqlite(
            sqlx::query(select)
                .fetch_one(&mut conn)
                .await
                .expect("a row"),
        )
    }

    #[tokio::test]
    async fn a_row_is_read_as_its_columns_in_order() {
        let row = row_of("SELECT 'a', 2, 1, 0.5").await;
        let first: String = FromRow::from_row(&row).expect("read");
        assert_eq!(first, "a");
        let two: (String, i64) = FromRow::from_row(&row).expect("read");
        assert_eq!(two, ("a".to_owned(), 2));
        let three: (String, i64, bool) = FromRow::from_row(&row).expect("read");
        assert_eq!(three, ("a".to_owned(), 2, true));
        assert_eq!(f64::column(&row, 3).expect("read"), 0.5);
    }

    #[tokio::test]
    async fn null_is_read_as_the_empty_value() {
        let row = row_of("SELECT NULL, NULL, NULL").await;
        let three: (String, i64, bool) = FromRow::from_row(&row).expect("read");
        assert_eq!(three, (String::new(), 0, false));
    }

    #[tokio::test]
    async fn a_column_of_another_type_is_an_error() {
        let row = row_of("SELECT 'a'").await;
        assert!(<i64 as FromRow>::from_row(&row).is_err());
    }
}
