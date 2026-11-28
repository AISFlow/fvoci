//! Personal API tokens: an account's credential for an agent.
//!
//! A token is no organization's (`api_tokens`, as `sessions` is not): a request is read
//! for its token before it is known which organization it is for, so the lookup
//! ([`Store::live_api_token`]) reaches no organization's rows. Which workspaces it opens
//! is behind the wall of their organizations (`api_token_workspaces`), and is asked
//! once a workspace is entered, by a handle of the workspace's organization
//! ([`Store::api_token_reach`]).
//!
//! A token ends with what ends its account's sessions, in the same transaction: a
//! password changed, set or reset, the email taken back, an identity unlinked, a
//! provider removed or changed, an account put out of use or erased
//! ([`revoke_of_user`] and its siblings). It stops reaching a workspace its owner
//! leaves, is made a guest of, or that goes to the trash ([`forget_workspace_of_user`],
//! [`forget_workspace`]). The digest of the token is stored, never the token.

use fvoci_model::{
    API_TOKEN_MAX_LIVE, ApiToken, ApiTokenScope, ApiTokenWorkspace, AuditAction, MemberApiToken,
};

use crate::Store;
use crate::audit::{self, AuditEntry};
use crate::db::{Dialect, FromRow, Id, LockRank, OptId, Row, RowLock, Scope};
use crate::error::StoreError;
use crate::oidc::{Identity, StoredProvider};

/// The columns of a token as its list shows it, in the order [`ApiToken::from_row`] reads.
const LISTED_COLUMNS: &str = "t.id, t.user_id, t.name, t.token_prefix, t.scope, \
     t.allow_personal_space, t.notify_self, t.last_used_at, t.last_used_address, \
     t.created_at, t.expires_at";

/// A workspace a new token is to reach, in its organization.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct GrantedWorkspace<'a> {
    /// The workspace, which the maker of the token was let into.
    pub workspace_id: &'a str,
    /// Its organization, as the server found it when it let the maker in.
    pub org_id: &'a str,
}

/// The provider the session that makes a token was made through, at its revision then,
/// as the session's row keeps it: what the token is stored with.
#[derive(Debug)]
struct TokenOrigin {
    /// Which of the organization's providers it is.
    provider_id: String,
    /// Its revision when the session was made.
    revision: i64,
    /// The identity the session was made for: the provider's issuer then.
    issuer: String,
    /// The subject of that identity; `None` for a session that kept none, which is of
    /// every identity of its issuer.
    subject: Option<String>,
}

/// A token to be stored ([`Store::issue_api_token`]).
#[derive(Debug, Clone, Copy)]
pub struct TokenToIssue<'a> {
    /// Its identifier.
    pub id: &'a str,
    /// Whose it is.
    pub user_id: &'a str,
    /// The session of theirs that makes it: one that has ended makes none.
    pub session_id: &'a str,
    /// Its name, as checked.
    pub name: &'a str,
    /// The characters it is shown by.
    pub token_prefix: &'a str,
    /// The digest of the token.
    pub token_digest: &'a str,
    /// What it may do.
    pub scope: ApiTokenScope,
    /// The workspaces of organizations it reaches.
    pub workspaces: &'a [GrantedWorkspace<'a>],
    /// Whether it reaches the owner's own space.
    pub allow_personal_space: bool,
    /// Whether the owner is told of what it does. The provider it was made through, if
    /// any, is the store's to read from the session.
    pub notify_self: bool,
    /// When it is made.
    pub created_at: i64,
    /// When it ends.
    pub expires_at: i64,
}

/// What came of storing a token.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Issuance {
    /// Stored, as its list shows it.
    Issued(ApiToken),
    /// Not stored: the maker no longer works in this workspace (left it, was made its
    /// guest, or it went to the trash) since they were let in.
    NoLongerWorksIn(String),
    /// Not stored: the session that makes it ended since it was read, with what ended
    /// the account's sessions (a password changed or reset, the account erased...),
    /// which ends their tokens too.
    SessionEnded,
    /// Not stored: the session that makes it was made through a provider that has been
    /// removed, switched off or changed how signing in through it goes since (its
    /// revision). The session proves the provider only as it was, and a token it made
    /// now would outlive the change that takes back the tokens of the revision before.
    ProviderChanged,
    /// Not stored: the account has [`API_TOKEN_MAX_LIVE`] tokens that open anything.
    TooMany,
}

/// A token that opens what it names: in use, not past its end, of an account in use.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LiveApiToken {
    /// Its identifier.
    pub id: String,
    /// Whose it is.
    pub user_id: String,
    /// Its name.
    pub name: String,
    /// What it may do.
    pub scope: ApiTokenScope,
    /// Whether it reaches the owner's own space.
    pub allow_personal_space: bool,
    /// Whether the owner is told of what it does.
    pub notify_self: bool,
    /// The provider of the session that made it, if any.
    pub origin_provider_id: Option<String>,
    /// When its use was last recorded.
    pub last_used_at: Option<i64>,
    /// When it was made.
    pub created_at: i64,
    /// When it ends.
    pub expires_at: i64,
}

/// What a workspace's organization says of a token that names the workspace
/// ([`Store::api_token_reach`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TokenReach {
    /// Whether the organization lets its members give its workspaces to tokens, which
    /// it is asked of only while it takes no session but those made through its
    /// provider.
    pub allow_api_tokens: bool,
}

impl Store {
    /// Stores a token and the workspaces it reaches, each with its organization, in
    /// one transaction that reaches them all: of each, only while `user_id` works in it
    /// (a member who is no guest of a workspace in use), which is asked again here, so
    /// that a maker taken out of a workspace since they were let in does not give it to
    /// a token. A handle of any reach stores it.
    ///
    /// The account is held for the transaction, and the session that makes the token
    /// with it: what ends the account's sessions (and so its tokens) waits for the token
    /// to be stored and ends it then, or the token is not stored, as a session that
    /// ended makes none. A session made through a provider makes one only while the
    /// provider is as it was then (there, switched on, at the session's revision), and
    /// holds it so: a change of the provider waits for the token and takes it back
    /// with the others, or the token is not stored. An account that has
    /// [`API_TOKEN_MAX_LIVE`] tokens that open anything makes no more until one ends.
    ///
    /// # Errors
    ///
    /// Returns [`StoreError::InvalidId`] for an identifier that is none, and
    /// [`StoreError::Database`] if a statement fails, also for a digest another token
    /// is stored under.
    pub async fn issue_api_token(&self, token: &TokenToIssue<'_>) -> Result<Issuance, StoreError> {
        self.system()
            .transaction(async |scope| {
                let origin = match admitted(scope, token).await? {
                    Ok(origin) => origin,
                    Err(refused) => return Ok(refused),
                };
                let mut sql = scope.sql(
                    "INSERT INTO api_tokens (id, user_id, name, token_prefix, token_hash, \
                     scope, allow_personal_space, notify_self, origin_provider_id, \
                     origin_provider_revision, origin_issuer, origin_subject, created_at, \
                     expires_at) VALUES (",
                );
                sql.bind(Id(token.id))
                    .push_bind(", ", Id(token.user_id))
                    .push_bind(", ", token.name)
                    .push_bind(", ", token.token_prefix)
                    .push_bind(", ", token.token_digest)
                    .push_bind(", ", token.scope.as_str())
                    .push_bind(", ", token.allow_personal_space)
                    .push_bind(", ", token.notify_self)
                    .push_bind(
                        ", ",
                        OptId(origin.as_ref().map_or("", |origin| &origin.provider_id)),
                    )
                    .push_bind(
                        ", ",
                        crate::db::Param::OptInt(origin.as_ref().map(|origin| origin.revision)),
                    )
                    .push_bind(
                        ", ",
                        crate::db::Param::OptText(origin.as_ref().map(|origin| origin.issuer.clone())),
                    )
                    .push_bind(
                        ", ",
                        crate::db::Param::OptText(origin.and_then(|origin| origin.subject)),
                    )
                    .push_bind(", ", token.created_at)
                    .push_bind(", ", token.expires_at)
                    .push(")");
                scope.execute(sql).await?;
                for granted in token.workspaces {
                    let mut sql = scope.sql(
                        "INSERT INTO api_token_workspaces (org_id, token_id, workspace_id) \
                         SELECT w.org_id, ",
                    );
                    sql.bind(Id(token.id))
                        .push(", w.id FROM workspaces AS w JOIN workspace_members AS wm ON wm.workspace_id = w.id")
                        .push(&scope.same_organization(&[("wm", "w")]))
                        .push_bind(" WHERE w.id = ", Id(granted.workspace_id))
                        .push_bind(" AND w.org_id = ", Id(granted.org_id))
                        .push(" AND w.deleted_at IS NULL AND w.is_system = ")
                        .bind(false)
                        .push_bind(" AND wm.user_id = ", Id(token.user_id))
                        .push(" AND wm.role IN ('admin', 'member')");
                    if scope.execute(sql).await? == 0 {
                        // Out as an error, so that nothing of the token stays.
                        return Err(StoreError::NoLongerWorksIn(granted.workspace_id.to_owned()));
                    }
                }
                Ok(Issuance::Issued(
                    listed_of(scope, token.id).await?.ok_or(StoreError::NoRows)?,
                ))
            })
            .await
            .or_else(|error| match error {
                StoreError::NoLongerWorksIn(workspace_id) => {
                    Ok(Issuance::NoLongerWorksIn(workspace_id))
                }
                other => Err(other),
            })
    }

    /// The token stored under `token_digest`, if it opens anything at `now`: not taken
    /// back, not past its end, and of an account in use (one put out of use, or waiting
    /// to be deleted, opens nothing through its tokens). Read of no organization.
    ///
    /// # Errors
    ///
    /// Returns [`StoreError::Database`] if the query fails.
    pub async fn live_api_token(
        &self,
        token_digest: &str,
        now: i64,
    ) -> Result<Option<LiveApiToken>, StoreError> {
        self.with_conn(async |scope| {
            let mut sql = scope.sql(
                "SELECT t.id, t.user_id, t.name, t.scope, t.allow_personal_space, \
                 t.notify_self, t.origin_provider_id, t.last_used_at, t.created_at, \
                 t.expires_at \
                 FROM api_tokens AS t JOIN users AS u ON u.id = t.user_id \
                 WHERE t.token_hash = ",
            );
            sql.bind(token_digest)
                .push(" AND t.revoked_at IS NULL")
                .push_bind(" AND t.expires_at > ", now)
                .push(" AND u.deleted_at IS NULL");
            let Some(row) = scope.fetch_optional(sql).await? else {
                return Ok(None);
            };
            let origin_provider_id = row.id(6)?;
            Ok(Some(LiveApiToken {
                id: row.id(0)?,
                user_id: row.id(1)?,
                name: row.text(2)?,
                scope: scope_of(&row, 3)?,
                allow_personal_space: row.bool(4)?,
                notify_self: row.bool(5)?,
                origin_provider_id: Some(origin_provider_id).filter(|id| !id.is_empty()),
                last_used_at: row.opt_int(7)?,
                created_at: row.int(8)?,
                expires_at: row.int(9)?,
            }))
        })
        .await
    }

    /// Records that a token was used at `now`, from `address` if it is known. A use
    /// older than what is recorded changes nothing.
    ///
    /// # Errors
    ///
    /// Returns [`StoreError::Database`] if the statement fails.
    pub async fn record_api_token_use(
        &self,
        token_id: &str,
        now: i64,
        address: Option<&str>,
    ) -> Result<(), StoreError> {
        self.with_write_conn(async |scope| {
            let mut sql = scope.sql("UPDATE api_tokens SET last_used_at = ");
            sql.bind(now)
                .push_bind(
                    ", last_used_address = ",
                    crate::db::Param::OptText(address.map(str::to_owned)),
                )
                .push_bind(" WHERE id = ", Id(token_id))
                .push_bind(" AND (last_used_at IS NULL OR last_used_at < ", now)
                .push(")");
            scope.execute(sql).await?;
            Ok(())
        })
        .await
    }

    /// Whether the token reaches the workspace, asked of the workspace's organization,
    /// the one this handle names: `Some` with what the organization says of tokens if
    /// the token names the workspace, `None` if it does not.
    ///
    /// # Errors
    ///
    /// Returns [`StoreError::NoOrganization`] for a handle of none, and
    /// [`StoreError::Database`] if the query fails.
    pub async fn api_token_reach(
        &self,
        token_id: &str,
        workspace_id: &str,
    ) -> Result<Option<TokenReach>, StoreError> {
        self.with_conn(async |scope| {
            let mut sql = scope.sql(
                "SELECT o.allow_api_tokens FROM api_token_workspaces AS tw \
                 JOIN organizations AS o ON o.id = tw.org_id WHERE tw.token_id = ",
            );
            sql.bind(Id(token_id))
                .push_bind(" AND tw.workspace_id = ", Id(workspace_id))
                .push_bind(" AND tw.org_id = ", scope.org()?);
            Ok(match scope.fetch_optional(sql).await? {
                Some(row) => Some(TokenReach {
                    allow_api_tokens: row.bool(0)?,
                }),
                None => None,
            })
        })
        .await
    }

    /// The tokens of an account that are not taken back, the newest first, each with
    /// the workspaces it reaches that are in use, in whichever organization. Those past
    /// their end are among them until they are taken back.
    ///
    /// # Errors
    ///
    /// Returns [`StoreError::Database`] if a query fails.
    pub async fn api_tokens_of_user(&self, user_id: &str) -> Result<Vec<ApiToken>, StoreError> {
        self.system()
            .with_conn(async |scope| {
                let mut sql = scope.sql([
                    "SELECT ",
                    LISTED_COLUMNS,
                    " FROM api_tokens AS t WHERE t.user_id = ",
                ]);
                sql.bind(Id(user_id))
                    .push(" AND t.revoked_at IS NULL ORDER BY t.created_at DESC, t.id");
                let rows = scope.fetch_all(sql).await?;
                let mut tokens: Vec<ApiToken> = rows
                    .iter()
                    .map(ApiToken::from_row)
                    .collect::<Result<_, _>>()?;
                let mut sql = scope.sql(
                    "SELECT tw.token_id, tw.workspace_id, tw.org_id, w.name \
                     FROM api_token_workspaces AS tw \
                     JOIN api_tokens AS t ON t.id = tw.token_id \
                     JOIN workspaces AS w ON w.id = tw.workspace_id",
                );
                sql.push(&scope.same_organization(&[("tw", "w")]))
                    .push_bind(" WHERE t.user_id = ", Id(user_id))
                    .push(
                        " AND t.revoked_at IS NULL AND w.deleted_at IS NULL ORDER BY w.name, w.id",
                    );
                attach_workspaces(&mut tokens, &scope.fetch_all(sql).await?)?;
                Ok(tokens)
            })
            .await
    }

    /// Takes back a token of `user_id` at `now`: from then on it opens nothing. Whether
    /// there was one to take back.
    ///
    /// # Errors
    ///
    /// Returns [`StoreError::Database`] if the statement fails.
    pub async fn revoke_api_token(
        &self,
        user_id: &str,
        token_id: &str,
        now: i64,
    ) -> Result<bool, StoreError> {
        self.with_write_conn(async |scope| {
            let mut sql = scope.sql("UPDATE api_tokens SET revoked_at = ");
            sql.bind(now)
                .push_bind(" WHERE id = ", Id(token_id))
                .push_bind(" AND user_id = ", Id(user_id))
                .push(" AND revoked_at IS NULL");
            Ok(scope.execute(sql).await? > 0)
        })
        .await
    }

    /// The tokens that reach a workspace of the handle's organization and open anything
    /// at `now`, the newest first, each with those of its workspaces that are of this
    /// organization: what its administrators see of their members' tokens, which is
    /// none of what the token opens elsewhere or where it was used from.
    ///
    /// # Errors
    ///
    /// Returns [`StoreError::NoOrganization`] for a handle of none, and
    /// [`StoreError::Database`] if a query fails.
    pub async fn api_tokens_of_organization(
        &self,
        now: i64,
    ) -> Result<Vec<MemberApiToken>, StoreError> {
        self.with_conn(async |scope| {
            let org = scope.org()?;
            let mut sql = scope.sql(["SELECT ", LISTED_COLUMNS, " FROM api_tokens AS t WHERE "]);
            sql.push("EXISTS (SELECT 1 FROM api_token_workspaces AS tw WHERE tw.token_id = t.id")
                .push_bind(" AND tw.org_id = ", org.clone())
                .push(") AND t.revoked_at IS NULL")
                .push_bind(" AND t.expires_at > ", now)
                .push(" ORDER BY t.created_at DESC, t.id");
            let rows = scope.fetch_all(sql).await?;
            let mut tokens: Vec<ApiToken> = rows
                .iter()
                .map(ApiToken::from_row)
                .collect::<Result<_, _>>()?;
            let mut sql = scope.sql(
                "SELECT tw.token_id, tw.workspace_id, tw.org_id, w.name \
                 FROM api_token_workspaces AS tw JOIN workspaces AS w ON w.id = tw.workspace_id",
            );
            sql.push(&scope.same_organization(&[("tw", "w")]))
                .push_bind(" WHERE tw.org_id = ", org)
                .push(" AND w.deleted_at IS NULL ORDER BY w.name, w.id");
            attach_workspaces(&mut tokens, &scope.fetch_all(sql).await?)?;
            Ok(tokens.into_iter().map(MemberApiToken::from).collect())
        })
        .await
    }

    /// Takes a token that opens anything at `now` out of the handle's organization, for
    /// its administrator `actor_id`: it reaches none of the organization's workspaces
    /// any more, and the record of the audit log says so. What it reaches elsewhere it
    /// keeps; a token left reaching nothing (no workspace, not its owner's own space)
    /// is taken back. Whether there was one to take out.
    ///
    /// The organizations of the token are read with the system's reach: an
    /// organization's handle sees only its own of them, and would take every token for
    /// one that reaches nothing else.
    ///
    /// # Errors
    ///
    /// Returns [`StoreError::NoOrganization`] for a handle of none, and
    /// [`StoreError::Database`] if a statement fails.
    pub async fn revoke_api_token_of_organization(
        &self,
        actor_id: &str,
        token_id: &str,
        now: i64,
    ) -> Result<bool, StoreError> {
        let org_id = self.tenant().org_id().ok_or(StoreError::NoOrganization)?;
        self.system()
            .transaction(async |scope| {
                let mut sql = scope
                    .sql("SELECT t.user_id, t.token_prefix FROM api_tokens AS t WHERE t.id = ");
                sql.bind(Id(token_id))
                    .push(" AND t.revoked_at IS NULL")
                    .push_bind(" AND t.expires_at > ", now)
                    .push_lock(scope.row_lock(LockRank::ApiToken, token_id, RowLock::Update));
                let Some(row) = scope.fetch_optional(sql).await? else {
                    return Ok(false);
                };
                let (owner, prefix) = (row.id(0)?, row.text(1)?);
                let mut sql = scope.sql("DELETE FROM api_token_workspaces WHERE token_id = ");
                sql.bind(Id(token_id))
                    .push_bind(" AND org_id = ", Id(&org_id));
                if scope.execute(sql).await? == 0 {
                    return Ok(false);
                }
                let mut sql = scope.sql("UPDATE api_tokens SET revoked_at = ");
                sql.bind(now)
                    .push_bind(" WHERE id = ", Id(token_id))
                    .push_bind(" AND allow_personal_space = ", false)
                    .push(
                        " AND NOT EXISTS (SELECT 1 FROM api_token_workspaces AS tw \
                         WHERE tw.token_id = api_tokens.id)",
                    );
                scope.execute(sql).await?;
                let mut entry =
                    AuditEntry::new("", actor_id, AuditAction::ApiTokenRevoked, &org_id);
                entry.subject_id = owner;
                entry.detail = prefix;
                audit::record_entry_in(scope, &org_id, &entry).await?;
                Ok(true)
            })
            .await
    }
}

/// Holds the provider, the account and the session that make `token`
/// ([`Store::issue_api_token`]): the provider the session was made through, which the
/// token is stored with, or why the token is not stored.
async fn admitted(
    scope: &mut Scope<'_>,
    token: &TokenToIssue<'_>,
) -> Result<Result<Option<TokenOrigin>, Issuance>, StoreError> {
    let origin = match provider_of_session(scope, token).await? {
        Ok(origin) => origin,
        Err(refused) => return Ok(Err(refused)),
    };
    // Who changes the account (its password, its erasure) waits for this, and so does
    // another token of it, which counts this one then.
    let mut sql = scope.sql("SELECT id FROM users WHERE id = ");
    sql.bind(Id(token.user_id));
    sql.push_lock(scope.row_lock(LockRank::Users, token.user_id, RowLock::Update));
    if !scope.exists(sql).await? {
        return Ok(Err(Issuance::SessionEnded));
    }
    // What ends the session without changing the account (a provider changed or
    // removed, an identity unlinked) waits for this as well, and the token with it.
    let mut sql = scope.sql("SELECT id FROM sessions WHERE id = ");
    sql.bind(Id(token.session_id))
        .push_bind(" AND user_id = ", Id(token.user_id))
        .push_bind(" AND expires_at > ", token.created_at)
        .push_lock(scope.row_lock(LockRank::Session, token.session_id, RowLock::Share));
    if !scope.exists(sql).await? {
        return Ok(Err(Issuance::SessionEnded));
    }
    let mut sql = scope.sql("SELECT COUNT(*) FROM api_tokens WHERE user_id = ");
    sql.bind(Id(token.user_id))
        .push(" AND revoked_at IS NULL")
        .push_bind(" AND expires_at > ", token.created_at);
    let live: i64 = scope.first(sql).await?.unwrap_or(0);
    if usize::try_from(live).unwrap_or(usize::MAX) >= API_TOKEN_MAX_LIVE {
        return Ok(Err(Issuance::TooMany));
    }
    Ok(Ok(origin))
}

/// The provider the session that makes `token` was made through, if it was, held as
/// signing in through it took when the session was made: there, switched on, and at
/// the session's revision (as `app/sso.rs` asks of a session that proves a provider).
/// Says why the token is not stored if the session is gone, or the provider is not so.
///
/// A change or removal of the provider rewrites or deletes its row, so it waits for
/// this token and then takes it back with the others; or it went first, and the row is
/// read as it left it. The provider is held before the account ([`LockRank::Provider`]),
/// as a sign-in through it holds it before the account it lets in
/// (`oidc::finish_oidc_sign_in`), and a trial of pending settings before the account
/// that tried them (`oidc_pending::finish_oidc_trial`).
async fn provider_of_session(
    scope: &mut Scope<'_>,
    token: &TokenToIssue<'_>,
) -> Result<Result<Option<TokenOrigin>, Issuance>, StoreError> {
    // How a session was made does not change; that it is still there is asked again
    // under the account's lock.
    let mut sql = scope.sql(
        "SELECT login_provider_id, login_revision, login_issuer, login_subject \
         FROM sessions WHERE id = ",
    );
    sql.bind(Id(token.session_id))
        .push_bind(" AND user_id = ", Id(token.user_id));
    let Some(row) = scope.fetch_optional(sql).await? else {
        return Ok(Err(Issuance::SessionEnded));
    };
    let provider_id = row.id(0)?;
    if provider_id.is_empty() {
        return Ok(Ok(None));
    }
    // A session made before the revisions were kept proves none of them.
    let Some(revision) = row.opt_int(1)? else {
        return Ok(Err(Issuance::ProviderChanged));
    };
    let origin = TokenOrigin {
        revision,
        issuer: row.text(2)?,
        subject: row.opt_text(3)?,
        provider_id,
    };
    let mut sql = scope.sql("SELECT 1 FROM oidc_providers WHERE id = ");
    sql.bind(Id(&origin.provider_id))
        .push_bind(" AND enabled = ", true)
        .push_bind(" AND revision = ", revision);
    sql.push_lock(scope.row_lock(LockRank::Provider, &origin.provider_id, RowLock::Share));
    if !scope.exists(sql).await? {
        return Ok(Err(Issuance::ProviderChanged));
    }
    Ok(Ok(Some(origin)))
}

/// Takes back every token of an account: with its sessions, in their transaction.
pub(crate) async fn revoke_of_user(scope: &mut Scope<'_>, user_id: &str) -> Result<(), StoreError> {
    let mut sql = scope.sql("UPDATE api_tokens SET revoked_at = ");
    sql.bind(scope.now_millis())
        .push_bind(" WHERE user_id = ", Id(user_id))
        .push(" AND revoked_at IS NULL");
    scope.execute(sql).await?;
    Ok(())
}

/// Takes back the tokens of an account that were made by a session made for
/// `identity`, as its sessions are ended (`sessions::end_sessions_of_identity`): those
/// of the identity's issuer whose session kept no subject too. A token made through
/// another identity, or with the password, stays.
pub(crate) async fn revoke_of_identity(
    scope: &mut Scope<'_>,
    user_id: &str,
    identity: Identity<'_>,
) -> Result<(), StoreError> {
    let mut sql = scope.sql("UPDATE api_tokens SET revoked_at = ");
    sql.bind(scope.now_millis())
        .push_bind(" WHERE user_id = ", Id(user_id))
        .push_bind(" AND origin_issuer = ", identity.issuer)
        .push_bind(
            " AND (origin_subject IS NULL OR origin_subject = ",
            identity.subject,
        )
        .push(") AND revoked_at IS NULL");
    scope.execute(sql).await?;
    Ok(())
}

/// Where the name of a token is copied, found by the token's owner through the token:
/// what an agent wrote through it, kept as a version of a body or waiting in a body's
/// document.
const TOKEN_NAMES_BY_TOKEN: [&str; 2] = [
    "UPDATE body_versions SET actor_token_name = NULL WHERE actor_token_name IS NOT NULL \
     AND actor_token_id IN (SELECT id FROM api_tokens WHERE user_id = ",
    "UPDATE body_docs SET agent_token_name = NULL WHERE agent_token_name IS NOT NULL \
     AND agent_token_id IN (SELECT id FROM api_tokens WHERE user_id = ",
];

/// Where the name of a token is copied, found by its owner: what others are told, or
/// are to be told, of what the owner did through it.
const TOKEN_NAMES_BY_ACTOR: [&str; 2] = [
    "UPDATE notifications SET actor_token_name = NULL WHERE actor_token_name IS NOT NULL \
     AND actor_id = ",
    "UPDATE notification_outbox SET actor_token_name = NULL WHERE actor_token_name IS NOT NULL \
     AND actor_id = ",
];

/// Takes the names of the tokens of `user_id` out of every copy, in every organization,
/// before the tokens go: a name is what the person wrote, as their own name is. What
/// was written through a token stays, and says no more than that an agent wrote it;
/// a comment, no more than a person's comment does.
/// The last step of account erasure calls it with a scope of system reach, so the
/// organizations the account left before it was erased are reached too.
pub(crate) async fn forget_names_of_tokens_of(
    scope: &mut Scope<'_>,
    user_id: &str,
) -> Result<(), StoreError> {
    for update in TOKEN_NAMES_BY_TOKEN {
        let mut sql = scope.sql(update);
        sql.bind(Id(user_id)).push(")");
        scope.execute(sql).await?;
    }
    for update in TOKEN_NAMES_BY_ACTOR {
        let mut sql = scope.sql(update);
        sql.bind(Id(user_id));
        scope.execute(sql).await?;
    }
    forget_names_in_comments_of(scope, user_id).await
}

/// How a comment that says which agent wrote it (`fields.agentTokenName`,
/// [`crate::comment_places::sign`]) is found and unsigned, live and in the trash: by the
/// name, whatever the block is now, since a comment made a card for a while keeps it.
struct SignedComments {
    /// The statement up to the names it takes away: those of the user's tokens.
    unsign: [&'static str; 2],
    /// Who made the comment and who changed it last, as the row has them.
    made_by: [&'static str; 2],
    changed_by: [&'static str; 2],
    /// Whether those are text in JSON rather than identifiers.
    authors_in_json: bool,
}

/// The comments of live blocks, then of blocks in the trash, which keep the whole block
/// as JSON: each on PostgreSQL, then on SQLite.
const SIGNED_COMMENTS: [SignedComments; 2] = [
    SignedComments {
        unsign: [
            "UPDATE blocks SET fields = (fields::jsonb - 'agentTokenName')::json \
             WHERE fields->>'agentTokenName' IN ",
            "UPDATE blocks SET fields = json_remove(fields, '$.agentTokenName') \
             WHERE json_extract(fields, '$.agentTokenName') IN ",
        ],
        made_by: [") AND (created_by = "; 2],
        changed_by: [" OR modified_by = "; 2],
        authors_in_json: false,
    },
    SignedComments {
        unsign: [
            "UPDATE deleted_blocks SET block = (block::jsonb #- '{fields,agentTokenName}')::json \
             WHERE block->'fields'->>'agentTokenName' IN ",
            "UPDATE deleted_blocks SET block = json_remove(block, '$.fields.agentTokenName') \
             WHERE json_extract(block, '$.fields.agentTokenName') IN ",
        ],
        made_by: [
            ") AND (block->>'createdBy' = ",
            ") AND (json_extract(block, '$.createdBy') = ",
        ],
        changed_by: [
            " OR block->>'modifiedBy' = ",
            " OR json_extract(block, '$.modifiedBy') = ",
        ],
        authors_in_json: true,
    },
];

/// Takes the names of the tokens of `user_id` out of the comments their agents wrote,
/// live and in the trash. A comment is theirs to unsign if they made it, changed it
/// last or wrote one of its texts (anyone who edits a board may write a comment's
/// text), and it names one of their tokens: a token of somebody else's that is named
/// the same stays on what that one's agent wrote.
async fn forget_names_in_comments_of(
    scope: &mut Scope<'_>,
    user_id: &str,
) -> Result<(), StoreError> {
    let on = usize::from(scope.dialect() != Dialect::Postgres);
    for comments in &SIGNED_COMMENTS {
        let mut sql = scope.sql(comments.unsign[on]);
        sql.push_bind("(SELECT name FROM api_tokens WHERE user_id = ", Id(user_id));
        if comments.authors_in_json {
            sql.push_bind(comments.made_by[on], user_id)
                .push_bind(comments.changed_by[on], user_id);
        } else {
            sql.push_bind(comments.made_by[on], Id(user_id))
                .push_bind(comments.changed_by[on], Id(user_id));
        }
        sql.push_bind(
            " OR id IN (SELECT block_id FROM comment_versions WHERE written_by = ",
            Id(user_id),
        )
        .push("))");
        scope.execute(sql).await?;
    }
    Ok(())
}

/// Takes back the tokens made through the provider `before` that its change to `after`
/// ends: every one where it is removed (`after` is `None`), and those of another
/// revision where it signs in otherwise. A token proves what the session that made it
/// proved, the revision it was made at; a change that replaced the row gave the
/// provider another identifier, so the tokens are found by the one before.
pub(crate) async fn follow_provider(
    scope: &mut Scope<'_>,
    before: Option<&StoredProvider>,
    after: Option<&StoredProvider>,
) -> Result<(), StoreError> {
    let Some(before) = before else {
        return Ok(());
    };
    match after {
        None => revoke_of_provider(scope, &before.id, None).await,
        Some(after) if after.revision != before.revision => {
            revoke_of_provider(scope, &before.id, Some(after.revision)).await
        }
        Some(_) => Ok(()),
    }
}

/// Takes back the tokens made by sessions made through `provider_id`, but those of
/// `keeping_revision` if one is given: with the provider removed, given another issuer,
/// or given a revision that signs in otherwise.
async fn revoke_of_provider(
    scope: &mut Scope<'_>,
    provider_id: &str,
    keeping_revision: Option<i64>,
) -> Result<(), StoreError> {
    let mut sql = scope.sql("UPDATE api_tokens SET revoked_at = ");
    sql.bind(scope.now_millis())
        .push_bind(" WHERE origin_provider_id = ", Id(provider_id))
        .push(" AND revoked_at IS NULL");
    if let Some(revision) = keeping_revision {
        sql.push_bind(" AND origin_provider_revision <> ", revision);
    }
    scope.execute(sql).await?;
    Ok(())
}

/// Takes back the tokens made through a provider of the scope's organization: with the
/// organization, as it is erased.
pub(crate) async fn revoke_of_organization_providers(
    scope: &mut Scope<'_>,
) -> Result<(), StoreError> {
    let mut sql = scope.sql("UPDATE api_tokens SET revoked_at = ");
    sql.bind(scope.now_millis())
        .push(" WHERE revoked_at IS NULL AND origin_provider_id IN (SELECT id FROM oidc_providers WHERE org_id = ")
        .bind(scope.org()?)
        .push(")");
    scope.execute(sql).await?;
    Ok(())
}

/// Forgets that the tokens of `user_id` reach a workspace of the scope's organization:
/// they leave it, or are made its guest.
pub(crate) async fn forget_workspace_of_user(
    scope: &mut Scope<'_>,
    workspace_id: &str,
    user_id: &str,
) -> Result<(), StoreError> {
    let mut sql = scope.sql("DELETE FROM api_token_workspaces WHERE workspace_id = ");
    sql.bind(Id(workspace_id))
        .push_bind(" AND org_id = ", scope.org()?)
        .push(" AND token_id IN (SELECT id FROM api_tokens WHERE user_id = ")
        .bind(Id(user_id))
        .push(")");
    scope.execute(sql).await?;
    Ok(())
}

/// Forgets that any token reaches a workspace of the scope's organization: it goes to
/// the trash, and coming back from it gives it to no token again.
pub(crate) async fn forget_workspace(
    scope: &mut Scope<'_>,
    workspace_id: &str,
) -> Result<(), StoreError> {
    let mut sql = scope.sql("DELETE FROM api_token_workspaces WHERE workspace_id = ");
    sql.bind(Id(workspace_id))
        .push_bind(" AND org_id = ", scope.org()?);
    scope.execute(sql).await?;
    Ok(())
}

/// Forgets that the tokens of `user_id` reach any workspace of the scope's
/// organization: they leave it.
pub(crate) async fn forget_organization_of_user(
    scope: &mut Scope<'_>,
    user_id: &str,
) -> Result<(), StoreError> {
    let mut sql = scope.sql("DELETE FROM api_token_workspaces WHERE org_id = ");
    sql.bind(scope.org()?)
        .push(" AND token_id IN (SELECT id FROM api_tokens WHERE user_id = ")
        .bind(Id(user_id))
        .push(")");
    scope.execute(sql).await?;
    Ok(())
}

/// A token as its list shows it, by its identifier, with the workspaces it reaches:
/// for the answer to its making, read in its transaction.
async fn listed_of(scope: &mut Scope<'_>, token_id: &str) -> Result<Option<ApiToken>, StoreError> {
    let mut sql = scope.sql([
        "SELECT ",
        LISTED_COLUMNS,
        " FROM api_tokens AS t WHERE t.id = ",
    ]);
    sql.bind(Id(token_id));
    let Some(row) = scope.fetch_optional(sql).await? else {
        return Ok(None);
    };
    let mut tokens = vec![ApiToken::from_row(&row)?];
    let mut sql = scope.sql(
        "SELECT tw.token_id, tw.workspace_id, tw.org_id, w.name \
         FROM api_token_workspaces AS tw JOIN workspaces AS w ON w.id = tw.workspace_id",
    );
    sql.push(&scope.same_organization(&[("tw", "w")]))
        .push_bind(" WHERE tw.token_id = ", Id(token_id))
        .push(" ORDER BY w.name, w.id");
    attach_workspaces(&mut tokens, &scope.fetch_all(sql).await?)?;
    Ok(tokens.pop())
}

/// Reads a token of [`LISTED_COLUMNS`], with no workspace yet.
impl FromRow for ApiToken {
    fn from_row(row: &Row) -> Result<Self, StoreError> {
        Ok(ApiToken {
            id: row.id(0)?,
            user_id: row.id(1)?,
            name: row.text(2)?,
            token_prefix: row.text(3)?,
            scope: scope_of(row, 4)?,
            workspaces: Vec::new(),
            allow_personal_space: row.bool(5)?,
            notify_self: row.bool(6)?,
            last_used_at: row.opt_int(7)?,
            last_used_address: row.opt_text(8)?,
            created_at: row.int(9)?,
            expires_at: row.int(10)?,
        })
    }
}

/// Puts rows of `(token_id, workspace_id, org_id, name)` with their tokens.
fn attach_workspaces(tokens: &mut [ApiToken], rows: &[Row]) -> Result<(), StoreError> {
    for row in rows {
        let token_id = row.id(0)?;
        if let Some(token) = tokens.iter_mut().find(|token| token.id == token_id) {
            token.workspaces.push(ApiTokenWorkspace {
                workspace_id: row.id(1)?,
                org_id: row.id(2)?,
                name: row.text(3)?,
            });
        }
    }
    Ok(())
}

fn scope_of(row: &Row, index: usize) -> Result<ApiTokenScope, StoreError> {
    let text = row.text(index)?;
    text.parse().map_err(|_| StoreError::Corrupt {
        column: "api_tokens.scope",
        reason: format!("{text:?} is no scope"),
    })
}
