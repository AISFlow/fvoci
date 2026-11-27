//! Who sends a request: a person by their session, or an agent by a personal API token
//! of theirs. Both are asked what they open by the same rule (`App::enter_workspace_as`);
//! a token is asked more of besides.

use std::net::IpAddr;
use std::time::Instant;

use fvoci_model::{ApiTokenScope, Session};
use fvoci_store::{ByAgent, LiveApiToken, Making};

use super::{App, Asker};
use crate::auth::api_token::{self, Delegation, USE_RECORDING_INTERVAL_MILLIS};
use crate::auth::{now_millis, secret};
use crate::error::AppError;

/// How a caller showed who they are.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CallerAuth {
    /// A session of theirs.
    Session(Session),
    /// A personal API token of theirs.
    ApiToken(Delegation),
}

/// Who sends a request: the user, and by what.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Caller {
    /// The user it is sent as.
    pub user_id: String,
    /// What they showed.
    pub auth: CallerAuth,
}

impl Caller {
    /// The session the request came with; `None` for a token.
    #[must_use]
    pub fn session(&self) -> Option<&Session> {
        match &self.auth {
            CallerAuth::Session(session) => Some(session),
            CallerAuth::ApiToken(_) => None,
        }
    }

    /// The token the request came with; `None` for a session.
    #[must_use]
    pub fn delegation(&self) -> Option<&Delegation> {
        match &self.auth {
            CallerAuth::Session(_) => None,
            CallerAuth::ApiToken(delegation) => Some(delegation),
        }
    }

    /// The agent the caller writes as, named by its token, as the versions of what it
    /// writes are kept; `None` for a session.
    #[must_use]
    pub fn agent(&self) -> Option<ByAgent<'_>> {
        self.delegation().map(Delegation::by_agent)
    }

    /// Refuses a caller who came with a personal API token that does not cover
    /// `needed`; a session covers all.
    ///
    /// # Errors
    ///
    /// Returns 403 with the reason `token_scope_insufficient` for a token that reads
    /// only, asked to write.
    pub fn require_scope(&self, needed: ApiTokenScope) -> Result<(), AppError> {
        match self.delegation() {
            Some(delegation) if !delegation.scope.covers(needed) => Err(api_token::reads_only()),
            _ => Ok(()),
        }
    }

    /// The caller as the rule of what opens what asks of them.
    #[must_use]
    pub fn asker(&self) -> Asker<'_> {
        match &self.auth {
            CallerAuth::Session(session) => Asker::from(session),
            CallerAuth::ApiToken(delegation) => {
                let making = Making::of_api_token(delegation.origin_provider_id.as_deref());
                Asker {
                    user_id: &self.user_id,
                    making: if delegation.allow_personal_space {
                        making.having_confirmed_password()
                    } else {
                        making
                    },
                    delegation: Some(delegation),
                }
            }
        }
    }
}

impl From<Session> for Caller {
    fn from(session: Session) -> Self {
        Self {
            user_id: session.user_id.clone(),
            auth: CallerAuth::Session(session),
        }
    }
}

impl App {
    /// The caller a personal API token stands for: a token in use, not past its end,
    /// of an account in use. Its use is counted (`TokenLimits`) and,
    /// every few minutes, written down with the address it came from.
    ///
    /// A token that stands for nobody is counted against `address`, apart from the
    /// attempts to sign in: an address that sent as many as it may this minute is
    /// refused before its next token is looked up, so that tokens are not guessed at
    /// faster than passwords. A request is answered 401 once, here or by the limit of
    /// its body (`App::token_writes`), and counted once.
    ///
    /// # Errors
    ///
    /// Returns 401 for a token that stands for nobody, 429 for one that sent too many
    /// requests or from an address that sent too many such, and the store's error.
    pub async fn caller_by_token(
        &self,
        token: &str,
        address: Option<IpAddr>,
    ) -> Result<Caller, AppError> {
        let now = now_millis();
        let live = self.live_token(token, address, now).await?;
        self.authenticator()
            .token_limits()
            .admit_token(&live.id, Instant::now())?;
        let due = live
            .last_used_at
            .is_none_or(|at| now.saturating_sub(at) >= USE_RECORDING_INTERVAL_MILLIS);
        if due {
            let address = address.map(|address| address.to_string());
            // A token whose use cannot be written down still stands for its owner.
            if let Err(error) = self
                .store()
                .record_api_token_use(&live.id, now, address.as_deref())
                .await
            {
                tracing::warn!(%error, "unable to record the use of a personal API token");
            }
        }
        Ok(Caller {
            user_id: live.user_id.clone(),
            auth: CallerAuth::ApiToken(Delegation::from(live)),
        })
    }

    /// Whether a personal API token may send a body as large as a session may: one
    /// that writes. Asked before the body is read, of a request whose route then asks
    /// again ([`App::caller_by_token`]); its use is not counted here.
    ///
    /// # Errors
    ///
    /// As [`App::caller_by_token`], but for the counts of the token's requests.
    pub async fn token_writes(
        &self,
        token: &str,
        address: Option<IpAddr>,
    ) -> Result<bool, AppError> {
        let live = self.live_token(token, address, now_millis()).await?;
        Ok(live.scope.covers(ApiTokenScope::Write))
    }

    /// The token stored under the digest of `token` that opens anything at `now`. One
    /// that stands for nobody is counted against `address`, and one that is of no
    /// token's form is not looked up.
    async fn live_token(
        &self,
        token: &str,
        address: Option<IpAddr>,
        now: i64,
    ) -> Result<LiveApiToken, AppError> {
        let limits = self.authenticator().token_limits();
        limits.admit_address(address, Instant::now())?;
        let found = if api_token::is_well_formed(token) {
            self.store()
                .live_api_token(&secret::token_digest(token), now)
                .await
                .map_err(|error| AppError::from(error).context("looking up the token"))?
        } else {
            None
        };
        if let Some(live) = found {
            return Ok(live);
        }
        limits.count_guess(address, Instant::now());
        Err(AppError::unauthorized("invalid personal API token"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use fvoci_store::SessionTaken;

    /// Whether a token of the tests was given the own space.
    #[derive(Clone, Copy, PartialEq, Eq)]
    enum OwnSpace {
        Given,
        NotGiven,
    }

    fn delegation(origin: Option<&str>, own_space: OwnSpace) -> Caller {
        Caller {
            user_id: "u".to_owned(),
            auth: CallerAuth::ApiToken(Delegation {
                token_id: "t".to_owned(),
                name: "agent".to_owned(),
                scope: ApiTokenScope::Read,
                allow_personal_space: own_space == OwnSpace::Given,
                notify_self: true,
                origin_provider_id: origin.map(str::to_owned),
                created_at: 0,
                expires_at: 1,
            }),
        }
    }

    #[test]
    fn a_token_opens_what_the_session_that_made_it_opened() {
        let through = delegation(Some("p"), OwnSpace::NotGiven);
        assert!(SessionTaken::Through("p".to_owned()).opened_by(through.asker().making));
        assert!(!SessionTaken::Through("q".to_owned()).opened_by(through.asker().making));
        let with_password = delegation(None, OwnSpace::NotGiven);
        assert!(
            !SessionTaken::Through("p".to_owned()).opened_by(with_password.asker().making),
            "an organization that takes no password takes no token made with one"
        );
        assert!(SessionTaken::Any.opened_by(with_password.asker().making));
        assert!(
            SessionTaken::WithPassword
                .opened_by(delegation(Some("p"), OwnSpace::Given).asker().making),
            "a token given the own space was given the password"
        );
        assert!(through.asker().delegation.is_some());
    }
}
