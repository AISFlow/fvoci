//! Login sessions.
//!
//! A session is found by the digest of its token; the token itself is never stored, so
//! that whoever reads the database (or a backup of it) cannot sign in with what is
//! there. A session ends when it has not been used for longer than the idle limit, and
//! at its `expires_at` whatever its use. All times are milliseconds since the Unix
//! epoch; the callers pass the current time and the limits.

use fvoci_model::{LoginOrigin, Session};

use crate::Store;
use crate::api_tokens;
use crate::db::{FromRow, Id, LockRank, OptId, Param, Row, RowLock, Scope, Sql};
use crate::error::StoreError;
use crate::oidc::{Identity, StoredProvider};

/// The columns a session is stored with, in the order it is stored in.
const STORED_COLUMNS: &str = "id, user_id, created_at, last_used_at, expires_at, user_agent, login_provider_id, \
     login_org_id, login_issuer, login_subject, login_revision";

/// The column that says when a session confirmed its account's password, 0 for one
/// that has not. A session is stored without it ([`insert_session`]), and one
/// statement sets it ([`Store::confirm_session_password`]).
const CONFIRMED_COLUMN: &str = "password_confirmed_at";

pub use fvoci_model::EndedSessionScope;

/// The witness of the sessions an operation of the store ended, returned by every
/// operation that ends any, so that its caller closes the connections made with them
/// (`Hub::close`). Only the store makes one.
#[must_use = "close the connections of the sessions it ended with Hub::close"]
#[derive(Debug, PartialEq, Eq)]
pub struct SessionsEnded(Option<Box<EndedSessionScope>>);

/// The account whose password was set anew, and its sessions that ended with the old
/// password.
#[must_use = "close the connections of the sessions it ended with Hub::close"]
#[derive(Debug, PartialEq, Eq)]
pub struct PasswordSet {
    /// The account.
    pub user_id: String,
    /// Its sessions, all of which ended.
    pub ended: SessionsEnded,
}

/// A provider as it was stored in place of the one before, and the sessions that ended
/// with the one before: those made through it, when the issuer changed.
#[must_use = "close the connections of the sessions it ended with Hub::close"]
#[derive(Debug, PartialEq, Eq)]
pub struct ProviderPut {
    /// The provider as stored.
    pub provider: StoredProvider,
    /// The sessions that ended with the provider before.
    pub ended: SessionsEnded,
}

impl SessionsEnded {
    /// The witness of the sessions of `scope`.
    pub(crate) fn new(scope: EndedSessionScope) -> Self {
        match scope {
            EndedSessionScope::None => Self(None),
            other => Self(Some(Box::new(other))),
        }
    }

    /// The witness that no session ended.
    pub(crate) const fn none() -> Self {
        Self(None)
    }

    /// The sessions that ended.
    #[must_use]
    pub fn scope(&self) -> &EndedSessionScope {
        self.0.as_deref().unwrap_or(&EndedSessionScope::None)
    }

    /// The sessions that ended, for closing their connections.
    #[must_use]
    pub fn into_scope(self) -> EndedSessionScope {
        self.0.map_or(EndedSessionScope::None, |boxed| *boxed)
    }

    /// The identifiers of the sessions that ended, when they are named one by one.
    #[must_use]
    pub fn session_ids(&self) -> &[String] {
        match self.scope() {
            EndedSessionScope::Sessions(ids) => ids,
            _ => &[],
        }
    }
}

impl Store {
    /// Stores a new session under the digest of its token, provided the password hash of
    /// its user is still `seen_password_hash`: the hash the password was checked
    /// against. Returns whether the session was stored. Its `last_used_at` is stored as
    /// its `created_at`. One of an account that waits to be deleted is stored too: it
    /// opens the way back and nothing else ([`Store::leaving_session`]).
    ///
    /// Checking a password takes a while. If the password is set anew in that time, the
    /// reset ends the user's sessions, and a session opened with the old password must
    /// not appear after it. The check and the insertion are one step, and a reset that
    /// is under way is waited for.
    ///
    /// # Errors
    ///
    /// Returns [`StoreError::InvalidId`] if the session's identifier is not one, and
    /// [`StoreError::Database`] if a session with the same identifier or the same token
    /// digest exists. A user that does not exist, also one named by text that is no
    /// identifier, has no password hash to be the seen one: the answer is `false`.
    pub async fn create_session(
        &self,
        session: &Session,
        token_digest: &str,
        seen_password_hash: &str,
    ) -> Result<bool, StoreError> {
        self.transaction(async |scope| {
            if hold_unchanged_account(scope, &session.user_id, seen_password_hash)
                .await?
                .is_none()
            {
                return Ok(false);
            }
            insert_session(scope, session, token_digest).await?;
            Ok(true)
        })
        .await
    }

    /// Records at `now` that the session `session_id` of `user_id` confirmed the
    /// account's password, and puts it under `token_digest`: it opens the account's own
    /// space from then on, so its old token no longer finds it. Returns whether it was
    /// recorded: not for a session that is gone or another account's, not for an
    /// account that is not in use, and not unless the account's password hash is still
    /// `seen_password_hash`, the one the password was checked against, and is one. A
    /// change of the password ends the account's other sessions; this is one step
    /// with the check, and a change under way is waited for, so that no session comes
    /// out of it having confirmed a password that is the account's no more.
    ///
    /// The only statement that sets what a session confirmed.
    ///
    /// # Errors
    ///
    /// Returns [`StoreError::Database`] if a statement fails, also for a token digest
    /// another session is stored under.
    pub async fn confirm_session_password(
        &self,
        confirmed: ConfirmedPassword<'_>,
        token_digest: &str,
        now: i64,
    ) -> Result<bool, StoreError> {
        // An account without a password has none to confirm, and its space takes any
        // session: nothing is recorded that a first password would find there.
        if confirmed.seen_password_hash.is_empty() {
            return Ok(false);
        }
        self.transaction(async |scope| {
            let held =
                hold_unchanged_account(scope, confirmed.user_id, confirmed.seen_password_hash)
                    .await?;
            if held != Some(SignedInto::InUse) {
                return Ok(false);
            }
            let mut sql = scope.sql(["UPDATE sessions SET ", CONFIRMED_COLUMN, " = "]);
            sql.bind(now)
                .push_bind(", token_hash = ", token_digest)
                .push_bind(" WHERE id = ", Id(confirmed.session_id))
                .push_bind(" AND user_id = ", Id(confirmed.user_id));
            Ok(scope.execute(sql).await? > 0)
        })
        .await
    }

    /// Returns the session with the given token digest, if it is still valid at `now`:
    /// not past its `expires_at`, used after `idle_since`, and of a user who is not
    /// deactivated.
    ///
    /// # Errors
    ///
    /// Returns [`StoreError::NoRows`] if there is no such session or it has ended.
    pub async fn get_session(
        &self,
        token_digest: &str,
        now: i64,
        idle_since: i64,
    ) -> Result<Session, StoreError> {
        self.with_conn(async |scope| {
            let mut sql = scope.sql([
                "SELECT ",
                STORED_COLUMNS,
                ", ",
                CONFIRMED_COLUMN,
                " FROM sessions WHERE token_hash = ",
            ]);
            sql.bind(token_digest)
                .push_bind(" AND expires_at > ", now)
                .push_bind(" AND last_used_at > ", idle_since)
                .push(
                    " AND EXISTS (SELECT 1 FROM users WHERE users.id = sessions.user_id \
                     AND users.deleted_at IS NULL)",
                );
            scope.first(sql).await?.ok_or(StoreError::NoRows)
        })
        .await
    }

    /// Counts the sessions that are live at `now`, as [`Store::get_session`] would find
    /// them: not past their `expires_at`, used after `idle_since`, and of a user who is
    /// not deactivated.
    ///
    /// # Errors
    ///
    /// Returns [`StoreError::Database`] if the query fails.
    pub async fn count_live_sessions(&self, now: i64, idle_since: i64) -> Result<i64, StoreError> {
        self.with_conn(async |scope| {
            let mut sql = scope.sql("SELECT COUNT(*) FROM sessions WHERE expires_at > ");
            sql.bind(now)
                .push_bind(" AND last_used_at > ", idle_since)
                .push(
                    " AND EXISTS (SELECT 1 FROM users WHERE users.id = sessions.user_id \
                     AND users.deleted_at IS NULL)",
                );
            Ok(scope.first(sql).await?.unwrap_or(0))
        })
        .await
    }

    /// Records that a session was in use at `now`. A session that is gone is left gone,
    /// and the record only ever moves forward: a use that is older than what is recorded
    /// changes nothing.
    ///
    /// # Errors
    ///
    /// Returns [`StoreError::Database`] if the statement fails.
    pub async fn touch_session(&self, session_id: &str, now: i64) -> Result<(), StoreError> {
        self.with_write_conn(async |scope| {
            let mut sql = scope.sql("UPDATE sessions SET last_used_at = ");
            sql.bind(now)
                .push_bind(" WHERE id = ", Id(session_id))
                .push_bind(" AND last_used_at < ", now);
            scope.execute(sql).await?;
            Ok(())
        })
        .await
    }

    /// Sets when a session was last in use, to any time: for tests that need a session
    /// of an age.
    ///
    /// # Errors
    ///
    /// Returns [`StoreError::Database`] if the statement fails.
    #[cfg(feature = "test-util")]
    pub async fn set_session_last_used(
        &self,
        session_id: &str,
        last_used_at: i64,
    ) -> Result<(), StoreError> {
        self.with_write_conn(async |scope| {
            let mut sql = scope.sql("UPDATE sessions SET last_used_at = ");
            sql.bind(last_used_at)
                .push_bind(" WHERE id = ", Id(session_id));
            scope.execute(sql).await?;
            Ok(())
        })
        .await
    }

    /// Deletes the session with the given identifier. Deleting one that does not exist
    /// succeeds.
    ///
    /// # Errors
    ///
    /// Returns [`StoreError::Database`] if the statement fails.
    pub async fn delete_session(&self, session_id: &str) -> Result<SessionsEnded, StoreError> {
        self.with_write_conn(async |scope| {
            let mut sql = scope.sql("DELETE FROM sessions WHERE id = ");
            sql.bind(Id(session_id));
            scope.execute(sql).await?;
            Ok(SessionsEnded::new(EndedSessionScope::Session(
                session_id.to_owned(),
            )))
        })
        .await
    }

    /// Deletes every session that has ended at `now`: past its `expires_at`, or not
    /// used after `idle_since`. Returns a [`SessionsEnded`] witness naming the sessions
    /// that went.
    ///
    /// # Errors
    ///
    /// Returns [`StoreError::Database`] if a statement fails.
    pub async fn clean_up_sessions(
        &self,
        now: i64,
        idle_since: i64,
    ) -> Result<SessionsEnded, StoreError> {
        self.transaction(async |scope| {
            let mut sql = scope.sql("SELECT id FROM sessions WHERE expires_at <= ");
            sql.bind(now).push_bind(" OR last_used_at <= ", idle_since);
            let ended: Vec<String> = scope.all(sql).await?;
            let mut sql = scope.sql("DELETE FROM sessions WHERE expires_at <= ");
            sql.bind(now).push_bind(" OR last_used_at <= ", idle_since);
            scope.execute(sql).await?;
            Ok(if ended.is_empty() {
                SessionsEnded::none()
            } else {
                SessionsEnded::new(EndedSessionScope::Sessions(ended))
            })
        })
        .await
    }
}

/// A session that gave its account's password, and what the password was checked
/// against.
#[derive(Debug, Clone, Copy)]
pub struct ConfirmedPassword<'a> {
    /// The session that gave the password.
    pub session_id: &'a str,
    /// The account of the session.
    pub user_id: &'a str,
    /// The account's password hash as it was when the password was checked.
    pub seen_password_hash: &'a str,
}

impl FromRow for Session {
    fn from_row(row: &Row) -> Result<Self, StoreError> {
        Ok(Session {
            id: row.id(0)?,
            user_id: row.id(1)?,
            created_at: row.int(2)?,
            last_used_at: row.int(3)?,
            expires_at: row.int(4)?,
            user_agent: row.text(5)?,
            login: login_origin(row)?,
            password_confirmed_at: Some(row.int(11)?).filter(|at| *at != 0),
        })
    }
}

/// The origin of a session row: the provider, its organization and its issuer, or
/// none of them (the schema holds them so); the subject where the session knows it.
fn login_origin(row: &Row) -> Result<Option<LoginOrigin>, StoreError> {
    let provider_id = row.id(6)?;
    if provider_id.is_empty() {
        return Ok(None);
    }
    Ok(Some(LoginOrigin {
        provider_id,
        org_id: row.id(7)?,
        issuer: row.text(8)?,
        subject: row.opt_text(9)?,
        revision: row.opt_int(10)?,
    }))
}

/// Starts the deletion of a user's sessions; the caller may narrow it further.
fn delete_sessions_where_user(scope: &Scope<'_>, user_id: &str) -> Sql {
    let mut sql = scope.sql("DELETE FROM sessions WHERE user_id = ");
    sql.bind(Id(user_id));
    sql
}

/// Deletes every session of a user, and takes back every token of theirs: what ends
/// their sessions ends what their sessions made.
pub(crate) async fn delete_sessions_of_user(
    scope: &mut Scope<'_>,
    user_id: &str,
) -> Result<SessionsEnded, StoreError> {
    let sql = delete_sessions_where_user(scope, user_id);
    scope.execute(sql).await?;
    api_tokens::revoke_of_user(scope, user_id).await?;
    Ok(SessionsEnded::new(EndedSessionScope::User(
        user_id.to_owned(),
    )))
}

/// Puts a session under another token digest: its old token no longer finds it.
pub(crate) async fn replace_token_digest(
    scope: &mut Scope<'_>,
    session_id: &str,
    token_digest: &str,
) -> Result<(), StoreError> {
    let mut sql = scope.sql("UPDATE sessions SET token_hash = ");
    sql.bind(token_digest)
        .push_bind(" WHERE id = ", Id(session_id));
    scope.execute(sql).await?;
    Ok(())
}

/// Deletes every session of a user but one, and takes back every token of theirs: a
/// token is not the session that stays.
pub(crate) async fn delete_other_sessions_of_user(
    scope: &mut Scope<'_>,
    user_id: &str,
    kept_session_id: &str,
) -> Result<SessionsEnded, StoreError> {
    let mut sql = delete_sessions_where_user(scope, user_id);
    sql.push_bind(" AND id <> ", Id(kept_session_id));
    scope.execute(sql).await?;
    api_tokens::revoke_of_user(scope, user_id).await?;
    Ok(SessionsEnded::new(EndedSessionScope::UserExcept {
        user_id: user_id.to_owned(),
        kept_session_id: kept_session_id.to_owned(),
    }))
}

/// Ends the sessions made through a provider of the organization `org_id` under
/// `issuer`, whoever's: for when that provider is removed or takes another issuer.
pub(crate) async fn end_sessions_of_origin(
    scope: &mut Scope<'_>,
    org_id: &str,
    issuer: &str,
) -> Result<SessionsEnded, StoreError> {
    let mut sql = scope.sql("DELETE FROM sessions WHERE login_org_id = ");
    sql.bind(Id(org_id))
        .push_bind(" AND login_issuer = ", issuer);
    scope.execute(sql).await?;
    Ok(SessionsEnded::new(EndedSessionScope::Origin {
        org_id: org_id.to_owned(),
        issuer: issuer.to_owned(),
    }))
}

/// Ends the sessions of a user made for `identity`, through a provider of its issuer
/// in whichever organization, and those of its issuer that know no subject
/// ([`LoginOrigin::is_of_identity`]): for when the user unlinks that identity.
pub(crate) async fn end_sessions_of_identity(
    scope: &mut Scope<'_>,
    user_id: &str,
    identity: Identity<'_>,
) -> Result<SessionsEnded, StoreError> {
    let mut sql = delete_sessions_where_user(scope, user_id);
    sql.push_bind(" AND login_issuer = ", identity.issuer)
        .push_bind(
            " AND (login_subject IS NULL OR login_subject = ",
            identity.subject,
        )
        .push(")");
    scope.execute(sql).await?;
    api_tokens::revoke_of_identity(scope, user_id, identity).await?;
    Ok(SessionsEnded::new(EndedSessionScope::Identity {
        user_id: user_id.to_owned(),
        issuer: identity.issuer.to_owned(),
        subject: identity.subject.to_owned(),
    }))
}

/// What the account somebody signs in to is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum SignedInto {
    /// In use: the session opens what is the account's.
    InUse,
    /// Waiting to be deleted: the session opens the way back and nothing else.
    Waiting,
}

/// What a sign-in that holds an account was proved by.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Proof {
    /// The account's password, which a locked one is not.
    Password,
    /// A provider's word, which a lock on the password does not reach.
    Provider,
}

/// Holds the row of an account somebody can sign in to (one in use, or one that waits
/// to be deleted) whose password hash is still `seen_password_hash`, till the end of
/// the transaction, and says which of the two it is; `None` for anybody else, and for
/// an account whose password is locked: the password was what proved it. A reset of
/// the password waits for this, or this for it, and so does the link that locks it. On
/// SQLite a transaction that writes holds the whole database.
pub(crate) async fn hold_unchanged_account(
    scope: &mut Scope<'_>,
    user_id: &str,
    seen_password_hash: &str,
) -> Result<Option<SignedInto>, StoreError> {
    hold_account_proved_by(scope, user_id, seen_password_hash, Proof::Password).await
}

/// As [`hold_unchanged_account`], for a sign-in through a provider: a locked password
/// is no part of what proved it.
pub(crate) async fn hold_unchanged_account_for_provider(
    scope: &mut Scope<'_>,
    user_id: &str,
    seen_password_hash: &str,
) -> Result<Option<SignedInto>, StoreError> {
    hold_account_proved_by(scope, user_id, seen_password_hash, Proof::Provider).await
}

async fn hold_account_proved_by(
    scope: &mut Scope<'_>,
    user_id: &str,
    seen_password_hash: &str,
    proof: Proof,
) -> Result<Option<SignedInto>, StoreError> {
    let mut sql = scope.sql([
        "SELECT password_hash, deleted_at, password_locked_at FROM users \
         WHERE (deleted_at IS NULL OR (",
        crate::users::WAITS,
        ")) AND id = ",
    ]);
    sql.bind(Id(user_id));
    sql.push_lock(scope.row_lock(LockRank::Users, user_id, RowLock::Share));
    let Some(row) = scope.fetch_optional(sql).await? else {
        return Ok(None);
    };
    if row.text(0)? != seen_password_hash || (proof == Proof::Password && row.int(2)? != 0) {
        return Ok(None);
    }
    Ok(Some(if row.opt_int(1)?.is_none() {
        SignedInto::InUse
    } else {
        SignedInto::Waiting
    }))
}

/// The session stored under the token digest, whatever its state and its account's.
pub(crate) async fn session_by_digest(
    scope: &mut Scope<'_>,
    token_digest: &str,
) -> Result<Option<Session>, StoreError> {
    let mut sql = scope.sql([
        "SELECT ",
        STORED_COLUMNS,
        ", ",
        CONFIRMED_COLUMN,
        " FROM sessions WHERE token_hash = ",
    ]);
    sql.bind(token_digest);
    scope.first(sql).await
}

/// Stores a session under the digest of its token, as one that has confirmed nothing:
/// what a session confirms, it confirms after it is there.
pub(crate) async fn insert_session(
    scope: &mut Scope<'_>,
    session: &Session,
    token_digest: &str,
) -> Result<(), StoreError> {
    let mut sql = scope.sql([
        "INSERT INTO sessions (token_hash, ",
        STORED_COLUMNS,
        ") VALUES (",
    ]);
    sql.bind(token_digest)
        .push_bind(", ", Id(&session.id))
        .push_bind(", ", Id(&session.user_id))
        .push_bind(", ", session.created_at)
        .push_bind(", ", session.created_at)
        .push_bind(", ", session.expires_at)
        .push_bind(", ", &session.user_agent)
        .push_bind(", ", OptId(session.login_provider_id().unwrap_or_default()))
        .push_bind(
            ", ",
            OptId(
                session
                    .login
                    .as_ref()
                    .map_or("", |login| login.org_id.as_str()),
            ),
        )
        .push_bind(
            ", ",
            Param::OptText(session.login.as_ref().map(|login| login.issuer.clone())),
        )
        .push_bind(
            ", ",
            Param::OptText(
                session
                    .login
                    .as_ref()
                    .and_then(|login| login.subject.clone()),
            ),
        )
        .push_bind(
            ", ",
            Param::OptInt(session.login.as_ref().and_then(|login| login.revision)),
        )
        .push(")");
    scope.execute(sql).await?;
    Ok(())
}
