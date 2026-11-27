//! Users looked up by their identifiers: also what is left of an erased account, which
//! is answered to whoever asks, and tells nothing of the person nor when it came or went.

use fvoci_model::User;

use crate::db::{Id, Scope, Sql};
use crate::error::StoreError;
use crate::users::{fetch_users, user_not_found};

/// The columns of [`crate::users::USER_COLUMNS`], in its order, with the times of an erased
/// account read as zero (and its `deleted_at` as `NULL`).
const ERASED_AS_NOBODY: &str = concat!(
    "id, username, email, password_hash, password_locked_at, \
     CASE WHEN erased_at > 0 THEN 0 ELSE created_at END, \
     CASE WHEN erased_at > 0 THEN 0 ELSE updated_at END, \
     CASE WHEN erased_at > 0 THEN NULL ELSE deleted_at END, is_admin, display_name, \
     CASE WHEN erased_at > 0 THEN 0 ELSE email_verified_at END, ",
    crate::person_names::name_columns!()
);

/// The users with the given identifiers among those `narrow` keeps, oldest first.
///
/// # Errors
///
/// Returns [`StoreError::NotFound`] if none of the users is found and
/// [`StoreError::NotAllFound`] unless exactly one user is found per identifier.
pub(crate) async fn users_by_ids(
    scope: &mut Scope<'_>,
    user_ids: &[String],
    narrow: impl Fn(&mut Sql, &Scope<'_>),
) -> Result<Vec<User>, StoreError> {
    let users = found_users_by_ids(scope, user_ids, narrow).await?;
    if users.is_empty() {
        return Err(user_not_found());
    }
    // Comparing counts ensures duplicate requested IDs fail rather than silently collapsing.
    if users.len() != user_ids.len() {
        return Err(StoreError::NotAllFound {
            entity: "user",
            ids: user_ids.to_vec(),
        });
    }
    Ok(users)
}

/// The users with the given identifiers among those `narrow` keeps, oldest first, each
/// once: an identifier of nobody it keeps finds nothing, as one of nobody at all.
///
/// An account that waits to be deleted is among them as it is, with its name: until it
/// is erased it is a member of what it was, and wrote what it wrote. One that was
/// erased is found whatever `narrow` keeps, as the row it left: its identifier, no
/// name and its three times zero, by which whoever reads what it wrote, or a mention
/// of it, is told that the account is gone and not when it came or went. Nothing of a
/// person is in that row to keep from anybody. Looked up by name ([`crate::users::search_users`])
/// neither is found.
///
/// # Errors
///
/// Returns [`StoreError::Database`] if the query fails.
pub(crate) async fn found_users_by_ids(
    scope: &mut Scope<'_>,
    user_ids: &[String],
    narrow: impl Fn(&mut Sql, &Scope<'_>),
) -> Result<Vec<User>, StoreError> {
    if user_ids.is_empty() {
        return Ok(Vec::new());
    }
    let mut sql = scope.sql(["SELECT ", ERASED_AS_NOBODY, " FROM users WHERE id IN ("]);
    sql.bind_list(user_ids.iter().map(|id| Id(id))).push(
        ") AND (erased_at > 0 OR ((deleted_at IS NULL OR delete_after > 0 OR erase_started_at > 0)",
    );
    narrow(&mut sql, scope);
    // Oldest first by the times answered, so that the order tells no more than they do.
    sql.push(")) ORDER BY CASE WHEN erased_at > 0 THEN 0 ELSE created_at END, id");
    fetch_users(scope, sql).await
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The row is read by position (`users::user_from_row`): the columns are those of
    /// a user, in the same order.
    #[test]
    fn an_erased_account_is_read_from_the_columns_of_a_user_in_their_order() {
        let named: Vec<&str> = ERASED_AS_NOBODY
            .split(", ")
            .map(|column| {
                column
                    .strip_prefix("CASE WHEN erased_at > 0 THEN 0 ELSE ")
                    .or_else(|| column.strip_prefix("CASE WHEN erased_at > 0 THEN NULL ELSE "))
                    .and_then(|column| column.strip_suffix(" END"))
                    .unwrap_or(column)
            })
            .collect();
        assert_eq!(named.join(", "), crate::users::USER_COLUMNS);
    }
}
