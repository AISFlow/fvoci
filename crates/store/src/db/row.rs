//! Result-row decoding across SQLite and PostgreSQL.

use serde_json::Value;
use sqlx::Row as _;
use sqlx::postgres::PgRow;
use sqlx::sqlite::SqliteRow;
use sqlx::types::Uuid;

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
                Ok(value.map(sqlite_id).unwrap_or_default())
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
