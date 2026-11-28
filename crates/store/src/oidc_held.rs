//! Carrying an account over to the provider an organization moved to.
//!
//! While a former provider is kept (`oidc_former.rs`), a sign-in through the
//! organization's provider that ends in an identity tied to no account makes no
//! account at once: what the provider said is held, for the browser that signed in
//! and for a short while, until the person says whether they had an account. If they
//! had, they sign in through the former provider, and the held identity is tied to
//! the account that one vouches for, which has a place in the organization (a member
//! of it, or a guest of one of its workspaces), with a session made through the
//! provider ([`Store::finish_oidc_carry_over`]): both identities are shown, so no
//! address is compared. If not, the held sign-in is taken and ends as a
//! first sign-in does (`Store::finish_oidc_sign_in`). Either takes it, once. A sign-in
//! through the former provider ends in nothing else: never in a session of its own.

use fvoci_model::{AuditAction, LoginOrigin, Session};

use crate::Store;
use crate::audit::{self, AuditEntry};
use crate::db::{FromRow, Id, LockRank, Row, RowLock, Scope};
use crate::error::StoreError;
use crate::oidc::{self, Identity, hold_provider_of_organization, read_provider};
use crate::oidc_former::read_former;
use crate::sessions;

/// The columns of a held sign-in, in the order [`HeldSignIn::from_row`] reads them.
const HELD_COLUMNS: &str = "held_hash, org_id, provider_id, revision, issuer, subject, email, \
                            email_unverified, name, preferred_username, created_at, expires_at";

/// A sign-in through a provider that is held: who the provider vouches for, who is
/// nobody's yet, and what it said of them.
#[derive(Clone, PartialEq, Eq)]
pub struct HeldSignIn {
    /// The digest of the secret in the cookie of the browser that signed in, by which
    /// it is found.
    pub held_hash: String,
    /// The organization whose provider it came through; the handle's.
    pub org_id: String,
    /// The provider it came through.
    pub provider_id: String,
    /// The provider's revision when it vouched: the session this ends in proves it.
    pub revision: i64,
    /// The issuer, exactly as in the ID token.
    pub issuer: String,
    /// The subject, exactly as in the ID token.
    pub subject: String,
    /// The email address the provider gave, unchecked; empty for none.
    pub email: String,
    /// Whether the provider said it has not checked the address.
    pub email_unverified: bool,
    /// The person's name as the provider gave it; empty for none.
    pub name: String,
    /// The name the person goes by at the provider; empty for none.
    pub preferred_username: String,
    /// When it was held, in milliseconds since the Unix epoch.
    pub created_at: i64,
    /// When it can no longer be taken, in milliseconds since the Unix epoch.
    pub expires_at: i64,
}

impl std::fmt::Debug for HeldSignIn {
    /// Leaves out who the person is.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("HeldSignIn")
            .field("org_id", &self.org_id)
            .field("provider_id", &self.provider_id)
            .field("issuer", &self.issuer)
            .field("expires_at", &self.expires_at)
            .finish_non_exhaustive()
    }
}

/// The end of a sign-in through the former provider that carries an account over.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CarryOver<'a> {
    /// The provider the sign-in was started for: the organization's, as it was.
    pub provider_id: &'a str,
    /// The digest of the held sign-in it carries over.
    pub held_hash: &'a str,
    /// Who the former provider vouches for.
    pub former: Identity<'a>,
    /// Now, in milliseconds since the Unix epoch.
    pub now: i64,
}

/// An account that was carried over.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CarriedOver {
    /// The account the former provider vouched for, which the session is of.
    pub user_id: String,
    /// The issuer of the identity that is tied to it now: the provider's.
    pub issuer: String,
}

/// Why a sign-in through the former provider carried nothing over. Told apart for the
/// server's log only: whoever asks is answered the same for each.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CarryOverRefusal {
    /// The provider is switched off, removed or another one since the sign-in began.
    ProviderGone,
    /// No former provider of that issuer is kept any more.
    FormerGone,
    /// The held sign-in was taken already, ended, or is of another state of the
    /// provider.
    HeldGone,
    /// The former provider vouches for nobody who has an account in use.
    NoAccount,
    /// The account it vouches for has no place in the organization: it is no member
    /// of it, and no guest of one of its workspaces in use.
    NoPlace,
    /// The held identity was tied to another account meanwhile.
    IdentityTaken,
}

impl Store {
    /// Holds a sign-in through the provider of the handle's organization, if that is
    /// still the provider it came through, on, at the revision it vouched at, and a
    /// former provider is kept at `held.created_at`; and says whether it is held.
    /// `held.org_id` is the handle's. One that is not held goes on as a sign-in
    /// does, which reads the provider again where it ends.
    ///
    /// # Errors
    ///
    /// Returns [`StoreError::NoOrganization`] if the handle names none, and
    /// [`StoreError::Database`] if the statement fails.
    pub async fn hold_oidc_sign_in(&self, held: &HeldSignIn) -> Result<bool, StoreError> {
        self.with_write_conn(async |scope| {
            let mut sql = scope.sql([
                "INSERT INTO oidc_held_sign_ins (",
                HELD_COLUMNS,
                ") SELECT ",
            ]);
            sql.bind(&held.held_hash)
                .push(", p.org_id, p.id, p.revision")
                .push_bind(", ", &held.issuer)
                .push_bind(", ", &held.subject)
                .push_bind(", ", &held.email)
                .push_bind(", ", held.email_unverified)
                .push_bind(", ", &held.name)
                .push_bind(", ", &held.preferred_username)
                .push_bind(", ", held.created_at)
                .push_bind(", ", held.expires_at)
                .push_bind(
                    " FROM oidc_providers AS p WHERE p.id = ",
                    Id(&held.provider_id),
                )
                .push_bind(" AND p.org_id = ", scope.org()?)
                .push_bind(" AND p.enabled = ", true)
                .push_bind(" AND p.revision = ", held.revision)
                .push(
                    " AND EXISTS (SELECT 1 FROM oidc_former_providers AS former \
                     WHERE former.org_id = p.org_id",
                )
                .push_bind(" AND former.expires_at > ", held.created_at)
                .push(")");
            Ok(scope.execute(sql).await? > 0)
        })
        .await
    }

    /// The held sign-in of the handle's organization under `held_hash`, if it has not
    /// ended at `now`: read, and left for whoever takes it.
    ///
    /// # Errors
    ///
    /// Returns [`StoreError::NoOrganization`] if the handle names none, and
    /// [`StoreError::Database`] if the query fails.
    pub async fn oidc_held_sign_in(
        &self,
        held_hash: &str,
        now: i64,
    ) -> Result<Option<HeldSignIn>, StoreError> {
        self.with_conn(async |scope| {
            let mut sql = scope.sql([
                "SELECT ",
                HELD_COLUMNS,
                " FROM oidc_held_sign_ins WHERE held_hash = ",
            ]);
            sql.bind(held_hash)
                .push_bind(" AND org_id = ", scope.org()?)
                .push_bind(" AND expires_at > ", now);
            scope.first(sql).await
        })
        .await
    }

    /// Takes the held sign-in of the handle's organization under `held_hash`, so that
    /// it is taken once: it is removed, ended or not, and returned if it had not
    /// ended at `now`.
    ///
    /// # Errors
    ///
    /// Returns [`StoreError::NoOrganization`] if the handle names none, and
    /// [`StoreError::Database`] if the statement fails.
    pub async fn take_oidc_held_sign_in(
        &self,
        held_hash: &str,
        now: i64,
    ) -> Result<Option<HeldSignIn>, StoreError> {
        self.with_write_conn(async |scope| {
            Ok(take_held(scope, held_hash)
                .await?
                .filter(|held| held.expires_at > now))
        })
        .await
    }

    /// Ends a sign-in through the former provider of the handle's organization that
    /// was started to carry the held sign-in `carry.held_hash` over, in one
    /// transaction that holds the provider. The held sign-in is taken, whatever
    /// comes of it. Then, if the provider is still the one the sign-in was started
    /// for, on and at the revision the held identity was vouched at, a former
    /// provider of the issuer of `carry.former` is still kept, and `carry.former` is
    /// the identity of an account in use that has a place in the organization (a
    /// member, or a guest of one of its workspaces): the held identity is tied to that
    /// account, who joins nothing by it, with the address its provider gave,
    /// that is recorded as the account's doing, and `session` is stored for the
    /// account under `token_digest`, made through the provider. An identity that
    /// is the account's already is left as it is, and nothing is recorded of it.
    /// Nothing else is stored if any of it is refused: no identity, no session.
    ///
    /// # Errors
    ///
    /// Returns [`StoreError::NoOrganization`] if the handle names none, and
    /// [`StoreError::Database`] if a statement fails.
    pub async fn finish_oidc_carry_over(
        &self,
        carry: CarryOver<'_>,
        session: &mut Session,
        token_digest: &str,
    ) -> Result<Result<CarriedOver, CarryOverRefusal>, StoreError> {
        self.transaction(async |scope| {
            hold_provider_of_organization(scope).await?;
            let held = take_held(scope, carry.held_hash).await?;
            let Some(provider) = read_provider(scope)
                .await?
                .filter(|provider| provider.id == carry.provider_id && provider.enabled)
            else {
                return Ok(Err(CarryOverRefusal::ProviderGone));
            };
            let Some(held) = held.filter(|held| {
                held.expires_at > carry.now
                    && held.provider_id == provider.id
                    && held.revision == provider.revision
            }) else {
                return Ok(Err(CarryOverRefusal::HeldGone));
            };
            let kept = read_former(scope, carry.now).await?;
            if kept.is_none_or(|kept| kept.issuer != carry.former.issuer) {
                return Ok(Err(CarryOverRefusal::FormerGone));
            }
            let Some(user_id) = hold_account_of(scope, carry.former).await? else {
                return Ok(Err(CarryOverRefusal::NoAccount));
            };
            if !oidc::has_place(scope, &user_id).await? {
                return Ok(Err(CarryOverRefusal::NoPlace));
            }
            let identity = Identity {
                issuer: &held.issuer,
                subject: &held.subject,
            };
            let tying = Tying {
                user_id: &user_id,
                email: &held.email,
                now: carry.now,
            };
            match tie_unless_taken(scope, identity, tying).await? {
                Tied::Anothers => return Ok(Err(CarryOverRefusal::IdentityTaken)),
                Tied::Already => {}
                Tied::Newly => {
                    let entry = AuditEntry::new(
                        "",
                        &user_id,
                        AuditAction::OidcIdentityCarriedOver,
                        &provider.org_id,
                    )
                    .with_subject(&user_id)
                    .with_detail(&held.issuer)
                    .with_identity(&held.email, &held.subject);
                    audit::record_entry(scope, &entry).await?;
                }
            }
            session.user_id.clone_from(&user_id);
            session.login = Some(LoginOrigin {
                provider_id: provider.id,
                org_id: provider.org_id,
                issuer: provider.issuer.clone(),
                subject: Some(held.subject),
                revision: Some(provider.revision),
            });
            sessions::insert_session(scope, session, token_digest).await?;
            Ok(Ok(CarriedOver {
                user_id,
                issuer: provider.issuer,
            }))
        })
        .await
    }
}

/// Removes the held sign-in of the transaction's organization under `held_hash`, and
/// returns it as it was, ended or not.
async fn take_held(
    scope: &mut Scope<'_>,
    held_hash: &str,
) -> Result<Option<HeldSignIn>, StoreError> {
    let mut sql = scope.sql("DELETE FROM oidc_held_sign_ins WHERE held_hash = ");
    sql.bind(held_hash)
        .push_bind(" AND org_id = ", scope.org()?)
        .push([" RETURNING ", HELD_COLUMNS]);
    scope.first(sql).await
}

/// The account in use that `identity` is tied to, held until the transaction ends: a
/// removal of the identity under way (`Store::unlink_identity`) is waited for, and
/// seen.
async fn hold_account_of(
    scope: &mut Scope<'_>,
    identity: Identity<'_>,
) -> Result<Option<String>, StoreError> {
    let mut sql = scope.sql(
        "SELECT u.id FROM users AS u JOIN user_identities AS i ON i.user_id = u.id \
         WHERE u.deleted_at IS NULL AND i.issuer = ",
    );
    sql.bind(identity.issuer)
        .push_bind(" AND i.subject = ", identity.subject);
    let Some(user_id) = scope
        .first_held::<String>(LockRank::Users, sql, RowLock::Share.of("u"))
        .await?
    else {
        return Ok(None);
    };
    // Read again now that the account is held: unlinked meanwhile, it is nobody's.
    let mut sql = scope.sql("SELECT 1 FROM user_identities WHERE issuer = ");
    sql.bind(identity.issuer)
        .push_bind(" AND subject = ", identity.subject)
        .push_bind(" AND user_id = ", Id(&user_id));
    Ok(scope.fetch_optional(sql).await?.map(|_| user_id))
}

/// The account an identity is to be tied to, the address its provider gave (empty
/// for none), and the time it is tied at.
#[derive(Debug, Clone, Copy)]
pub(crate) struct Tying<'a> {
    pub(crate) user_id: &'a str,
    pub(crate) email: &'a str,
    pub(crate) now: i64,
}

/// What came of tying an identity to an account.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Tied {
    /// It is the account's now, by this.
    Newly,
    /// It was the account's already, and is left as it was.
    Already,
    /// It is another account's, in use or deleted: nothing is tied.
    Anothers,
}

/// Ties `identity` to an account unless it is somebody's already, in one statement:
/// of two at once the second waits for the first and ties nothing.
pub(crate) async fn tie_unless_taken(
    scope: &mut Scope<'_>,
    identity: Identity<'_>,
    tying: Tying<'_>,
) -> Result<Tied, StoreError> {
    // Later than any revert link of the account, also one made in the same millisecond:
    // reverting a change made before this tie always removes it.
    let created = crate::email_revert::after_latest(scope, tying.user_id, tying.now).await?;
    let mut sql = scope
        .sql("INSERT INTO user_identities (issuer, subject, user_id, email, created_at) VALUES (");
    sql.bind(identity.issuer)
        .push_bind(", ", identity.subject)
        .push_bind(", ", Id(tying.user_id))
        .push_bind(", ", tying.email)
        .push_bind(", ", created)
        .push(") ON CONFLICT (issuer, subject) DO NOTHING");
    if scope.execute(sql).await? > 0 {
        return Ok(Tied::Newly);
    }
    let mut sql = scope.sql("SELECT user_id FROM user_identities WHERE issuer = ");
    sql.bind(identity.issuer)
        .push_bind(" AND subject = ", identity.subject);
    match scope.fetch_optional(sql).await? {
        Some(row) if row.id(0)? == tying.user_id => Ok(Tied::Already),
        Some(_) => Ok(Tied::Anothers),
        None => Err(StoreError::NoRows),
    }
}

impl FromRow for HeldSignIn {
    fn from_row(row: &Row) -> Result<Self, StoreError> {
        Ok(HeldSignIn {
            held_hash: row.text(0)?,
            org_id: row.id(1)?,
            provider_id: row.id(2)?,
            revision: row.int(3)?,
            issuer: row.text(4)?,
            subject: row.text(5)?,
            email: row.text(6)?,
            email_unverified: row.bool(7)?,
            name: row.text(8)?,
            preferred_username: row.text(9)?,
            created_at: row.int(10)?,
            expires_at: row.int(11)?,
        })
    }
}
