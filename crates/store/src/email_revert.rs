//! The link mailed to the address an account had, with the notice that it was changed,
//! that gives the account that address back: for its owner, should somebody who knew
//! the password have moved it. Stored as a digest only, outside any organization. While
//! it stands its address is the account's to have again, and nobody else takes it.

use crate::db::{Id, LockRank, RowLock, Scope};
use crate::email_change::{self, EMAIL_CHANGE_LIFETIME_MILLIS};
use crate::{Store, StoreError, oidc_identities, password_reset, sessions, signups, users};

/// A link that gives an address back stands this long from the change it was mailed
/// for, in milliseconds: as long as a change of address waits to be confirmed.
pub const EMAIL_REVERT_LIFETIME_MILLIS: i64 = EMAIL_CHANGE_LIFETIME_MILLIS;

/// What a link back to an address came to: the account it gave the address back to, and
/// the account's sessions, which ended.
#[must_use = "close the connections of the sessions it ended with Hub::close"]
#[derive(Debug, PartialEq, Eq)]
pub struct EmailRevert {
    /// The account and its address.
    pub reverted: EmailReverted,
    /// Its sessions, all of which ended.
    pub ended: sessions::SessionsEnded,
}

/// An account a link gave its address back to, for what follows.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EmailReverted {
    /// The account.
    pub user_id: String,
    /// Its login name.
    pub username: String,
    /// The address it had before the link was used.
    pub before: String,
    /// The address the link gave back, or none where another account or a registration
    /// had it by then and the account's email address was cleared instead.
    pub restored: Option<String>,
    /// How many SSO identities linked after this revert link was created were removed.
    pub unlinked_identities: u64,
}

impl Store {
    /// Uses the link under `digest`, all of it or none: the address it was mailed to is
    /// the account's again, unless another account or a registration that has not
    /// ended at `now` has it — in which case the account's email address is cleared so
    /// the address set after the change cannot recover it. In any case the account's
    /// sessions end, its password is locked (`users::lock_password`), any SSO identity
    /// linked after this link was created is removed (those linked at or before stay),
    /// and what was asked or mailed under the account as it was goes: the change of
    /// address that waited, the reset links, and the links of this kind made after this
    /// one. Those made before it stand. Works for an account that waits to be deleted,
    /// as a reset link does. Returns none for a link that is unknown, used, ended or of
    /// an account whose erasing began.
    ///
    /// # Errors
    ///
    /// Returns the database error; nothing is changed then.
    pub async fn revert_email_change(
        &self,
        digest: &str,
        now: i64,
    ) -> Result<Option<EmailRevert>, StoreError> {
        let known = self
            .with_conn(async |scope| Ok(find(scope, digest, now).await?.is_some()))
            .await?;
        if !known {
            return Ok(None);
        }
        self.transaction(async |scope| {
            signups::claim_account_names(scope).await?;
            let Some(link) = find(scope, digest, now).await? else {
                return Ok(None);
            };
            let Some((username, before)) = hold_account(scope, &link.user_id).await? else {
                return Ok(None);
            };
            let mut take = scope.sql("DELETE FROM email_reverts WHERE token_hash = ");
            take.bind(digest).push_bind(" AND expires_at > ", now);
            if scope.execute(take).await? == 0 {
                return Ok(None);
            }
            let mut later = scope.sql("DELETE FROM email_reverts WHERE user_id = ");
            later
                .bind(Id(&link.user_id))
                .push_bind(" AND created_at > ", link.created_at);
            scope.execute(later).await?;
            let restored =
                if email_change::is_held(scope, &link.email, Some(&link.user_id), now).await? {
                    email_change::set_address(scope, &link.user_id, "", 0, now).await?;
                    None
                } else {
                    email_change::set_address(scope, &link.user_id, &link.email, now.max(1), now)
                        .await?;
                    Some(link.email)
                };
            let unlinked_identities =
                oidc_identities::remove_linked_after(scope, &link.user_id, link.created_at).await?;
            password_reset::end_links_of_user(scope, &link.user_id).await?;
            users::lock_password(scope, &link.user_id, now).await?;
            let ended = sessions::delete_sessions_of_user(scope, &link.user_id).await?;
            Ok(Some(EmailRevert {
                reverted: EmailReverted {
                    user_id: link.user_id,
                    username,
                    before,
                    restored,
                    unlinked_identities,
                },
                ended,
            }))
        })
        .await
    }

    /// Whether a link of this kind that stands at `now` holds the address, in any
    /// letter case: a new account is not made with it.
    ///
    /// # Errors
    ///
    /// Returns the database error.
    pub async fn email_is_held_for_revert(
        &self,
        email: &str,
        now: i64,
    ) -> Result<bool, StoreError> {
        self.with_conn(async |scope| holds(scope, email, None, now).await)
            .await
    }
}

/// A link that stands.
struct Link {
    user_id: String,
    /// The address it gives back.
    email: String,
    created_at: i64,
}

async fn find(scope: &mut Scope<'_>, digest: &str, now: i64) -> Result<Option<Link>, StoreError> {
    let mut sql =
        scope.sql("SELECT user_id, email, created_at FROM email_reverts WHERE token_hash = ");
    sql.bind(digest).push_bind(" AND expires_at > ", now);
    scope
        .fetch_optional(sql)
        .await?
        .map(|row| {
            Ok(Link {
                user_id: row.id(0)?,
                email: row.text(1)?,
                created_at: row.int(2)?,
            })
        })
        .transpose()
}

/// Holds the row of an account somebody can sign in to (in use, or waiting to be
/// deleted) until the transaction ends, and returns its name and address.
async fn hold_account(
    scope: &mut Scope<'_>,
    user_id: &str,
) -> Result<Option<(String, String)>, StoreError> {
    let mut sql = scope.sql([
        "SELECT username, email FROM users WHERE (deleted_at IS NULL OR (",
        users::WAITS,
        ")) AND id = ",
    ]);
    sql.bind(Id(user_id));
    sql.push_lock(scope.row_lock(LockRank::Users, user_id, RowLock::Update));
    scope.first(sql).await
}

/// A timestamp strictly after every revert link of `user_id` currently in `email_reverts`
/// (and at least `now`): so an identity tied after an email change is never mistaken for
/// one tied before it, even in the same millisecond or under minor clock skew.
pub(crate) async fn after_latest(
    scope: &mut Scope<'_>,
    user_id: &str,
    now: i64,
) -> Result<i64, StoreError> {
    let mut latest = scope.sql("SELECT MAX(created_at) FROM email_reverts WHERE user_id = ");
    latest.bind(Id(user_id));
    let latest = match scope.fetch_optional(latest).await? {
        Some(row) => row.opt_int(0)?,
        None => None,
    };
    Ok(latest.map_or(now, |latest| now.max(latest.saturating_add(1))))
}

/// Stores the link under `digest` that gives the account `user_id`, whose row the
/// transaction holds, the address `email` back. It is made at `now`, or else strictly
/// after the latest link of the account and at least as late as every identity already
/// tied to the account: ordering among links is strict even at the same moment, and
/// pre-existing identities stay at or before this link even under lock contention or
/// clock step-back.
pub(crate) async fn insert(
    scope: &mut Scope<'_>,
    user_id: &str,
    email: &str,
    digest: &str,
    now: i64,
) -> Result<(), StoreError> {
    let after_reverts = after_latest(scope, user_id, now).await?;
    let latest_identity = oidc_identities::latest_created_at(scope, user_id).await?;
    let made = after_reverts.max(latest_identity.unwrap_or(i64::MIN));
    let mut sql = scope.sql(
        "INSERT INTO email_reverts (token_hash, user_id, email, created_at, expires_at) VALUES (",
    );
    sql.bind(digest)
        .push_bind(", ", Id(user_id))
        .push_bind(", ", email)
        .push_bind(", ", made)
        .push_bind(", ", now.saturating_add(EMAIL_REVERT_LIFETIME_MILLIS))
        .push(")");
    scope.execute(sql).await?;
    Ok(())
}

/// Whether a link of an account other than `but` that stands at `now` holds the
/// address, in any letter case.
pub(crate) async fn holds(
    scope: &mut Scope<'_>,
    email: &str,
    but: Option<&str>,
    now: i64,
) -> Result<bool, StoreError> {
    let mut sql = scope.sql("SELECT 1 FROM email_reverts WHERE lower(email) = lower(");
    sql.bind(email).push_bind(") AND expires_at > ", now);
    if let Some(but) = but {
        sql.push_bind(" AND user_id <> ", Id(but));
    }
    scope.exists(sql).await
}

/// Ends the links of the account: its erasing began.
pub(crate) async fn end_of_user(scope: &mut Scope<'_>, user_id: &str) -> Result<(), StoreError> {
    let mut sql = scope.sql("DELETE FROM email_reverts WHERE user_id = ");
    sql.bind(Id(user_id));
    scope.execute(sql).await?;
    Ok(())
}

/// Removes the links that ended by `now`.
pub(crate) async fn delete_expired(scope: &mut Scope<'_>, now: i64) -> Result<(), StoreError> {
    let mut sql = scope.sql("DELETE FROM email_reverts WHERE expires_at <= ");
    sql.bind(now);
    scope.execute(sql).await?;
    Ok(())
}
