//! The pending settings of an organization's provider, as its administrators keep them:
//! saved aside while the organization takes no password, tried by a sign-in through
//! them that makes them the provider's when it ends well, or thrown away. Settings that
//! do not work so lock nobody out: until the trial ends well, people sign in through
//! the provider as it was. The trial's start and end are with the other sign-ins, in
//! `oidc.rs`; the store's part is `Store::finish_oidc_trial`.

use std::net::IpAddr;

use fvoci_model::{OidcProvider, Session};
use fvoci_store::{
    Applied, Binding, FlowPurpose, Identity, LinkingSession, PendingSettings, StoredProvider,
    Trial, Tried,
};

use super::App;
use super::oidc::{LoginFailure, StartedLogin, TrialEnd, logged, refused};
use super::oidc_settings::Beside;
use super::sso;
use crate::auth::now_millis;
use crate::auth::oidc::provider::IdClaims;
use crate::auth::secret::token_digest;
use crate::error::{AppError, Reason};

/// How long before a trial comes back its administrator may have signed in through
/// the provider, at the most, for the trial to tie an identity that is nobody's to
/// their account, in milliseconds: who only holds an older session binds nothing.
pub(super) const FRESH_SIGN_IN_MILLIS: i64 = 5 * 60 * 1000;

/// Refuses, with a 403 of the reason `sign_in_again`, a session that is not the
/// provider's word of a moment ago: one made with a password, or made through a
/// provider more than [`FRESH_SIGN_IN_MILLIS`] before `now`. It is what vouches for an
/// account that has no password to give where one would be asked: when it asks to be
/// deleted, and when it sets its first password. `to` ends the refusal's sentence with
/// what was asked for. The one place the rule is written: who holds an older session
/// of such an account does neither.
pub(super) fn require_fresh_sign_in(session: &Session, now: i64, to: &str) -> Result<(), AppError> {
    let made_just_now =
        session.login.is_some() && now.saturating_sub(session.created_at) <= FRESH_SIGN_IN_MILLIS;
    if made_just_now {
        return Ok(());
    }
    let refusal = format!("sign in through the provider again to {to}");
    Err(AppError::forbidden(refusal).with_reason(Reason::SignInAgain))
}

impl App {
    /// Saves the sign-in of `next` (its issuer, client and secret) as the pending
    /// settings of `enforced`, the provider as it was read, which takes no password;
    /// what else `next` changes (whether newcomers join) is stored at once.
    ///
    /// # Errors
    ///
    /// Returns 409 (`sso_enforced`, naming `enabled`) for a provider switched off with
    /// it, 409 if the provider changed meanwhile, and the store's error.
    pub(super) async fn save_pending(
        &self,
        session: &Session,
        enforced: &StoredProvider,
        next: StoredProvider,
    ) -> Result<OidcProvider, AppError> {
        if !next.enabled {
            return Err(sso::enforced().with_field("enabled"));
        }
        let settings = PendingSettings {
            issuer: next.issuer.clone(),
            client_id: next.client_id.clone(),
            sealed_client_secret: next.sealed_client_secret.clone(),
        };
        let (stored, pending) = self
            .store()
            .put_oidc_pending_provider(&settings, &next, enforced, &session.user_id)
            .await?
            .ok_or_else(|| {
                AppError::conflict("the provider was changed meanwhile; read it and try again")
            })?;
        tracing::info!(org_id = %stored.org_id, issuer = %pending.issuer, "settings of a provider were saved aside, to be tried");
        let former = self.store().oidc_former_provider(now_millis()).await?;
        let beside = Beside {
            pending: Some(&pending),
            former: former.as_ref(),
        };
        Ok(self.provider_view(&stored, beside, session))
    }

    /// Throws the pending settings of the handle's organization's provider away, as
    /// the administrator `actor_id`. Throwing none away changes nothing.
    ///
    /// # Errors
    ///
    /// Returns the store's error.
    pub async fn discard_oidc_pending(&self, actor_id: &str) -> Result<(), AppError> {
        if self.store().discard_oidc_pending_provider(actor_id).await? {
            tracing::info!("the pending settings of a provider were thrown away");
        }
        Ok(())
    }

    /// Starts a sign-in through the pending settings of the provider of the
    /// organization `org_id`, which the handle reaches, for the administrator of
    /// `session`: when it ends well, in their account, they become the provider's, and
    /// the session it makes proves them.
    ///
    /// # Errors
    ///
    /// Returns 409 with the reason `oidc_unavailable` if there are none, the provider is
    /// off, or their issuer cannot be reached, and 429 for too many sign-ins from
    /// `address`.
    pub async fn start_oidc_trial(
        &self,
        org_id: &str,
        session: &Session,
        address: Option<IpAddr>,
    ) -> Result<StartedLogin, AppError> {
        // The settings it tries are those pending when it starts: their time is
        // stamped then (`App::provider_to_start`).
        let purpose = FlowPurpose::Trial {
            pending_at: 0,
            user_id: session.user_id.clone(),
            session_id: session.id.clone(),
        };
        self.start_oidc_flow(org_id, address, purpose).await
    }

    /// The trial that `end` ends, of the pending settings of `stored`, as the store is
    /// asked to end it.
    pub(super) fn trial_of<'a>(&self, stored: &'a StoredProvider, end: TrialEnd<'a>) -> Trial<'a> {
        Trial {
            provider_id: &stored.id,
            pending_at: end.pending_at,
            trier_id: end.trier_id,
            keep_former: self.limits().former_provider_kept,
        }
    }

    /// Ends a trial of the pending settings `stored` (the provider with them) that
    /// vouched, by `claims`, for an identity tied to no account, on the handle of
    /// its organization: the identity is tied to the account of the administrator
    /// who started the trial, and the settings applied, if the browser came back
    /// with the session the trial was started from and that session was made through
    /// the provider as it is no more than [`FRESH_SIGN_IN_MILLIS`] before this
    /// moment: the trial is to come back within them. What decides is read where it
    /// is stored (`Store::finish_oidc_trial`). No address is compared: who signed in
    /// through the provider a moment ago is who tries.
    pub(super) async fn bind_by_trial(
        &self,
        claims: &IdClaims,
        stored: &StoredProvider,
        end: TrialEnd<'_>,
        session: &mut Session,
        digest: &str,
    ) -> Result<Applied, LoginFailure> {
        let Some(token) = end.came_with.filter(|token| !token.is_empty()) else {
            tracing::info!(org_id = %stored.org_id, "a trial of pending settings vouched for an identity tied to no account, and came back without a session");
            return Err(Reason::OidcTrialStale.into());
        };
        let now = now_millis();
        let binding = Binding {
            identity: Identity {
                issuer: &claims.issuer,
                subject: &claims.subject,
            },
            provider_email: claims.email.as_deref().unwrap_or_default(),
            session: LinkingSession {
                session_id: end.session_id,
                token_digest: &token_digest(token),
                now,
                idle_since: now.saturating_sub(self.config().session_idle_millis()),
            },
            made_since: now.saturating_sub(FRESH_SIGN_IN_MILLIS),
        };
        let applied = self
            .store()
            .finish_oidc_trial(
                self.trial_of(stored, end),
                Tried::Unbound(binding),
                session,
                digest,
            )
            .await
            .map_err(|error| logged(&error, "storing a trial"))?
            .map_err(|refusal| refused(refusal, stored))?;
        tracing::info!(user_id = %end.trier_id, org_id = %stored.org_id, issuer = %claims.issuer, "an administrator tied their identity at another issuer to their account by a trial of its settings");
        Ok(applied)
    }

    /// Follows the pending settings that became the provider's: the sessions of the
    /// issuer before end with their connections, and where the organization takes
    /// only the provider, now another one, who it shuts out is shut out.
    pub(super) async fn follow_applied(&self, applied: Applied) {
        let Applied {
            before,
            provider,
            ended,
            joined: _,
        } = applied;
        tracing::info!(org_id = %provider.org_id, issuer = %provider.issuer, "the pending settings of a provider were tried and taken");
        self.ws().close(ended.into_scope());
        if provider.sso_required {
            if provider.id != before.id {
                self.collab().put_out_shut_out(self, &provider.id).await;
            }
            self.end_subscriptions_shut_out(&provider).await;
        }
    }
}

#[cfg(test)]
mod tests {
    use fvoci_model::{LoginOrigin, Session};

    use super::{FRESH_SIGN_IN_MILLIS, require_fresh_sign_in};
    use crate::error::Reason;

    const NOW: i64 = 1_000_000_000;

    /// A session made at `created_at` through a provider.
    fn through_a_provider(created_at: i64) -> Session {
        let login = LoginOrigin {
            provider_id: "provider".to_owned(),
            org_id: "organization".to_owned(),
            issuer: "https://provider.example".to_owned(),
            subject: None,
            revision: None,
        };
        Session {
            created_at,
            login: Some(login),
            ..Session::default()
        }
    }

    /// A session made at `created_at` with a password.
    fn with_a_password(created_at: i64) -> Session {
        Session {
            created_at,
            ..Session::default()
        }
    }

    /// A session vouches while it was made through a provider no more than five
    /// minutes ago, to the millisecond; one made with a password never does, and each
    /// refusal carries the reason a client reads and says what was asked for.
    #[test]
    fn a_session_vouches_while_it_is_the_providers_word_of_a_moment_ago() {
        let fresh = |session: &Session| require_fresh_sign_in(session, NOW, "set a password");
        assert!(fresh(&through_a_provider(NOW)).is_ok());
        assert!(fresh(&through_a_provider(NOW - FRESH_SIGN_IN_MILLIS)).is_ok());
        for stale in [
            through_a_provider(NOW - FRESH_SIGN_IN_MILLIS - 1),
            through_a_provider(0),
            with_a_password(NOW),
        ] {
            let refused = fresh(&stale).expect_err("the session vouches for nothing");
            assert_eq!(refused.reason(), Some(Reason::SignInAgain), "{stale:?}");
            assert_eq!(
                refused.to_string(),
                "sign in through the provider again to set a password"
            );
        }
    }
}
