//! Single-use password reset credentials, stored only as digests, outside any organization.

use crate::db::{Dialect, Id, Scope};
use crate::{Store, StoreError};
use crate::{email_change, sessions, users};

/// A reset link is valid for one hour from its creation, in milliseconds.
pub const PASSWORD_RESET_LIFETIME_MILLIS: i64 = 60 * 60 * 1_000;

/// The condition on a user row whose password may be reset: it has one, and somebody
/// can sign in to the account (in use, or waiting to be deleted). Not one being erased,
/// nor one put out of use otherwise.
const RESETTABLE: [&str; 3] = [
    "password_hash <> '' AND (deleted_at IS NULL OR (",
    users::WAITS,
    "))",
];

impl Store {
    /// Stores a reset credential for an account that has a password and that somebody can
    /// sign in to: in use, or waiting to be deleted (`users::WAITS`), whose person may
    /// have forgotten the password they would take the deletion back with. Expired
    /// credentials are removed before the new one is stored. `email` is the address the
    /// credential is mailed to: none is stored once that is no more the account's, in
    /// any letter case, so that no link made after a change of address goes to the
    /// address before it.
    ///
    /// # Errors
    /// Returns the database error if the credential cannot be stored.
    pub async fn create_password_reset(
        &self,
        user_id: &str,
        email: &str,
        digest: &str,
        now: i64,
    ) -> Result<bool, StoreError> {
        // Release expired-token locks before taking the foreign-key lock on a user.
        // A consumer holds that user before removing its token.
        self.with_write_conn(async |scope| delete_expired_password_resets(scope, now).await)
            .await?;
        self.with_write_conn(async |scope| {
            insert_password_reset(scope, user_id, Some(email), digest, now).await
        })
        .await
    }

    /// Consumes a live credential, changes its user's password and ends all sessions atomically.
    /// Every other credential of the user goes with it: they were issued for the old password.
    /// Returns the user whose sessions ended, or none for an unavailable credential: one
    /// whose account is no more one somebody can sign in to. An account that waits to be
    /// deleted waits still: signing in with the new password shows the way back.
    ///
    /// # Errors
    /// Returns the database error; none of the changes then take effect.
    pub async fn consume_password_reset(
        &self,
        digest: &str,
        password_hash: &str,
        now: i64,
    ) -> Result<Option<sessions::PasswordSet>, StoreError> {
        let began = std::time::Instant::now();
        self.transaction(async |scope| {
            let mut find = scope.sql("SELECT user_id FROM password_resets WHERE token_hash = ");
            find.bind(digest).push_bind(" AND expires_at > ", now);
            let Some(row) = scope.fetch_optional(find).await? else {
                return Ok(None);
            };
            let user_id = row.id(0)?;
            let mut hold = scope.sql("SELECT id FROM users WHERE ");
            hold.push(RESETTABLE).push_bind(" AND id = ", Id(&user_id));
            if scope.dialect() == Dialect::Postgres {
                hold.push(" FOR UPDATE");
            }
            if scope.fetch_optional(hold).await?.is_none() {
                return Ok(None);
            }
            let now =
                now.saturating_add(i64::try_from(began.elapsed().as_millis()).unwrap_or(i64::MAX));
            let mut take = scope.sql("DELETE FROM password_resets WHERE token_hash = ");
            take.bind(digest).push_bind(" AND expires_at > ", now);
            if scope.execute(take).await? == 0 {
                return Ok(None);
            }
            end_links_of_user(scope, &user_id).await?;
            let mut change = scope.sql("UPDATE users SET password_hash = ");
            change
                .bind(password_hash)
                .push_bind(", password_locked_at = ", 0_i64)
                .push_bind(", updated_at = ", now)
                .push_bind(" WHERE id = ", Id(&user_id));
            scope.execute(change).await?;
            let ended = sessions::delete_sessions_of_user(scope, &user_id).await?;
            Ok(Some(sessions::PasswordSet { user_id, ended }))
        })
        .await
    }
}

/// Ends the links mailed for a user whose password was just set, whose address just
/// changed, or whose account is put out of use: every reset credential, which must
/// not set the password again, and the change of address that waited
/// ([`email_change`]), which was asked for under what the account was before.
pub(crate) async fn end_links_of_user(
    scope: &mut Scope<'_>,
    user_id: &str,
) -> Result<(), StoreError> {
    let mut sql = scope.sql("DELETE FROM password_resets WHERE user_id = ");
    sql.bind(Id(user_id));
    scope.execute(sql).await?;
    email_change::discard_of_user(scope, user_id).await?;
    Ok(())
}

/// Removes the reset credentials that expired by `now`.
pub(crate) async fn delete_expired_password_resets(
    scope: &mut Scope<'_>,
    now: i64,
) -> Result<(), StoreError> {
    let mut expired = scope.sql("DELETE FROM password_resets WHERE expires_at <= ");
    expired.bind(now);
    scope.execute(expired).await?;
    Ok(())
}

/// Stores a reset credential under `digest` for the user, if the account is one whose
/// password may be reset ([`resettable`]); says whether it did. On PostgreSQL the row of
/// the user is held, so that an account whose erasing began meanwhile is seen as it is
/// now, and gets none. Where `email` names an address, the account's is still that
/// one, in any letter case.
pub(crate) async fn insert_password_reset(
    scope: &mut Scope<'_>,
    user_id: &str,
    email: Option<&str>,
    digest: &str,
    now: i64,
) -> Result<bool, StoreError> {
    let mut sql =
        scope.sql("INSERT INTO password_resets (token_hash, user_id, expires_at) SELECT ");
    sql.bind(digest)
        .push(", id")
        .push_bind(", ", now.saturating_add(PASSWORD_RESET_LIFETIME_MILLIS))
        .push_bind(" FROM users WHERE id = ", Id(user_id))
        .push(" AND ")
        .push(RESETTABLE);
    if let Some(email) = email {
        sql.push_bind(" AND lower(email) = lower(", email).push(")");
    }
    if scope.dialect() == Dialect::Postgres {
        sql.push(" FOR UPDATE");
    }
    Ok(scope.execute(sql).await? == 1)
}
