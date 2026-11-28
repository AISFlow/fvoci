//! Linking a provider to an account: somebody who is signed in ties who they are at an
//! organization's provider to their account, so that the provider signs in to it
//! afterwards; reading what is tied; and removing it.
//!
//! A link is a sign-in through the provider that was started from a session, with the
//! account's password. It ends by storing the identity for that session's account in
//! place of opening a session, and only for the browser that started it: the one that
//! has the cookie of the sign-in and still the session it was started from. Accounts
//! are never joined: an identity that is another account's is refused.

use std::net::IpAddr;

use fvoci_model::{LinkedIdentity, Session, UnlinkRequest, User, is_email_valid};
use fvoci_store::{FlowPurpose, Identity, LinkRefusal, LinkingSession, StoredProvider, Unlinked};

use super::App;
use super::auth::WrongPassword;
use super::oidc::{Callback, LoginFailure, StartedLogin};
use crate::auth::now_millis;
use crate::auth::oidc::provider::IdClaims;
use crate::auth::secret::token_digest;
use crate::error::{AppError, Reason};

/// How the password of a link or of a removal is refused.
const WRONG: WrongPassword = WrongPassword {
    words: "the password is wrong",
    field: "password",
};

impl App {
    /// The account of the session, whose holder has shown to know its password.
    ///
    /// # Errors
    ///
    /// Returns 409 with the reason `password_not_set` for an account without a
    /// password, which has nothing to confirm with; and as
    /// [`App::confirm_password`] does.
    async fn confirmed_account(
        &self,
        session: &Session,
        password: &str,
        wrong: WrongPassword,
        address: Option<IpAddr>,
    ) -> Result<User, AppError> {
        let user = self.store().get_user_by_id(&session.user_id).await?;
        if user.password_hash.is_empty() {
            return Err(password_not_set());
        }
        self.confirm_password(&user, password, wrong, address)
            .await?;
        Ok(user)
    }

    /// Starts a sign-in through the provider of the organization `org_id` that links:
    /// it comes back to tie who the provider vouches for to the account of `session`.
    ///
    /// # Errors
    ///
    /// Returns 400 (`invalid_credentials`, naming `linkPassword`) for a wrong password,
    /// 409 (`password_not_set`) for an account without one, 429 for too many tries, and
    /// what [`App::start_oidc_login`] returns.
    pub async fn start_oidc_link(
        &self,
        org_id: &str,
        session: &Session,
        password: &str,
        address: Option<IpAddr>,
    ) -> Result<StartedLogin, AppError> {
        let wrong = WrongPassword {
            field: "linkPassword",
            ..WRONG
        };
        self.confirmed_account(session, password, wrong, address)
            .await?;
        self.start_oidc_flow(
            org_id,
            address,
            FlowPurpose::Link {
                session_id: session.id.clone(),
            },
        )
        .await
    }

    /// Ends a sign-in that was started to link, on the handle of its organization:
    /// ties who the provider vouches for to the account of the session it was started
    /// from, if the browser comes back with that very session, still good. What
    /// decides is read again where it is stored (`Store::finish_oidc_link`).
    pub(super) async fn link_through(
        &self,
        claims: &IdClaims,
        stored: &StoredProvider,
        session_id: &str,
        callback: &Callback,
    ) -> Result<(), LoginFailure> {
        let Some(token) = callback
            .session_token
            .as_deref()
            .filter(|token| !token.is_empty())
        else {
            tracing::info!(org_id = %stored.org_id, "a sign-in to link came back without the session it was started from");
            return Err(Reason::OidcStateInvalid.into());
        };
        let now = now_millis();
        let link = LinkingSession {
            session_id,
            token_digest: &token_digest(token),
            now,
            idle_since: now.saturating_sub(self.config().session_idle_millis()),
        };
        let identity = Identity {
            issuer: &claims.issuer,
            subject: &claims.subject,
        };
        // The provider's word, as at a first sign-in: an address it has checked. It is
        // the account's own, or nothing is tied (`Store::finish_oidc_link`).
        let email = claims
            .email
            .as_deref()
            .map(str::trim)
            .filter(|email| is_email_valid(email))
            .ok_or(Reason::OidcEmailMissing)?;
        if claims.email_verified != Some(true) {
            return Err(Reason::OidcEmailUnverified.into());
        }
        let linked = self
            .store()
            .finish_oidc_link(&stored.id, link, identity, email)
            .await
            .map_err(|error| {
                tracing::error!(%error, "a sign-in to link failed where it is stored");
                LoginFailure::from(Reason::OidcUnavailable)
            })?;
        match linked {
            Ok(linked) => {
                if linked.newly {
                    tracing::info!(user_id = %linked.user_id, org_id = %stored.org_id, issuer = %stored.issuer, "an account linked a provider to itself");
                }
                Ok(())
            }
            Err(LinkRefusal::ProviderGone) => Err(Reason::OidcUnavailable.into()),
            Err(LinkRefusal::SessionEnded) => {
                tracing::info!(org_id = %stored.org_id, "a sign-in to link came back to a session that ended, or to another");
                Err(Reason::OidcStateInvalid.into())
            }
            Err(LinkRefusal::IdentityTaken) => Err(Reason::OidcIdentityTaken.into()),
            Err(LinkRefusal::AnotherAddress) => {
                tracing::info!(org_id = %stored.org_id, "a sign-in to link was of another address than the account's");
                Err(Reason::OidcEmailMismatch.into())
            }
        }
    }

    /// The identities tied to the account, oldest first.
    ///
    /// # Errors
    ///
    /// Returns the store's error.
    pub async fn identities(&self, session: &Session) -> Result<Vec<LinkedIdentity>, AppError> {
        let tied = self.store().identities_of_user(&session.user_id).await?;
        Ok(tied
            .into_iter()
            .map(|tied| LinkedIdentity {
                made_this_session: session
                    .login
                    .as_ref()
                    .is_some_and(|login| login.is_of_identity(&tied.issuer, &tied.subject)),
                issuer: tied.issuer,
                subject: tied.subject,
                email: tied.email,
                created_at: tied.created_at,
            })
            .collect())
    }

    /// Removes an identity from the account of the session, whose holder shows to know
    /// its password.
    ///
    /// # Errors
    ///
    /// Returns 404 for an identity that is not the account's; 409 with the reason
    /// `last_sign_in` for the only identity of an account without a password, which
    /// nothing else lets in, and `password_not_set` for another identity of such an
    /// account; 400 (`invalid_credentials`) for a wrong password and 429 for too many;
    /// and 409 if the account changed meanwhile.
    pub async fn unlink_identity(
        &self,
        session: &Session,
        request: &UnlinkRequest,
        address: Option<IpAddr>,
    ) -> Result<(), AppError> {
        let not_tied = || AppError::not_found("identity");
        let last = || {
            AppError::conflict("the account has no password and no other identity to sign in by")
                .with_reason(Reason::LastSignIn)
        };
        let tied = self.store().identities_of_user(&session.user_id).await?;
        if !tied.iter().any(|identity| {
            identity.issuer == request.issuer && identity.subject == request.subject
        }) {
            return Err(not_tied());
        }
        let user = self.store().get_user_by_id(&session.user_id).await?;
        if user.password_hash.is_empty() {
            return Err(if tied.len() == 1 {
                last()
            } else {
                password_not_set()
            });
        }
        self.confirm_password(&user, &request.password, WRONG, address)
            .await?;
        let identity = Identity {
            issuer: &request.issuer,
            subject: &request.subject,
        };
        match self
            .store()
            .unlink_identity(&user.id, &user.password_hash, identity)
            .await?
        {
            Unlinked::Removed(ended) => {
                self.ws().close(ended.into_scope());
                tracing::info!(user_id = %user.id, issuer = %request.issuer, "an account removed an identity from itself");
                Ok(())
            }
            Unlinked::NotTied => Err(not_tied()),
            Unlinked::LastWayIn => Err(last()),
            Unlinked::AccountChanged => Err(AppError::conflict(
                "the account was changed meanwhile; read it and try again",
            )),
        }
    }
}

fn password_not_set() -> AppError {
    AppError::conflict("the account has no password to confirm this with")
        .with_reason(Reason::PasswordNotSet)
}
