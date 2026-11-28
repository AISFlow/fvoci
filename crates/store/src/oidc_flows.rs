//! A sign-in through a provider that was started and has not come back: what is kept of
//! it until the provider sends the browser back (`oidc_login_flows`).

use crate::Store;
use crate::db::{FromRow, Id, OptId, Param, Row};
use crate::error::StoreError;

/// The columns of a sign-in under way, in the order [`LoginFlow::from_row`] reads them.
pub(crate) const FLOW_COLUMNS: &str = "state_hash, browser_hash, nonce, pkce_verifier, org_id, \
                                       provider_id, created_at, expires_at, link_session_id, \
                                       pending_at, pending_by, held_hash";

/// A sign-in through a provider that was started and has not come back.
#[derive(Clone, PartialEq, Eq)]
pub struct LoginFlow {
    /// The digest of the `state` sent to the provider, by which it is found.
    pub state_hash: String,
    /// The digest of the secret in the cookie of the browser that started it.
    pub browser_hash: String,
    /// The `nonce` the ID token must carry.
    pub nonce: String,
    /// The PKCE code verifier.
    pub pkce_verifier: String,
    /// The organization whose provider it goes through.
    pub org_id: String,
    /// The provider it goes through.
    pub provider_id: String,
    /// When it was started, in milliseconds since the Unix epoch.
    pub created_at: i64,
    /// When it can no longer come back, in milliseconds since the Unix epoch.
    pub expires_at: i64,
    /// What it is for: a sign-in, a link, or a trial of the pending settings.
    pub purpose: FlowPurpose,
}

/// What a sign-in through a provider was started for.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FlowPurpose {
    /// To sign in through the provider as it is.
    SignIn,
    /// To tie what the provider vouches for to the account of the session it was
    /// started from, in place of signing in.
    Link {
        /// The session it was started from.
        session_id: String,
    },
    /// To sign in through the pending settings saved at `pending_at`, which become the
    /// provider's when it ends well: in the account of the administrator who started
    /// it, and no other.
    Trial {
        /// When the pending settings it tries were saved: their state.
        pending_at: i64,
        /// The administrator who started it.
        user_id: String,
        /// The session it was started from, kept in the column a link keeps its
        /// session in: an identity nobody has is tied to the administrator's
        /// account only for the browser that comes back with this session. Empty
        /// for a trial started before that was kept, which ties nothing.
        session_id: String,
    },
    /// To sign in through the former provider, and so tie the identity of the held
    /// sign-in to the account the former provider vouches for (`oidc_held.rs`). It
    /// names the organization's provider, as every sign-in does, and ends with it.
    CarryOver {
        /// The digest of the held sign-in it carries over.
        held_hash: String,
    },
}

impl std::fmt::Debug for LoginFlow {
    /// Leaves out the nonce and the verifier.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("LoginFlow")
            .field("org_id", &self.org_id)
            .field("provider_id", &self.provider_id)
            .field("expires_at", &self.expires_at)
            .field("purpose", &self.purpose)
            .finish_non_exhaustive()
    }
}

impl Store {
    /// Stores a sign-in that is started through the handle's organization's provider.
    /// `flow.org_id` is the handle's.
    ///
    /// # Errors
    ///
    /// Returns [`StoreError::NoOrganization`] if the handle names none, and
    /// [`StoreError::Database`] if the statement fails, as it does for a provider that
    /// is not the organization's.
    pub async fn create_oidc_login_flow(&self, flow: &LoginFlow) -> Result<(), StoreError> {
        self.with_write_conn(async |scope| {
            let mut sql = scope.sql(
                "INSERT INTO oidc_login_flows (state_hash, browser_hash, nonce, pkce_verifier, \
                 org_id, provider_id, created_at, expires_at, link_session_id, pending_at, \
                 pending_by, held_hash) SELECT ",
            );
            let (link_session_id, pending_at, pending_by, held_hash) = match &flow.purpose {
                FlowPurpose::SignIn => ("", None, "", None),
                FlowPurpose::Link { session_id } => (session_id.as_str(), None, "", None),
                FlowPurpose::Trial {
                    pending_at,
                    user_id,
                    session_id,
                } => (
                    session_id.as_str(),
                    Some(*pending_at),
                    user_id.as_str(),
                    None,
                ),
                FlowPurpose::CarryOver { held_hash } => ("", None, "", Some(held_hash.clone())),
            };
            sql.bind(&flow.state_hash)
                .push_bind(", ", &flow.browser_hash)
                .push_bind(", ", &flow.nonce)
                .push_bind(", ", &flow.pkce_verifier)
                .push(", org_id, id")
                .push_bind(", ", flow.created_at)
                .push_bind(", ", flow.expires_at)
                .push_bind(", ", OptId(link_session_id))
                .push_bind(", ", Param::OptInt(pending_at))
                .push_bind(", ", OptId(pending_by))
                .push_bind(", ", Param::OptText(held_hash.clone()))
                .push_bind(" FROM oidc_providers WHERE id = ", Id(&flow.provider_id))
                .push_bind(" AND org_id = ", scope.org()?);
            if let Some(held_hash) = held_hash {
                // A sign-in to carry over a held sign-in that was taken starts nothing.
                sql.push(
                    " AND EXISTS (SELECT 1 FROM oidc_held_sign_ins AS held \
                     WHERE held.provider_id = oidc_providers.id",
                )
                .push_bind(" AND held.held_hash = ", held_hash)
                .push(")");
            }
            if let Some(pending_at) = pending_at {
                // A trial of settings that were thrown away or saved anew starts nothing.
                sql.push(
                    " AND EXISTS (SELECT 1 FROM oidc_pending_providers AS pending \
                     WHERE pending.provider_id = oidc_providers.id",
                )
                .push_bind(" AND pending.created_at = ", pending_at)
                .push(")");
            }
            if scope.execute(sql).await? == 0 {
                return Err(StoreError::not_found("oidc provider"));
            }
            Ok(())
        })
        .await
    }
}

/// Reads a sign-in under way from the columns [`FLOW_COLUMNS`].
impl FromRow for LoginFlow {
    fn from_row(row: &Row) -> Result<Self, StoreError> {
        Ok(LoginFlow {
            state_hash: row.text(0)?,
            browser_hash: row.text(1)?,
            nonce: row.text(2)?,
            pkce_verifier: row.text(3)?,
            org_id: row.id(4)?,
            provider_id: row.id(5)?,
            created_at: row.int(6)?,
            expires_at: row.int(7)?,
            purpose: match (row.id(8)?, row.opt_int(9)?, row.opt_text(11)?) {
                (_, _, Some(held_hash)) => FlowPurpose::CarryOver { held_hash },
                (session_id, Some(pending_at), None) => FlowPurpose::Trial {
                    pending_at,
                    user_id: row.id(10)?,
                    session_id,
                },
                (session_id, None, None) if !session_id.is_empty() => {
                    FlowPurpose::Link { session_id }
                }
                (_, None, None) => FlowPurpose::SignIn,
            },
        })
    }
}
