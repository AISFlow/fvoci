//! Signing in through an organization's `OpenID Connect` provider: the provider as its
//! administrators set it, what a login page shows, and a sign-in from its start to
//! the session.
//!
//! While the organization keeps the provider it moved away from, somebody the provider
//! vouches for who has no account is asked whether they had one before any is made
//! (`oidc_carry.rs`). The account of a first sign-in is made in `oidc_account.rs`.
//!
//! A person is known by the issuer of the provider and the subject it gives them. An
//! account is never found by its email address: a person who signs in for the first
//! time gets an account of their own, and if their address is another account's the
//! sign-in is refused rather than joined to it.

use std::net::IpAddr;
use std::time::Instant;

use fvoci_model::{LoginOptions, Session, is_email_valid, new_id};
use fvoci_store::{
    Applied, FlowPurpose, Identity, LoginFlow, OidcAccount, OidcRefusal, PendingProvider,
    StoreError, StoredProvider, Tried,
};

use super::App;
use super::oidc_account::{FoundAccount, Newcomer, proved_address};
use super::oidc_purpose::Purpose as _;
use super::oidc_settings::secret_context;
use crate::auth::limiter::RetryAfter;
use crate::auth::now_millis;
use crate::auth::oidc::provider::{
    self, Client, Discovery, Expected, Grant, IdClaims, ProviderError,
};
use crate::auth::secret::{new_secret, token_digest};
use crate::error::{AppError, Reason};

/// The path, under the server's address, below which a provider sends a browser back:
/// each organization has an address of its own there, `…/callback/{orgId}`.
pub const CALLBACK_PATH: &str = "/api/v1/auth/oidc/callback";

/// How long a sign-in may take from its start to its return, in milliseconds.
pub const LOGIN_FLOW_LIFETIME_MILLIS: i64 = 10 * 60 * 1000;

/// The scopes a sign-in asks for: the ID token, with the email address and the name.
const SCOPES: &str = "openid email profile";

/// Why a sign-in through a provider did not end in a session: what the login page is
/// told, by the name of the problem.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LoginFailure {
    /// The reason, one of the `Oidc…` ones.
    Reason(Reason),
}

impl LoginFailure {
    /// The name the login page gets in its `oidc_error` parameter.
    pub fn name(self) -> String {
        match self {
            Self::Reason(reason) => reason.name(),
        }
    }
}

impl From<Reason> for LoginFailure {
    fn from(reason: Reason) -> Self {
        Self::Reason(reason)
    }
}

/// What came of a sign-in that came back from a provider, and where the browser goes
/// to be told.
#[derive(Debug)]
pub struct Returned {
    /// The organization of the sign-in, where there is one.
    pub org_id: Option<String>,
    /// The page that tells a failure.
    pub back_to: BackTo,
    /// How it ended.
    pub result: Result<FinishedLogin, LoginFailure>,
}

/// The page a sign-in that came back goes on to with a failure.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BackTo {
    /// The login page of the organization.
    LoginPage,
    /// The organization's page: a trial of its pending settings was started there by
    /// an administrator who is signed in.
    Organization,
}

/// A sign-in that was started: where the browser goes, and the secret its cookie keeps
/// so that only this browser can finish it.
#[derive(Clone)]
pub struct StartedLogin {
    /// The provider's address to send the browser to.
    pub authorization_url: String,
    /// The secret for the browser's cookie.
    pub browser_secret: String,
}

impl std::fmt::Debug for StartedLogin {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("StartedLogin").finish_non_exhaustive()
    }
}

/// What a browser brings back from the provider.
#[derive(Clone, Default)]
pub struct Callback {
    /// The `state` parameter.
    pub state: String,
    /// The `code` parameter.
    pub code: String,
    /// The `error` parameter: the provider did not let the person sign in.
    pub error: Option<String>,
    /// The secret of the browser's cookie.
    pub browser_secret: String,
    /// The organization the address it came back to names: each organization has an
    /// address of its own (`…/callback/{orgId}`).
    pub org_id: String,
    /// The `iss` parameter (RFC 9207): the issuer that answered, if it says.
    pub issuer: Option<String>,
    /// The token of the session cookie the browser came back with, if it has one: a
    /// sign-in that links ends only for the session it was started from.
    pub session_token: Option<String>,
    /// The secret of the cookie of the sign-in held for this browser, if it has one:
    /// a sign-in that carries over ends only for the browser it is held for.
    pub held_secret: Option<String>,
    /// The request's `Accept-Language`: what an account that comes into the
    /// organization by the sign-in has its welcome board written by, where its
    /// settings name no language.
    pub accept_language: Option<String>,
}

impl std::fmt::Debug for Callback {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Callback")
            .field("error", &self.error)
            .finish_non_exhaustive()
    }
}

/// How a sign-in through a provider ended where it is stored.
#[derive(Debug)]
pub(super) struct Ended {
    /// What a trial of the pending settings did; `None` for any other sign-in.
    pub(super) applied: Option<Applied>,
    /// Whether the account came into the organization by it, as a member.
    pub(super) joined: bool,
}

/// How a sign-in through a provider ended well.
#[derive(Clone)]
pub enum FinishedLogin {
    /// In a session.
    SignedIn {
        /// The token of the new session.
        token: String,
    },
    /// In an identity tied to the account of the session it was started from: no
    /// session is made, and the one there goes on.
    Linked,
    /// In a session made through the pending settings, which are the provider's now.
    Applied {
        /// The token of the new session.
        token: String,
    },
    /// In no session yet: the provider vouches for somebody without an account while
    /// the organization keeps its former provider, and what it said is held until
    /// they say whether they had an account (`oidc_carry.rs`).
    Held {
        /// The secret for the browser's cookie of the held sign-in.
        secret: String,
    },
}

impl std::fmt::Debug for FinishedLogin {
    /// Leaves out the token.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::SignedIn { .. } => "SignedIn",
            Self::Linked => "Linked",
            Self::Applied { .. } => "Applied",
            Self::Held { .. } => "Held",
        })
    }
}

impl App {
    /// The address the provider of the organization `org_id` sends a browser back to:
    /// from the server's own address (`server_url`), never from what a request says its
    /// host is. Each organization has its own, so that what one organization's provider
    /// sends back is never taken for another's (a mix-up of providers); it is known
    /// before the provider is set, and stays when the provider is changed.
    pub fn oidc_redirect_uri(&self, org_id: &str) -> String {
        self.settings()
            .server_url()
            .at(&format!("{CALLBACK_PATH}/{org_id}"))
    }

    /// What the login page shows: the organization `org_id`, or without one the only
    /// organization of a server that holds one, and whether a registration is taken
    /// without an invitation. Nothing of an organization is told to whoever does not
    /// name it, but on a server that holds only it.
    ///
    /// # Errors
    ///
    /// Returns the store's error.
    pub async fn login_options(&self, org_id: Option<&str>) -> Result<LoginOptions, AppError> {
        let organization = match org_id {
            Some(org_id) if fvoci_model::is_id(org_id) => {
                self.store().login_organization(Some(org_id)).await?
            }
            Some(_) => None,
            None if self.settings().takes_several_organizations() => None,
            None => self.store().login_organization(None).await?,
        };
        let has_first_user = self.store().is_bootstrapped().await?;
        Ok(LoginOptions {
            organization,
            registration_open: self.settings().takes_several_organizations() || !has_first_user,
            organization_required: !has_first_user,
            mail_enabled: self.config().mail_enabled(),
            email_verification: self.verifies_email(),
        })
    }

    /// Starts a sign-in through the provider of the organization `org_id`: stores what
    /// the browser must bring back, and says where to send the browser.
    ///
    /// # Errors
    ///
    /// Returns 409 with [`Reason::OidcUnavailable`] if the organization has no provider
    /// that is on or it cannot be reached, and 429 for too many sign-ins from `address`.
    pub async fn start_oidc_login(
        &self,
        org_id: &str,
        address: Option<IpAddr>,
    ) -> Result<StartedLogin, AppError> {
        self.start_oidc_flow(org_id, address, FlowPurpose::SignIn)
            .await
    }

    /// Starts a sign-in as [`Self::start_oidc_login`] does, for `purpose`: to sign in,
    /// to link (`App::start_oidc_link`), to try the pending settings
    /// (`App::start_oidc_trial`), or to carry a held sign-in over
    /// (`App::start_oidc_carry_over`). A trial's `pending_at` is the time of the
    /// settings pending when it starts, whatever it was asked with.
    pub(super) async fn start_oidc_flow(
        &self,
        org_id: &str,
        address: Option<IpAddr>,
        mut purpose: FlowPurpose,
    ) -> Result<StartedLogin, AppError> {
        // Counted against the address as a login is, and let go at once: the requests
        // to the provider below are no password work, and must not keep the address's
        // other logins waiting.
        drop(
            self.login_limiter()
                .admit_registration(address, Instant::now())
                .map_err(|RetryAfter(wait)| AppError::rate_limited(wait))?,
        );
        // What is in the way is the provider's (409); the store's failures are the
        // server's own (500).
        let unavailable = || {
            AppError::conflict("no sign-in through the organization's provider can start now")
                .with_reason(Reason::OidcUnavailable)
        };
        if !self
            .is_organization_in_use(org_id)
            .await
            .map_err(|error| AppError::from(error).context("reading the organization"))?
        {
            return Err(purpose.nothing_to_start());
        }
        let org = self
            .enter_org_for_sign_in(org_id)
            .ok_or_else(|| purpose.nothing_to_start())?;
        let stored = org
            .provider_to_start(&mut purpose)
            .await
            .map_err(|error| AppError::from(error).context("reading the provider"))?
            .ok_or_else(|| purpose.nothing_to_start())?;
        let discovery = provider::discover(self.oidc_fetcher(), &stored.issuer)
            .await
            .map_err(|error| {
                provider_failure(&error, Reason::OidcUnavailable);
                unavailable()
            })?;

        let (state, nonce, verifier, browser_secret) =
            (new_secret(), new_secret(), new_secret(), new_secret());
        // The address first: a provider whose address is none starts nothing.
        let mut url = reqwest::Url::parse(&discovery.authorization_endpoint).map_err(|_| {
            tracing::warn!(
                org_id,
                "the authorization endpoint of a provider is no address"
            );
            unavailable()
        })?;
        url.query_pairs_mut()
            .append_pair("response_type", "code")
            .append_pair("client_id", &stored.client_id)
            .append_pair("redirect_uri", &self.oidc_redirect_uri(org_id))
            .append_pair("scope", SCOPES)
            .append_pair("state", &state)
            .append_pair("nonce", &nonce)
            .append_pair("code_challenge", &pkce_challenge(&verifier))
            .append_pair("code_challenge_method", "S256");

        let now = now_millis();
        if let Err(error) = self.store().delete_ended_oidc_login_flows(now).await {
            tracing::warn!(%error, "unable to remove the sign-ins that ended");
        }
        org.store()
            .create_oidc_login_flow(&LoginFlow {
                state_hash: token_digest(&state),
                browser_hash: token_digest(&browser_secret),
                nonce: nonce.clone(),
                pkce_verifier: verifier.clone(),
                org_id: org_id.to_owned(),
                provider_id: stored.id.clone(),
                created_at: now,
                expires_at: now.saturating_add(LOGIN_FLOW_LIFETIME_MILLIS),
                purpose: purpose.clone(),
            })
            .await
            .map_err(|error| {
                // What it was to go through went a moment ago: the pending settings,
                // the held sign-in, or the provider itself.
                if error.is_not_found() {
                    purpose.nothing_to_start()
                } else {
                    AppError::from(error).context("storing a sign-in")
                }
            })?;
        Ok(StartedLogin {
            authorization_url: url.into(),
            browser_secret,
        })
    }

    /// Finishes a sign-in that comes back from a provider: takes the sign-in it is,
    /// once, checks that this browser started it, exchanges the code, checks the ID
    /// token, finds or makes the account, and opens a session made through the
    /// provider. Returns the organization of the sign-in with the result: the login
    /// page of a failure is that organization's. Not counted against the address: only
    /// a sign-in that was started comes back, and the start is counted.
    pub async fn finish_oidc_login(&self, callback: &Callback, user_agent: &str) -> Returned {
        let flow = match self.take_flow(callback).await {
            Ok(flow) => flow,
            Err((org_id, failure)) => {
                return Returned {
                    org_id,
                    back_to: BackTo::LoginPage,
                    result: Err(failure),
                };
            }
        };
        let result = self.finish_flow(&flow, callback, user_agent).await;
        if let Err(failure) = &result {
            tracing::info!(org_id = %flow.org_id, failure = %failure.name(), "a sign-in through a provider failed");
            if let Some(held_hash) = flow.purpose.held_hash() {
                self.drop_held(&flow.org_id, held_hash).await;
            }
        }
        Returned {
            back_to: flow.purpose.back_to(),
            org_id: Some(flow.org_id),
            result,
        }
    }

    /// The sign-in a callback is for, taken so that it is not taken again, if this
    /// browser started it and it has not ended. Another browser takes nothing: one who
    /// learned the `state` cannot end the sign-in of the browser that started it. A
    /// refusal names the organization of the sign-in, where there is one, for the login
    /// page to go back to.
    async fn take_flow(
        &self,
        callback: &Callback,
    ) -> Result<LoginFlow, (Option<String>, LoginFailure)> {
        let invalid = LoginFailure::Reason(Reason::OidcStateInvalid);
        if callback.state.is_empty() || callback.browser_secret.is_empty() {
            return Err((None, invalid));
        }
        let state_hash = token_digest(&callback.state);
        let taken = self
            .store()
            .take_oidc_login_flow(&state_hash, &token_digest(&callback.browser_secret))
            .await
            .map_err(|error| (None, logged(&error, "taking a sign-in")))?;
        if let Some(flow) = taken {
            if flow.expires_at <= now_millis() {
                return Err((Some(flow.org_id), invalid));
            }
            return Ok(flow);
        }
        let org_id = self
            .store()
            .oidc_login_flow_org(&state_hash)
            .await
            .map_err(|error| (None, logged(&error, "reading a sign-in")))?;
        if let Some(org_id) = &org_id {
            tracing::info!(org_id = %org_id, "a sign-in came back to another browser than the one that started it");
        }
        Err((org_id, invalid))
    }

    async fn finish_flow(
        &self,
        flow: &LoginFlow,
        callback: &Callback,
        user_agent: &str,
    ) -> Result<FinishedLogin, LoginFailure> {
        came_back_with_a_code(flow, callback)?;
        let unavailable = LoginFailure::Reason(Reason::OidcUnavailable);
        if !self
            .is_organization_in_use(&flow.org_id)
            .await
            .map_err(|error| logged(&error, "reading the organization"))?
        {
            return Err(unavailable);
        }
        let org = self
            .enter_org_for_sign_in(&flow.org_id)
            .ok_or(unavailable)?;
        let stored = org
            .provider_for(flow)
            .await?
            .ok_or_else(|| flow.purpose.gone())?;
        let secret = self.open_client_secret(&stored)?;
        let discovery = provider::discover(self.oidc_fetcher(), &stored.issuer)
            .await
            .map_err(|error| provider_failure(&error, Reason::OidcUnavailable))?;
        // RFC 9207: the issuer that answered, where it says, is the one asked; a
        // provider that says it always says must have.
        let answered_by_it = match &callback.issuer {
            Some(issuer) => *issuer == stored.issuer,
            None => !discovery.authorization_response_iss_parameter_supported,
        };
        if !answered_by_it {
            tracing::info!(org_id = %flow.org_id, "a sign-in came back with another issuer than its provider's");
            return Err(Reason::OidcStateInvalid.into());
        }
        // A link that comes back without a session ends here, before the provider is
        // asked for anything: which session it is, is asked where the link is stored.
        if flow.purpose.needs_session() && callback.session_token.is_none() {
            tracing::info!(org_id = %flow.org_id, "a sign-in to link came back without a session");
            return Err(Reason::OidcStateInvalid.into());
        }
        let claims = self
            .checked_claims(&discovery, &stored, secret.as_deref(), flow, &callback.code)
            .await?;
        org.end_flow(flow, &claims, &stored, callback, user_agent)
            .await
    }

    /// A session to be stored for whoever a sign-in ends in, made at `now` by the
    /// browser of `user_agent`.
    pub(super) fn new_session(&self, user_agent: &str, now: i64) -> Session {
        Session {
            id: new_id(),
            created_at: now,
            expires_at: now.saturating_add(self.config().session_max_millis()),
            user_agent: user_agent.to_owned(),
            ..Session::default()
        }
    }

    /// Whether `org_id` names an organization in use that is not the system's: none
    /// other is signed in to.
    pub(super) async fn is_organization_in_use(&self, org_id: &str) -> Result<bool, StoreError> {
        if !fvoci_model::is_id(org_id) {
            return Ok(false);
        }
        Ok(self
            .store()
            .login_organization(Some(org_id))
            .await?
            .is_some())
    }

    /// The client secret of a provider, opened; `None` for a client without one.
    fn open_client_secret(&self, stored: &StoredProvider) -> Result<Option<String>, LoginFailure> {
        if stored.sealed_client_secret.is_empty() {
            return Ok(None);
        }
        let opened = self.config().secret_key.as_ref().map(|key| {
            key.open(
                &stored.sealed_client_secret,
                &secret_context(&stored.org_id, &stored.issuer, &stored.client_id),
            )
        });
        if let Some(Ok(secret)) = opened {
            return Ok(Some(secret));
        }
        tracing::error!(org_id = %stored.org_id, "the client secret of the provider does not open with the server's secret_key");
        Err(Reason::OidcUnavailable.into())
    }

    /// Exchanges the code and checks the ID token that comes for it.
    async fn checked_claims(
        &self,
        discovery: &Discovery,
        stored: &StoredProvider,
        secret: Option<&str>,
        flow: &LoginFlow,
        code: &str,
    ) -> Result<IdClaims, LoginFailure> {
        let redirect_uri = self.oidc_redirect_uri(&stored.org_id);
        let id_token = provider::exchange_code(
            self.oidc_fetcher(),
            discovery,
            Client {
                id: &stored.client_id,
                secret,
            },
            Grant {
                code,
                redirect_uri: &redirect_uri,
                verifier: &flow.pkce_verifier,
            },
        )
        .await
        .map_err(|error| provider_failure(&error, Reason::OidcTokenRefused))?;
        let keys = provider::keys(self.oidc_fetcher(), discovery)
            .await
            .map_err(|error| provider_failure(&error, Reason::OidcTokenRefused))?;
        provider::verify_id_token(
            &id_token,
            &keys,
            Expected {
                issuer: &stored.issuer,
                client_id: &stored.client_id,
                nonce: &flow.nonce,
            },
        )
        .map_err(|error| provider_failure(&error, Reason::OidcTokenInvalid))
    }

    /// Signs the person the provider vouches for in, a member of the handle's
    /// organization: to the account of the identity, which joins the organization if it
    /// is no member and the provider lets newcomers in, or else to a new account; and
    /// stores `session` for it under `digest`. What decides is read again when that is
    /// stored (`Store::finish_oidc_sign_in`): the provider may have changed while the
    /// person was at it.
    pub(super) async fn sign_in_through(
        &self,
        claims: &IdClaims,
        stored: &StoredProvider,
        ending: Ending<'_>,
        session: &mut Session,
        digest: &str,
    ) -> Result<Ended, LoginFailure> {
        let identity = Identity {
            issuer: &claims.issuer,
            subject: &claims.subject,
        };
        let found = self
            .store()
            .user_of_identity(identity)
            .await
            .map_err(|error| logged(&error, "looking up an identity"))?;
        if let Some(user) = found {
            let account = FoundAccount {
                user: &user,
                identity,
                proved_email: proved_address(claims),
            };
            return self.let_in(account, stored, ending, session, digest).await;
        }
        // The identity of an account that is deleted: not let in, and not made anew.
        let of_a_deleted_account = self
            .store()
            .is_identity_known(identity)
            .await
            .map_err(|error| logged(&error, "looking up an identity"))?;
        // A trial ends in the account of the administrator who started it, and makes
        // none: an identity that is nobody's is tied to theirs only where their
        // session shows they signed in through the provider just now
        // (`oidc_pending.rs`), and one of a deleted account not at all.
        if let Ending::Trial(end) = ending {
            if of_a_deleted_account {
                tracing::info!(org_id = %stored.org_id, "a trial of pending settings signed in somebody whose account is deleted");
                return Err(Reason::OidcTrialAccount.into());
            }
            return self
                .bind_by_trial(claims, stored, end, session, digest)
                .await
                .map(|applied| Ended {
                    joined: applied.joined,
                    applied: Some(applied),
                });
        }
        if of_a_deleted_account {
            tracing::info!(issuer = %claims.issuer, "a sign-in through a provider was for a deleted account");
            return Err(Reason::OidcUnavailable.into());
        }
        if !stored.auto_join {
            return Err(Reason::OidcJoinClosed.into());
        }
        let email = claims
            .email
            .as_deref()
            .map(str::trim)
            .filter(|email| is_email_valid(email))
            .ok_or(Reason::OidcEmailMissing)?;
        if claims.email_verified != Some(true) {
            return Err(Reason::OidcEmailUnverified.into());
        }
        // An account whose own address this is, one with a password: most likely the
        // same person, who is to sign in to it and link the provider to it, not to have
        // two accounts. What a provider gave for another account is no account's
        // address, and stands in nobody's way. An account that waits to be deleted has
        // its address still: its person signs in to it, and takes the asking back. So
        // has one that is being erased, until that ends.
        match self.store().is_address_held(email).await {
            Ok(true) => return Err(Reason::OidcEmailTaken.into()),
            Ok(false) => {}
            Err(error) => return Err(logged(&error, "looking up an email address")),
        }
        let newcomer = Newcomer {
            claims,
            identity,
            email,
        };
        self.make_account(newcomer, stored, ending, session, digest)
            .await
    }

    /// Signs in to the account of the identity, which joins the handle's organization
    /// if it is no member and the provider lets newcomers in. A registration that
    /// waits at the address the provider vouches for is removed with it.
    pub(super) async fn let_in(
        &self,
        found: FoundAccount<'_>,
        stored: &StoredProvider,
        ending: Ending<'_>,
        session: &mut Session,
        digest: &str,
    ) -> Result<Ended, LoginFailure> {
        let FoundAccount {
            user,
            identity,
            proved_email,
        } = found;
        let account = OidcAccount::Known {
            user_id: &user.id,
            password_hash: &user.password_hash,
            identity,
            proved_email,
        };
        let (joined, applied) = self
            .finish_in_store(stored, ending, account, session, digest)
            .await
            .map_err(|error| logged(&error, "storing a sign-in"))?
            .map_err(|refusal| refused(refusal, stored))?;
        if joined {
            tracing::info!(user_id = %user.id, org_id = %stored.org_id, "an account joined the organization by signing in through its provider");
        }
        Ok(Ended { applied, joined })
    }

    /// Stores the end of a sign-in as `ending` says: a session through the provider as
    /// it is, or through the pending settings, which become the provider's.
    pub(super) async fn finish_in_store(
        &self,
        stored: &StoredProvider,
        ending: Ending<'_>,
        account: OidcAccount<'_>,
        session: &mut Session,
        digest: &str,
    ) -> Result<Result<(bool, Option<Applied>), OidcRefusal>, StoreError> {
        Ok(match ending {
            Ending::SignIn => self
                .store()
                .finish_oidc_sign_in(&stored.id, stored.revision, account, session, digest)
                .await?
                .map(|joined| (joined, None)),
            Ending::Trial(end) => {
                // A trial makes no account: one that came to make one is no trier's.
                let OidcAccount::Known {
                    user_id,
                    password_hash,
                    identity,
                    ..
                } = account
                else {
                    return Ok(Err(OidcRefusal::NotTheTrier));
                };
                let tried = Tried::Known {
                    user_id,
                    password_hash,
                    identity,
                };
                self.store()
                    .finish_oidc_trial(self.trial_of(stored, end), tried, session, digest)
                    .await?
                    .map(|applied| (applied.joined, Some(applied)))
            }
        })
    }
}

/// Checks that what came back for `flow` came to its own organization's address, and
/// with a code: no error of the provider's, and nothing exchanged otherwise.
fn came_back_with_a_code(flow: &LoginFlow, callback: &Callback) -> Result<(), LoginFailure> {
    // Came back to the address of another organization than the one it was started
    // for: an answer of one organization's provider handed to another's.
    if callback.org_id != flow.org_id {
        tracing::info!(org_id = %flow.org_id, "a sign-in came back to the address of another organization");
        return Err(Reason::OidcStateInvalid.into());
    }
    if let Some(error) = &callback.error {
        // The provider's code of the error, short, for the log only.
        let error: String = error.chars().take(64).collect();
        tracing::info!(org_id = %flow.org_id, error, "the provider did not let the person sign in");
        return Err(Reason::OidcDenied.into());
    }
    if callback.code.is_empty() {
        return Err(Reason::OidcTokenRefused.into());
    }
    Ok(())
}

/// How a sign-in that ends in a session is stored.
#[derive(Debug, Clone, Copy)]
pub(super) enum Ending<'a> {
    /// Through the provider as it is.
    SignIn,
    /// Through the pending settings, which become the provider's, in the account of
    /// the administrator who tries them only.
    Trial(TrialEnd<'a>),
}

/// The end of a trial of the pending settings, as the sign-in that came back names it.
#[derive(Clone, Copy)]
pub(super) struct TrialEnd<'a> {
    /// When the pending settings were saved.
    pub(super) pending_at: i64,
    /// The administrator who started it.
    pub(super) trier_id: &'a str,
    /// The session it was started from; empty for none.
    pub(super) session_id: &'a str,
    /// The token of the session cookie the browser came back with, if it has one.
    pub(super) came_with: Option<&'a str>,
}

impl std::fmt::Debug for TrialEnd<'_> {
    /// Leaves out the token.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("TrialEnd")
            .field("pending_at", &self.pending_at)
            .field("trier_id", &self.trier_id)
            .finish_non_exhaustive()
    }
}

/// The provider as a trial signs in through it: with the pending settings' issuer,
/// client and secret, if they are of this very provider.
pub(super) fn trying(stored: &StoredProvider, pending: &PendingProvider) -> Option<StoredProvider> {
    (pending.provider_id == stored.id).then(|| pending.applied_to(stored))
}

/// The failure of a sign-in that the store refused at its end, logged.
pub(super) fn refused(refusal: OidcRefusal, stored: &StoredProvider) -> LoginFailure {
    match refusal {
        OidcRefusal::ProviderGone => {
            tracing::info!(org_id = %stored.org_id, "the provider was switched off or changed while a sign-in through it was under way");
            Reason::OidcUnavailable.into()
        }
        OidcRefusal::JoinClosed => Reason::OidcJoinClosed.into(),
        OidcRefusal::NotTheTrier => {
            tracing::info!(org_id = %stored.org_id, "a trial of pending settings vouched for somebody who is not its administrator, or who administers the organization no more");
            Reason::OidcTrialAccount.into()
        }
        OidcRefusal::NotFresh => {
            tracing::info!(org_id = %stored.org_id, "a trial of pending settings vouched for an identity tied to no account, from a session that does not show a sign-in through the provider just now");
            Reason::OidcTrialStale.into()
        }
        OidcRefusal::AccountChanged => {
            tracing::info!(org_id = %stored.org_id, "an account was deleted, given a password or unlinked from the identity while signing in through a provider");
            Reason::OidcUnavailable.into()
        }
    }
}

/// The PKCE challenge of a verifier: its SHA-256 in unpadded base64url (S256).
fn pkce_challenge(verifier: &str) -> String {
    use base64::Engine as _;
    use sha2::{Digest as _, Sha256};
    base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(Sha256::digest(verifier.as_bytes()))
}

/// The failure of a sign-in that the server is to blame for, logged.
pub(super) fn logged(error: &StoreError, doing: &str) -> LoginFailure {
    tracing::error!(%error, doing, "a sign-in through a provider failed");
    Reason::OidcUnavailable.into()
}

/// The failure of a sign-in that the provider's answer is to blame for, logged in the
/// server's words.
fn provider_failure(error: &ProviderError, reason: Reason) -> LoginFailure {
    tracing::info!(%error, "a provider's answer was not taken");
    reason.into()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_pkce_challenge_is_the_s256_of_the_verifier() {
        // RFC 7636, appendix B.
        assert_eq!(
            pkce_challenge("dBjftJeZ4CVP-mB92K27uhbUJU1p1r_wW1gFWFOEjXk"),
            "E9Melhoa2OwvFrEMTJguCHaoeK1t8URWbuGJSstw-cM"
        );
    }
}
