//! An organization's provider as its administrators set it: reading it, saving it
//! (after reading the discovery document of a new issuer), removing it, and the client
//! secret, which is sealed for the organization and never answered. Signing in through
//! it is in `oidc.rs`; its pending settings, in `oidc_pending.rs`. The provider it took
//! the place of is kept beside it for a while (`former`), and ended here before its
//! time.

use fvoci_model::{
    OidcFormerProvider, OidcPendingProvider, OidcProvider, OidcProviderSettings, RefusalReason,
    Refusals, Session,
};
use fvoci_store::{FormerEnd, FormerProvider, PendingProvider, Removal, StoredProvider};

use super::App;
use super::oidc_pending::FRESH_SIGN_IN_MILLIS;
use super::sso;
use crate::auth::now_millis;
use crate::auth::oidc::provider;
use crate::auth::sealed::SecretKey;
use crate::error::{AppError, Reason};

impl App {
    /// The provider of the handle's organization, for its administrators.
    ///
    /// # Errors
    ///
    /// Returns the store's error.
    pub async fn oidc_provider(&self, session: &Session) -> Result<Option<OidcProvider>, AppError> {
        let Some(stored) = self.store().oidc_provider().await? else {
            return Ok(None);
        };
        self.provider_as_read(&stored, session).await.map(Some)
    }

    /// Sets the provider of the organization `org_id`, which the handle reaches, after
    /// reading the discovery document of an issuer that is new to the organization, to
    /// see that it is what it says.
    ///
    /// # Errors
    ///
    /// Returns 400 for settings that are refused or an issuer that is not verified,
    /// 409 (`secret_key_required`) if the server has no key to seal the secret with,
    /// 409 (`sso_unproven`, `sso_enforced`) for what the organization's taking no
    /// password does not let `session` do (`sso::check_change`), and the store's
    /// error.
    pub async fn put_oidc_provider(
        &self,
        org_id: &str,
        session: &Session,
        settings: &OidcProviderSettings,
    ) -> Result<OidcProvider, AppError> {
        settings.validate()?;
        let Some(key) = &self.config().secret_key else {
            return Err(AppError::conflict(
                "the server has no secret_key to seal the client secret with",
            )
            .with_reason(Reason::SecretKeyRequired));
        };
        let existing = self.store().oidc_provider().await?;
        // The issuer that stays was read when it was set. Not read again: a change of
        // what else there is, taking a password again among it, does not wait for a
        // provider that does not answer.
        let issuer_stays = existing
            .as_ref()
            .is_some_and(|existing| existing.issuer == settings.issuer);
        if !issuer_stays
            && let Err(error) = provider::discover(self.oidc_fetcher(), &settings.issuer).await
        {
            tracing::info!(%error, "the issuer of a provider could not be verified");
            return Err(AppError::bad_request(format_args!(
                "the issuer could not be verified: {error}"
            ))
            .with_reason(Reason::OidcIssuerUnverified)
            .with_field("issuer"));
        }
        let sealed_client_secret = sealed_secret(key, org_id, settings, existing.as_ref())?;
        let next = next_provider(org_id, settings, existing.as_ref(), sealed_client_secret);
        // While the organization takes no password and goes on taking none, other
        // settings of signing in are kept aside, to be tried before they are taken.
        if let Some(enforced) = existing
            .as_ref()
            .filter(|existing| existing.sso_required && next.sso_required)
            && !is_same_sign_in(enforced, &next)
        {
            return self.save_pending(session, enforced, next).await;
        }
        // Decided from the provider as it was read; the store takes the change only if
        // that is still the provider, so no other change slips between the two.
        sso::check_change(session, existing.as_ref(), &next)?;
        let fvoci_store::ProviderPut {
            provider: stored,
            ended,
        } = self
            .store()
            .put_oidc_provider_as_read(
                &next,
                existing.as_ref(),
                &session.user_id,
                self.limits().former_provider_kept,
            )
            .await?
            // Changed by another request since it was read here: what was kept from it
            // may be kept no more.
            .ok_or_else(|| {
                AppError::conflict("the provider was changed meanwhile; read it and try again")
            })?;
        tracing::info!(org_id, issuer = %stored.issuer, "the provider of an organization was set");
        self.ws().close(ended.into_scope());
        sso::record_if_changed(existing.as_ref(), &stored, &session.user_id);
        if stored.sso_required {
            if !existing
                .as_ref()
                .is_some_and(|existing| existing.sso_required)
            {
                // Begun now: who it shuts out is out of the rooms of its cards' bodies
                // before the request answers. A connection in one may follow no workspace.
                self.collab().put_out_shut_out(self, &stored.id).await;
            }
            self.end_subscriptions_shut_out(&stored).await;
        }
        // Pending settings go with a change of what signing in takes.
        self.provider_as_read(&stored, session).await
    }

    /// Removes the provider of the handle's organization, and the sign-ins under way
    /// through it.
    ///
    /// # Errors
    ///
    /// Returns 409 with the reason `sso_enforced` while the organization takes no
    /// password, and the store's error.
    pub async fn delete_oidc_provider(&self, actor_id: &str) -> Result<(), AppError> {
        match self.store().delete_oidc_provider(actor_id).await? {
            Removal::Removed { ended, .. } => {
                self.ws().close(ended.into_scope());
                tracing::info!("the provider of an organization was removed");
            }
            Removal::NoneThere => {}
            Removal::Enforced => return Err(sso::enforced()),
        }
        Ok(())
    }

    /// Ends the keeping of the former provider of the handle's organization before
    /// its time, as the administrator `actor_id`, if it is the one they read: the one
    /// kept since `kept_since` (its `createdAt`). From then on nobody carries an
    /// account over from it. Ending none changes nothing.
    ///
    /// # Errors
    ///
    /// Returns 409 if another former provider is kept than the one read (the
    /// organization moved again since), and the store's error.
    pub async fn end_oidc_former_provider(
        &self,
        actor_id: &str,
        kept_since: i64,
    ) -> Result<(), AppError> {
        match self
            .store()
            .end_oidc_former_provider(actor_id, kept_since, now_millis())
            .await?
        {
            FormerEnd::Ended => {
                tracing::info!("the keeping of a former provider was ended before its time");
                Ok(())
            }
            FormerEnd::NoneKept => Ok(()),
            FormerEnd::Another => Err(AppError::conflict(
                "another former provider is kept than the one read; read the provider and try \
                 again",
            )),
        }
    }

    /// `stored` as the administrators read it, with what is kept beside it now: its
    /// pending settings, and the provider it took the place of.
    async fn provider_as_read(
        &self,
        stored: &StoredProvider,
        session: &Session,
    ) -> Result<OidcProvider, AppError> {
        let pending = self.store().oidc_pending_provider().await?;
        let former = self.store().oidc_former_provider(now_millis()).await?;
        let beside = Beside {
            pending: pending.as_ref(),
            former: former.as_ref(),
        };
        Ok(self.provider_view(stored, beside, session))
    }

    pub(super) fn provider_view(
        &self,
        stored: &StoredProvider,
        beside: Beside<'_>,
        session: &Session,
    ) -> OidcProvider {
        let Beside { pending, former } = beside;
        OidcProvider {
            issuer: stored.issuer.clone(),
            client_id: stored.client_id.clone(),
            has_client_secret: !stored.sealed_client_secret.is_empty(),
            auto_join: stored.auto_join,
            enabled: stored.enabled,
            sso_required: stored.sso_required,
            can_require_sso: sso::proves(session, stored),
            redirect_uri: self.oidc_redirect_uri(&stored.org_id),
            created_at: stored.created_at,
            updated_at: stored.updated_at,
            pending: pending
                .filter(|pending| pending.provider_id == stored.id)
                .map(|pending| OidcPendingProvider {
                    issuer: pending.issuer.clone(),
                    client_id: pending.client_id.clone(),
                    has_client_secret: !pending.sealed_client_secret.is_empty(),
                    created_at: pending.created_at,
                    // Judged by the server's clock: past it, the page is told none.
                    binds_identity_until: (pending.issuer != stored.issuer
                        && sso::proves(session, stored))
                    .then(|| session.created_at.saturating_add(FRESH_SIGN_IN_MILLIS))
                    .filter(|until| *until > now_millis()),
                }),
            former: former.map(|former| OidcFormerProvider {
                issuer: former.issuer.clone(),
                created_at: former.created_at,
                expires_at: former.expires_at,
            }),
        }
    }
}

/// What is kept beside an organization's provider, as its administrators read it with
/// the provider.
#[derive(Debug, Clone, Copy)]
pub(super) struct Beside<'a> {
    /// The pending settings, if there are.
    pub(super) pending: Option<&'a PendingProvider>,
    /// The former provider, if one is kept now.
    pub(super) former: Option<&'a FormerProvider>,
}

/// The provider `settings` ask for in the organization `org_id`, in place of
/// `existing`, with the secret as it is to be stored. What `settings` leave out stays
/// as it is stored, also for another client: newcomers join and signing in is on
/// when the organization has no provider, and it takes a password. The identifier,
/// the times and the revision are the store's.
fn next_provider(
    org_id: &str,
    settings: &OidcProviderSettings,
    existing: Option<&StoredProvider>,
    sealed_client_secret: String,
) -> StoredProvider {
    StoredProvider {
        id: String::new(),
        org_id: org_id.to_owned(),
        issuer: settings.issuer.clone(),
        client_id: settings.client_id.clone(),
        sealed_client_secret,
        auto_join: settings
            .auto_join
            .or(existing.map(|existing| existing.auto_join))
            .unwrap_or(true),
        enabled: settings
            .enabled
            .or(existing.map(|existing| existing.enabled))
            .unwrap_or(true),
        created_at: 0,
        updated_at: 0,
        // Kept for another client too, which is then refused.
        sso_required: settings
            .sso_required
            .or(existing.map(|existing| existing.sso_required))
            .unwrap_or(false),
        revision: 0,
    }
}

/// Whether signing in through `next` takes the same issuer, client and secret as
/// through `stored`.
fn is_same_sign_in(stored: &StoredProvider, next: &StoredProvider) -> bool {
    stored.issuer == next.issuer
        && stored.client_id == next.client_id
        && stored.sealed_client_secret == next.sealed_client_secret
}

/// The client secret to store with the settings, sealed for the organization: the one
/// given; or, left out or given as it is stored, the stored one while the issuer and
/// the client stay, so that the provider's revision stays with it.
///
/// # Errors
///
/// Returns 400 naming `clientSecret` if it is left out for another issuer or client
/// while one is stored: the stored secret is the old client's.
fn sealed_secret(
    key: &SecretKey,
    org_id: &str,
    settings: &OidcProviderSettings,
    existing: Option<&StoredProvider>,
) -> Result<String, AppError> {
    Ok(match settings.client_secret.as_deref() {
        // The secret stays with the client it was given for: one that went on to
        // another issuer or client would be sent to it.
        None => match existing {
            Some(existing)
                if existing.issuer == settings.issuer
                    && existing.client_id == settings.client_id =>
            {
                existing.sealed_client_secret.clone()
            }
            Some(existing) if !existing.sealed_client_secret.is_empty() => {
                let mut refused = Refusals::new();
                refused.refuse(
                    "clientSecret",
                    RefusalReason::Required,
                    "clientSecret is required with another issuer or client",
                );
                return Err(refused.into());
            }
            _ => String::new(),
        },
        Some("") => String::new(),
        Some(secret) => match existing {
            Some(existing) if is_stored_secret(key, existing, settings, secret) => {
                existing.sealed_client_secret.clone()
            }
            _ => key.seal(
                secret,
                &secret_context(org_id, &settings.issuer, &settings.client_id),
            ),
        },
    })
}

/// Whether `secret` is the one `existing` keeps for the issuer and client of
/// `settings`: given again, it is no change.
fn is_stored_secret(
    key: &SecretKey,
    existing: &StoredProvider,
    settings: &OidcProviderSettings,
    secret: &str,
) -> bool {
    existing.issuer == settings.issuer
        && existing.client_id == settings.client_id
        && !existing.sealed_client_secret.is_empty()
        && key
            .open(
                &existing.sealed_client_secret,
                &secret_context(&existing.org_id, &existing.issuer, &existing.client_id),
            )
            .is_ok_and(|stored| stored == secret)
}

/// What a sealed client secret is bound to: the organization, the issuer and the client
/// it is for. Moved to another row, or kept for another issuer, it does not open.
pub fn secret_context(org_id: &str, issuer: &str, client_id: &str) -> Vec<u8> {
    // Each part is length-prefixed, so no two triples give the same bytes; the length
    // is eight bytes on every platform.
    let mut context = b"oidc client secret".to_vec();
    for part in [org_id, issuer, client_id] {
        let length = u64::try_from(part.len()).unwrap_or(u64::MAX);
        context.extend_from_slice(&length.to_be_bytes());
        context.extend_from_slice(part.as_bytes());
    }
    context
}
