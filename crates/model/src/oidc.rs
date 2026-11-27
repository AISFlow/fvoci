//! Signing in through an organization's `OpenID Connect` provider: the provider as its
//! administrators set it, and what the login page may know before anybody signs in.

use std::fmt;

use serde::{Deserialize, Serialize};

use crate::refusal::{RefusalReason, Refusals};

/// The longest client identifier and client secret taken, in bytes.
pub const OIDC_CLIENT_FIELD_MAX_BYTES: usize = 4096;

/// The longest issuer taken, in bytes.
pub const OIDC_ISSUER_MAX_BYTES: usize = 2048;

/// The provider of an organization, as its administrators read it. The client secret
/// is not in it: only whether there is one.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, utoipa::ToSchema)]
#[serde(rename_all = "camelCase")]
#[expect(
    clippy::struct_excessive_bools,
    reason = "each flag is a separate member of the wire format"
)]
pub struct OidcProvider {
    /// The issuer: an `https` address, exactly as the provider names itself.
    pub issuer: String,
    /// The client identifier the provider gave this server.
    pub client_id: String,
    /// Whether a client secret is stored.
    pub has_client_secret: bool,
    /// Whether somebody the provider vouches for who has no place in the organization
    /// (no member of it, nor a guest of one of its workspaces) becomes a member of it,
    /// and of its default workspace, by signing in. A guest signs in as a guest.
    pub auto_join: bool,
    /// Whether signing in through the provider is on.
    pub enabled: bool,
    /// Whether the organization takes no password: what is the organization's opens
    /// only for a session made through this provider.
    pub sso_required: bool,
    /// Whether this session was made through this provider, switched on, as it is
    /// stored now: a change of the issuer, the client, the secret or the switch since
    /// asks for signing in through it again before `ssoRequired` is switched on.
    pub can_require_sso: bool,
    /// The address the provider sends people back to, to be registered with it exactly:
    /// the organization's own, which does not change with the provider.
    pub redirect_uri: String,
    /// When the provider was first set, in milliseconds since the Unix epoch.
    pub created_at: i64,
    /// When the provider was last changed, in milliseconds since the Unix epoch.
    pub updated_at: i64,
    /// The settings saved aside while the organization takes no password, which
    /// nobody signs in through until a sign-in through them ends well; `null` for
    /// none.
    pub pending: Option<OidcPendingProvider>,
    /// The provider the organization signed in through before its issuer changed,
    /// while it is kept for its people to carry their accounts over; `null` for none.
    pub former: Option<OidcFormerProvider>,
}

/// The provider an organization moved away from, kept for a while: nobody signs in
/// through it, but somebody the new provider vouches for who has no account yet may
/// show who they were at it, and so tie the new identity to the account they had.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, utoipa::ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct OidcFormerProvider {
    /// Its issuer.
    pub issuer: String,
    /// When the organization moved away from it, in milliseconds since the Unix epoch.
    pub created_at: i64,
    /// When it is kept no longer, in milliseconds since the Unix epoch.
    pub expires_at: i64,
}

/// Which former provider an administrator ends the keeping of: the one they read.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, utoipa::ToSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct OidcFormerProviderEnd {
    /// The `createdAt` of the former provider as it was read.
    pub created_at: i64,
}

/// The pending settings of an organization's provider, without the client secret:
/// `hasClientSecret` says whether there is one.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, utoipa::ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct OidcPendingProvider {
    /// The issuer's URL, as the provider's ID tokens name it (`iss`).
    pub issuer: String,
    /// The client identifier.
    pub client_id: String,
    /// Whether a client secret is saved with them.
    pub has_client_secret: bool,
    /// When they were saved, in milliseconds since the Unix epoch.
    pub created_at: i64,
    /// Until when a trial from the session that asks, come back by then, ties an
    /// identity of another issuer that is nobody's yet to the administrator's own
    /// account, in milliseconds since the Unix epoch: five minutes after the session
    /// was made through the provider as it is stored. `null` once that has passed,
    /// for a session that was not made so, and for pending settings of the same
    /// issuer, which tie nothing.
    pub binds_identity_until: Option<i64>,
}

/// What an administrator sets of the organization's provider.
#[derive(Clone, PartialEq, Eq, Default, Serialize, Deserialize, utoipa::ToSchema)]
#[serde(default, rename_all = "camelCase")]
pub struct OidcProviderSettings {
    /// The issuer: an `https` address without query or fragment.
    pub issuer: String,
    /// The client identifier the provider gave this server.
    pub client_id: String,
    /// The client secret. Left out, the stored one stays while the issuer and the client
    /// stay the same (none for a new provider), and is required with another issuer or
    /// client when one is stored; the empty string stores none.
    pub client_secret: Option<String>,
    /// Whether somebody the provider vouches for who is no member becomes one. Left
    /// out, it stays as it is; on when the organization has no provider.
    pub auto_join: Option<bool>,
    /// Whether signing in through the provider is on. Left out, it stays as it is; on
    /// when the organization has no provider.
    pub enabled: Option<bool>,
    /// Whether the organization takes no password. Left out, it stays as it is; off
    /// when the organization has no provider.
    pub sso_required: Option<bool>,
}

impl OidcProviderSettings {
    /// Checks the members that can be checked without asking the provider.
    ///
    /// # Errors
    ///
    /// Returns every member that is refused: the issuer if it is blank, too long, or no
    /// `https` address of a host without user, query or fragment; the client identifier
    /// if it is blank or too long; the client secret if it is too long.
    pub fn validate(&self) -> Result<(), Refusals> {
        let mut refused = Refusals::new();
        if self.issuer.trim().is_empty() {
            refused.refuse("issuer", RefusalReason::Required, "issuer is required");
        } else if self.issuer.len() > OIDC_ISSUER_MAX_BYTES {
            refused.refuse(
                "issuer",
                RefusalReason::TooLong,
                format!("issuer must be at most {OIDC_ISSUER_MAX_BYTES} bytes"),
            );
        } else if !is_issuer_well_formed(&self.issuer) {
            refused.refuse(
                "issuer",
                RefusalReason::InvalidFormat,
                "issuer must be an https address without user, query or fragment",
            );
        }
        if self.client_id.trim().is_empty() {
            refused.refuse("clientId", RefusalReason::Required, "clientId is required");
        } else if self.client_id.len() > OIDC_CLIENT_FIELD_MAX_BYTES {
            refused.refuse(
                "clientId",
                RefusalReason::TooLong,
                format!("clientId must be at most {OIDC_CLIENT_FIELD_MAX_BYTES} bytes"),
            );
        }
        if self
            .client_secret
            .as_ref()
            .is_some_and(|secret| secret.len() > OIDC_CLIENT_FIELD_MAX_BYTES)
        {
            refused.refuse(
                "clientSecret",
                RefusalReason::TooLong,
                format!("clientSecret must be at most {OIDC_CLIENT_FIELD_MAX_BYTES} bytes"),
            );
        }
        refused.into_result()
    }
}

/// Whether an issuer has the form RFC 8414 and `OpenID Connect` Discovery give it: `https`,
/// a host, maybe a port and a path, and nothing else. What the text is made of is
/// checked, not what the host is; that is asked when the provider is reached.
fn is_issuer_well_formed(issuer: &str) -> bool {
    let Some(rest) = issuer.strip_prefix("https://") else {
        return false;
    };
    let authority = rest.split('/').next().unwrap_or_default();
    !authority.is_empty()
        && !rest.contains(['?', '#', '@', ' ', '\\'])
        && rest.chars().all(|c| c.is_ascii_graphic())
}

impl fmt::Debug for OidcProviderSettings {
    /// Leaves out the client secret.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("OidcProviderSettings")
            .field("issuer", &self.issuer)
            .field("client_id", &self.client_id)
            .field("auto_join", &self.auto_join)
            .field("enabled", &self.enabled)
            .finish_non_exhaustive()
    }
}

/// What a sign-in through a provider is started with. Nothing, to sign in.
#[derive(Clone, PartialEq, Eq, Default, Serialize, Deserialize, utoipa::ToSchema)]
#[serde(default, rename_all = "camelCase")]
pub struct OidcStartRequest {
    /// The password of the account of the request's session. With it the sign-in
    /// links: what the provider vouches for is tied to that account, in place of
    /// signing in.
    #[schema(nullable = false)]
    pub link_password: Option<String>,
}

impl fmt::Debug for OidcStartRequest {
    /// Leaves out the password.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("OidcStartRequest")
            .field("links", &self.link_password.is_some())
            .finish_non_exhaustive()
    }
}

/// An identity tied to an account: who its owner is at a provider, by which they sign
/// in to it through the provider of an organization.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, utoipa::ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct LinkedIdentity {
    /// The issuer, as the provider names itself.
    pub issuer: String,
    /// Who the account's owner is at the issuer.
    pub subject: String,
    /// The email address the provider gave when the identity was tied: its word,
    /// checked by nobody. Empty if it gave none.
    pub email: String,
    /// When it was tied, in milliseconds since the Unix epoch.
    pub created_at: i64,
    /// Whether the session that asks was made for this identity, or through a provider
    /// of its issuer before sessions knew their subject: removing the identity ends
    /// that session too.
    pub made_this_session: bool,
}

/// A request to remove an identity from the account of the session.
#[derive(Clone, PartialEq, Eq, Default, Serialize, Deserialize, utoipa::ToSchema)]
#[serde(default, rename_all = "camelCase")]
pub struct UnlinkRequest {
    /// The issuer of the identity.
    pub issuer: String,
    /// Its subject.
    pub subject: String,
    /// The account's password.
    pub password: String,
}

impl UnlinkRequest {
    /// Checks that the identity is named. The password is the account's to judge: an
    /// account without one is told so, whatever is sent.
    ///
    /// # Errors
    ///
    /// Returns `issuer` and `subject` if they are empty.
    pub fn validate(&self) -> Result<(), Refusals> {
        let mut refused = Refusals::new();
        for (name, value) in [("issuer", &self.issuer), ("subject", &self.subject)] {
            if value.is_empty() {
                refused.refuse(name, RefusalReason::Required, format!("{name} is required"));
            }
        }
        refused.into_result()
    }
}

impl fmt::Debug for UnlinkRequest {
    /// Leaves out the password.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("UnlinkRequest")
            .field("issuer", &self.issuer)
            .field("subject", &self.subject)
            .finish_non_exhaustive()
    }
}

/// What the login page may know of an organization before anybody signs in.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, utoipa::ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct LoginOrganization {
    /// The organization's identifier, for starting a sign-in through its provider.
    pub id: String,
    /// The organization's name; empty if it was given none.
    pub name: String,
    /// Whether signing in through the organization's provider is on.
    pub oidc: bool,
}

/// A sign-in through a provider that was started: where the page sends the browser.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, utoipa::ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct OidcStart {
    /// The provider's address that asks the person to sign in, for this sign-in.
    pub authorization_url: String,
}

/// What the login page shows: the organization it is for, if it is for one.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, utoipa::ToSchema)]
#[serde(rename_all = "camelCase")]
#[expect(
    clippy::struct_excessive_bools,
    reason = "four facts of the server that a login page reads, each a member of the wire form"
)]
pub struct LoginOptions {
    /// The organization the page is for: the one named, or the only one of a server
    /// that holds one. `null` for none, also for an identifier that names none.
    pub organization: Option<LoginOrganization>,
    /// Whether somebody without an invitation may register and start an organization: on a
    /// server where everybody may, and on any server until its first user did. Where it
    /// is `false`, the link of an invitation is the way in.
    pub registration_open: bool,
    /// Whether a registration must name the organization it starts
    /// (`organizationName` of `register`): `true` until the server has its first user,
    /// who starts the server's own organization. After that a registration may leave
    /// the name out and start with the account's own space alone.
    pub organization_required: bool,
    /// Whether this server can deliver password recovery and invitation mail.
    pub mail_enabled: bool,
    /// Whether a registration opens nothing until the link mailed to its address
    /// confirms it, which signs it in (`email_verification`).
    pub email_verification: bool,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn settings(issuer: &str) -> OidcProviderSettings {
        OidcProviderSettings {
            issuer: issuer.to_owned(),
            client_id: "client".to_owned(),
            ..OidcProviderSettings::default()
        }
    }

    #[test]
    fn an_issuer_is_an_https_address_of_a_host_and_nothing_more() {
        for good in [
            "https://idp.example.com",
            "https://idp.example.com/",
            "https://idp.example.com:8443/realms/ours",
            "https://[2001:db8::1]/tenant",
        ] {
            assert!(settings(good).validate().is_ok(), "{good}");
        }
        for bad in [
            "http://idp.example.com",
            "https://",
            "https:///path",
            "https://idp.example.com/?x=1",
            "https://idp.example.com/#x",
            "https://user@idp.example.com",
            "https://idp.example.com/a b",
            "idp.example.com",
        ] {
            assert!(settings(bad).validate().is_err(), "{bad}");
        }
    }

    #[test]
    fn the_secret_is_not_in_the_debug_output() {
        let mut with_secret = settings("https://idp.example.com");
        with_secret.client_secret = Some("very-secret".to_owned());
        assert!(
            !format!("{with_secret:?}").contains("very-secret"),
            "{with_secret:?}"
        );
    }
}
