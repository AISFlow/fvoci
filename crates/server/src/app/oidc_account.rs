//! The account of somebody who signs in through a provider for the first time: made
//! under a user name of what the provider says, a member of the organization; and
//! what a sign-in knows of the account it ends in, with the address the provider
//! vouches for. The sign-in is in `oidc.rs`.

use fvoci_model::{PersonName, Session, User, is_email_valid, new_id};
use fvoci_store::{Identity, OidcAccount, StoreError, StoredProvider, Taken};

use super::App;
use super::oidc::{Ended, Ending, LoginFailure, logged, refused};
use crate::auth::oidc::provider::IdClaims;
use crate::auth::secret::new_secret;
use crate::error::Reason;

/// The longest user name made from what a provider says.
const USERNAME_MAX_CHARS: usize = 64;

impl App {
    /// Makes the account of a person who signs in for the first time, a member of the
    /// handle's organization and its default workspace, and signs in to it.
    pub(super) async fn make_account(
        &self,
        newcomer: Newcomer<'_>,
        stored: &StoredProvider,
        ending: Ending<'_>,
        session: &mut Session,
        digest: &str,
    ) -> Result<Ended, LoginFailure> {
        let Newcomer {
            claims,
            identity,
            email,
        } = newcomer;
        let base = username_base(claims, email);
        for attempt in 0..3 {
            let username = if attempt == 0 {
                base.clone()
            } else {
                format!("{base}-{}", new_secret()[..6].to_ascii_lowercase())
            };
            match self.store().get_user_by_username(&username).await {
                Ok(_) => continue,
                Err(error) if error.is_not_found() => {}
                Err(error) => return Err(logged(&error, "looking up a user name")),
            }
            // No address of its own: the provider's is kept with the identity.
            let mut user = User {
                id: new_id(),
                username,
                name: PersonName::from_provider(
                    claims.name.as_deref(),
                    claims.given_name.as_deref(),
                    claims.family_name.as_deref(),
                ),
                ..User::default()
            };
            let account = OidcAccount::New {
                user: &mut user,
                identity,
                provider_email: email,
            };
            match self
                .finish_in_store(stored, ending, account, session, digest)
                .await
            {
                Ok(Ok((_, applied))) => {
                    self.metrics().registered();
                    tracing::info!(user_id = %user.id, org_id = %stored.org_id, issuer = %stored.issuer, "an account was made at its first sign-in through a provider, and joined the organization");
                    return Ok(Ended {
                        applied,
                        joined: true,
                    });
                }
                Ok(Err(refusal)) => return Err(refused(refusal, stored)),
                Err(StoreError::Conflict(Taken::Username)) => {}
                // Another sign-in of the same person made the account a moment ago.
                Err(StoreError::Conflict(Taken::Identity)) => {
                    let user = self
                        .store()
                        .user_of_identity(identity)
                        .await
                        .map_err(|error| logged(&error, "looking up an identity"))?
                        .ok_or(Reason::OidcUnavailable)?;
                    let found = FoundAccount {
                        user: &user,
                        identity,
                        proved_email: Some(email),
                    };
                    return self.let_in(found, stored, ending, session, digest).await;
                }
                // A registration at the address was confirmed while the person was
                // at the provider: the account is theirs to sign in to and link.
                Err(StoreError::Conflict(Taken::Email)) => {
                    return Err(Reason::OidcEmailTaken.into());
                }
                Err(error) => return Err(logged(&error, "storing a new account")),
            }
        }
        tracing::warn!("no free user name was found for a new account");
        Err(Reason::OidcUnavailable.into())
    }
}

/// Who signs in through a provider for the first time, and has no account yet.
#[derive(Debug, Clone, Copy)]
pub(super) struct Newcomer<'a> {
    /// What the provider says of them.
    pub(super) claims: &'a IdClaims,
    /// Who the provider vouches for.
    pub(super) identity: Identity<'a>,
    /// The address the provider vouches for.
    pub(super) email: &'a str,
}

/// The account a sign-in through a provider ends in, as it was found.
#[derive(Debug, Clone, Copy)]
pub(super) struct FoundAccount<'a> {
    /// The account of the identity.
    pub(super) user: &'a User,
    /// Who the provider vouches for.
    pub(super) identity: Identity<'a>,
    /// The address the provider vouches for at this sign-in, if it does.
    pub(super) proved_email: Option<&'a str>,
}

/// The address a provider vouches for: one it gives, that reads as an address, and
/// that it explicitly marks as verified (`email_verified == Some(true)`, `H-8`).
pub(super) fn proved_address(claims: &IdClaims) -> Option<&str> {
    claims
        .email
        .as_deref()
        .map(str::trim)
        .filter(|email| is_email_valid(email) && claims.email_verified == Some(true))
}

/// The user name of a new account, before it is seen to be free: what the provider
/// says the person goes by, or else the part of the email address before the `@`, of
/// letters, digits, `.`, `_` and `-` only.
fn username_base(claims: &IdClaims, email: &str) -> String {
    let clean = |text: &str| -> String {
        text.chars()
            .filter(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-'))
            .take(USERNAME_MAX_CHARS)
            .collect()
    };
    let local = email.split('@').next().unwrap_or_default();
    [
        claims.preferred_username.as_deref().unwrap_or_default(),
        local,
    ]
    .into_iter()
    .map(clean)
    .find(|name| !name.is_empty())
    .unwrap_or_else(|| "user".to_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn claims(preferred: Option<&str>) -> IdClaims {
        IdClaims {
            issuer: String::new(),
            subject: String::new(),
            email: None,
            email_verified: None,
            name: None,
            given_name: None,
            family_name: None,
            preferred_username: preferred.map(str::to_owned),
        }
    }

    #[test]
    fn a_provider_vouches_for_an_address_it_gives_and_does_not_call_unverified() {
        let said = |email: Option<&str>, verified: Option<bool>| IdClaims {
            email: email.map(str::to_owned),
            email_verified: verified,
            ..claims(None)
        };
        let vouched = |claims: &IdClaims| proved_address(claims).map(str::to_owned);
        assert_eq!(
            vouched(&said(Some(" a@b.test "), Some(true))).as_deref(),
            Some("a@b.test")
        );
        assert_eq!(
            vouched(&said(Some("a@b.test"), None)),
            None,
            "missing email_verified is never treated as verified"
        );
        assert_eq!(vouched(&said(Some("a@b.test"), Some(false))), None);
        assert_eq!(vouched(&said(Some("no address"), Some(true))), None);
        assert_eq!(vouched(&said(Some(""), Some(true))), None);
        assert_eq!(vouched(&said(None, Some(true))), None);
    }

    #[test]
    fn a_user_name_is_made_of_what_the_provider_says_or_of_the_address() {
        assert_eq!(username_base(&claims(Some("ada.l")), "x@y.z"), "ada.l");
        assert_eq!(username_base(&claims(Some("한글")), "ada+1@y.z"), "ada1");
        assert_eq!(username_base(&claims(None), "@y.z"), "user");
    }
}
