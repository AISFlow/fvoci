//! Workspace integrations: outgoing webhooks, the GitHub App, document AI
//! actions (source `apps/server/src/domains/integrations`).

pub mod ai;
pub mod github;
pub mod outbound;
pub mod webhooks;

use std::sync::Arc;

use crate::auth::password::Keyring;

/// Integration settings shared by the routes and the outbox consumers.
#[derive(Clone)]
pub struct Integrations {
    /// `ENCRYPTION_KEYS` / `ENCRYPTION_ACTIVE_KEY_ID`. Without them webhook
    /// secrets can be neither sealed nor opened: creation answers 503 and
    /// pending deliveries fail closed.
    pub encryption_keys: Option<Arc<Keyring>>,
    pub outbound: outbound::Outbound,
    pub github: Option<github::GithubConfig>,
    pub ai: Option<ai::AiConfig>,
}

impl std::fmt::Debug for Integrations {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Integrations")
            .field("encryption_keys", &self.encryption_keys)
            .field("outbound", &self.outbound.policy())
            .field("github", &self.github)
            .field("ai", &self.ai)
            .finish()
    }
}

impl Integrations {
    /// Nothing configured: routes exist, create/install/AI answer unavailable.
    pub fn disabled() -> Self {
        Self {
            encryption_keys: None,
            outbound: outbound::Outbound::system(outbound::OutboundPolicy::default()),
            github: None,
            ai: None,
        }
    }

    pub fn from_env() -> Result<Self, String> {
        let keys = std::env::var("ENCRYPTION_KEYS")
            .ok()
            .filter(|v| !v.trim().is_empty());
        let active = std::env::var("ENCRYPTION_ACTIVE_KEY_ID")
            .ok()
            .filter(|v| !v.trim().is_empty());
        let encryption_keys = match (keys, active) {
            (None, None) => None,
            (Some(keys), Some(active)) => Some(Arc::new(
                Keyring::parse_named(&keys, &active, "ENCRYPTION_KEYS")
                    .map_err(|err| format!("invalid ENCRYPTION_KEYS: {err}"))?,
            )),
            _ => {
                return Err(
                    "ENCRYPTION_KEYS and ENCRYPTION_ACTIVE_KEY_ID must be set together".into(),
                )
            }
        };
        let github = github::GithubConfig::from_env(encryption_keys.as_deref())?;
        Ok(Self {
            encryption_keys,
            outbound: outbound::Outbound::system(outbound::OutboundPolicy::from_env()?),
            github,
            ai: ai::AiConfig::from_env(),
        })
    }
}
