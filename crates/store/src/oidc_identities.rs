//! The identities tied to an account: listing them, tying one at the end of a sign-in
//! to link, and removing one again, which ends the account's sessions made for it.
//! Signing in through a provider, and the provider itself, are in `oidc.rs`.

use crate::Store;
use crate::db::{Id, LockRank, RowLock, Scope};
use crate::email_revert;
use crate::error::StoreError;
use crate::oidc::Identity;
use crate::sessions;

impl Store {
    /// The identities tied to an account, each with the address its provider gave:
    /// oldest first.
    ///
    /// # Errors
    ///
    /// Returns [`StoreError::Database`] if the query fails.
    pub async fn identities_of_user(&self, user_id: &str) -> Result<Vec<TiedIdentity>, StoreError> {
        self.with_conn(async |scope| {
            let mut sql = scope.sql(
                "SELECT issuer, subject, email, created_at FROM user_identities WHERE user_id = ",
            );
            sql.bind(Id(user_id))
                .push(" ORDER BY created_at, issuer, subject");
            scope
                .fetch_all(sql)
                .await?
                .iter()
                .map(|row| {
                    Ok(TiedIdentity {
                        issuer: row.text(0)?,
                        subject: row.text(1)?,
                        email: row.text(2)?,
                        created_at: row.int(3)?,
                    })
                })
                .collect()
        })
        .await
    }

    /// Ends a sign-in through the provider `provider_id` of the handle's organization
    /// that was started from a session to link: ties `identity` to the account of that
    /// session, in one transaction. The provider is read again and held, as at the end
    /// of a sign-in; the session `link.session_id` must still be there under the token
    /// digest the browser came back with, good at `link.now`, and of an account in use.
    /// Nothing is stored otherwise. An identity that is the account's already is left
    /// as it is.
    ///
    /// The identity is tied only where `provider_email`, the address the provider
    /// vouches for, is the account's own (whatever the case of its letters): who knows
    /// an account's password ties no other person of the provider to it, who would
    /// then sign in to it without the password.
    ///
    /// # Errors
    ///
    /// Returns [`StoreError::NoOrganization`] if the handle names no organization, and
    /// [`StoreError::Database`] if a statement fails.
    pub async fn finish_oidc_link(
        &self,
        provider_id: &str,
        link: LinkingSession<'_>,
        identity: Identity<'_>,
        provider_email: &str,
    ) -> Result<Result<Linked, LinkRefusal>, StoreError> {
        self.transaction(async |scope| {
            let mut sql = scope.sql("SELECT 1 FROM oidc_providers WHERE org_id = ");
            sql.bind(scope.org()?)
                .push_bind(" AND id = ", Id(provider_id))
                .push_bind(" AND enabled = ", true)
                .push_lock(scope.row_lock(LockRank::Provider, provider_id, RowLock::Share));
            if !scope.exists(sql).await? {
                return Ok(Err(LinkRefusal::ProviderGone));
            }
            let mut sql = scope.sql(
                "SELECT s.user_id, u.email FROM sessions AS s JOIN users AS u \
                 ON u.id = s.user_id WHERE s.id = ",
            );
            sql.bind(Id(link.session_id))
                .push_bind(" AND s.token_hash = ", link.token_digest)
                .push_bind(" AND s.expires_at > ", link.now)
                .push_bind(" AND s.last_used_at > ", link.idle_since)
                .push(" AND u.deleted_at IS NULL");
            // The account stays while its identity is stored.
            let Some((user_id, email)) = scope
                .first_held::<(String, String)>(LockRank::Users, sql, RowLock::Share.of("u"))
                .await?
            else {
                return Ok(Err(LinkRefusal::SessionEnded));
            };
            let is_the_accounts_address = !provider_email.is_empty()
                && provider_email.to_lowercase() == email.to_lowercase();
            if !is_the_accounts_address {
                // Nothing is tied; one that is the account's already is that still.
                let mut sql = scope.sql("SELECT 1 FROM user_identities WHERE issuer = ");
                sql.bind(identity.issuer)
                    .push_bind(" AND subject = ", identity.subject)
                    .push_bind(" AND user_id = ", Id(&user_id));
                return Ok(match scope.fetch_optional(sql).await? {
                    Some(_) => Ok(Linked {
                        user_id,
                        newly: false,
                    }),
                    None => Err(LinkRefusal::AnotherAddress),
                });
            }
            // Later than any revert link of the account, also one made in the same
            // millisecond: reverting a change made before this link always removes
            // identities tied after it.
            let created = email_revert::after_latest(scope, &user_id, link.now).await?;
            // Tied unless it is somebody's already, in one statement: of two links of
            // one identity at once the second waits for the first and ties nothing, and
            // the transaction goes on to read whose it is.
            let mut sql = scope.sql(
                "INSERT INTO user_identities (issuer, subject, user_id, email, created_at) VALUES (",
            );
            sql.bind(identity.issuer)
                .push_bind(", ", identity.subject)
                .push_bind(", ", Id(&user_id))
                .push_bind(", ", provider_email)
                .push_bind(", ", created)
                .push(") ON CONFLICT (issuer, subject) DO NOTHING");
            let newly = scope.execute(sql).await? > 0;
            let mut sql = scope.sql("SELECT user_id FROM user_identities WHERE issuer = ");
            sql.bind(identity.issuer)
                .push_bind(" AND subject = ", identity.subject);
            let owner = match scope.fetch_optional(sql).await? {
                Some(row) => row.id(0)?,
                None => return Err(StoreError::NoRows),
            };
            Ok(if owner == user_id {
                Ok(Linked { user_id, newly })
            } else {
                Err(LinkRefusal::IdentityTaken)
            })
        })
        .await
    }

    /// Removes an identity from the account `user_id`, whose password hash was read as
    /// `seen_password_hash`, in one transaction that holds the account: so that of two
    /// removals at once the second sees what the first left. The last identity of an
    /// account without a password stays: nothing else lets its owner in.
    ///
    /// # Errors
    ///
    /// Returns [`StoreError::Database`] if a statement fails.
    pub async fn unlink_identity(
        &self,
        user_id: &str,
        seen_password_hash: &str,
        identity: Identity<'_>,
    ) -> Result<Unlinked, StoreError> {
        self.transaction(async |scope| {
            let mut sql = scope.sql(
                "SELECT password_hash, password_locked_at FROM users WHERE deleted_at IS NULL AND id = ",
            );
            sql.bind(Id(user_id))
                .push_lock(scope.row_lock(LockRank::Users, user_id, RowLock::Update));
            let (password_hash, locked) = match scope.fetch_optional(sql).await? {
                Some(row) => (row.text(0)?, row.int(1)? != 0),
                None => return Ok(Unlinked::AccountChanged),
            };
            // The password was checked against the hash: a lock since leaves it proving
            // nothing.
            if password_hash != seen_password_hash || (locked && !password_hash.is_empty()) {
                return Ok(Unlinked::AccountChanged);
            }
            let mut sql = scope.sql("SELECT issuer, subject FROM user_identities WHERE user_id = ");
            sql.bind(Id(user_id));
            let tied: Vec<(String, String)> = scope
                .fetch_all(sql)
                .await?
                .iter()
                .map(|row| Ok((row.text(0)?, row.text(1)?)))
                .collect::<Result<_, StoreError>>()?;
            if !tied
                .iter()
                .any(|(issuer, subject)| issuer == identity.issuer && subject == identity.subject)
            {
                return Ok(Unlinked::NotTied);
            }
            if password_hash.is_empty() && tied.len() == 1 {
                return Ok(Unlinked::LastWayIn);
            }
            let mut sql = scope.sql("DELETE FROM user_identities WHERE user_id = ");
            sql.bind(Id(user_id))
                .push_bind(" AND issuer = ", identity.issuer)
                .push_bind(" AND subject = ", identity.subject);
            scope.execute(sql).await?;
            let ended = sessions::end_sessions_of_identity(scope, user_id, identity).await?;
            Ok(Unlinked::Removed(ended))
        })
        .await
    }

    /// The email addresses the providers gave for an account, one for each identity of
    /// it, oldest first: their word, checked by nobody. For tests: nothing shows them
    /// yet, and what does is to mark them as unchecked.
    ///
    /// # Errors
    ///
    /// Returns [`StoreError::Database`] if the query fails.
    #[cfg(any(test, feature = "test-util"))]
    pub async fn provider_emails(&self, user_id: &str) -> Result<Vec<String>, StoreError> {
        self.with_conn(async |scope| {
            let mut sql = scope.sql("SELECT email FROM user_identities WHERE user_id = ");
            sql.bind(Id(user_id))
                .push(" ORDER BY created_at, issuer, subject");
            scope.all(sql).await
        })
        .await
    }

    /// Ties an identity to an account, as a provider gave it. For tests that need an
    /// account that signs in through a provider without the sign-in that makes one.
    ///
    /// # Errors
    ///
    /// Returns [`StoreError::Conflict`] if the identity is an account's already, and
    /// [`StoreError::Database`] if the statement fails.
    #[cfg(any(test, feature = "test-util"))]
    pub async fn add_identity(
        &self,
        user_id: &str,
        identity: Identity<'_>,
        provider_email: &str,
    ) -> Result<(), StoreError> {
        self.add_identity_at(user_id, identity, provider_email, self.now_millis())
            .await
    }

    /// Ties an identity to an account at `created_at` (milliseconds since the Unix
    /// epoch). For tests that verify ordering against other timestamps.
    ///
    /// # Errors
    ///
    /// Returns [`StoreError::Conflict`] if the identity is an account's already, and
    /// [`StoreError::Database`] if the statement fails.
    #[cfg(any(test, feature = "test-util"))]
    pub async fn add_identity_at(
        &self,
        user_id: &str,
        identity: Identity<'_>,
        provider_email: &str,
        created_at: i64,
    ) -> Result<(), StoreError> {
        self.with_write_conn(async |scope| {
            crate::oidc::insert_identity(scope, identity, user_id, provider_email, created_at).await
        })
        .await
    }
}

/// The latest `created_at` among identities tied to `user_id`, if any exists.
pub(crate) async fn latest_created_at(
    scope: &mut Scope<'_>,
    user_id: &str,
) -> Result<Option<i64>, StoreError> {
    let mut latest = scope.sql("SELECT MAX(created_at) FROM user_identities WHERE user_id = ");
    latest.bind(Id(user_id));
    match scope.fetch_optional(latest).await? {
        Some(row) => row.opt_int(0),
        None => Ok(None),
    }
}

/// Removes every identity tied to `user_id` strictly after `after` (milliseconds since
/// the Unix epoch) and returns how many were removed; identities tied at or before
/// `after` stay.
pub(crate) async fn remove_linked_after(
    scope: &mut Scope<'_>,
    user_id: &str,
    after: i64,
) -> Result<u64, StoreError> {
    let mut sql = scope.sql("DELETE FROM user_identities WHERE user_id = ");
    sql.bind(Id(user_id)).push_bind(" AND created_at > ", after);
    scope.execute(sql).await
}

/// An identity tied to an account, as it is stored.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TiedIdentity {
    /// The issuer, as the provider names itself.
    pub issuer: String,
    /// Who the account's owner is at the issuer.
    pub subject: String,
    /// The email address the provider gave when it was tied; empty for none.
    pub email: String,
    /// When it was tied, in milliseconds since the Unix epoch.
    pub created_at: i64,
}

/// The session a sign-in to link was started from, as the browser brings it back.
#[derive(Debug, Clone, Copy)]
pub struct LinkingSession<'a> {
    /// The session the flow names.
    pub session_id: &'a str,
    /// The digest of the session token the browser came back with.
    pub token_digest: &'a str,
    /// Now, in milliseconds since the Unix epoch: the session must not have ended.
    pub now: i64,
    /// The session must have been used after this time.
    pub idle_since: i64,
}

/// An identity that is tied to an account at the end of a sign-in to link.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Linked {
    /// The account.
    pub user_id: String,
    /// Whether this tied it; false if it was the account's already.
    pub newly: bool,
}

/// Why a sign-in to link tied nothing at its end.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LinkRefusal {
    /// The provider is switched off, removed, or another one, since it began.
    ProviderGone,
    /// The session it was started from has ended, or is not the one that came back.
    SessionEnded,
    /// The identity is another account's.
    IdentityTaken,
    /// The address the provider vouches for is not the account's own.
    AnotherAddress,
}

/// What came of removing an identity from an account.
#[derive(Debug, PartialEq, Eq)]
pub enum Unlinked {
    /// It is removed, and the sessions made for it ended.
    Removed(sessions::SessionsEnded),
    /// It is not the account's.
    NotTied,
    /// It stays: the account has no password, and no other identity.
    LastWayIn,
    /// The account was deleted, or its password changed, since it was read.
    AccountChanged,
}

/// Ties an identity to an account, as the provider gave its address.
pub(crate) async fn insert_identity(
    scope: &mut Scope<'_>,
    identity: Identity<'_>,
    user_id: &str,
    email: &str,
    now: i64,
) -> Result<(), StoreError> {
    let mut sql = scope
        .sql("INSERT INTO user_identities (issuer, subject, user_id, email, created_at) VALUES (");
    sql.bind(identity.issuer)
        .push_bind(", ", identity.subject)
        .push_bind(", ", Id(user_id))
        .push_bind(", ", email)
        .push_bind(", ", now)
        .push(")");
    match scope.execute(sql).await {
        Ok(_) => Ok(()),
        Err(StoreError::Database(error)) => Err(crate::users::unique_violation(error)),
        Err(error) => Err(error),
    }
}
