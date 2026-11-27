//! The pending settings of an organization's provider: what an administrator saved of
//! its sign-in (issuer, client, secret) while the organization takes no password. They
//! are kept aside, and nobody signs in through them, until a sign-in through them ends
//! well (a trial): then, in the transaction of that sign-in, they become the
//! provider's. Until then the provider is signed in through as it was, so that settings
//! that do not work lock nobody out. The provider and its sign-ins are in `oidc.rs`.

use std::time::Duration;

use fvoci_model::{AuditAction, LoginOrigin, OrgRole, Session};

use crate::Store;
use crate::audit::{self, AuditEntry};
use crate::db::{Dialect, Id, LockRank, Row, Scope};
use crate::error::StoreError;
use crate::oidc::{
    self, Identity, Newcomers, OidcAccount, OidcRefusal, StoredProvider,
    hold_provider_of_organization, read_provider,
};
use crate::oidc_former::Keeping;
use crate::oidc_held::{self, Tied, Tying};
use crate::oidc_identities::LinkingSession;
use crate::org_admin;
use crate::sessions::{self, SessionsEnded};

/// The columns of pending settings, in the order [`pending_from_row`] reads them.
const PENDING_COLUMNS: &str = "provider_id, issuer, client_id, client_secret, created_at";

/// The pending settings of the provider of an organization, as they are stored.
#[derive(Clone, PartialEq, Eq)]
pub struct PendingProvider {
    /// The provider they are of; they go with it.
    pub provider_id: String,
    /// The issuer, exactly as the provider names itself.
    pub issuer: String,
    /// The client identifier.
    pub client_id: String,
    /// The client secret as the server sealed it, for this issuer and client; empty for
    /// none.
    pub sealed_client_secret: String,
    /// When they were saved, in milliseconds since the Unix epoch: later than the
    /// settings saved before, so that it names this state of them. The store's.
    pub created_at: i64,
}

impl std::fmt::Debug for PendingProvider {
    /// Leaves out the sealed secret.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PendingProvider")
            .field("provider_id", &self.provider_id)
            .field("issuer", &self.issuer)
            .field("client_id", &self.client_id)
            .field("created_at", &self.created_at)
            .finish_non_exhaustive()
    }
}

impl PendingProvider {
    /// `provider` with these settings' issuer, client and secret: what a trial signs in
    /// through, and what the provider becomes when it ends well.
    #[must_use]
    pub fn applied_to(&self, provider: &StoredProvider) -> StoredProvider {
        StoredProvider {
            issuer: self.issuer.clone(),
            client_id: self.client_id.clone(),
            sealed_client_secret: self.sealed_client_secret.clone(),
            ..provider.clone()
        }
    }
}

/// What an administrator saves aside of the provider's sign-in.
#[derive(Clone, PartialEq, Eq)]
pub struct PendingSettings {
    /// The issuer, exactly as the provider names itself.
    pub issuer: String,
    /// The client identifier.
    pub client_id: String,
    /// The client secret as the server sealed it, for this issuer and client; empty for
    /// none.
    pub sealed_client_secret: String,
}

impl std::fmt::Debug for PendingSettings {
    /// Leaves out the sealed secret.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PendingSettings")
            .field("issuer", &self.issuer)
            .field("client_id", &self.client_id)
            .finish_non_exhaustive()
    }
}

/// A trial of the pending settings, as the sign-in that ends it names it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Trial<'a> {
    /// The provider whose pending settings are tried.
    pub provider_id: &'a str,
    /// When the pending settings it tries were saved: their state.
    pub pending_at: i64,
    /// The administrator who started it, whose account alone it may end in.
    pub trier_id: &'a str,
    /// How long the provider before is kept as the former one if the trial moves
    /// the organization to another issuer; no time keeps none.
    pub keep_former: Duration,
}

/// What a trial of the pending settings that ended well did.
#[derive(Debug, PartialEq, Eq)]
pub struct Applied {
    /// The provider as it was before.
    pub before: StoredProvider,
    /// The provider as it is now, with the pending settings: another one, under a new
    /// identifier, for another issuer or client.
    pub provider: StoredProvider,
    /// Whether the account joined the organization by it.
    pub joined: bool,
    /// Sessions ended if the issuer changed.
    pub ended: SessionsEnded,
}

impl Store {
    /// The pending settings of the provider of the handle's organization, if there are.
    ///
    /// # Errors
    ///
    /// Returns [`StoreError::NoOrganization`] if the handle names none, and
    /// [`StoreError::Database`] if the query fails.
    pub async fn oidc_pending_provider(&self) -> Result<Option<PendingProvider>, StoreError> {
        self.with_conn(async |scope| read_pending(scope).await)
            .await
    }

    /// Saves `settings` as the pending settings of the provider of the handle's
    /// organization, in place of the ones there are, if the provider is still the one
    /// read, `read` (the same provider, last changed at the same time), and the
    /// organization still takes no password; and in the same transaction stores
    /// `kept`, the provider with what else the save changes (whether newcomers join),
    /// if that differs. Both are recorded as `actor_id`'s. Returns the provider and
    /// the pending settings as stored, or `None` if the provider changed meanwhile, and
    /// nothing is stored.
    ///
    /// # Errors
    ///
    /// Returns [`StoreError::NoOrganization`] if the handle names none, and
    /// [`StoreError::Database`] if a statement fails.
    pub async fn put_oidc_pending_provider(
        &self,
        settings: &PendingSettings,
        kept: &StoredProvider,
        read: &StoredProvider,
        actor_id: &str,
    ) -> Result<Option<(StoredProvider, PendingProvider)>, StoreError> {
        self.transaction(async |scope| {
            let held = hold_provider_of_organization(scope).await?;
            let Some(mut provider) = read_provider(scope).await? else {
                return Ok(None);
            };
            if (&provider.id, provider.updated_at) != (&read.id, read.updated_at)
                || !provider.sso_required
            {
                return Ok(None);
            }
            if kept.auto_join != provider.auto_join {
                let before = provider;
                let kept = StoredProvider {
                    auto_join: kept.auto_join,
                    ..before.clone()
                };
                let written =
                    oidc::write_provider(scope, held, Some(&before), &kept, Keeping::NOTHING)
                        .await?;
                let _ = written.ended;
                provider = written.provider;
                oidc::record_provider_change(scope, actor_id, Some(&before), &provider).await?;
            }
            // Later than the settings before, also within one millisecond or when the
            // clock went back: a trial names one state of them.
            let base_now = scope.now_millis();
            let now = match read_pending(scope).await? {
                Some(before) => base_now.max(before.created_at.saturating_add(1)),
                None => base_now,
            };
            let mut sql = scope.sql([
                "INSERT INTO oidc_pending_providers (org_id, ",
                PENDING_COLUMNS,
                ") VALUES (",
            ]);
            sql.bind(scope.org()?)
                .push_bind(", ", Id(&provider.id))
                .push_bind(", ", &settings.issuer)
                .push_bind(", ", &settings.client_id)
                .push_bind(", ", &settings.sealed_client_secret)
                .push_bind(", ", now)
                .push(
                    ") ON CONFLICT (org_id) DO UPDATE SET provider_id = excluded.provider_id, \
                     issuer = excluded.issuer, client_id = excluded.client_id, \
                     client_secret = excluded.client_secret, created_at = excluded.created_at",
                )
                .push([" RETURNING ", PENDING_COLUMNS]);
            let row = scope.fetch_optional(sql).await?.ok_or(StoreError::NoRows)?;
            let stored = pending_from_row(&row)?;
            let entry = AuditEntry::new(
                "",
                actor_id,
                AuditAction::OidcProviderPendingSaved,
                &provider.org_id,
            );
            audit::record_entry(scope, &entry).await?;
            Ok(Some((provider, stored)))
        })
        .await
    }

    /// Throws the pending settings of the provider of the handle's organization away,
    /// recorded as `actor_id`'s. Returns whether there were any.
    ///
    /// # Errors
    ///
    /// Returns [`StoreError::NoOrganization`] if the handle names none, and
    /// [`StoreError::Database`] if a statement fails.
    pub async fn discard_oidc_pending_provider(&self, actor_id: &str) -> Result<bool, StoreError> {
        let org_id = self.tenant().org_id().ok_or(StoreError::NoOrganization)?;
        self.transaction(async |scope| {
            hold_provider_of_organization(scope).await?;
            let mut sql = scope.sql("DELETE FROM oidc_pending_providers WHERE org_id = ");
            sql.bind(scope.org()?);
            if scope.execute(sql).await? == 0 {
                return Ok(false);
            }
            let entry = AuditEntry::new(
                "",
                actor_id,
                AuditAction::OidcProviderPendingDiscarded,
                &org_id,
            );
            audit::record_entry(scope, &entry).await?;
            Ok(true)
        })
        .await
    }

    /// Ends `trial`, a trial of the pending settings of the provider of the handle's
    /// organization, in one transaction that holds the provider: they must still be
    /// the provider's pending settings, it must still be on, and who they vouched for
    /// (`tried`) must be the trier, who must still administer the organization. No
    /// account is made by a trial: one that ended in a newcomer or another person
    /// would put settings in use that no administrator can sign in through.
    ///
    /// An identity that is tied to the trier's account lets them in as a sign-in does
    /// (`Store::finish_oidc_sign_in`). One that is tied to no account
    /// ([`Tried::Unbound`]) is tied to the trier's, and that recorded, only where the
    /// pending settings move the organization to another issuer and the trial was
    /// started from a session of the trier's that the browser still has, made through
    /// the provider as it is, at its revision, no earlier than
    /// [`Binding::made_since`], a moment before the trial ends: who only holds a
    /// session, and cannot sign in through the provider, binds nothing.
    ///
    /// Then the pending settings are made the provider's, as a save of them would
    /// (another issuer ends the sessions of the one before, and keeps the provider
    /// before as the former one), that is recorded as the trier's doing, and
    /// `session` is stored under `token_digest`, made through the provider as it is
    /// then: it proves it. Nothing is stored if any of it is refused: no identity
    /// either.
    ///
    /// # Errors
    ///
    /// Returns [`StoreError::NoOrganization`] if the handle names no organization, and
    /// [`StoreError::Database`] if a statement fails.
    pub async fn finish_oidc_trial(
        &self,
        trial: Trial<'_>,
        tried: Tried<'_>,
        session: &mut Session,
        token_digest: &str,
    ) -> Result<Result<Applied, OidcRefusal>, StoreError> {
        let Trial {
            provider_id,
            pending_at,
            trier_id,
            keep_former,
        } = trial;
        self.transaction(async |scope| {
            let held = hold_provider_of_organization(scope).await?;
            let (Some(before), Some(pending)) =
                (read_provider(scope).await?, read_pending(scope).await?)
            else {
                return Ok(Err(OidcRefusal::ProviderGone));
            };
            if before.id != provider_id
                || !before.enabled
                || pending.provider_id != provider_id
                || pending.created_at != pending_at
            {
                return Ok(Err(OidcRefusal::ProviderGone));
            }
            // The provider that is written below, before the account of the trier: a token
            // made through it holds it before the account too.
            oidc::hold_provider_row(scope, &before.id).await?;
            if org_admin::org_role_of(scope, trier_id).await? != Some(OrgRole::Admin) {
                return Ok(Err(OidcRefusal::NotTheTrier));
            }
            let subject = tried.identity().subject.to_owned();
            let joined = match tried {
                Tried::Known {
                    user_id,
                    password_hash,
                    identity,
                } => {
                    if !is_trier_in_use(scope, user_id, trier_id).await? {
                        return Ok(Err(OidcRefusal::NotTheTrier));
                    }
                    let account = OidcAccount::Known {
                        user_id,
                        password_hash,
                        identity,
                        proved_email: None,
                    };
                    let newcomers = Newcomers::from(&before);
                    match oidc::admit(scope, account, newcomers, session).await? {
                        Ok(joined) => joined,
                        Err(refusal) => return Ok(Err(refusal)),
                    }
                }
                Tried::Unbound(binding) => {
                    let moving = Moving {
                        before: &before,
                        pending: &pending,
                    };
                    if let Err(refusal) = bind_to_trier(scope, &binding, trier_id, moving).await? {
                        return Ok(Err(refusal));
                    }
                    // Nothing below refuses: what was tied is stored with the rest, or
                    // not at all.
                    trier_id.clone_into(&mut session.user_id);
                    false
                }
            };
            let mut sql = scope.sql("DELETE FROM oidc_pending_providers WHERE org_id = ");
            sql.bind(scope.org()?);
            scope.execute(sql).await?;
            let applied = pending.applied_to(&before);
            let keeping = Keeping {
                time: keep_former,
                actor_id: Some(trier_id),
            };
            let written =
                oidc::write_provider(scope, held, Some(&before), &applied, keeping).await?;
            let provider = written.provider;
            let action = AuditAction::OidcProviderPendingApplied;
            let entry = AuditEntry::new("", &session.user_id, action, &provider.org_id);
            audit::record_entry(scope, &entry).await?;
            session.login = Some(LoginOrigin {
                provider_id: provider.id.clone(),
                org_id: provider.org_id.clone(),
                issuer: provider.issuer.clone(),
                subject: Some(subject),
                revision: Some(provider.revision),
            });
            sessions::insert_session(scope, session, token_digest).await?;
            Ok(Ok(Applied {
                before,
                provider,
                joined,
                ended: written.ended,
            }))
        })
        .await
    }
}

/// Who the pending settings vouched for at the end of a trial.
pub enum Tried<'a> {
    /// Somebody whose identity is tied to an account in use already: the trier's,
    /// or the trial is refused.
    Known {
        /// The account of the identity.
        user_id: &'a str,
        /// Its password hash as it was read: empty for an account without one.
        password_hash: &'a str,
        /// Who the pending settings vouched for, which must still be the account's.
        identity: Identity<'a>,
    },
    /// Somebody whose identity is tied to no account: bound to the trier's by a
    /// trial started from a session made just now, or the trial is refused.
    Unbound(Binding<'a>),
}

impl std::fmt::Debug for Tried<'_> {
    /// Leaves out the password hash, and what [`Binding`] leaves out.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Known {
                user_id, identity, ..
            } => f
                .debug_struct("Known")
                .field("user_id", user_id)
                .field("issuer", &identity.issuer)
                .finish_non_exhaustive(),
            Self::Unbound(binding) => f.debug_tuple("Unbound").field(binding).finish(),
        }
    }
}

impl Tried<'_> {
    /// Who the pending settings vouched for.
    fn identity(&self) -> Identity<'_> {
        match self {
            Self::Known { identity, .. } => *identity,
            Self::Unbound(binding) => binding.identity,
        }
    }
}

/// An identity to tie to the account of the administrator who tries the pending
/// settings, and what shows that it is they who try.
#[derive(Clone, Copy)]
pub struct Binding<'a> {
    /// Who the pending settings vouched for: tied to no account when it was read.
    pub identity: Identity<'a>,
    /// The address their provider gave, unchecked; empty for none. Kept with the
    /// identity, and compared with nothing.
    pub provider_email: &'a str,
    /// The session the trial was started from, as the browser brings it back.
    pub session: LinkingSession<'a>,
    /// The earliest time that session may have been made at, in milliseconds since
    /// the Unix epoch: a moment before the trial comes back.
    pub made_since: i64,
}

impl std::fmt::Debug for Binding<'_> {
    /// Leaves out the address and the session.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Binding")
            .field("issuer", &self.identity.issuer)
            .field("made_since", &self.made_since)
            .finish_non_exhaustive()
    }
}

/// The provider of an organization and the pending settings that a trial is about to
/// make its own.
#[derive(Debug, Clone, Copy)]
struct Moving<'a> {
    before: &'a StoredProvider,
    pending: &'a PendingProvider,
}

/// Ties the identity of `binding` to the account of the trier, in the trial's
/// transaction, where `moving` moves the organization to another issuer, the one the
/// identity is of, and the session the trial was started from shows that the trier
/// signed in through the provider just now ([`is_made_just_now`]); and records it as
/// the trier's, with who the identity is.
async fn bind_to_trier(
    scope: &mut Scope<'_>,
    binding: &Binding<'_>,
    trier_id: &str,
    moving: Moving<'_>,
) -> Result<Result<(), OidcRefusal>, StoreError> {
    let Moving { before, pending } = moving;
    // Settings of the same issuer vouch for the people the provider vouches for
    // already: another of them is another person, not the trier under a new name.
    if pending.issuer == before.issuer || binding.identity.issuer != pending.issuer {
        return Ok(Err(OidcRefusal::NotTheTrier));
    }
    if !is_made_just_now(scope, binding, trier_id, before).await? {
        return Ok(Err(OidcRefusal::NotFresh));
    }
    let tying = Tying {
        user_id: trier_id,
        email: binding.provider_email,
        now: binding.session.now,
    };
    match oidc_held::tie_unless_taken(scope, binding.identity, tying).await? {
        Tied::Anothers => return Ok(Err(OidcRefusal::NotTheTrier)),
        // Tied to the trier by another way meanwhile: theirs, and nothing to record.
        Tied::Already => {}
        Tied::Newly => {
            let entry =
                AuditEntry::new("", trier_id, AuditAction::OidcIdentityBound, &before.org_id)
                    .with_subject(trier_id)
                    .with_detail(binding.identity.issuer)
                    .with_identity(binding.provider_email, binding.identity.subject);
            audit::record_entry(scope, &entry).await?;
        }
    }
    Ok(Ok(()))
}

/// Whether the session a trial was started from is still there under the token the
/// browser came back with, good, of the trier, whose account is in use and is held
/// until the transaction ends; made through `provider` as it is (this row, at its
/// revision); and made no earlier than `binding.made_since`.
async fn is_made_just_now(
    scope: &mut Scope<'_>,
    binding: &Binding<'_>,
    trier_id: &str,
    provider: &StoredProvider,
) -> Result<bool, StoreError> {
    let session = binding.session;
    let mut sql = scope.sql(
        "SELECT 1 FROM sessions AS s JOIN users AS u ON u.id = s.user_id \
         WHERE u.deleted_at IS NULL AND s.id = ",
    );
    sql.bind(Id(session.session_id))
        .push_bind(" AND s.token_hash = ", session.token_digest)
        .push_bind(" AND s.expires_at > ", session.now)
        .push_bind(" AND s.last_used_at > ", session.idle_since)
        .push_bind(" AND s.user_id = ", Id(trier_id))
        .push_bind(" AND s.login_provider_id = ", Id(&provider.id))
        .push_bind(" AND s.login_org_id = ", Id(&provider.org_id))
        .push_bind(" AND s.login_revision = ", provider.revision)
        .push_bind(" AND s.created_at >= ", binding.made_since);
    scope.record_lock(LockRank::Users, trier_id);
    if scope.dialect() == Dialect::Postgres {
        sql.push(" FOR SHARE OF u");
    }
    Ok(scope.fetch_optional(sql).await?.is_some())
}

/// The pending settings of the transaction's organization's provider, if there are.
async fn read_pending(scope: &mut Scope<'_>) -> Result<Option<PendingProvider>, StoreError> {
    let mut sql = scope.sql([
        "SELECT ",
        PENDING_COLUMNS,
        " FROM oidc_pending_providers WHERE org_id = ",
    ]);
    sql.bind(scope.org()?);
    if scope.dialect() == Dialect::Postgres {
        sql.push(" FOR UPDATE");
    }
    scope
        .fetch_optional(sql)
        .await?
        .as_ref()
        .map(pending_from_row)
        .transpose()
}

fn pending_from_row(row: &Row) -> Result<PendingProvider, StoreError> {
    Ok(PendingProvider {
        provider_id: row.id(0)?,
        issuer: row.text(1)?,
        client_id: row.text(2)?,
        sealed_client_secret: row.text(3)?,
        created_at: row.int(4)?,
    })
}

/// Whether the account a trial ended in is the trier's own, in use: it is held in use
/// to the end of the transaction, so that an account that waits to be deleted changes
/// nothing of its organization (signing in to one opens the way back alone), and one
/// asked to be deleted meanwhile waits for this.
async fn is_trier_in_use(
    scope: &mut Scope<'_>,
    user_id: &str,
    trier_id: &str,
) -> Result<bool, StoreError> {
    if user_id != trier_id {
        return Ok(false);
    }
    crate::users::hold_in_use(scope, trier_id).await
}
