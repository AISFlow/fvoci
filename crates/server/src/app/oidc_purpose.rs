//! What a sign-in through a provider is for ([`FlowPurpose`]), read in one place, from
//! the top down: what it goes through when it starts and when it comes back, how it
//! ends, where the browser goes after it, and how it is refused.
//!
//! A sign-in signs in (`SignIn`), links an identity to the account of the session it
//! is started from (`Link`, `oidc_link.rs`), tries the pending settings
//! (`Trial`, `oidc_pending.rs`), or carries a held sign-in over through the former
//! provider (`CarryOver`, `oidc_carry.rs`). The purpose is stored with the sign-in and
//! read again when it comes back, a request later.

use fvoci_store::{FlowPurpose, LoginFlow, StoreError, StoredProvider};

use super::App;
use super::oidc::{
    BackTo, Callback, Ending, FinishedLogin, LoginFailure, TrialEnd, logged, trying,
};
use super::oidc_carry::carry_over_refused;
use crate::auth::now_millis;
use crate::auth::oidc::provider::IdClaims;
use crate::auth::secret::{new_secret, token_digest};
use crate::error::{AppError, Reason};

/// What the purpose of a sign-in decides of it apart from what it goes through.
pub(super) trait Purpose {
    /// Where the browser goes when the sign-in ends, well or not: a trial goes back to
    /// the organization's settings it was started from, any other to the login page.
    fn back_to(&self) -> BackTo;

    /// The refusal of a sign-in that starts with nothing to go through: no provider
    /// that is on, no pending settings to try, or nothing to carry over (told as every
    /// refusal of carrying over is).
    fn nothing_to_start(&self) -> AppError;

    /// The failure of a sign-in whose provider is gone when it comes back: switched
    /// off, removed or another one. What a sign-in to carry over went through (the
    /// former provider too) is told as every refusal of carrying over is.
    fn gone(&self) -> LoginFailure;

    /// Whether it ends only for a browser that comes back with a session: a link,
    /// whose session is the account the identity is tied to.
    fn needs_session(&self) -> bool;

    /// The digest of the held sign-in it carries over, if it is to carry one over:
    /// that one is dropped when it fails.
    fn held_hash(&self) -> Option<&str>;
}

impl Purpose for FlowPurpose {
    fn back_to(&self) -> BackTo {
        match self {
            Self::Trial { .. } => BackTo::Organization,
            Self::SignIn | Self::Link { .. } | Self::CarryOver { .. } => BackTo::LoginPage,
        }
    }

    fn nothing_to_start(&self) -> AppError {
        match self {
            Self::SignIn | Self::Link { .. } | Self::Trial { .. } => {
                AppError::conflict("no sign-in through the organization's provider can start now")
                    .with_reason(Reason::OidcUnavailable)
            }
            Self::CarryOver { .. } => carry_over_refused(),
        }
    }

    fn gone(&self) -> LoginFailure {
        match self {
            Self::CarryOver { .. } => Reason::OidcCarryOverRefused.into(),
            Self::SignIn | Self::Link { .. } | Self::Trial { .. } => Reason::OidcUnavailable.into(),
        }
    }

    fn needs_session(&self) -> bool {
        matches!(self, Self::Link { .. })
    }

    fn held_hash(&self) -> Option<&str> {
        match self {
            Self::CarryOver { held_hash } => Some(held_hash),
            Self::SignIn | Self::Link { .. } | Self::Trial { .. } => None,
        }
    }
}

impl App {
    /// What a sign-in for `purpose` goes through as it starts, on the handle of its
    /// organization: the provider, if it is on; for a trial, with its pending
    /// settings, if there are, whose time `purpose` then names (`pending_at`): a trial
    /// tries the settings pending when it starts; to carry over, the former provider,
    /// if one is kept and the browser's sign-in is held. `None` if there is nothing to
    /// go through.
    pub(super) async fn provider_to_start(
        &self,
        purpose: &mut FlowPurpose,
    ) -> Result<Option<StoredProvider>, StoreError> {
        let Some(stored) = self
            .store()
            .oidc_provider()
            .await?
            .filter(|stored| stored.enabled)
        else {
            return Ok(None);
        };
        Ok(match purpose {
            FlowPurpose::SignIn | FlowPurpose::Link { .. } => Some(stored),
            FlowPurpose::CarryOver { held_hash } => {
                self.former_to_start(&stored, held_hash).await?
            }
            FlowPurpose::Trial { pending_at, .. } => self
                .store()
                .oidc_pending_provider()
                .await?
                .and_then(|pending| {
                    *pending_at = pending.created_at;
                    trying(&stored, &pending)
                }),
        })
    }

    /// What a sign-in that came back goes through, on the handle of its organization:
    /// the provider it was started through, still on; for a trial, with the pending
    /// settings it was started for, and no other; to carry over, with the former
    /// provider's issuer, client and secret. `None` if that is gone.
    pub(super) async fn provider_for(
        &self,
        flow: &LoginFlow,
    ) -> Result<Option<StoredProvider>, LoginFailure> {
        let Some(stored) = self
            .store()
            .oidc_provider()
            .await
            .map_err(|error| logged(&error, "reading the provider"))?
            .filter(|stored| stored.enabled && stored.id == flow.provider_id)
        else {
            return Ok(None);
        };
        let pending_at = match &flow.purpose {
            FlowPurpose::SignIn | FlowPurpose::Link { .. } => return Ok(Some(stored)),
            FlowPurpose::CarryOver { .. } => {
                return self
                    .former_as_provider(&stored)
                    .await
                    .map_err(|error| logged(&error, "reading the former provider"));
            }
            FlowPurpose::Trial { pending_at, .. } => pending_at,
        };
        Ok(self
            .store()
            .oidc_pending_provider()
            .await
            .map_err(|error| logged(&error, "reading the provider"))?
            .filter(|pending| pending.created_at == *pending_at)
            .and_then(|pending| trying(&stored, &pending)))
    }

    /// Ends a sign-in that came back through `stored` with the checked `claims`, on the
    /// handle of its organization, as its purpose says: links, carries over, signs in
    /// (or holds what the provider said, while the former provider is kept), or tries
    /// the pending settings, which become the provider's.
    pub(super) async fn end_flow(
        &self,
        flow: &LoginFlow,
        claims: &IdClaims,
        stored: &StoredProvider,
        callback: &Callback,
        user_agent: &str,
    ) -> Result<FinishedLogin, LoginFailure> {
        let ending = match &flow.purpose {
            FlowPurpose::Link { session_id } => {
                self.link_through(claims, stored, session_id, callback)
                    .await?;
                return Ok(FinishedLogin::Linked);
            }
            FlowPurpose::CarryOver { held_hash } => {
                return self
                    .carry_over_through(claims, stored, held_hash, callback, user_agent)
                    .await;
            }
            FlowPurpose::SignIn => {
                // Somebody without an account, while the former provider is kept, is
                // asked whether they had one before any is made.
                if let Some(secret) = self.hold_if_moving(claims, stored).await? {
                    return Ok(FinishedLogin::Held { secret });
                }
                Ending::SignIn
            }
            FlowPurpose::Trial {
                pending_at,
                user_id,
                session_id,
            } => Ending::Trial(TrialEnd {
                pending_at: *pending_at,
                trier_id: user_id,
                session_id,
                came_with: callback.session_token.as_deref(),
            }),
        };
        let token = new_secret();
        let mut session = self.new_session(user_agent, now_millis());
        // Boxed: making the account and applying the pending settings make a large
        // future, which every caller of a callback would otherwise carry.
        let ended = Box::pin(self.sign_in_through(
            claims,
            stored,
            ending,
            &mut session,
            &token_digest(&token),
        ))
        .await?;
        if ended.joined {
            self.welcome_newcomer(&session.user_id, callback.accept_language.as_deref())
                .await;
        }
        Ok(match ended.applied {
            Some(applied) => {
                self.follow_applied(applied).await;
                FinishedLogin::Applied { token }
            }
            None => FinishedLogin::SignedIn { token },
        })
    }
}
