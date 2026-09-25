//! Second-factor and external sign-in settings shared by the MFA and OIDC
//! routes (source `packages/config` ENCRYPTION_KEYS / OIDC_* / PUBLIC_URL).

use std::sync::Arc;

use crate::auth::password::Keyring;
use crate::oidc::OidcSettings;

/// Milliseconds since the epoch for TOTP steps; tests pin it. id_token
/// times are checked against the real clock.
pub type Clock = Arc<dyn Fn() -> i64 + Send + Sync>;

pub fn system_clock() -> Clock {
    Arc::new(|| chrono::Utc::now().timestamp_millis())
}

#[derive(Clone)]
pub struct Identity {
    /// `ENCRYPTION_KEYS` / `ENCRYPTION_ACTIVE_KEY_ID`. Without them TOTP
    /// secrets, workspace client secrets and OIDC flow state can be neither
    /// sealed nor opened: those routes answer 503.
    pub encryption_keys: Option<Arc<Keyring>>,
    /// TOTP issuer: the public URL's host name (source).
    pub totp_issuer: String,
    pub oidc: OidcSettings,
    pub clock: Clock,
}

impl std::fmt::Debug for Identity {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Identity")
            .field("encryption_keys", &self.encryption_keys)
            .field("totp_issuer", &self.totp_issuer)
            .field("oidc", &self.oidc)
            .finish()
    }
}

pub fn totp_issuer_from_origin(public_origin: &str) -> String {
    url::Url::parse(public_origin)
        .ok()
        .and_then(|u| u.host_str().map(str::to_string))
        .unwrap_or_else(|| "fvoci".to_string())
}

pub fn encryption_keys_from_env() -> Result<Option<Arc<Keyring>>, String> {
    let keys = std::env::var("ENCRYPTION_KEYS")
        .ok()
        .filter(|v| !v.trim().is_empty());
    let active = std::env::var("ENCRYPTION_ACTIVE_KEY_ID")
        .ok()
        .filter(|v| !v.trim().is_empty());
    match (keys, active) {
        (None, None) => Ok(None),
        (Some(keys), Some(active)) => Ok(Some(Arc::new(
            Keyring::parse_named(&keys, &active, "ENCRYPTION_KEYS")
                .map_err(|err| format!("invalid ENCRYPTION_KEYS: {err}"))?,
        ))),
        _ => Err("ENCRYPTION_KEYS and ENCRYPTION_ACTIVE_KEY_ID must be set together".into()),
    }
}

impl Identity {
    /// Nothing configured: MFA setup and OIDC answer unavailable.
    pub fn disabled(public_origin: &str) -> Self {
        Self {
            encryption_keys: None,
            totp_issuer: totp_issuer_from_origin(public_origin),
            oidc: OidcSettings::default(),
            clock: system_clock(),
        }
    }

    pub fn from_env(public_origin: &str) -> Result<Self, String> {
        let encryption_keys = encryption_keys_from_env()?;
        let oidc = OidcSettings::from_env(public_origin)?;
        if encryption_keys.is_none() && !oidc.providers.is_empty() {
            return Err("OIDC providers need ENCRYPTION_KEYS for the flow state".into());
        }
        Ok(Self {
            encryption_keys,
            totp_issuer: totp_issuer_from_origin(public_origin),
            oidc,
            clock: system_clock(),
        })
    }

    pub fn now_ms(&self) -> i64 {
        (self.clock)()
    }
}

/// AAD contexts (source `secret-maintenance.ts`).
pub fn user_mfa_context(user_id: uuid::Uuid) -> String {
    format!("user-mfa:{user_id}")
}

pub fn workspace_oidc_context(workspace_id: uuid::Uuid) -> String {
    format!("workspace-oidc:{workspace_id}")
}
