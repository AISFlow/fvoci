//! The parts of a person's name, and the name they are shown by that is made of them.
//!
//! `users.display_name` is the name the account is shown by (`PersonName::shown`), kept
//! with the row so that whatever reads a person's name reads that column alone. It is
//! written where the parts are: when the account is made, and when its name changes.

use fvoci_model::{PersonName, PersonNameChange, User};

use crate::Store;
use crate::db::{Id, Row, Sql};
use crate::error::StoreError;
use crate::users;

/// The columns of the parts of a name, in the order [`name_from_row`] reads them: a
/// literal, for `concat!`.
macro_rules! name_columns {
    () => {
        "custom_name, first_name, last_name, latin_first_name, latin_last_name, name_shown, \
         title, credentials"
    };
}
pub(crate) use name_columns;

/// The parts of a name from the columns of [`name_columns!`], the first at `start`. A
/// code this build does not know is read as none, or as the default choice.
pub(crate) fn name_from_row(row: &Row, start: usize) -> Result<PersonName, StoreError> {
    Ok(PersonName {
        custom_name: row.text(start)?,
        first_name: row.text(start + 1)?,
        last_name: row.text(start + 2)?,
        latin_first_name: row.text(start + 3)?,
        latin_last_name: row.text(start + 4)?,
        name_shown: row.text(start + 5)?.parse().unwrap_or_default(),
        title: row.text(start + 6)?.parse().ok(),
        credentials: PersonName::credentials_from_stored(&row.text(start + 7)?),
    })
}

/// Appends the values of [`name_columns!`], each after a comma.
pub(crate) fn push_name_values(sql: &mut Sql, name: &PersonName) {
    sql.push_bind(", ", &name.custom_name)
        .push_bind(", ", &name.first_name)
        .push_bind(", ", &name.last_name)
        .push_bind(", ", &name.latin_first_name)
        .push_bind(", ", &name.latin_last_name)
        .push_bind(", ", name.name_shown.as_str())
        .push_bind(", ", name.title.map_or("", |title| title.as_str()))
        .push_bind(", ", name.stored_credentials());
}

/// A user as stored after a change of their name, and whether it changed it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChangedName {
    /// The user as stored now.
    pub user: User,
    /// Whether the name is another than it was: a change to what it already was writes
    /// nothing, not even the time.
    pub changed: bool,
}

impl Store {
    /// Changes what `change` names of the user's name, makes the name they are shown by
    /// anew from it, and returns the user as stored now. Whether each part is one a
    /// person may give is the caller's to check.
    ///
    /// # Errors
    ///
    /// Returns [`StoreError::NotFound`] if there is no such user in use, and
    /// [`StoreError::Database`] if a statement fails.
    pub async fn change_person_name(
        &self,
        user_id: &str,
        change: &PersonNameChange,
    ) -> Result<ChangedName, StoreError> {
        self.transaction(async |scope| {
            users::lock_user(scope, user_id)
                .await
                .map_err(|error| match error {
                    StoreError::UserNotFound(_) => StoreError::not_found("user"),
                    error => error,
                })?;
            let mut user = users::get_user_by_id(scope, user_id).await?;
            let before = user.name.clone();
            user.name.apply(change);
            if user.name == before {
                return Ok(ChangedName {
                    user,
                    changed: false,
                });
            }
            let name = &user.name;
            let mut sql = scope.sql("UPDATE users SET display_name = ");
            sql.bind(name.shown())
                .push_bind(", custom_name = ", &name.custom_name)
                .push_bind(", first_name = ", &name.first_name)
                .push_bind(", last_name = ", &name.last_name)
                .push_bind(", latin_first_name = ", &name.latin_first_name)
                .push_bind(", latin_last_name = ", &name.latin_last_name)
                .push_bind(", name_shown = ", name.name_shown.as_str())
                .push_bind(", title = ", name.title.map_or("", |title| title.as_str()))
                .push_bind(", credentials = ", name.stored_credentials())
                .push_bind(", updated_at = ", scope.now_millis())
                .push_bind(" WHERE deleted_at IS NULL AND id = ", Id(user_id));
            scope.execute(sql).await?;
            Ok(ChangedName {
                user: users::get_user_by_id(scope, user_id).await?,
                changed: true,
            })
        })
        .await
    }
}
