//! Registrations that wait for their address to be confirmed (`email_verification`):
//! the link mailed to the address makes the account, starts what the registration
//! starts and signs its person in. Until then there is no account.
//!
//! Only who registered finishes it: the browser the registration was made from, which
//! keeps a secret of it in a cookie ([`SignupMarker`]), or whoever knows the password it
//! was made with. The mailbox alone confirms nothing, so that nobody is signed in to
//! an account whose password somebody else chose.

use std::net::IpAddr;
use std::time::Instant;

use fvoci_model::{ContainerKind, Registration, Session, User, new_id};
use fvoci_store::{
    Accepted, Confirmed, InvitedConfirmation, Proof, SIGNUP_LIFETIME_MILLIS, Signup, SignupStart,
    Started, StoreError,
};

use super::App;
use super::auth::{PasswordHolder, account_error, incorrect_login};
use super::language::Language;
use super::mail::{Content, Letter, SecretLink, SecretPage};
use crate::auth::limiter::RetryAfter;
use crate::auth::now_millis;
use crate::auth::secret::{new_secret, token_digest};
use crate::collab::AccessScope;
use crate::error::{AppError, Reason};
use crate::metrics::ResetMailDrop;

impl App {
    /// Whether a registration opens nothing until the link mailed to its address
    /// confirms it (`email_verification`).
    #[must_use]
    pub fn verifies_email(&self) -> bool {
        self.config().mail.email_verification
    }

    /// Stores the registration of somebody who signs up, to be confirmed by the link
    /// mailed to its address, and mails that link in the background, in the language of
    /// the screen it came from: nothing is made of it until then. Returns the marker of
    /// the browser it was made from, for its cookie.
    ///
    /// `browser` is the secret of the cookie the browser has already, of a
    /// registration it made before that waits still: this one replaces it, in one
    /// transaction, so that whoever mistyped an address registers again at once under
    /// the same name. Its name and address are free for this one, and the use it took
    /// of a link is given back. A registration that is refused replaces nothing.
    ///
    /// # Errors
    ///
    /// As [`App::sign_up`]: a name or an address that an account or another
    /// registration holds is the same conflict.
    pub async fn sign_up_to_confirm(
        &self,
        request: &Registration,
        browser: Option<&str>,
        address: Option<IpAddr>,
    ) -> Result<SignupMarker, AppError> {
        let _attempt = self.admit_registration(address)?;
        let start = if let Some(name) = request.organization() {
            SignupStart::Organization(name.to_owned())
        } else {
            self.require_organization_of_first(request).await?;
            SignupStart::Alone
        };
        let replacing = self.replaced_by(browser);
        let replacing = replacing.as_deref();
        let user = self.new_account(request.account(), replacing).await?;
        let now = now_millis();
        let marker = SignupMarker::new();
        let signup = waiting(&user, request.language(), start, &marker, now);
        self.store()
            .create_signup(&signup, replacing, now)
            .await
            .map_err(account_error)?;
        tracing::info!(signup_id = %signup.id, "a registration waits for its address");
        let app = self.clone();
        self.spawn_background(async move { app.mail_signup_link(&signup, None).await });
        Ok(marker)
    }

    /// The digest of the marker of the registration that a new one from the browser
    /// of `browser` (the secret of its cookie) replaces; none where the server asks no
    /// registration to wait, and makes no marker.
    pub(super) fn replaced_by(&self, browser: Option<&str>) -> Option<String> {
        browser.filter(|_| self.verifies_email()).map(token_digest)
    }

    /// The registration of `user`, an account asked for by the link `invitation_id` from
    /// a screen in `language` of the browser of `marker`, to be stored as that link's
    /// (`reserve_invitation`). The registrations that ended go first: they hold no
    /// name, but their rows do.
    pub(super) async fn signup_by_link(
        &self,
        user: &User,
        invitation_id: &str,
        language: &str,
        marker: &SignupMarker,
        now: i64,
    ) -> Result<Signup, AppError> {
        self.store().remove_ended_signups(now).await?;
        let start = SignupStart::Invitation {
            id: invitation_id.to_owned(),
            // The link's organization, which the store sets as it holds the link.
            org_id: String::new(),
        };
        Ok(waiting(user, language, start, marker, now))
    }

    /// Admits a request for a new confirmation link identically whether or not a
    /// registration has the name or the address, and returns unstarted work: the HTTP
    /// adapter releases it only after handing off its response. The registration's
    /// address is sent no more than an account's recovery messages
    /// (`mail_reset_per_account_hour`, the first link counted), and further requests are
    /// answered the same and lock nothing. Requests are limited by where they come from
    /// (`address`), with the requests for a reset link. A registration made while
    /// `email_verification` was on still gets its link once it is off, while mail is sent.
    ///
    /// # Errors
    ///
    /// Returns 400 for a request that names neither a name nor an address, 404 where no
    /// mail is sent, and 429 for too many requests from
    /// `address`.
    pub fn prepare_signup_mail(
        &self,
        username: &str,
        email: &str,
        language: &str,
        address: Option<IpAddr>,
    ) -> Result<impl Future<Output = ()> + Send + 'static + use<>, AppError> {
        let (username, email) = (username.trim().to_owned(), email.trim().to_owned());
        if username.is_empty() && email.is_empty() {
            return Err(AppError::bad_request(
                "the user name or the email address of the registration is required",
            )
            .with_field("email"));
        }
        if !self.config().mail_enabled() {
            return Err(AppError::not_found("route"));
        }
        let _request = self
            .inner
            .reset_limiter
            .admit_from_address(address, Instant::now())
            .map_err(|RetryAfter(wait)| AppError::rate_limited(wait))?;
        let app = self.clone();
        let requested = Language::written_in(language);
        Ok(async move {
            match app.find_signup(&username, &email).await {
                Ok(Some(signup)) => app.mail_signup_link(&signup, requested).await,
                Ok(None) => {}
                Err(error) => tracing::warn!(%error, "a confirmation link was not resent"),
            }
        })
    }

    /// Removes the registrations that ended unconfirmed by `now` and the links that
    /// ended: their names and addresses are free again. For the housekeeping.
    ///
    /// # Errors
    ///
    /// Returns the store's error; nothing is removed then.
    pub async fn remove_ended_signups(&self, now: i64) -> Result<u64, AppError> {
        Ok(self.store().remove_ended_signups(now).await?)
    }

    /// Mails the registration a new link that confirms it: the link it had confirms
    /// nothing any more. What became of the message is logged, never told: the answer
    /// that started it is the same whatever it was.
    pub(super) async fn mail_signup_link(&self, signup: &Signup, requested: Option<Language>) {
        let queued = match self.inner.mail.queue_verification(&signup.email) {
            Ok(queued) => queued,
            Err(ResetMailDrop::AccountLimit) => {
                tracing::info!("confirmation mail dropped: the address's hourly limit");
                return;
            }
            Err(ResetMailDrop::QueueFull) => {
                tracing::warn!("confirmation mail dropped: its queue is full");
                return;
            }
        };
        let language = Language::first_of([Language::written_in(&signup.language), requested]);
        // The digest of the link the message carries, once it is written: a retry is
        // sent only while that link still confirms the registration.
        let digest = std::sync::Mutex::new(String::new());
        let delivered = queued
            .deliver(
                &self.inner.mail,
                || async {
                    let token = new_secret();
                    let link = self.renew_signup_link(&signup.id, &token).await?;
                    *digest
                        .lock()
                        .expect("the link's digest is never held across a panic") =
                        token_digest(&token);
                    Ok::<_, AppError>(Letter {
                        to: signup.email.clone(),
                        content: Content::verification(&link, &signup.username, language),
                    })
                },
                || async {
                    let digest = digest
                        .lock()
                        .expect("the link's digest is never held across a panic")
                        .clone();
                    Ok(self
                        .store()
                        .signup_link_stands(&signup.id, &digest, now_millis())
                        .await?)
                },
            )
            .await;
        match delivered {
            Ok(_) => {}
            // Confirmed or ended while the message waited for its turn.
            Err(error) if error.is_not_found() => {
                tracing::debug!("confirmation mail not written: the registration is gone");
            }
            Err(error) => tracing::warn!(%error, "confirmation mail failed"),
        }
    }

    /// A new link for the registration, of `token`, made as its message has its turn.
    async fn renew_signup_link(
        &self,
        signup_id: &str,
        token: &str,
    ) -> Result<SecretLink, AppError> {
        let renewed = self
            .store()
            .renew_signup_link(signup_id, &token_digest(token), now_millis())
            .await?;
        if !renewed {
            return Err(AppError::not_found("registration"));
        }
        Ok(SecretLink::new(
            self.settings().server_url(),
            SecretPage::VerifyEmail,
            token,
        ))
    }

    /// Confirms the registration of the link `token` is of, for `registrant`: makes its
    /// account, starts its organization, and signs it in. The link alone confirms
    /// nothing: the browser the registration was made from finishes it at once, and any
    /// other only with the password it was made with, which is counted against the
    /// registration's user name and the address as a login's is. A registrant who is
    /// refused leaves the link as it was. Returns the token of the new session and
    /// where the account was let in.
    ///
    /// # Errors
    ///
    /// Returns 400 naming `token` for a link that confirms nothing (none, ended, used,
    /// or replaced by a newer one); for a link that does, 403 (`password_required`) from
    /// another browser without a password, the 401 of a login for a wrong one, and 429
    /// for too many wrong ones; 403 (`invitation_required`) if the server takes one
    /// organization and it has its first user now, which removes the registration;
    /// 429 for too many attempts from `address`; or the store's error.
    pub async fn confirm_signup(
        &self,
        token: &str,
        registrant: Registrant<'_>,
        user_agent: &str,
        address: Option<IpAddr>,
    ) -> Result<ConfirmedSignup, AppError> {
        let turn = self.admit_registration(address)?;
        let digest = token_digest(token);
        let proof = Proof::Link(&digest);
        let link_refused = || {
            AppError::bad_request("the confirmation link is invalid or expired").with_field("token")
        };
        let signup = self
            .store()
            .signup_of(proof, now_millis())
            .await?
            .ok_or_else(link_refused)?;
        let made_here = registrant
            .marker
            .is_some_and(|secret| SignupMarker::is_of(secret, &signup));
        // Nothing of the registration is touched before its registrant is known: what
        // confirming finds (a link that lets nobody in, a server that has its first
        // user) removes it.
        if made_here {
            // Nothing is tried at a password: another from the address waits no
            // longer, and not for the welcome board of this one.
            drop(turn);
        } else {
            if registrant.password.is_empty() {
                return Err(password_required());
            }
            // The same turn of the address, now an attempt at the password of the
            // registration's name, as a login's is.
            let attempt = turn
                .at_account(&signup.username, Instant::now())
                .map_err(|RetryAfter(wait)| AppError::rate_limited(wait))?;
            let holder = PasswordHolder {
                username: &signup.username,
                password_hash: &signup.password_hash,
                locked: false,
            };
            if !self
                .is_password_of(holder, registrant.password, attempt)
                .await?
            {
                tracing::debug!(signup_id = %signup.id, "invalid password for a registration");
                return Err(incorrect_login());
            }
        }
        // Read again: checking a password takes a while, and a link that ended
        // meanwhile confirms nothing.
        let confirmed = self
            .confirm(&signup, proof, now_millis())
            .await?
            .ok_or_else(link_refused)?;
        let session_token = new_secret();
        let now = now_millis();
        let session = Session {
            id: new_id(),
            user_id: confirmed.user.id.clone(),
            created_at: now,
            expires_at: now.saturating_add(self.config().session_max_millis()),
            user_agent: user_agent.to_owned(),
            ..Session::default()
        };
        let stored = self
            .store()
            .create_session(
                &session,
                &token_digest(&session_token),
                &confirmed.user.password_hash,
            )
            .await
            .map_err(|error| AppError::from(error).context("unable to create session"))?;
        if !stored {
            // The password was set anew in the instant since: the account is there.
            return Err(AppError::conflict(
                "the account was changed in the meantime; sign in",
            ));
        }
        let container_kind = self
            .kind_of_admitted_in(&confirmed.org_id, confirmed.container_id.as_deref())
            .await?;
        Ok(ConfirmedSignup {
            session_token,
            confirmed,
            container_kind,
            made_here,
        })
    }

    /// Confirms the registration with the user name `username`, in any letter case, on
    /// the word of the server's operator (the administration socket), for a person whose
    /// mail does not arrive: as its link would, but signing nobody in. A registration
    /// made by a link for members is recorded as the operator's in the organization it
    /// joins; one of its own starts its organization only now, and a guest is no member
    /// of the organization, so neither is recorded. Returns the account's identifier.
    ///
    /// # Errors
    ///
    /// Returns 404 if no registration that waits has the name, the 404 of
    /// [`App::accept_invitation`] if its link lets nobody in any more (and the
    /// registration is removed), and as [`App::confirm_signup`] otherwise.
    pub async fn confirm_signup_by_operator(&self, username: &str) -> Result<String, AppError> {
        let proof = Proof::Operator(username);
        let now = now_millis();
        let gone = || AppError::not_found("registration");
        let signup = self.store().signup_of(proof, now).await?.ok_or_else(gone)?;
        let confirmed = self.confirm(&signup, proof, now).await?.ok_or_else(gone)?;
        Ok(confirmed.user.id)
    }

    /// Confirms `signup`, which `proof` names at `now`: starts its organization, or
    /// lets its account in by the link it was made by. `None` if it is gone meanwhile.
    async fn confirm(
        &self,
        signup: &Signup,
        proof: Proof<'_>,
        now: i64,
    ) -> Result<Option<Confirmed>, AppError> {
        let confirmed = match &signup.start {
            SignupStart::Organization(_) => self.confirm_founding(signup, proof, now).await?,
            SignupStart::Alone => self.confirm_alone(signup, proof, now).await?,
            SignupStart::Invitation { id, org_id } => {
                self.confirm_invited(signup, proof, (id, org_id), now)
                    .await?
            }
        };
        if let Some(confirmed) = &confirmed {
            self.metrics().registered();
            tracing::info!(
                user_id = %confirmed.user.id,
                workspace_id = %confirmed.workspace_id,
                first = confirmed.user.is_admin,
                "a registration was confirmed"
            );
            let language = (!signup.language.is_empty()).then_some(signup.language.as_str());
            match confirmed.start {
                Started::Organization => {
                    self.welcome_workspace_in(
                        &confirmed.org_id,
                        &confirmed.user.id,
                        &confirmed.workspace_id,
                        fvoci_model::OrgKind::Shared,
                        language,
                    )
                    .await;
                    if let Ok(workspaces) = self
                        .store()
                        .workspaces_of_user(
                            &confirmed.user.id,
                            fvoci_store::Making::with_password(),
                        )
                        .await
                    {
                        for ws in workspaces {
                            if ws.kind == fvoci_model::OrgKind::Personal {
                                self.welcome_workspace_in(
                                    &ws.org_id,
                                    &confirmed.user.id,
                                    &ws.id,
                                    fvoci_model::OrgKind::Personal,
                                    language,
                                )
                                .await;
                            }
                        }
                    }
                }
                Started::Alone => {
                    self.welcome_workspace_in(
                        &confirmed.org_id,
                        &confirmed.user.id,
                        &confirmed.workspace_id,
                        fvoci_model::OrgKind::Personal,
                        language,
                    )
                    .await;
                }
                Started::Invited if confirmed.container_id.is_none() => {
                    self.welcome_member_in(
                        &confirmed.org_id,
                        &confirmed.user.id,
                        &confirmed.workspace_id,
                        language,
                    )
                    .await;
                }
                Started::Invited => {}
            }
        }
        Ok(confirmed)
    }

    /// Confirms a registration of its own, starting its organization as the first of the
    /// server's, or a new one where the server takes several.
    async fn confirm_founding(
        &self,
        signup: &Signup,
        proof: Proof<'_>,
        now: i64,
    ) -> Result<Option<Confirmed>, AppError> {
        let store = self.store();
        let one_organization = !self.settings().takes_several_organizations();
        let policy = if one_organization {
            fvoci_store::FoundingPolicy::FirstOnly
        } else {
            fvoci_store::FoundingPolicy::FirstOrNew
        };
        let confirmed = store
            .confirm_signup_with_policy(signup, proof, policy, now)
            .await;
        if matches!(confirmed, Err(StoreError::NotFirstUser)) && one_organization {
            // Nobody else gets in without an invitation now: the registration is of
            // no use, and its name and address are free again. A link of none
            // confirms nothing, as anywhere.
            if !store.discard_signup(proof, now).await? {
                return Ok(None);
            }
            return Err(AppError::invitation_required());
        }
        if one_organization
            && confirmed
                .as_ref()
                .is_ok_and(|found| found.as_ref().is_some_and(|c| c.user.is_admin))
        {
            // The first user is in: no other registration of its own can start anything
            // on this server, and their names and addresses are free for invitations.
            let removed = store.remove_founding_signups().await?;
            if removed > 0 {
                tracing::info!(
                    removed,
                    "registrations that could start nothing were removed"
                );
            }
        }
        confirmed.map_err(account_error)
    }

    /// Confirms a registration of its own that starts no organization: its account
    /// and the account's own space. Not on a server that takes one organization, where
    /// who is not its first user gets in by an invitation: the registration is
    /// removed, as one that would start a second organization there is.
    async fn confirm_alone(
        &self,
        signup: &Signup,
        proof: Proof<'_>,
        now: i64,
    ) -> Result<Option<Confirmed>, AppError> {
        let store = self.store();
        if !self.settings().takes_several_organizations() {
            if !store.discard_signup(proof, now).await? {
                return Ok(None);
            }
            return Err(AppError::invitation_required());
        }
        store
            .confirm_signup_alone(signup, proof, now)
            .await
            .map_err(account_error)
    }

    /// Confirms a registration made by the link `link` (its identifier and its
    /// organization's), letting its account in as an acceptance would.
    ///
    /// # Errors
    ///
    /// Returns 404 (`invitation_unavailable`) if the link lets nobody in any more: the
    /// registration is removed, and its use of the link given back.
    async fn confirm_invited(
        &self,
        signup: &Signup,
        proof: Proof<'_>,
        (link_id, org_id): (&str, &str),
        now: i64,
    ) -> Result<Option<Confirmed>, AppError> {
        let Some(app) = self.enter_org_for_sign_in(org_id) else {
            return Err(AppError::internal(
                "the organization of a registration as the store has it is no identifier",
            ));
        };
        for _ in 0..super::invitations::ATTEMPTS {
            let found = app.maker_of_link(link_id).await?;
            let inviter = found
                .as_ref()
                .map(|(invitation, maker)| maker.inviter(&invitation.created_by));
            let outcome = app
                .store()
                .confirm_invited_signup(signup, proof, inviter, now)
                .await
                .map_err(account_error)?;
            match outcome {
                InvitedConfirmation::Confirmed(confirmed) => {
                    let accepted = Accepted {
                        user_id: confirmed.user.id.clone(),
                        workspace_id: confirmed.workspace_id.clone(),
                        container_id: confirmed.container_id.clone(),
                        used: true,
                    };
                    app.collab()
                        .recheck_access(
                            &app,
                            AccessScope::workspace(&accepted.workspace_id)
                                .of_user(&accepted.user_id),
                        )
                        .await;
                    app.announce_admission(&accepted).await;
                    return Ok(Some(*confirmed));
                }
                InvitedConfirmation::Unavailable => {
                    return Err(AppError::invitation_unavailable());
                }
                InvitedConfirmation::Gone => return Ok(None),
                // Whether the maker may still invite is asked again, of what they are now.
                InvitedConfirmation::InviterChanged => {}
            }
        }
        // Changed every time: as good as no link for now, but the registration stays,
        // to be confirmed again once its maker holds still.
        Err(AppError::invitation_unavailable())
    }
}

/// The secret of the browser a registration was made from, which its cookie keeps for
/// as long as the registration waits: no session, and nothing is reached by it. It
/// shows the browser to be the registrant's, where the link of the mail confirms the
/// registration at once. The store has its digest only.
#[derive(Clone)]
pub struct SignupMarker(String);

impl SignupMarker {
    /// How long a browser keeps one, in seconds: as long as its registration waits.
    pub const MAX_AGE_SECONDS: i64 = SIGNUP_LIFETIME_MILLIS / 1_000;

    /// A new one, of 256 random bits.
    pub(super) fn new() -> Self {
        Self(new_secret())
    }

    /// The secret, for the cookie.
    #[must_use]
    pub fn secret(&self) -> &str {
        &self.0
    }

    /// Whether `secret`, as a browser's cookie has it, is the one `signup` was made
    /// with. One made before browsers were told apart has none.
    fn is_of(secret: &str, signup: &Signup) -> bool {
        !signup.browser_hash.is_empty() && token_digest(secret) == signup.browser_hash
    }
}

impl std::fmt::Debug for SignupMarker {
    /// Leaves out the secret.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("SignupMarker(..)")
    }
}

/// What whoever opens a confirmation link shows of being who registered.
#[derive(Clone, Copy)]
pub struct Registrant<'a> {
    /// The secret of the browser's cookie, if it has one.
    pub marker: Option<&'a str>,
    /// The password the registration was made with; empty for none given.
    pub password: &'a str,
}

impl std::fmt::Debug for Registrant<'_> {
    /// Leaves out the secret and the password.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Registrant").finish_non_exhaustive()
    }
}

/// A confirmed registration, signed in.
pub struct ConfirmedSignup {
    /// The token of the new session.
    pub session_token: String,
    /// The account, and where it was let in.
    pub confirmed: Confirmed,
    /// For a guest's link: whether a board or a wiki let it in.
    pub container_kind: Option<ContainerKind>,
    /// Whether the browser was the one the registration was made from: its cookie has
    /// nothing left to show.
    pub made_here: bool,
}

impl ConfirmedSignup {
    /// Whether the account started an organization by registering: not one that came
    /// in by a link, and not one that registered without the name of an organization.
    #[must_use]
    pub fn started_organization(&self) -> bool {
        self.confirmed.start == Started::Organization
    }
}

impl std::fmt::Debug for ConfirmedSignup {
    /// Leaves out the token.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ConfirmedSignup")
            .field("confirmed", &self.confirmed)
            .field("made_here", &self.made_here)
            .finish_non_exhaustive()
    }
}

/// The refusal of a confirmation link opened in another browser than the registration
/// was made from, without the password it was made with.
fn password_required() -> AppError {
    AppError::forbidden("the password of the registration is required")
        .with_reason(Reason::PasswordRequired)
        .with_field("password")
}

/// The registration of `user`, made now from a screen in `language` of the browser of
/// `marker`, to start `start`.
fn waiting(
    user: &User,
    language: &str,
    start: SignupStart,
    marker: &SignupMarker,
    now: i64,
) -> Signup {
    Signup {
        id: user.id.clone(),
        username: user.username.clone(),
        email: user.email.clone(),
        custom_name: user.name.custom_name.clone(),
        password_hash: user.password_hash.clone(),
        language: Language::written_in(language)
            .map_or_default(Language::tag)
            .to_owned(),
        start,
        browser_hash: token_digest(marker.secret()),
        created_at: now,
        expires_at: now.saturating_add(SIGNUP_LIFETIME_MILLIS),
    }
}
