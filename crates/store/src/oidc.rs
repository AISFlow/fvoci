//! Signing in through an organization's `OpenID Connect` provider: the provider, the
//! sign-ins under way, and who a provider vouches for.
//!
//! The provider and a sign-in that is started are the rows of the handle's organization
//! ([`Tenant::Org`](crate::Tenant::Org)). An identity, like the account it belongs to,
//! is nobody's. What is asked before the organization is known (which sign-in comes
//! back, which organization a login page is for) is in `directory.rs`; tying an
//! identity to an account and removing it, in `oidc_identities.rs`.

use std::time::Duration;

use fvoci_model::{
    AuditAction, InstanceRole, LoginOrigin, OrgRole, Session, User, WorkspaceRole, new_id,
};

use crate::Store;
use crate::accounts::{self, Which};
use crate::audit::{self, AuditActor, AuditEntry};
use crate::db::{FromRow, Id, LockRank, OrganizationHeld, Row, RowLock, Scope};
use crate::error::StoreError;
use crate::oidc_former::{self, Keeping};
use crate::sessions::{self, ProviderPut, SessionsEnded};

/// The columns of a provider, in the order [`StoredProvider::from_row`] reads them.
const PROVIDER_COLUMNS: &str = "id, org_id, issuer, client_id, client_secret, auto_join, enabled, \
                                created_at, updated_at, sso_required, revision";

/// The provider of an organization, as it is stored.
#[derive(Clone, PartialEq, Eq)]
pub struct StoredProvider {
    /// The provider's identifier, which a session made through it names.
    pub id: String,
    /// The organization whose provider it is.
    pub org_id: String,
    /// The issuer, exactly as the provider names itself.
    pub issuer: String,
    /// The client identifier.
    pub client_id: String,
    /// The client secret as the server sealed it; empty for none. The store never
    /// sees the secret itself.
    pub sealed_client_secret: String,
    /// Whether somebody it vouches for who is no member becomes one by signing in.
    pub auto_join: bool,
    /// Whether signing in through it is on.
    pub enabled: bool,
    /// When it was first set, in milliseconds since the Unix epoch.
    pub created_at: i64,
    /// When it was last changed, in milliseconds since the Unix epoch.
    pub updated_at: i64,
    /// Whether what is the organization's opens only for a session made through it.
    pub sso_required: bool,
    /// Which state of what signing in through it takes (issuer, client, secret, and
    /// whether it is on): raised by every change of that, kept by the others. A
    /// session made through the provider keeps the revision it was made at, and
    /// proves this one only ([`Self::signs_in_like`]). The store's.
    pub revision: i64,
}

impl StoredProvider {
    /// Whether signing in through `other` takes the same as through this one: the same
    /// issuer, client and sealed secret, on or off alike. A change that keeps it keeps
    /// the revision; any other raises it. A secret given again is sealed anew and
    /// counts as another here; the server keeps the sealed one for a secret sent again
    /// as it is stored.
    #[must_use]
    pub fn signs_in_like(&self, other: &Self) -> bool {
        self.issuer == other.issuer
            && self.client_id == other.client_id
            && self.sealed_client_secret == other.sealed_client_secret
            && self.enabled == other.enabled
    }
}

impl std::fmt::Debug for StoredProvider {
    /// Leaves out the sealed secret.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("StoredProvider")
            .field("id", &self.id)
            .field("org_id", &self.org_id)
            .field("issuer", &self.issuer)
            .field("client_id", &self.client_id)
            .field("auto_join", &self.auto_join)
            .field("enabled", &self.enabled)
            .field("sso_required", &self.sso_required)
            .field("revision", &self.revision)
            .finish_non_exhaustive()
    }
}

/// The identity a provider vouches for: who the subject is at the issuer.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Identity<'a> {
    /// The issuer, exactly as in the ID token.
    pub issuer: &'a str,
    /// The subject, exactly as in the ID token.
    pub subject: &'a str,
}

impl Store {
    /// The provider of the handle's organization, if it has one.
    ///
    /// # Errors
    ///
    /// Returns [`StoreError::NoOrganization`] if the handle names none, and
    /// [`StoreError::Database`] if the query fails.
    pub async fn oidc_provider(&self) -> Result<Option<StoredProvider>, StoreError> {
        self.with_conn(async |scope| read_provider(scope).await)
            .await
    }

    /// Stores the provider of the handle's organization, in place of the one it has,
    /// and returns it as stored. A provider whose issuer and client stay keeps its
    /// identifier and the time it was first set; one with another issuer or client is
    /// another provider, under a new identifier, and the sign-ins under way through the
    /// one before end with it. `provider.id`, `org_id` and the times are the store's.
    ///
    /// # Errors
    ///
    /// Returns [`StoreError::NoOrganization`] if the handle names none, and
    /// [`StoreError::Database`] if a statement fails.
    #[cfg(any(test, feature = "test-util"))]
    pub async fn put_oidc_provider(
        &self,
        provider: &StoredProvider,
    ) -> Result<StoredProvider, StoreError> {
        let put = self
            .put_provider(provider, AsRead::Whatever, Keeping::NOTHING)
            .await?
            .ok_or(StoreError::NoRows)?;
        Ok(put.provider)
    }

    /// Stores the provider of the handle's organization, in place of the one it has, if
    /// that one is still the one read, `read` (the same provider, last changed at the
    /// same time), or there is still none for `None`; and returns it as stored with the
    /// sessions ended when the issuer changed. `None`
    /// if it changed or was removed meanwhile, and nothing is stored: what was decided
    /// from it (the secret kept, the settings kept) is to be decided again. A provider whose issuer and client stay keeps its
    /// identifier and the time it was first set; one with another issuer or client is
    /// another provider, under a new identifier, and the sign-ins under way through the
    /// one before end with it; with another issuer, so do the sessions made through a
    /// provider of the organization under the issuer before, and the provider before is
    /// kept as the former one for `keep_former` (none for no time), in place of
    /// the one kept until then. `provider.id`, `org_id` and the times are the store's;
    /// the time of a change of a provider is always later than that of the one before,
    /// and a provider removed and set again is another, so that a reading names one
    /// state of the provider.
    ///
    /// # Errors
    ///
    /// Returns [`StoreError::NoOrganization`] if the handle names none, and
    /// [`StoreError::Database`] if a statement fails.
    pub async fn put_oidc_provider_as_read(
        &self,
        provider: &StoredProvider,
        read: Option<&StoredProvider>,
        actor_id: &str,
        keep_former: Duration,
    ) -> Result<Option<ProviderPut>, StoreError> {
        let read = read.map(|read| (read.id.clone(), read.updated_at));
        let keeping = Keeping {
            time: keep_former,
            actor_id: Some(actor_id),
        };
        self.put_provider(provider, AsRead::Was(read), keeping)
            .await
    }

    async fn put_provider(
        &self,
        provider: &StoredProvider,
        as_read: AsRead,
        keeping: Keeping<'_>,
    ) -> Result<Option<ProviderPut>, StoreError> {
        self.transaction(async |scope| {
            let held = hold_provider_of_organization(scope).await?;
            let before = read_provider(scope).await?;
            let there = before
                .as_ref()
                .map(|before| (before.id.clone(), before.updated_at));
            if let AsRead::Was(read) = &as_read
                && there != *read
            {
                return Ok(None);
            }
            let written = write_provider(scope, held, before.as_ref(), provider, keeping).await?;
            if let Some(actor_id) = keeping.actor_id {
                record_provider_change(scope, actor_id, before.as_ref(), &written.provider).await?;
                if written.dropped_pending {
                    let entry = AuditEntry::new(
                        "",
                        actor_id,
                        AuditAction::OidcProviderPendingDiscarded,
                        &written.provider.org_id,
                    );
                    audit::record_entry(scope, &entry).await?;
                }
            }
            Ok(Some(ProviderPut {
                provider: written.provider,
                ended: written.ended,
            }))
        })
        .await
    }

    /// Removes the provider of the handle's organization, the sign-ins under way
    /// through it, the former provider kept beside it, and the sessions (and tokens)
    /// made through a provider of the organization under its issuer, unless it is the
    /// one the organization takes nothing else than (`sso_required`): that one stays,
    /// and is released first. Asked and removed in one transaction that holds the
    /// provider, so that no removal passes an enforcement that is switched on beside it.
    ///
    /// # Errors
    ///
    /// Returns [`StoreError::NoOrganization`] if the handle names none, and
    /// [`StoreError::Database`] if a statement fails.
    pub async fn delete_oidc_provider(&self, actor_id: &str) -> Result<Removal, StoreError> {
        self.transaction(async |scope| {
            hold_provider_of_organization(scope).await?;
            let provider = match read_provider(scope).await? {
                None => return Ok(Removal::NoneThere),
                Some(provider) if provider.sso_required => return Ok(Removal::Enforced),
                Some(provider) => provider,
            };
            let mut sql = scope.sql("DELETE FROM oidc_providers WHERE org_id = ");
            sql.bind(Id(&provider.org_id));
            scope.execute(sql).await?;
            let ended =
                sessions::end_sessions_of_origin(scope, &provider.org_id, &provider.issuer).await?;
            crate::api_tokens::follow_provider(scope, Some(&provider), None).await?;
            oidc_former::give_way(scope, Some(actor_id)).await?;
            let entry = AuditEntry::new(
                "",
                actor_id,
                AuditAction::OidcProviderRemoved,
                &provider.org_id,
            );
            audit::record_entry(scope, &entry).await?;
            Ok(Removal::Removed { provider, ended })
        })
        .await
    }

    /// Releases the handle's organization from taking only sessions made through its
    /// provider, on the word of whoever runs the server (the administration socket),
    /// whatever the provider is: the way out when nobody gets in through it. It is
    /// recorded as the server operator's; nothing that a person asks for calls it.
    /// Returns the provider as it is then if this released it, `None` if there was
    /// nothing to release.
    ///
    /// # Errors
    ///
    /// Returns [`StoreError::NoOrganization`] if the handle names none, and
    /// [`StoreError::Database`] if a statement fails.
    pub async fn release_sso_by_operator(&self) -> Result<Option<StoredProvider>, StoreError> {
        self.transaction(async |scope| {
            hold_provider_of_organization(scope).await?;
            let mut sql = scope.sql("SELECT updated_at FROM oidc_providers WHERE org_id = ");
            sql.bind(scope.org()?)
                .push_bind(" AND sso_required = ", true);
            let Some(row) = scope.fetch_optional(sql).await? else {
                return Ok(None);
            };
            // Another state of the provider, with a later time, as every change is.
            let now = scope.now_millis().max(row.int(0)?.saturating_add(1));
            let mut sql = scope.sql("UPDATE oidc_providers SET sso_required = ");
            sql.bind(false)
                .push_bind(", updated_at = ", now)
                .push_bind(" WHERE org_id = ", scope.org()?)
                .push([" RETURNING ", PROVIDER_COLUMNS]);
            let row = scope.fetch_optional(sql).await?.ok_or(StoreError::NoRows)?;
            let stored = StoredProvider::from_row(&row)?;
            let entry = AuditEntry::by(
                &AuditActor::ServerOperator,
                "",
                AuditAction::SsoReleased,
                &stored.org_id,
            );
            audit::record_entry(scope, &entry).await?;
            Ok(Some(stored))
        })
        .await
    }

    /// The account a provider vouches for under `identity`, if there is one somebody
    /// can sign in to: in use, or waiting to be deleted (`Store::account_by_id`). A
    /// deactivated account is none.
    ///
    /// # Errors
    ///
    /// Returns [`StoreError::Database`] if the query fails.
    pub async fn user_of_identity(
        &self,
        identity: Identity<'_>,
    ) -> Result<Option<User>, StoreError> {
        self.with_conn(async |scope| {
            let mut sql =
                scope.sql("SELECT user_id FROM user_identities AS i JOIN users AS u ON u.id = i.user_id WHERE i.issuer = ");
            sql.bind(identity.issuer)
                .push_bind(" AND i.subject = ", identity.subject)
                .push(" AND (u.deleted_at IS NULL OR (u.delete_after > 0 AND u.erase_started_at = 0))");
            let Some(user_id) = scope.first::<String>(sql).await? else {
                return Ok(None);
            };
            crate::users::account_by_id(scope, &user_id).await.map(Some)
        })
        .await
    }

    /// Whether the identity is an account's, whether the account is deleted or not.
    ///
    /// # Errors
    ///
    /// Returns [`StoreError::Database`] if the query fails.
    pub async fn is_identity_known(&self, identity: Identity<'_>) -> Result<bool, StoreError> {
        self.with_conn(async |scope| {
            let mut sql = scope.sql("SELECT 1 FROM user_identities WHERE issuer = ");
            sql.bind(identity.issuer)
                .push_bind(" AND subject = ", identity.subject);
            scope.exists(sql).await
        })
        .await
    }

    /// Ends a sign-in through the provider `provider_id` of the handle's organization,
    /// at its `revision`, in one transaction: reads the provider again, and holds it on
    /// PostgreSQL, so that a provider switched off, removed, replaced, given another
    /// secret or closed to newcomers while the person was at the provider lets nobody
    /// in, and the session proves the revision it was signed in at; then makes or lets in the account,
    /// and stores `session` (for `account`'s user) under `token_digest`. Says whether
    /// the account joined the organization, or why nothing was stored.
    ///
    /// A new account ([`OidcAccount::New`]) is stored with no password (an empty
    /// hash, which no password matches) and no email address of its own:
    /// `provider_email`, the address the provider gave, is kept with the identity,
    /// unchecked, and holds no account's place.
    ///
    /// A registration by a link of the organization that waits for the address the
    /// provider vouches for is removed with the sign-in, new account or known: the
    /// provider's word stands for the mail's there, and no second account comes of
    /// the registration. The use it took of the link is given back. `user` is updated to what was stored:
    /// the current time as creation and modification time, not deleted, no
    /// administrator of the instance. It joins the organization and its default
    /// workspace. A known account ([`OidcAccount::Known`]) joins them if it has no
    /// place in the organization (a guest of one of its workspaces has one), as often
    /// as that is asked at once.
    ///
    /// # Errors
    ///
    /// Returns [`StoreError::Conflict`] if another account has the new account's name
    /// in any letter case, the identity is another account's already, or
    /// ([`crate::Taken::Email`]) an account has the provider's address as its own by now;
    /// [`StoreError::NoOrganization`] if the handle names no organization; and
    /// [`StoreError::Database`] if a statement fails. Nothing is stored then.
    pub async fn finish_oidc_sign_in(
        &self,
        provider_id: &str,
        revision: i64,
        account: OidcAccount<'_>,
        session: &mut Session,
        token_digest: &str,
    ) -> Result<Result<bool, OidcRefusal>, StoreError> {
        self.transaction(async |scope| {
            // Before the account it lets in, as a token made through it and a trial of
            // its pending settings hold it ([`LockRank::Provider`]).
            let mut sql = scope.sql(
                "SELECT auto_join, issuer, org_id, revision FROM oidc_providers WHERE org_id = ",
            );
            sql.bind(scope.org()?)
                .push_bind(" AND id = ", Id(provider_id))
                .push_bind(" AND enabled = ", true)
                .push_bind(" AND revision = ", revision);
            sql.push_lock(scope.row_lock(LockRank::Provider, provider_id, RowLock::Share));
            let Some(row) = scope.fetch_optional(sql).await? else {
                return Ok(Err(OidcRefusal::ProviderGone));
            };
            let newcomers = if row.bool(0)? {
                Newcomers::Join
            } else {
                Newcomers::Refused
            };
            let origin = LoginOrigin {
                provider_id: provider_id.to_owned(),
                org_id: row.id(2)?,
                issuer: row.text(1)?,
                subject: Some(account.identity().subject.to_owned()),
                revision: Some(row.int(3)?),
            };
            let joined = match admit(scope, account, newcomers, session).await? {
                Ok(joined) => joined,
                Err(refusal) => return Ok(Err(refusal)),
            };
            session.login = Some(origin);
            sessions::insert_session(scope, session, token_digest).await?;
            Ok(Ok(joined))
        })
        .await
    }

    /// Whether the user is a member of the handle's organization.
    ///
    /// # Errors
    ///
    /// Returns [`StoreError::NoOrganization`] if the handle names none, and
    /// [`StoreError::Database`] if the query fails.
    #[cfg(any(test, feature = "test-util"))]
    pub async fn is_organization_member(&self, user_id: &str) -> Result<bool, StoreError> {
        self.with_conn(async |scope| is_member(scope, user_id).await)
            .await
    }
}

/// Holds the provider of the transaction's organization, there or not, until the
/// transaction ends: changes and removals of it are one after the other. On PostgreSQL
/// the organization's row is held (no key of it changes, so its references are not held
/// up); on SQLite a transaction that writes holds the database.
pub(crate) async fn hold_provider_of_organization<'c>(
    scope: &mut Scope<'c>,
) -> Result<OrganizationHeld<'c>, StoreError> {
    let org_key = scope.org_id().unwrap_or_default();
    let mut sql = scope.sql("SELECT 1 FROM organizations WHERE id = ");
    sql.bind(scope.org()?);
    scope
        .hold(LockRank::Organization, &org_key, sql, RowLock::NoKeyUpdate)
        .await?;
    Ok(OrganizationHeld::new())
}

/// Holds the row of the provider `provider_id` against its sign-ins and tokens, which
/// hold it for share, and its other writers, until the transaction ends
/// ([`LockRank::Provider`]). A trial of pending settings, which writes the provider and
/// holds the account of its trier, holds the provider first, as a sign-in and a token
/// through it hold it before their account: the other way round, a trial and a token
/// would wait for each other. On SQLite a transaction that writes holds the database.
pub(crate) async fn hold_provider_row(
    scope: &mut Scope<'_>,
    provider_id: &str,
) -> Result<(), StoreError> {
    let mut sql = scope.sql("SELECT 1 FROM oidc_providers WHERE id = ");
    sql.bind(Id(provider_id));
    scope
        .hold(LockRank::Provider, provider_id, sql, RowLock::Update)
        .await?;
    Ok(())
}

/// The provider of the transaction's organization, as it is stored, if it has one.
pub(crate) async fn read_provider(
    scope: &mut Scope<'_>,
) -> Result<Option<StoredProvider>, StoreError> {
    let mut sql = scope.sql([
        "SELECT ",
        PROVIDER_COLUMNS,
        " FROM oidc_providers WHERE org_id = ",
    ]);
    sql.bind(scope.org()?);
    scope.first(sql).await
}

/// A provider as [`write_provider`] stored it.
pub(crate) struct Written {
    /// The provider as stored.
    pub(crate) provider: StoredProvider,
    /// Whether pending settings of the provider were thrown away with the change.
    pub(crate) dropped_pending: bool,
    /// Sessions ended when the issuer changed.
    pub(crate) ended: SessionsEnded,
}

/// Stores `provider` as the transaction's organization's, in place of `before`, under
/// `held`. A provider whose issuer and client stay keeps its identifier and the time it was
/// first set; one with another issuer or client is another provider, under a new
/// identifier, and the sign-ins under way through the one before end with it; with
/// another issuer, so do the sessions made through a provider of the organization
/// under the issuer before, and the provider before is kept as the former one as
/// `keeping` says (`oidc_former.rs`). A change of what signing in takes raises the
/// revision and throws away the pending settings, which were saved against what
/// signing in took.
pub(crate) async fn write_provider<'c>(
    scope: &mut Scope<'c>,
    held: OrganizationHeld<'c>,
    before: Option<&StoredProvider>,
    provider: &StoredProvider,
    keeping: Keeping<'_>,
) -> Result<Written, StoreError> {
    // Later than the change before, also within one millisecond or when the clock went
    // back: no two states of one provider have one time.
    let base_now = scope.now_millis();
    let now = before.map_or(base_now, |before| {
        base_now.max(before.updated_at.saturating_add(1))
    });
    let signs_in_alike = before.is_some_and(|before| before.signs_in_like(provider));
    // Raised across a replacement too: no session of the one before names it.
    let revision = match before {
        None => 1,
        Some(before) if signs_in_alike => before.revision,
        Some(before) => before.revision.saturating_add(1),
    };
    let dropped_pending = before.is_some() && !signs_in_alike && {
        let mut sql = scope.sql("DELETE FROM oidc_pending_providers WHERE org_id = ");
        sql.bind(scope.org()?);
        scope.execute(sql).await? > 0
    };
    let mut sql = scope.sql("DELETE FROM oidc_providers WHERE org_id = ");
    sql.bind(scope.org()?)
        .push_bind(" AND (issuer <> ", &provider.issuer)
        .push_bind(" OR client_id <> ", &provider.client_id)
        .push(")");
    scope.execute(sql).await?;
    let mut sql = scope.sql([
        "INSERT INTO oidc_providers (",
        PROVIDER_COLUMNS,
        ") VALUES (",
    ]);
    sql.bind(Id(&new_id()))
        .push_bind(", ", scope.org()?)
        .push_bind(", ", &provider.issuer)
        .push_bind(", ", &provider.client_id)
        .push_bind(", ", &provider.sealed_client_secret)
        .push_bind(", ", provider.auto_join)
        .push_bind(", ", provider.enabled)
        .push_bind(", ", now)
        .push_bind(", ", now)
        .push_bind(", ", provider.sso_required)
        .push_bind(", ", revision)
        .push(
            ") ON CONFLICT (org_id) DO UPDATE SET \
             issuer = excluded.issuer, client_id = excluded.client_id, \
             client_secret = excluded.client_secret, \
             auto_join = excluded.auto_join, enabled = excluded.enabled, \
             updated_at = excluded.updated_at, sso_required = excluded.sso_required, \
             revision = excluded.revision",
        )
        .push([" RETURNING ", PROVIDER_COLUMNS]);
    let row = scope.fetch_optional(sql).await?.ok_or(StoreError::NoRows)?;
    let stored = StoredProvider::from_row(&row)?;
    if !before.is_some_and(|b| b.sso_required) && stored.sso_required {
        let mut lock_copy = scope.sql("UPDATE organizations SET allow_copy_out = ");
        lock_copy
            .bind(false)
            .push_bind(" WHERE id = ", scope.org()?)
            .push_bind(" AND allow_copy_out = ", true);
        scope.execute(lock_copy).await?;
    }
    let ended = if let Some(before) = before
        && before.issuer != stored.issuer
    {
        let ended = sessions::end_sessions_of_origin(scope, &before.org_id, &before.issuer).await?;
        oidc_former::keep_after_move(scope, held, before, keeping).await?;
        ended
    } else {
        SessionsEnded::none()
    };
    // After the sessions, as where the provider is removed: a token that one of them
    // was making is stored by now (ending them waited for it), and goes with the rest.
    crate::api_tokens::follow_provider(scope, before, Some(&stored)).await?;
    Ok(Written {
        provider: stored,
        dropped_pending,
        ended,
    })
}

/// Whether a provider lets somebody who has no place in the organization in, a member
/// of it from then on (`auto_join`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Newcomers {
    /// They join.
    Join,
    /// They are refused.
    Refused,
}

impl From<&StoredProvider> for Newcomers {
    fn from(provider: &StoredProvider) -> Self {
        if provider.auto_join {
            Self::Join
        } else {
            Self::Refused
        }
    }
}

/// Lets the account a sign-in through the provider ends in into the transaction's
/// organization: makes a new one, a member of it, or holds a known one, which joins
/// it if it has no place in it and `newcomers` lets it; and sets the user of
/// `session`. Says whether the account joined the organization, or why it is not let
/// in.
pub(crate) async fn admit(
    scope: &mut Scope<'_>,
    account: OidcAccount<'_>,
    newcomers: Newcomers,
    session: &mut Session,
) -> Result<Result<bool, OidcRefusal>, StoreError> {
    let auto_join = newcomers == Newcomers::Join;
    Ok(Ok(match account {
        OidcAccount::New {
            user,
            identity,
            provider_email,
        } => {
            if !auto_join {
                return Ok(Err(OidcRefusal::JoinClosed));
            }
            accounts::take_address_from_signup(scope, provider_email).await?;
            user.password_hash = String::new();
            user.email = String::new();
            accounts::insert_account(scope, user, InstanceRole::User).await?;
            insert_identity(scope, identity, &user.id, provider_email, user.created_at).await?;
            join_as_member(scope, &user.id).await?;
            session.user_id.clone_from(&user.id);
            true
        }
        OidcAccount::Known {
            user_id,
            password_hash,
            identity,
            proved_email,
        } => {
            // Held, the account waits for an unlinking under way, which ends the
            // sessions of the identity: one made after it is refused here.
            let Some(signed_into) =
                sessions::hold_unchanged_account_for_provider(scope, user_id, password_hash)
                    .await?
            else {
                return Ok(Err(OidcRefusal::AccountChanged));
            };
            if !is_identity_of(scope, identity, user_id).await? {
                return Ok(Err(OidcRefusal::AccountChanged));
            }
            user_id.clone_into(&mut session.user_id);
            // A guest of one of its workspaces is let in as they are, and joins
            // nothing: their board is what they have there.
            let has_place = has_place(scope, user_id).await?;
            if !has_place && !auto_join {
                return Ok(Err(OidcRefusal::JoinClosed));
            }
            // An account that waits to be deleted is signed in to for the way back
            // alone, through a door that is open to it: it joins nothing, and the
            // registration at its address stays.
            if signed_into == sessions::SignedInto::Waiting {
                return Ok(Ok(false));
            }
            // Let in: only now does a registration at the address go, its link held
            // before the account joins anything, as an acceptance holds it.
            if let Some(email) = proved_email {
                accounts::discard_signup_at(scope, email).await?;
            }
            !has_place && join_as_member(scope, user_id).await?
        }
    }))
}

/// What came of removing the provider of an organization.
#[derive(Debug, PartialEq, Eq)]
pub enum Removal {
    /// It is removed, and the sessions made through a provider of the organization
    /// under its issuer ended with it.
    Removed {
        /// The provider as it was.
        provider: StoredProvider,
        /// Sessions ended with the provider.
        ended: SessionsEnded,
    },
    /// The organization had none.
    NoneThere,
    /// It stays: the organization takes only sessions made through it.
    Enforced,
}

/// Records what this save changed from `before`, in the transaction that stores it:
/// the provider's settings, and the switch of enforcement, each when it changed. A
/// save that changes nothing records nothing.
pub(crate) async fn record_provider_change(
    scope: &mut Scope<'_>,
    actor_id: &str,
    before: Option<&StoredProvider>,
    stored: &StoredProvider,
) -> Result<(), StoreError> {
    let configured = before.is_none_or(|before| {
        (
            &before.issuer,
            &before.client_id,
            &before.sealed_client_secret,
            before.auto_join,
            before.enabled,
        ) != (
            &stored.issuer,
            &stored.client_id,
            &stored.sealed_client_secret,
            stored.auto_join,
            stored.enabled,
        )
    });
    let was_required = before.is_some_and(|before| before.sso_required);
    let enforcement = match (was_required, stored.sso_required) {
        (false, true) => Some(AuditAction::SsoRequired),
        (true, false) => Some(AuditAction::SsoReleased),
        _ => None,
    };
    let actions = configured
        .then_some(AuditAction::OidcProviderConfigured)
        .into_iter()
        .chain(enforcement);
    for action in actions {
        let entry = AuditEntry::new("", actor_id, action, &stored.org_id);
        audit::record_entry(scope, &entry).await?;
    }
    Ok(())
}

/// What a provider that is replaced must still be.
#[derive(Debug, Clone)]
enum AsRead {
    /// Anything: only for the tests, which set a provider without reading it first.
    #[cfg(any(test, feature = "test-util"))]
    Whatever,
    /// The provider of this identifier, last changed at this time; or none for `None`.
    Was(Option<(String, i64)>),
}

/// The account a sign-in through a provider ends in.
#[derive(Debug)]
pub enum OidcAccount<'a> {
    /// The account of the identity, with the password hash it was read with: a
    /// password set, the account deleted, or the identity unlinked meanwhile stops the
    /// sign-in.
    Known {
        /// The account.
        user_id: &'a str,
        /// Its password hash as it was read: empty for an account without one.
        password_hash: &'a str,
        /// Who the provider vouches for, which must still be the account's.
        identity: Identity<'a>,
        /// The address the provider vouches for at this sign-in, if it does: a
        /// registration by a link of the organization that waits at it is removed.
        /// None for a trial of pending settings, which removes nothing.
        proved_email: Option<&'a str>,
    },
    /// A new account for the identity, with the address the provider gave.
    New {
        /// The account to store; updated to what was stored.
        user: &'a mut User,
        /// Who the provider vouches for.
        identity: Identity<'a>,
        /// The address the provider vouches for: the caller has seen that it reads as
        /// one and is not called unverified. Kept with the identity, and a
        /// registration by a link of the organization that waits at it is removed.
        provider_email: &'a str,
    },
}

impl OidcAccount<'_> {
    /// Who the provider vouches for.
    #[must_use]
    pub fn identity(&self) -> Identity<'_> {
        match self {
            Self::Known { identity, .. } | Self::New { identity, .. } => *identity,
        }
    }
}

/// Why a sign-in through a provider stored nothing at its end.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OidcRefusal {
    /// The provider is switched off, removed, another one, or given another secret,
    /// since the sign-in began.
    ProviderGone,
    /// The organization lets no newcomer in through its provider.
    JoinClosed,
    /// The account was deleted, given a password, or its identity unlinked, while
    /// signing in.
    AccountChanged,
    /// A trial of the pending settings vouched for somebody who is not the
    /// administrator who started it (an identity of another account, or one that is
    /// nobody's where the settings do not move the organization to its issuer), or
    /// that administrator administers the organization no more.
    NotTheTrier,
    /// A trial of pending settings of another issuer vouched for an identity tied to
    /// no account, and the session it was started from does not show that the trier
    /// signed in through the provider just now: nothing is tied.
    NotFresh,
}

/// Whether the identity is still the account's.
async fn is_identity_of(
    scope: &mut Scope<'_>,
    identity: Identity<'_>,
    user_id: &str,
) -> Result<bool, StoreError> {
    let mut sql = scope.sql("SELECT user_id FROM user_identities WHERE issuer = ");
    sql.bind(identity.issuer)
        .push_bind(" AND subject = ", identity.subject)
        .push_bind(" AND user_id = ", Id(user_id));
    scope.exists(sql).await
}

/// Makes `user_id` a member of the scope's organization and default workspace unless
/// already in the organization; returns whether the user newly joined.
async fn join_as_member(scope: &mut Scope<'_>, user_id: &str) -> Result<bool, StoreError> {
    if !accounts::join_organization(scope, user_id, OrgRole::Member).await? {
        return Ok(false);
    }
    let which = Which::DefaultWorkspace;
    accounts::join_workspace(scope, which, user_id, WorkspaceRole::Member).await?;
    Ok(true)
}

/// Whether `user_id` is a member of the transaction's organization or of a live workspace in it.
pub(crate) async fn has_place(scope: &mut Scope<'_>, user_id: &str) -> Result<bool, StoreError> {
    if is_member(scope, user_id).await? {
        return Ok(true);
    }
    let mut sql = scope.sql(
        "SELECT 1 FROM workspace_members AS wm JOIN workspaces AS t ON t.id = wm.workspace_id \
         WHERE t.deleted_at IS NULL AND t.org_id = ",
    );
    sql.bind(scope.org()?)
        .push_bind(" AND wm.user_id = ", Id(user_id));
    scope.exists(sql).await
}

async fn is_member(scope: &mut Scope<'_>, user_id: &str) -> Result<bool, StoreError> {
    let mut sql = scope.sql("SELECT 1 FROM organization_members WHERE org_id = ");
    sql.bind(scope.org()?)
        .push_bind(" AND user_id = ", Id(user_id));
    scope.exists(sql).await
}

pub(crate) use crate::oidc_identities::insert_identity;

impl FromRow for StoredProvider {
    fn from_row(row: &Row) -> Result<Self, StoreError> {
        Ok(StoredProvider {
            id: row.id(0)?,
            org_id: row.id(1)?,
            issuer: row.text(2)?,
            client_id: row.text(3)?,
            sealed_client_secret: row.text(4)?,
            auto_join: row.bool(5)?,
            enabled: row.bool(6)?,
            created_at: row.int(7)?,
            updated_at: row.int(8)?,
            sso_required: row.bool(9)?,
            revision: row.int(10)?,
        })
    }
}
