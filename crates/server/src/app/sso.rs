//! An organization that takes no password: the one check every way into what is an
//! organization's passes, and what switches it on and off.
//!
//! The check is [`require_session_opens`]. A workspace is entered through it
//! (`App::enter_workspace`), an organization (`App::enter_org_as_admin`), the
//! instance's own organization for what changes who administers the instance
//! (`App::require_session_opens_instance`), and the organization the link of an
//! invitation leads into (`App::require_session_opens_organization`). The boards a
//! user lists across organizations leave out those of an organization the session
//! does not open (`Store::container_memberships_of_user`). It is asked of
//! every request: the sessions stay, and one that does not open an organization still
//! opens the others.
//!
//! The same check closes a user's own space, once its account has a password, to a
//! session made through a provider that has not confirmed that password
//! (`App::confirm_session_password`): what a workspace takes is one answer of the
//! store (`SessionTaken`), whichever of the two it is, and what is read of a session
//! is one value (`Making`).

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use fvoci_model::Session;
use fvoci_store::{Making, SessionTaken, StoredProvider};

use super::App;
use crate::auth::api_token::Delegation;
use crate::error::{AppError, Reason};

/// What was done to an organization's taking no password.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SsoChange {
    /// An administrator switched it on.
    Required,
    /// An administrator switched it off.
    Released,
    /// Who runs the server switched it off, on the administration socket.
    ReleasedByOperator,
}

impl SsoChange {
    fn as_str(self) -> &'static str {
        match self {
            Self::Required => "sso_required",
            Self::Released => "sso_released",
            Self::ReleasedByOperator => "sso_released_by_operator",
        }
    }
}

/// Writes a line of the log when an organization began or stopped taking no password.
/// `user_id` is the administrator who did it, `None` for who runs the server. The
/// record that stays is the organization's, written by the store in the provider's
/// transaction; this line is for whoever reads the server's log.
fn record(provider: &StoredProvider, change: SsoChange, user_id: Option<&str>) {
    tracing::info!(
        target: "audit",
        org_id = %provider.org_id,
        provider_id = %provider.id,
        issuer = %provider.issuer,
        user_id = user_id.unwrap_or_default(),
        action = change.as_str(),
        "an organization's taking no password was changed"
    );
}

/// Records that the administrator `user_id` switched an organization's taking no
/// password on or off, if the provider as stored differs in that from the one before.
/// What is recorded is the provider it was switched on for, and the one it was
/// switched off for: the one before, where the same change set another.
pub(crate) fn record_if_changed(
    before: Option<&StoredProvider>,
    stored: &StoredProvider,
    user_id: &str,
) {
    match (
        before.filter(|before| before.sso_required),
        stored.sso_required,
    ) {
        (None, true) => record(stored, SsoChange::Required, Some(user_id)),
        (Some(before), false) => record(before, SsoChange::Released, Some(user_id)),
        _ => {}
    }
}

/// The refusal of a change to the provider that an organization that takes no
/// password does not let be made.
pub(crate) fn enforced() -> AppError {
    AppError::conflict(
        "the organization takes no password: its provider is not switched off, removed \
         or set to another issuer or client until ssoRequired is off",
    )
    .with_reason(Reason::SsoEnforced)
}

/// Checks a change of an organization's provider, from `existing` (as it was read) to
/// `next`, against what keeps the organization from locking itself out.
///
/// - Taking no password is switched on by a session that proves the provider as it
///   is stored ([`proves`]), with a change that keeps what signing in takes
///   ([`StoredProvider::signs_in_like`]): who asks has shown that it lets them in. A
///   provider of another issuer, client or secret has not let anybody in yet.
/// - While it is on and stays on, the provider is not switched off. Switching it off
///   (`ssoRequired` false) comes first, or with the change.
///
/// # Errors
///
/// Returns 409 with the reason `sso_unproven` for the first and `sso_enforced` for
/// the second.
pub(crate) fn check_change(
    session: &Session,
    existing: Option<&StoredProvider>,
    next: &StoredProvider,
) -> Result<(), AppError> {
    if !next.sso_required {
        return Ok(());
    }
    // Another issuer, client or secret of a provider that is held went aside as pending
    // settings before this (`App::put_oidc_provider`): what is left to refuse is
    // switching it off.
    if existing.is_some_and(|existing| existing.sso_required) {
        return if next.enabled {
            Ok(())
        } else {
            Err(enforced().with_field("enabled"))
        };
    }
    let proven =
        existing.is_some_and(|existing| existing.signs_in_like(next) && proves(session, existing));
    if proven {
        Ok(())
    } else {
        Err(AppError::conflict(
            "ssoRequired is switched on from a session made through this provider, \
             switched on: sign in through it first",
        )
        .with_reason(Reason::SsoUnproven)
        .with_field("ssoRequired"))
    }
}

/// Whether `session` shows that signing in through the provider, as it is stored,
/// lets people in: it was made through this very provider at its revision, and the
/// provider is on. A session made before a change of the issuer, the client, the
/// secret or the switch proves nothing of the provider after it.
pub(crate) fn proves(session: &Session, provider: &StoredProvider) -> bool {
    provider.enabled
        && session.login.as_ref().is_some_and(|login| {
            login.provider_id == provider.id && login.revision == Some(provider.revision)
        })
}

/// Who asks for what is an organization's: the user, and how their session was made
/// and what it confirmed since ([`Making`]), all that the rule of which session opens
/// what reads. A request has its session; a websocket connection keeps these two of
/// the session its handshake came with.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Asker<'a> {
    /// The user of the session.
    pub user_id: &'a str,
    /// How the session was made, and what it confirmed since: of a token, how the
    /// session that made it was.
    pub making: Making<&'a str>,
    /// The personal API token the request came with; `None` for a session.
    pub delegation: Option<&'a Delegation>,
}

/// How a session was made and what it had confirmed at one moment ([`Making`]), to be
/// asked with after that moment: what is read of a connection's keeping
/// ([`SessionKept::made`]) under the lock of the connections and asked with outside
/// it.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct SessionMade(Making);

/// What a connection keeps of its session for as long as it lasts: how the session
/// was made, as the handshake found it, and whether it has given the account's
/// password, then or since. A clone is the same keeping: the websocket connection and
/// its place among the rooms share one, so that a session that confirms its password
/// (`App::confirm_session_password`) has confirmed it on every connection it has from
/// that moment, and none of them comes back to be told so. Nothing takes it back: what
/// a session confirmed goes only with the session, and its connections with it.
#[derive(Debug, Clone)]
pub struct SessionKept {
    /// As the handshake found the session.
    made: Making,
    /// The session gave the account's password after the handshake.
    confirmed_since: Arc<AtomicBool>,
}

impl SessionKept {
    /// What is kept of `session`, as it is now.
    #[must_use]
    pub fn of(session: &Session) -> Self {
        Self {
            made: Making::from(session).into_owned(),
            confirmed_since: Arc::default(),
        }
    }

    /// As of a session made with a password, through no provider: what a connection
    /// without a session is given, which opens nothing that asks for one.
    #[must_use]
    pub fn with_password() -> Self {
        Self {
            made: Making::with_password(),
            confirmed_since: Arc::default(),
        }
    }

    /// The value the rule of which session opens what is asked with, as it is now.
    #[must_use]
    pub fn making(&self) -> Making<&str> {
        let made = self.made.as_deref();
        if self.confirmed_since.load(Ordering::Acquire) {
            made.having_confirmed_password()
        } else {
            made
        }
    }

    /// The same, to be asked with later.
    #[must_use]
    pub fn made(&self) -> SessionMade {
        SessionMade(self.making().into_owned())
    }

    /// The session gave the account's password, and the store recorded it
    /// (`Store::confirm_session_password`): every holder of this keeping asks as a
    /// session that confirmed it from here on.
    pub fn gave_password(&self) {
        self.confirmed_since.store(true, Ordering::Release);
    }
}

impl SessionMade {
    /// What is kept of `session`, as it is now.
    #[must_use]
    pub fn of(session: &Session) -> Self {
        Self(Making::from(session).into_owned())
    }

    /// As of a session made with a password, through no provider: what a connection
    /// without a session is given, which opens nothing that asks for one.
    #[must_use]
    pub fn with_password() -> Self {
        Self(Making::with_password())
    }

    /// The value the rule of which session opens what is asked with.
    #[must_use]
    pub fn making(&self) -> Making<&str> {
        self.0.as_deref()
    }

    /// Whether an organization that takes only the sessions made through `provider_id`
    /// is closed to the session: the rule of the store (`SessionTaken::opened_by`), for
    /// a connection that follows the organization's workspaces as it begins to take no
    /// other session.
    #[must_use]
    pub fn is_made_otherwise_than(&self, provider_id: &str) -> bool {
        !SessionTaken::Through(provider_id.to_owned()).opened_by(self.making())
    }

    /// Whether the session has shown the account's password, by being made with it or
    /// by confirming it since: the rule of the store, for a connection that follows
    /// the account's own space as the space begins to take no other session.
    #[must_use]
    pub fn has_shown_password(&self) -> bool {
        SessionTaken::WithPassword.opened_by(self.making())
    }
}

impl<'a> Asker<'a> {
    /// The user with a session made with a password, through no provider.
    #[must_use]
    pub fn with_password(user_id: &'a str) -> Self {
        Self {
            user_id,
            making: Making::with_password(),
            delegation: None,
        }
    }
}

impl<'a> From<&'a Session> for Asker<'a> {
    fn from(session: &'a Session) -> Self {
        Self {
            user_id: &session.user_id,
            making: session.into(),
            delegation: None,
        }
    }
}

/// Refuses a session that does not open what is the organization `org_id`'s:
/// `takes` is which sessions it takes, as the store answered it for this user.
///
/// # Errors
///
/// Returns 401 with the reason `sso_required`, naming the organization, for one that
/// takes only sessions made through its provider, and 401 with the reason
/// `password_sign_in_required` for the user's own space, which takes only sessions
/// that have shown the account's password once it has one. It is for
/// who has a place in the organization, or holds a good link of an invitation into
/// it, only: the caller asks after it knows that, so that the answer tells nobody
/// else that the organization is there.
pub(crate) fn require_session_opens(
    asker: Asker<'_>,
    org_id: &str,
    takes: &SessionTaken,
) -> Result<(), AppError> {
    if takes.opened_by(asker.making) {
        return Ok(());
    }
    tracing::debug!(
        org_id,
        user_id = asker.user_id,
        ?takes,
        "a session of another making than the organization takes was refused"
    );
    Err(match takes {
        SessionTaken::WithPassword => AppError::password_sign_in_required(),
        SessionTaken::Any | SessionTaken::Through(_) => AppError::sso_required(org_id),
    })
}

impl App {
    /// Releases the organization `org_id` from taking no password, for who runs the
    /// server: the way back in when nobody gets in through the provider. Its provider
    /// stays as it is otherwise. Releasing one that takes a password already changes
    /// nothing.
    ///
    /// # Errors
    ///
    /// Returns 404 for an organization that is not there, not in use, or the
    /// system's, and the store's error.
    pub async fn release_sso_as_operator(&self, org_id: &str) -> Result<(), AppError> {
        let not_found = || AppError::not_found("organization");
        if !fvoci_model::is_id(org_id)
            || self
                .store()
                .login_organization(Some(org_id))
                .await?
                .is_none()
        {
            return Err(not_found());
        }
        let org = self.enter_org_for_sign_in(org_id).ok_or_else(not_found)?;
        if let Some(provider) = org.store().release_sso_by_operator().await? {
            record(&provider, SsoChange::ReleasedByOperator, None);
        }
        Ok(())
    }

    /// Stops what the websocket connections that the handle's organization shuts out as
    /// it takes only sessions made through `provider` follow of its workspaces, and
    /// tells them: what a connection follows of a workspace was asked when it
    /// subscribed. For after that is stored; also when it was on already, which ends
    /// again what a failure may have left.
    ///
    /// If the organization's workspaces cannot be read, every workspace followed with
    /// a session made otherwise is stopped, whose it may be: the pages follow them
    /// again, and are asked then.
    pub(crate) async fn end_subscriptions_shut_out(&self, provider: &StoredProvider) {
        match self.store().get_workspaces().await {
            Ok(workspaces) => {
                let ids: Vec<String> = workspaces
                    .into_iter()
                    .map(|workspace| workspace.id)
                    .collect();
                self.ws().end_subscriptions_shut_out(&ids, &provider.id);
            }
            Err(error) => {
                tracing::warn!(%error, org_id = %provider.org_id, "unable to read the workspaces of an organization that takes no password");
                self.ws().end_subscriptions_made_otherwise(&provider.id);
            }
        }
    }

    /// Refuses `asker` if its session does not open what is the organization this handle
    /// stands in, whether the user has a place in it yet or not: for one who holds the
    /// link of an invitation into it, which names the organization to them. The handle
    /// is the link's organization's (on PostgreSQL another one's would answer for that
    /// one). Asked before the acceptance is written, not with it: an organization that
    /// begins to take no password in between has the membership written, which opens
    /// nothing to that session from the next request on.
    ///
    /// # Errors
    ///
    /// Returns 401 with the reason `sso_required`; the store's error if the
    /// organization's provider cannot be read; and a server error for a handle that
    /// stands in no organization.
    pub(crate) async fn require_session_opens_organization(
        &self,
        asker: Asker<'_>,
    ) -> Result<(), AppError> {
        let org_id = self.org_of_handle()?;
        let required = self
            .store()
            .oidc_provider()
            .await?
            .filter(|provider| provider.sso_required)
            .map(|provider| provider.id);
        require_session_opens(asker, &org_id, &SessionTaken::of_organization(required))
    }

    /// Refuses a session that does not open the instance's own organization (the
    /// oldest one in use, `Store::instance_organization`), whose administrators follow
    /// who administers the instance (`setUserAdmin`). `Ok` while there is no
    /// organization, and for a user who has no place in it, who finds nothing of it
    /// anyway.
    ///
    /// # Errors
    ///
    /// Returns 401 with the reason `sso_required`, and the store's error if the
    /// organization or its provider cannot be read, which is no "yes".
    pub(crate) async fn require_session_opens_instance(
        &self,
        session: &Session,
    ) -> Result<(), AppError> {
        let Some(org_id) = self.store().instance_organization().await? else {
            return Ok(());
        };
        let required = self
            .store()
            .required_provider_for(&session.user_id, &org_id)
            .await?;
        require_session_opens(
            session.into(),
            &org_id,
            &SessionTaken::of_organization(required),
        )
    }

    /// The organization this handle stands in. A handle of no organization, or of all,
    /// is an error: nothing is opened for want of an organization to ask.
    fn org_of_handle(&self) -> Result<String, AppError> {
        self.store()
            .tenant()
            .org_id()
            .ok_or_else(|| AppError::internal("the handle stands in no organization"))
    }
}
