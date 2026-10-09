#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum Error {
    #[error("Failed to connect to database: `{0}`")]
    ConnectionFailed(String),
    #[error("SQLite failure: `{1}`")]
    SqliteFailure(std::ffi::c_int, String),
    #[error("Null value")]
    NullValue, // Not in rusqlite
    #[error("API misuse: `{0}`")]
    Misuse(String), // Not in rusqlite
    #[error("Execute returned rows")]
    ExecuteReturnedRows,
    #[error("Query returned no rows")]
    QueryReturnedNoRows,
    #[error("Invalid column name: `{0}`")]
    InvalidColumnName(String),
    #[error("SQL conversion failure: `{0}`")]
    ToSqlConversionFailure(crate::BoxError),
    #[error("Sync is not supported in databases opened in {0} mode.")]
    SyncNotSupported(String), // Not in rusqlite
    #[error("Loading extension is only supported in local databases.")]
    LoadExtensionNotSupported, // Not in rusqlite
    #[error("Authorizer is only supported in local databases.")]
    AuthorizerNotSupported, // Not in rusqlite
    #[error("Update hooks are only supported in local databases.")]
    UpdateHookNotSupported, // Not in rusqlite
    #[error("Column not found: {0}")]
    ColumnNotFound(i32), // Not in rusqlite
    #[error("Hrana: `{0}`")]
    Hrana(crate::BoxError), // Not in rusqlite
    #[error("Write delegation: `{0}`")]
    WriteDelegation(crate::BoxError), // Not in rusqlite
    #[error("bincode: `{0}`")]
    Bincode(crate::BoxError),
    #[error("invalid column index")]
    InvalidColumnIndex,
    #[error("invalid column type")]
    InvalidColumnType,
    #[error("syntax error around L{0}:{1}: `{2}`")]
    Sqlite3SyntaxError(u64, usize, String),
    #[error("unsupported statement")]
    Sqlite3UnsupportedStatement,
    #[error("sqlite3 parser error: `{0}`")]
    Sqlite3ParserError(crate::BoxError),
    #[error("Remote SQlite failure: `{0}:{1}:{2}`")]
    RemoteSqliteFailure(i32, i32, String),
    #[error("replication error: {0}")]
    Replication(crate::BoxError),
    #[error("path has invalid UTF-8")]
    InvalidUTF8Path,
    #[error("freeze is not supported in {0} mode.")]
    FreezeNotSupported(String),
    #[error("connection has reached an invalid state, started with {0}")]
    InvalidParserState(String),
    #[error("TLS error: {0}")]
    InvalidTlsConfiguration(std::io::Error),
    #[error("Transactional batch error: {0}")]
    TransactionalBatchError(String),
    #[error("Invalid blob size, expected {0}")]
    InvalidBlobSize(usize),
    #[error("sync error: {0}")]
    Sync(crate::BoxError),
    #[error("WAL frame insert conflict")]
    WalConflict,
    #[error("Reserved bytes not supported")]
    ReservedBytesNotSupported,
}

#[cfg(feature = "hrana")]
impl Error {
    /// Borrow the machine code of a structured Hrana statement failure.
    ///
    /// Returns `Some` only for the maintained stream error and cursor step
    /// error variants. Transport, HTTP, malformed response, arbitrary boxed
    /// errors and other variants return `None`. The message is not inspected;
    /// an unrelated, empty or unknown code is not a foreign-key failure.
    /// This does not imply transaction settlement or change the original error.
    pub fn hrana_error_code(&self) -> Option<&str> {
        let Self::Hrana(error) = self else {
            return None;
        };
        match error.downcast_ref::<crate::hrana::HranaError>()? {
            crate::hrana::HranaError::StreamError(error) => Some(error.code.as_str()),
            crate::hrana::HranaError::CursorError(
                crate::hrana::CursorResponseError::StepError { error, .. },
            ) => Some(error.code.as_str()),
            _ => None,
        }
    }
}

#[cfg(feature = "hrana")]
impl From<crate::hrana::HranaError> for Error {
    fn from(e: crate::hrana::HranaError) -> Self {
        Error::Hrana(e.into())
    }
}

#[cfg(feature = "sync")]
impl From<crate::sync::SyncError> for Error {
    fn from(e: crate::sync::SyncError) -> Self {
        Error::Sync(e.into())
    }
}

impl From<std::convert::Infallible> for Error {
    fn from(_: std::convert::Infallible) -> Self {
        unreachable!()
    }
}

#[cfg(feature = "core")]
pub(crate) fn error_from_handle(raw: *mut libsql_sys::ffi::sqlite3) -> String {
    let errmsg = unsafe { libsql_sys::ffi::sqlite3_errmsg(raw) };
    sqlite_errmsg_to_string(errmsg)
}

#[cfg(feature = "core")]
pub(crate) fn extended_error_code(raw: *mut libsql_sys::ffi::sqlite3) -> std::ffi::c_int {
    unsafe { libsql_sys::ffi::sqlite3_extended_errcode(raw) }
}

#[cfg(feature = "core")]
pub fn error_from_code(code: i32) -> String {
    let errmsg = unsafe { libsql_sys::ffi::sqlite3_errstr(code) };
    sqlite_errmsg_to_string(errmsg)
}

#[cfg(feature = "core")]
#[allow(clippy::not_unsafe_ptr_arg_deref)]
pub fn sqlite_errmsg_to_string(errmsg: *const std::ffi::c_char) -> String {
    let errmsg = unsafe { std::ffi::CStr::from_ptr(errmsg) }.to_bytes();
    String::from_utf8_lossy(errmsg).to_string()
}

#[cfg(feature = "replication")]
impl From<bincode::Error> for Error {
    fn from(e: bincode::Error) -> Self {
        Error::Bincode(e.into())
    }
}

#[cfg(all(test, feature = "hrana"))]
mod hrana_error_code_tests {
    use super::Error;
    use crate::hrana::{CursorResponseError, HranaError};

    fn stream(code: &str, message: &str) -> Error {
        Error::Hrana(Box::new(HranaError::StreamError(
            crate::hrana::proto::Error {
                code: code.to_owned(),
                message: message.to_owned(),
            },
        )))
    }

    fn step(code: &str, message: &str) -> Error {
        // Reuse the maintained cursor Error deserializer, inferred from the
        // StepError field; do not export its private module or duplicate a type.
        let error = serde_json::from_value(serde_json::json!({
            "code": code,
            "message": message,
        }))
        .unwrap();
        Error::Hrana(Box::new(HranaError::CursorError(
            CursorResponseError::StepError { step: 7, error },
        )))
    }

    #[test]
    fn stream_code_is_borrowed_and_does_not_replace_message_or_error() {
        let error = stream("SQLITE_CONSTRAINT_FOREIGNKEY", "literal non-code message");
        assert_eq!(
            error.hrana_error_code(),
            Some("SQLITE_CONSTRAINT_FOREIGNKEY")
        );
        let Error::Hrana(boxed) = &error else {
            panic!("wrong original variant")
        };
        let HranaError::StreamError(original) = boxed.downcast_ref::<HranaError>().unwrap() else {
            panic!("wrong maintained variant")
        };
        assert_eq!(
            error.hrana_error_code().unwrap().as_ptr(),
            original.code.as_ptr()
        );
        assert_eq!(original.message, "literal non-code message");
    }

    #[test]
    fn cursor_step_code_is_borrowed_and_preserves_step_and_message() {
        let error = step("SQLITE_CONSTRAINT_FOREIGNKEY", "different literal message");
        assert_eq!(
            error.hrana_error_code(),
            Some("SQLITE_CONSTRAINT_FOREIGNKEY")
        );
        let Error::Hrana(boxed) = &error else {
            panic!("wrong original variant")
        };
        let HranaError::CursorError(CursorResponseError::StepError {
            step,
            error: original,
        }) = boxed.downcast_ref::<HranaError>().unwrap()
        else {
            panic!("wrong maintained variant")
        };
        assert_eq!(*step, 7);
        assert_eq!(
            error.hrana_error_code().unwrap().as_ptr(),
            original.code.as_ptr()
        );
        assert_eq!(original.message, "different literal message");
    }

    #[test]
    fn structured_generic_unknown_and_empty_codes_are_not_inferred_from_message() {
        for code in [
            "SQLITE_CONSTRAINT",
            "SQLITE_CONSTRAINT_UNIQUE",
            "UNKNOWN",
            "",
        ] {
            for error in [
                stream(code, "SQLITE_CONSTRAINT_FOREIGNKEY"),
                step(code, "SQLITE_CONSTRAINT_FOREIGNKEY"),
            ] {
                assert_eq!(error.hrana_error_code(), Some(code));
                assert_ne!(
                    error.hrana_error_code(),
                    Some("SQLITE_CONSTRAINT_FOREIGNKEY")
                );
            }
        }
    }

    #[test]
    fn transport_and_nonstatement_hrana_variants_expose_no_code() {
        for error in [
            HranaError::Http("SQLITE_CONSTRAINT_FOREIGNKEY".into()),
            HranaError::Api("SQLITE_CONSTRAINT_FOREIGNKEY".into()),
            HranaError::UnexpectedResponse("SQLITE_CONSTRAINT_FOREIGNKEY".into()),
            HranaError::StreamClosed("SQLITE_CONSTRAINT_FOREIGNKEY".into()),
            HranaError::CursorError(CursorResponseError::Other(
                "SQLITE_CONSTRAINT_FOREIGNKEY".into(),
            )),
            HranaError::CursorError(CursorResponseError::CursorClosed),
            HranaError::CursorError(CursorResponseError::NotClosed {
                expected: 0,
                actual: 1,
            }),
            HranaError::CursorError(CursorResponseError::NoRowsFetched),
        ] {
            assert_eq!(Error::Hrana(Box::new(error)).hrana_error_code(), None);
        }
    }

    #[test]
    fn arbitrary_boxes_and_nonhrana_variants_expose_no_code() {
        for error in [
            Error::Hrana(Box::new(std::io::Error::other(
                "SQLITE_CONSTRAINT_FOREIGNKEY",
            ))),
            Error::Hrana(Box::new(Error::SqliteFailure(
                787,
                "literal nested error".into(),
            ))),
            Error::SqliteFailure(787, "literal sqlite error".into()),
            Error::RemoteSqliteFailure(19, 787, "literal remote error".into()),
            Error::ConnectionFailed("SQLITE_CONSTRAINT_FOREIGNKEY".into()),
            Error::Misuse("SQLITE_CONSTRAINT_FOREIGNKEY".into()),
            Error::NullValue,
        ] {
            assert_eq!(error.hrana_error_code(), None);
        }
    }
}
