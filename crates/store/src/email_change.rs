//! An account's change of its email address: one that waits for the new address to be
//! confirmed, its link stored only as a digest, outside any organization, or one made
//! at once where the server sends no mail.

use crate::db::{Dialect, Id, Scope};
use crate::signups::{self, SIGNUP_LIFETIME_MILLIS, SIGNUP_LINK_LIFETIME_MILLIS};
use crate::{Store, StoreError, Taken, email_revert, password_reset};

/// A change that is not confirmed ends this long after it was asked for, in
/// milliseconds: as a registration that is not confirmed does.
pub const EMAIL_CHANGE_LIFETIME_MILLIS: i64 = SIGNUP_LIFETIME_MILLIS;

/// The link of a mail confirms the change for this long from the mail, in milliseconds:
/// as the link of a registration's mail does.
pub const EMAIL_CHANGE_LINK_LIFETIME_MILLIS: i64 = SIGNUP_LINK_LIFETIME_MILLIS;

/// An account whose address was changed, for what follows the change.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EmailChanged {
    /// The account.
    pub user_id: String,
    /// Its login name.
    pub username: String,
    /// The address it had; empty for an account that had none.
    pub old_email: String,
}

impl Store {
    /// Has the account wait for `new_email` to be confirmed, in place of what it
    /// waited for: the link of a change asked before confirms nothing any more. The
    /// address is the account's only once it is confirmed
    /// ([`Store::confirm_email_change`]), and nothing holds it until then. Says whether
    /// it does: not an account that is not in use, nor one whose password is no more
    /// `seen_password_hash`, the hash the request's password was checked against.
    ///
    /// # Errors
    ///
    /// Returns the database error; nothing is changed then.
    pub async fn ask_email_change(
        &self,
        user_id: &str,
        seen_password_hash: &str,
        new_email: &str,
        now: i64,
    ) -> Result<bool, StoreError> {
        // As a reset credential: what ended goes first, in its own statement, before
        // the user's row is taken.
        self.with_write_conn(async |scope| delete_expired(scope, now).await)
            .await?;
        self.transaction(async |scope| {
            if hold_account(scope, user_id, Some(seen_password_hash))
                .await?
                .is_none()
            {
                return Ok(false);
            }
            discard_of_user(scope, user_id).await?;
            let mut sql = scope.sql(
                "INSERT INTO email_changes (user_id, new_email, created_at, expires_at) VALUES (",
            );
            sql.bind(Id(user_id))
                .push_bind(", ", new_email)
                .push_bind(", ", now)
                .push_bind(", ", now.saturating_add(EMAIL_CHANGE_LIFETIME_MILLIS))
                .push(")");
            scope.execute(sql).await?;
            Ok(true)
        })
        .await
    }

    /// The address the account waits to have confirmed at `now`, if it waits for one.
    ///
    /// # Errors
    ///
    /// Returns the database error.
    pub async fn email_change_of(
        &self,
        user_id: &str,
        now: i64,
    ) -> Result<Option<String>, StoreError> {
        self.with_conn(async |scope| {
            let mut sql = scope.sql("SELECT new_email FROM email_changes WHERE user_id = ");
            sql.bind(Id(user_id)).push_bind(" AND expires_at > ", now);
            scope
                .fetch_optional(sql)
                .await?
                .map(|row| row.text(0))
                .transpose()
        })
        .await
    }

    /// Ends what the account waits for: its link confirms nothing any more. Says
    /// whether it waited for anything.
    ///
    /// # Errors
    ///
    /// Returns the database error.
    pub async fn cancel_email_change(&self, user_id: &str) -> Result<bool, StoreError> {
        self.with_write_conn(async |scope| discard_of_user(scope, user_id).await)
            .await
    }

    /// Whether an account other than `asking`, deleted or not, a registration that has
    /// not ended at `now` or a link that gives another account its address back
    /// (`email_revert`) has the address, in any letter case: a change of `asking` to
    /// it would not be confirmed, so no link is mailed for it. A link of `asking`'s own
    /// holds the address for it, which may have it back by a change too.
    ///
    /// # Errors
    ///
    /// Returns the database error.
    pub async fn email_is_held(
        &self,
        email: &str,
        asking: &str,
        now: i64,
    ) -> Result<bool, StoreError> {
        self.with_conn(async |scope| is_held(scope, email, Some(asking), now).await)
            .await
    }

    /// Gives the change to `new_email` that the account waits for the one link under
    /// `digest`, for a day from `now`: the link of a mail before confirms nothing any
    /// more. Says whether it did: not once the account waits for another address or for
    /// none, the change ended, or the account is not in use.
    ///
    /// # Errors
    ///
    /// Returns the database error.
    pub async fn renew_email_change_link(
        &self,
        user_id: &str,
        new_email: &str,
        digest: &str,
        now: i64,
    ) -> Result<bool, StoreError> {
        self.with_write_conn(async |scope| {
            let mut sql = scope.sql("UPDATE email_changes SET token_hash = ");
            sql.bind(digest)
                .push_bind(
                    ", token_expires_at = ",
                    now.saturating_add(EMAIL_CHANGE_LINK_LIFETIME_MILLIS),
                )
                .push_bind(" WHERE user_id = ", Id(user_id))
                .push_bind(" AND new_email = ", new_email)
                .push_bind(" AND expires_at > ", now)
                .push(
                    " AND EXISTS (SELECT 1 FROM users \
                     WHERE users.id = email_changes.user_id AND users.deleted_at IS NULL)",
                );
            Ok(scope.execute(sql).await? == 1)
        })
        .await
    }

    /// Whether the link under `digest` still confirms the change the account waits for
    /// at `now`: for a message that carries it and is sent again.
    ///
    /// # Errors
    ///
    /// Returns the database error.
    pub async fn email_change_link_stands(
        &self,
        user_id: &str,
        digest: &str,
        now: i64,
    ) -> Result<bool, StoreError> {
        self.with_conn(async |scope| {
            let mut sql = scope.sql("SELECT 1 FROM email_changes WHERE user_id = ");
            sql.bind(Id(user_id))
                .push_bind(" AND token_hash = ", digest)
                .push_bind(" AND token_expires_at > ", now)
                .push_bind(" AND expires_at > ", now);
            Ok(scope.fetch_optional(sql).await?.is_some())
        })
        .await
    }

    /// Consumes the link under `digest` and makes the address it was mailed to the
    /// account's, all of it or none: the account is in use, and no other account,
    /// deleted or not, no registration that has not ended and no link that gives
    /// another account its address back has the address. What was mailed to the
    /// address before goes with it: the reset credentials of the account. The address
    /// before, if the account had one, may be had again by the link stored under
    /// `revert` ([`Store::revert_email_change`]), which holds it meanwhile. Returns the
    /// account as it was, or none where nothing is changed: a link that is unknown,
    /// used, replaced or ended, an account that is not in use, an address that was
    /// taken meanwhile. That last link is not consumed: the change waits on, and the
    /// link makes it once nobody has the address.
    ///
    /// # Errors
    ///
    /// Returns the database error; nothing is changed then.
    pub async fn confirm_email_change(
        &self,
        digest: &str,
        revert: &str,
        now: i64,
    ) -> Result<Option<EmailChanged>, StoreError> {
        // Whoever has the address of the server may ask this, with any text: one that
        // is no link waits for nobody, where a link takes its turn with every
        // registration (below).
        let known = self
            .with_conn(async |scope| Ok(find_link(scope, digest, now).await?.is_some()))
            .await?;
        if !known {
            return Ok(None);
        }
        self.transaction(async |scope| {
            // Whether the address is free is asked of two tables: no account and no
            // registration is made with it meanwhile.
            signups::claim_account_names(scope).await?;
            let Some((user_id, new_email)) = find_link(scope, digest, now).await? else {
                return Ok(None);
            };
            let Some(account) = hold_account(scope, &user_id, None).await? else {
                return Ok(None);
            };
            // Before the link is taken: one whose address is somebody's stays.
            if is_held(scope, &new_email, Some(&user_id), now).await? {
                return Ok(None);
            }
            // Held, the account's row is as it stays: the link is taken only if it is
            // still the account's, which a cancelling or a newer mail ends.
            let mut take = scope.sql("DELETE FROM email_changes WHERE token_hash = ");
            take.bind(digest).push_bind(" AND user_id = ", Id(&user_id));
            if scope.execute(take).await? == 0 {
                return Ok(None);
            }
            set_email(scope, &user_id, &new_email, now.max(1), now).await?;
            if !account.email.is_empty() {
                email_revert::insert(scope, &user_id, &account.email, revert, now).await?;
            }
            Ok(Some(EmailChanged {
                user_id,
                username: account.username,
                old_email: account.email,
            }))
        })
        .await
    }

    /// Makes `new_email` the account's address at once, for a server that sends no
    /// mail and so confirms none, all of it or none. The reset credentials of the
    /// account go with the address they were for. Returns the account as it was, or
    /// none for an account that is not in use or whose password is no more
    /// `seen_password_hash`.
    ///
    /// # Errors
    ///
    /// Returns [`StoreError::Conflict`] with [`Taken::Email`] if another account,
    /// deleted or not, or a registration that has not ended at `now` has the address,
    /// and the database error; nothing is changed then.
    pub async fn change_email_now(
        &self,
        user_id: &str,
        seen_password_hash: &str,
        new_email: &str,
        now: i64,
    ) -> Result<Option<EmailChanged>, StoreError> {
        self.transaction(async |scope| {
            signups::claim_account_names(scope).await?;
            let Some(account) = hold_account(scope, user_id, Some(seen_password_hash)).await?
            else {
                return Ok(None);
            };
            if is_held(scope, new_email, Some(user_id), now).await? {
                return Err(StoreError::Conflict(Taken::Email));
            }
            set_email(scope, user_id, new_email, 0, now).await?;
            Ok(Some(EmailChanged {
                user_id: user_id.to_owned(),
                username: account.username,
                old_email: account.email,
            }))
        })
        .await
    }
}

/// The account and the address of the link under `digest` that confirms a change at
/// `now`, if there is one.
async fn find_link(
    scope: &mut Scope<'_>,
    digest: &str,
    now: i64,
) -> Result<Option<(String, String)>, StoreError> {
    let mut sql = scope.sql("SELECT user_id, new_email FROM email_changes WHERE token_hash = ");
    sql.bind(digest)
        .push_bind(" AND token_expires_at > ", now)
        .push_bind(" AND expires_at > ", now);
    scope
        .fetch_optional(sql)
        .await?
        .map(|row| Ok((row.id(0)?, row.text(1)?)))
        .transpose()
}

/// What is read of an account that is held for a change of its address.
struct Held {
    username: String,
    email: String,
}

/// Holds the row of the account in use against other changes until the transaction
/// ends, and returns what it has: none for an account that is not in use, or, where
/// `password_hash` names the one a password was checked against, whose password is
/// another by now or locked.
async fn hold_account(
    scope: &mut Scope<'_>,
    user_id: &str,
    password_hash: Option<&str>,
) -> Result<Option<Held>, StoreError> {
    let mut sql = scope.sql("SELECT username, email FROM users WHERE deleted_at IS NULL AND id = ");
    sql.bind(Id(user_id));
    if let Some(password_hash) = password_hash {
        sql.push_bind(" AND password_hash = ", password_hash)
            .push_bind(" AND password_locked_at = ", 0_i64);
    }
    if scope.dialect() == Dialect::Postgres {
        sql.push(" FOR UPDATE");
    }
    scope
        .fetch_optional(sql)
        .await?
        .map(|row| {
            Ok(Held {
                username: row.text(0)?,
                email: row.text(1)?,
            })
        })
        .transpose()
}

/// Whether an account other than `but`, deleted or not, a registration that has not
/// ended at `now`, or a link that stands then and gives an account other than `but`
/// its address back has the address, in any letter case.
pub(crate) async fn is_held(
    scope: &mut Scope<'_>,
    email: &str,
    but: Option<&str>,
    now: i64,
) -> Result<bool, StoreError> {
    let mut account = scope.sql("SELECT 1 FROM users WHERE lower(email) = lower(");
    account.bind(email).push(")");
    if let Some(but) = but {
        account.push_bind(" AND id <> ", Id(but));
    }
    if scope.fetch_optional(account).await?.is_some() {
        return Ok(true);
    }
    let mut signup = scope.sql("SELECT 1 FROM signups WHERE lower(email) = lower(");
    signup.bind(email).push_bind(") AND expires_at > ", now);
    if scope.fetch_optional(signup).await?.is_some() {
        return Ok(true);
    }
    email_revert::holds(scope, email, but, now).await
}

/// Makes `new_email` the address of the account, and ends what was mailed for the one
/// before and what waited: its reset credentials and its change.
async fn set_email(
    scope: &mut Scope<'_>,
    user_id: &str,
    new_email: &str,
    email_verified_at: i64,
    now: i64,
) -> Result<(), StoreError> {
    let verified = if new_email.is_empty() {
        0
    } else {
        email_verified_at
    };
    set_address(scope, user_id, new_email, verified, now).await?;
    password_reset::end_links_of_user(scope, user_id).await
}

/// Makes `new_email` the address of the account, which nobody else has — or clears it
/// when `new_email` is empty (permitted for multiple accounts by the partial unique
/// index `WHERE email <> ''`): the caller asked under the claim on the names.
pub(crate) async fn set_address(
    scope: &mut Scope<'_>,
    user_id: &str,
    new_email: &str,
    email_verified_at: i64,
    now: i64,
) -> Result<(), StoreError> {
    let mut sql = scope.sql("UPDATE users SET email = ");
    sql.bind(new_email)
        .push_bind(", email_verified_at = ", email_verified_at)
        .push_bind(", updated_at = ", now)
        .push_bind(" WHERE id = ", Id(user_id));
    scope.execute(sql).await.map_err(|error| match error {
        // Asked above whether anybody has it, under the claim on the names: should
        // the index find somebody all the same, it decides, as for a registration.
        StoreError::Database(error) => crate::users::unique_violation(error),
        other => other,
    })?;
    Ok(())
}

/// Removes the change the account waits for; says whether there was one.
pub(crate) async fn discard_of_user(
    scope: &mut Scope<'_>,
    user_id: &str,
) -> Result<bool, StoreError> {
    let mut sql = scope.sql("DELETE FROM email_changes WHERE user_id = ");
    sql.bind(Id(user_id));
    Ok(scope.execute(sql).await? > 0)
}

/// Removes the changes that ended unconfirmed by `now`.
pub(crate) async fn delete_expired(scope: &mut Scope<'_>, now: i64) -> Result<(), StoreError> {
    let mut expired = scope.sql("DELETE FROM email_changes WHERE expires_at <= ");
    expired.bind(now);
    scope.execute(expired).await?;
    Ok(())
}
