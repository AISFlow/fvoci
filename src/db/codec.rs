//! Checked SQLite-family values. Business operations decode their named rows;
//! there is no JSON row bag, SQL conversion or lossy numeric coercion.
use chrono::{DateTime, NaiveDate, Utc};
use serde_json::Value;
use sqlx::{Column, Row, TypeInfo, ValueRef};
use uuid::Uuid;

#[derive(Debug, Clone, PartialEq)]
pub(crate) enum Cell {
    Null,
    Integer(i64),
    Text(String),
    Blob(Vec<u8>),
}

impl Cell {
    pub(crate) fn uuid(value: Uuid) -> Self {
        Self::Blob(value.as_bytes().to_vec())
    }
    pub(crate) fn optional_uuid(value: Option<Uuid>) -> Self {
        value.map(Self::uuid).unwrap_or(Self::Null)
    }
    pub(crate) fn text(value: impl Into<String>) -> Self {
        Self::Text(value.into())
    }
    pub(crate) fn optional_text(value: Option<&str>) -> Self {
        value.map(Self::text).unwrap_or(Self::Null)
    }
    pub(crate) fn instant(value: DateTime<Utc>) -> Result<Self, sqlx::Error> {
        // The request/receipt echo may retain finer precision elsewhere. A DB
        // instant must never silently discard it at this storage boundary.
        if !value.timestamp_subsec_nanos().is_multiple_of(1000) {
            return Err(invalid("instant exceeds microsecond precision"));
        }
        Ok(Self::Integer(value.timestamp_micros()))
    }
    /// PostgreSQL keeps these values as jsonb, which refuses U+0000 in any
    /// string or key with SQLSTATE 22P05 before the statement runs. The value
    /// tree is checked, so the refusal does not depend on how text escapes it.
    pub(crate) fn json(value: &Value) -> Result<Self, sqlx::Error> {
        if json_has_nul(value) {
            return Err(PostgresRefusal::NUL_JSON.into());
        }
        serde_json::to_string(value)
            .map(Self::Text)
            .map_err(|e| sqlx::Error::Encode(Box::new(e)))
    }
    pub(crate) fn integer(&self) -> Result<i64, sqlx::Error> {
        match self {
            Self::Integer(v) => Ok(*v),
            _ => Err(invalid("expected SQLite integer")),
        }
    }
    pub(crate) fn int32(&self) -> Result<i32, sqlx::Error> {
        i32::try_from(self.integer()?).map_err(|_| invalid("SQLite integer exceeds i32"))
    }
    pub(crate) fn boolean(&self) -> Result<bool, sqlx::Error> {
        match self.integer()? {
            0 => Ok(false),
            1 => Ok(true),
            _ => Err(invalid("invalid SQLite boolean")),
        }
    }
    pub(crate) fn string(&self) -> Result<String, sqlx::Error> {
        match self {
            Self::Text(v) => Ok(v.clone()),
            _ => Err(invalid("expected SQLite text")),
        }
    }
    pub(crate) fn bytes(&self) -> Result<Vec<u8>, sqlx::Error> {
        match self {
            Self::Blob(v) => Ok(v.clone()),
            _ => Err(invalid("expected SQLite blob")),
        }
    }
    pub(crate) fn id(&self) -> Result<Uuid, sqlx::Error> {
        Uuid::from_slice(&self.bytes()?).map_err(|e| sqlx::Error::Decode(Box::new(e)))
    }
    pub(crate) fn datetime(&self) -> Result<DateTime<Utc>, sqlx::Error> {
        DateTime::from_timestamp_micros(self.integer()?)
            .ok_or_else(|| invalid("SQLite instant out of range"))
    }
    // Decode canonical date-only values for storage and archive consumers.
    pub(crate) fn date(&self) -> Result<NaiveDate, sqlx::Error> {
        let raw = self.string()?;
        let date = NaiveDate::parse_from_str(&raw, "%Y-%m-%d")
            .map_err(|e| sqlx::Error::Decode(Box::new(e)))?;
        if date.format("%Y-%m-%d").to_string() != raw {
            return Err(invalid("noncanonical SQLite date"));
        }
        Ok(date)
    }
    pub(crate) fn value(&self) -> Result<Value, sqlx::Error> {
        serde_json::from_str(&self.string()?).map_err(|e| sqlx::Error::Decode(Box::new(e)))
    }
    pub(crate) fn optional<T>(
        &self,
        decode: impl FnOnce(&Self) -> Result<T, sqlx::Error>,
    ) -> Result<Option<T>, sqlx::Error> {
        if matches!(self, Self::Null) {
            Ok(None)
        } else {
            decode(self).map(Some)
        }
    }
}

pub(crate) enum FamilyRow {
    Local(sqlx::sqlite::SqliteRow),
    Remote(libsql::Row),
}
impl FamilyRow {
    pub(crate) fn column_count(&self) -> Result<usize, sqlx::Error> {
        match self {
            Self::Local(row) => Ok(row.columns().len()),
            Self::Remote(row) => {
                usize::try_from(row.column_count()).map_err(|_| invalid("column count overflow"))
            }
        }
    }
    pub(crate) fn column_name(&self, index: usize) -> Result<&str, sqlx::Error> {
        match self {
            Self::Local(row) => row
                .columns()
                .get(index)
                .map(|column| column.name())
                .ok_or_else(|| invalid("column index out of bounds")),
            Self::Remote(row) => row
                .column_name(i32::try_from(index).map_err(|_| invalid("column index overflow"))?)
                .ok_or_else(|| invalid("column metadata unavailable")),
        }
    }

    pub(crate) fn cell(&self, index: usize) -> Result<Cell, sqlx::Error> {
        match self {
            Self::Local(row) => {
                let value = row.try_get_raw(index)?;
                if value.is_null() {
                    return Ok(Cell::Null);
                }
                match value.type_info().name() {
                    "INTEGER" => row.try_get(index).map(Cell::Integer),
                    "TEXT" => row.try_get(index).map(Cell::Text),
                    "BLOB" => row.try_get(index).map(Cell::Blob),
                    _ => Err(invalid(
                        "unsupported SQLite storage class (REAL is not exact)",
                    )),
                }
            }
            Self::Remote(row) => match row
                .get_value(i32::try_from(index).map_err(|_| invalid("column index overflow"))?)
                .map_err(remote_error)?
            {
                libsql::Value::Null => Ok(Cell::Null),
                libsql::Value::Integer(v) => Ok(Cell::Integer(v)),
                libsql::Value::Text(v) => Ok(Cell::Text(v)),
                libsql::Value::Blob(v) => Ok(Cell::Blob(v)),
                libsql::Value::Real(_) => Err(invalid("remote REAL is not an exact value")),
            },
        }
    }
}

pub(crate) fn remote_error(error: libsql::Error) -> sqlx::Error {
    // The driver error can contain endpoint/query values. Keep it as the
    // internal source; HTTP boundaries use the existing redacted AppError.
    sqlx::Error::AnyDriverError(Box::new(error))
}
fn invalid(message: &'static str) -> sqlx::Error {
    sqlx::Error::Protocol(message.into())
}

fn json_has_nul(value: &Value) -> bool {
    match value {
        Value::String(text) => text.contains('\0'),
        Value::Array(items) => items.iter().any(json_has_nul),
        Value::Object(map) => map
            .iter()
            .any(|(key, item)| key.contains('\0') || json_has_nul(item)),
        Value::Null | Value::Bool(_) | Value::Number(_) => false,
    }
}

/// An error the family raises where PostgreSQL refuses the same write, with
/// PostgreSQL's SQLSTATE, kind and message so callers classify it the same
/// way. It never carries a bound value.
#[derive(Debug)]
pub(crate) struct PostgresRefusal {
    code: &'static str,
    message: &'static str,
}

impl PostgresRefusal {
    /// U+0000 in a text bind.
    pub(crate) const NUL_TEXT: Self = Self {
        code: "22021",
        message: "invalid byte sequence for encoding \"UTF8\": 0x00",
    };
    /// `\u0000` anywhere in a jsonb value.
    pub(crate) const NUL_JSON: Self = Self {
        code: "22P05",
        message: "unsupported Unicode escape sequence",
    };
    /// `app_user_update_email` refusing the new address.
    pub(crate) const INVALID_EMAIL: Self = Self {
        code: "P0001",
        message: "app_user_update_email: invalid email",
    };
    /// The deferred `assert_private_project_has_lead` constraint triggers.
    pub(crate) const PRIVATE_PROJECT_WITHOUT_LEAD: Self = Self {
        code: "23514",
        message: "private project requires a lead",
    };
}

impl std::fmt::Display for PostgresRefusal {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.message)
    }
}
impl std::error::Error for PostgresRefusal {}

impl sqlx::error::DatabaseError for PostgresRefusal {
    fn message(&self) -> &str {
        self.message
    }
    fn code(&self) -> Option<std::borrow::Cow<'_, str>> {
        Some(self.code.into())
    }
    fn as_error(&self) -> &(dyn std::error::Error + Send + Sync + 'static) {
        self
    }
    fn as_error_mut(&mut self) -> &mut (dyn std::error::Error + Send + Sync + 'static) {
        self
    }
    fn into_error(self: Box<Self>) -> Box<dyn std::error::Error + Send + Sync + 'static> {
        self
    }
    // sqlx's PostgreSQL error kind for the same SQLSTATE. A RAISE names no
    // constraint, so `constraint()` stays None as it does there.
    fn kind(&self) -> sqlx::error::ErrorKind {
        match self.code {
            "23514" => sqlx::error::ErrorKind::CheckViolation,
            _ => sqlx::error::ErrorKind::Other,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn storage_types_and_precision_are_checked() {
        let id = Uuid::now_v7();
        assert_eq!(Cell::uuid(id).id().unwrap(), id);
        assert!(Cell::Text(id.to_string()).id().is_err());
        assert!(Cell::Blob(vec![0; 15]).id().is_err());
        assert!(Cell::Integer(2).boolean().is_err());
        assert_eq!(Cell::Null.optional(Cell::string).unwrap(), None);
        assert_eq!(
            Cell::text("").optional(Cell::string).unwrap(),
            Some(String::new())
        );
        assert_eq!(Cell::text("null").value().unwrap(), Value::Null);
        assert!(Cell::Null.value().is_err());
        let date = DateTime::parse_from_rfc3339("2026-10-03T23:59:59.999999Z")
            .unwrap()
            .with_timezone(&Utc);
        assert_eq!(Cell::instant(date).unwrap().datetime().unwrap(), date);
        let nanos = DateTime::parse_from_rfc3339("2026-10-03T23:59:59.999999001Z")
            .unwrap()
            .with_timezone(&Utc);
        assert!(Cell::instant(nanos).is_err());
        assert!(Cell::text("2026-2-01").date().is_err());
        assert!(Cell::text("2026-02-30").date().is_err());
        assert_eq!(
            Cell::text("9007199254740993.125").string().unwrap(),
            "9007199254740993.125"
        );
    }

    #[test]
    fn json_with_nul_in_any_string_or_key_is_refused_like_jsonb() {
        let parsed: Value = serde_json::from_str(r#"{"a":["x","b\u0000"]}"#).unwrap();
        for value in [
            parsed,
            serde_json::json!("\0"),
            serde_json::json!({"outer": {"k\0": 1}}),
            serde_json::json!([1, {"note": "a\0b"}]),
        ] {
            let error = Cell::json(&value).unwrap_err();
            let database = error.as_database_error().expect("database error");
            assert_eq!(database.code().as_deref(), Some("22P05"));
            assert_eq!(database.message(), "unsupported Unicode escape sequence");
            assert!(matches!(database.kind(), sqlx::error::ErrorKind::Other));
            assert!(database.constraint().is_none());
        }
        // A backslash followed by `u0000` is six ordinary characters; jsonb keeps it.
        let literal = serde_json::json!({"note": ["\\u0000", null, true, 1.5]});
        assert_eq!(
            Cell::json(&literal).unwrap(),
            Cell::text(r#"{"note":["\\u0000",null,true,1.5]}"#)
        );
    }
}
