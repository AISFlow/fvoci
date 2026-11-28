//! Carrying an account over to the provider an organization moved to.
//!
//! While the provider before is kept (`former` of the provider), a sign-in through the
//! organization's provider that vouches for somebody who has no account is held in
//! place of making one: the browser gets a cookie for it, and the login page asks
//! whether the person had an account. If they had, a sign-in through the former
//! provider is started from that browser, and when it comes back the held identity is
//! tied to the account the former provider vouches for, which has a place in the
//! organization (a member, or a guest of one of its workspaces in use), and a session
//! made through the provider is opened: both identities are shown, so no address is
//! compared. If not, the held sign-in ends as a first sign-in does. A sign-in through
//! the former provider ends in nothing else, and never in a session of its own. The
//! store's part is `Store::finish_oidc_carry_over`.

use std::net::IpAddr;
use std::time::Instant;

use fvoci_store::{CarryOver, FlowPurpose, HeldSignIn, Identity, StoreError, StoredProvider};

use super::App;
use super::oidc::{Callback, Ending, FinishedLogin, LoginFailure, StartedLogin, logged};
use crate::auth::limiter::RetryAfter;
use crate::auth::now_millis;
use crate::auth::oidc::provider::IdClaims;
use crate::auth::secret::{new_secret, token_digest};
use crate::error::{AppError, Reason};

/// How long a held sign-in waits for the person to say whether they had an account,
/// and to come back from the former provider, in milliseconds.
pub const HELD_SIGN_IN_LIFETIME_MILLIS: i64 = 10 * 60 * 1000;

impl App {
    /// Holds a sign-in through `stored`, the provider of the handle's organization,
    /// that came back with `claims`, if the organization keeps a former provider and
    /// who `claims` vouch for is nobody's: `None` for a sign-in that goes on as ever
    /// (to the identity's account, or to what is said of a deleted one), or else the
    /// secret for the browser's cookie.
    pub(super) async fn hold_if_moving(
        &self,
        claims: &IdClaims,
        stored: &StoredProvider,
    ) -> Result<Option<String>, LoginFailure> {
        let now = now_millis();
        let kept = self
            .store()
            .oidc_former_provider(now)
            .await
            .map_err(|error| logged(&error, "reading the former provider"))?;
        if kept.is_none() {
            return Ok(None);
        }
        let identity = Identity {
            issuer: &claims.issuer,
            subject: &claims.subject,
        };
        if self
            .store()
            .is_identity_known(identity)
            .await
            .map_err(|error| logged(&error, "looking up an identity"))?
        {
            return Ok(None);
        }
        let secret = new_secret();
        let held = HeldSignIn {
            held_hash: token_digest(&secret),
            org_id: stored.org_id.clone(),
            provider_id: stored.id.clone(),
            revision: stored.revision,
            issuer: claims.issuer.clone(),
            subject: claims.subject.clone(),
            email: claims.email.clone().unwrap_or_default(),
            email_unverified: claims.email_verified != Some(true),
            name: claims.name.clone().unwrap_or_default(),
            preferred_username: claims.preferred_username.clone().unwrap_or_default(),
            created_at: now,
            expires_at: now.saturating_add(HELD_SIGN_IN_LIFETIME_MILLIS),
        };
        let is_held = self
            .store()
            .hold_oidc_sign_in(&held)
            .await
            .map_err(|error| logged(&error, "holding a sign-in"))?;
        if !is_held {
            // The keeping ended, or the provider changed, a moment ago: the sign-in
            // goes on as ever, and where it ends the provider is read again.
            return Ok(None);
        }
        tracing::info!(org_id = %stored.org_id, "a sign-in of somebody without an account is held while the organization moves to its provider");
        Ok(Some(secret))
    }

    /// Takes the sign-in held under `held_hash` in the organization `org_id`, for a
    /// sign-in to carry it over that came back and did not end well, wherever it
    /// stopped: a held sign-in is tried once. What fails is logged, and the held
    /// sign-in ends by its time.
    pub(super) async fn drop_held(&self, org_id: &str, held_hash: &str) {
        let Some(org) = self.enter_org_for_sign_in(org_id) else {
            return;
        };
        if let Err(error) = org
            .store()
            .take_oidc_held_sign_in(held_hash, now_millis())
            .await
        {
            tracing::warn!(%error, org_id, "unable to take a held sign-in whose carrying over failed");
        }
    }

    /// What a sign-in that carries the held sign-in `held_hash` over goes through, on
    /// the handle of its organization, whose provider is `stored`: the former
    /// provider, if one is kept and this browser's sign-in is held, not ended. `None`
    /// if there is nothing to carry over.
    pub(super) async fn former_to_start(
        &self,
        stored: &StoredProvider,
        held_hash: &str,
    ) -> Result<Option<StoredProvider>, StoreError> {
        let held = self
            .store()
            .oidc_held_sign_in(held_hash, now_millis())
            .await?
            .filter(|held| held.provider_id == stored.id && held.revision == stored.revision);
        if held.is_none() {
            return Ok(None);
        }
        self.former_as_provider(stored).await
    }

    /// `stored`, the provider of the handle's organization, as a sign-in to carry over
    /// goes through it: with the issuer, client and secret of the former provider, if
    /// one is kept now. The sign-in is the provider's own row's, and ends with it.
    pub(super) async fn former_as_provider(
        &self,
        stored: &StoredProvider,
    ) -> Result<Option<StoredProvider>, StoreError> {
        let kept = self.store().oidc_former_provider(now_millis()).await?;
        Ok(kept.map(|kept| StoredProvider {
            issuer: kept.issuer,
            client_id: kept.client_id,
            sealed_client_secret: kept.sealed_client_secret,
            ..stored.clone()
        }))
    }

    /// Starts a sign-in through the former provider of the organization `org_id`, from
    /// the browser whose cookie holds `held_secret`: when it comes back, the held
    /// identity is tied to the account the former provider vouches for.
    ///
    /// # Errors
    ///
    /// Returns 409 with the reason `oidc_carry_over_refused` if this browser holds no
    /// sign-in of the organization that has not ended, or no former provider is kept
    /// any more; 409 with `oidc_unavailable` if the former provider cannot be reached;
    /// and 429 for too many sign-ins from `address`.
    pub async fn start_oidc_carry_over(
        &self,
        org_id: &str,
        held_secret: &str,
        address: Option<IpAddr>,
    ) -> Result<StartedLogin, AppError> {
        if held_secret.is_empty() {
            return Err(carry_over_refused());
        }
        let held_hash = token_digest(held_secret);
        let purpose = FlowPurpose::CarryOver { held_hash };
        self.start_oidc_flow(org_id, address, purpose).await
    }

    /// Ends a sign-in through the former provider that was started to carry over, on
    /// the handle of its organization: `claims` are the former provider's, `stored`
    /// the organization's provider with the former one's issuer and client. What
    /// decides is read where it is stored (`Store::finish_oidc_carry_over`); every
    /// refusal is told the same.
    pub(super) async fn carry_over_through(
        &self,
        claims: &IdClaims,
        stored: &StoredProvider,
        held_hash: &str,
        callback: &Callback,
        user_agent: &str,
    ) -> Result<FinishedLogin, LoginFailure> {
        let refused = LoginFailure::from(Reason::OidcCarryOverRefused);
        // The browser that comes back still has the cookie of the sign-in it carries
        // over: the one that was held for it.
        let held_here = callback
            .held_secret
            .as_deref()
            .is_some_and(|secret| token_digest(secret) == held_hash);
        if !held_here {
            tracing::info!(org_id = %stored.org_id, "a sign-in to carry over came back without the held sign-in it was started for");
            return Err(refused);
        }
        let token = new_secret();
        let now = now_millis();
        let mut session = self.new_session(user_agent, now);
        let carry = CarryOver {
            provider_id: &stored.id,
            held_hash,
            former: Identity {
                issuer: &claims.issuer,
                subject: &claims.subject,
            },
            now,
        };
        let carried = self
            .store()
            .finish_oidc_carry_over(carry, &mut session, &token_digest(&token))
            .await
            .map_err(|error| logged(&error, "carrying an account over"))?;
        match carried {
            Ok(carried) => {
                tracing::info!(user_id = %carried.user_id, org_id = %stored.org_id, issuer = %carried.issuer, "an account was carried over to the organization's provider");
                Ok(FinishedLogin::SignedIn { token })
            }
            Err(refusal) => {
                tracing::info!(org_id = %stored.org_id, ?refusal, "a sign-in through the former provider carried nothing over");
                Err(refused)
            }
        }
    }

    /// Ends the sign-in held for the browser whose cookie holds `held_secret`, of the
    /// organization `org_id`, as a first sign-in through the provider ends: for
    /// somebody who had no account. The account is made by the rules of any first
    /// sign-in, and the token of its session returned. The held sign-in is taken,
    /// whatever comes of it.
    ///
    /// # Errors
    ///
    /// Returns 409 with the reason `oidc_carry_over_refused` if this browser holds no
    /// sign-in of the organization that has not ended, or the provider changed since;
    /// 409 with the reason a first sign-in is refused with (`oidc_join_closed`,
    /// `oidc_email_missing`, `oidc_email_unverified`, `oidc_email_taken`,
    /// `oidc_unavailable`); and 429 for too many sign-ins from `address`.
    ///
    /// The account that is made comes into the organization and is given its welcome
    /// board (`App::welcome_member`, by `accept_language`).
    pub async fn finish_held_oidc_login(
        &self,
        org_id: &str,
        held_secret: &str,
        address: Option<IpAddr>,
        user_agent: &str,
        accept_language: Option<&str>,
    ) -> Result<String, AppError> {
        drop(
            self.login_limiter()
                .admit_registration(address, Instant::now())
                .map_err(|RetryAfter(wait)| AppError::rate_limited(wait))?,
        );
        if held_secret.is_empty() || !self.is_organization_in_use(org_id).await? {
            return Err(carry_over_refused());
        }
        let org = self
            .enter_org_for_sign_in(org_id)
            .ok_or_else(carry_over_refused)?;
        let now = now_millis();
        let held = org
            .store()
            .take_oidc_held_sign_in(&token_digest(held_secret), now)
            .await?
            .ok_or_else(carry_over_refused)?;
        let stored = org
            .store()
            .oidc_provider()
            .await?
            .filter(|stored| {
                stored.enabled && stored.id == held.provider_id && stored.revision == held.revision
            })
            .ok_or_else(carry_over_refused)?;
        let text = |text: String| (!text.is_empty()).then_some(text);
        let claims = IdClaims {
            issuer: held.issuer,
            subject: held.subject,
            email: text(held.email),
            email_verified: Some(!held.email_unverified),
            name: text(held.name),
            // A held sign-in keeps the name the token gave whole, not its parts: the
            // account it makes has its parts to fill in.
            given_name: None,
            family_name: None,
            preferred_username: text(held.preferred_username),
        };
        let token = new_secret();
        let mut session = self.new_session(user_agent, now);
        let ended = Box::pin(org.sign_in_through(
            &claims,
            &stored,
            Ending::SignIn,
            &mut session,
            &token_digest(&token),
        ))
        .await
        .map_err(|LoginFailure::Reason(reason)| {
            tracing::info!(org_id, failure = %reason.name(), "a held sign-in did not end in an account");
            AppError::conflict("the sign-in did not end in an account").with_reason(reason)
        })?;
        if ended.joined {
            org.welcome_newcomer(&session.user_id, accept_language)
                .await;
        }
        Ok(token)
    }
}

/// The one refusal of everything that keeps an account from being carried over, and
/// of a held sign-in that is not there: who asks is not told which it was.
pub(super) fn carry_over_refused() -> AppError {
    AppError::conflict(
        "nothing is held for this browser to go on with, or the former provider is kept no more: \
         sign in again from the login page",
    )
    .with_reason(Reason::OidcCarryOverRefused)
}
