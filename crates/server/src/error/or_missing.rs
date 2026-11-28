//! What a lookup that finds nothing answers: a 404 that names what a route misses, or
//! nothing at all.

use fvoci_store::StoreError;

use super::AppError;

/// The answers of a lookup whose not-found error a caller says in its own words: of the
/// store ([`StoreError`]) or of the application ([`AppError`]). Any other error stays
/// what it is.
pub(crate) trait OrMissing<T> {
    /// A not-found error becomes not found `what`, the kind of thing the caller misses.
    ///
    /// # Errors
    ///
    /// Returns not found `what` for a not-found error, and any other as it is.
    fn or_missing(self, what: &str) -> Result<T, AppError>;

    /// A not-found error becomes `None`.
    ///
    /// # Errors
    ///
    /// Returns any other error as it is.
    fn found(self) -> Result<Option<T>, AppError>;
}

impl<T> OrMissing<T> for Result<T, StoreError> {
    fn or_missing(self, what: &str) -> Result<T, AppError> {
        self.map_err(|error| {
            if error.is_not_found() {
                AppError::not_found(what)
            } else {
                error.into()
            }
        })
    }

    fn found(self) -> Result<Option<T>, AppError> {
        match self {
            Ok(found) => Ok(Some(found)),
            Err(error) if error.is_not_found() => Ok(None),
            Err(error) => Err(error.into()),
        }
    }
}

impl<T> OrMissing<T> for Result<T, AppError> {
    fn or_missing(self, what: &str) -> Result<T, AppError> {
        self.map_err(|error| {
            if error.is_not_found() {
                AppError::not_found(what)
            } else {
                error
            }
        })
    }

    fn found(self) -> Result<Option<T>, AppError> {
        match self {
            Ok(found) => Ok(Some(found)),
            Err(error) if error.is_not_found() => Ok(None),
            Err(error) => Err(error),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn store_missing() -> Result<u8, StoreError> {
        Err(StoreError::NotFound("block b-1".to_owned()))
    }

    #[test]
    fn a_missing_row_is_said_in_the_callers_words() {
        let error = store_missing().or_missing("page").unwrap_err();
        assert!(error.is_not_found());
        assert_eq!(error.to_string(), "page not found");
        let error = Err::<u8, _>(AppError::not_found("block"))
            .or_missing("card")
            .unwrap_err();
        assert_eq!(error.to_string(), "card not found");
    }

    #[test]
    fn a_missing_row_is_found_to_be_none() {
        assert_eq!(store_missing().found().unwrap(), None);
        assert_eq!(Ok::<u8, StoreError>(7).found().unwrap(), Some(7));
        assert_eq!(
            Err::<u8, _>(AppError::not_found("block")).found().unwrap(),
            None
        );
    }

    #[test]
    fn another_error_stays_what_it_is() {
        let forbidden = Err::<u8, _>(AppError::forbidden("no")).or_missing("card");
        assert!(!forbidden.unwrap_err().is_not_found());
        assert!(Err::<u8, _>(AppError::conflict("late")).found().is_err());
    }
}
