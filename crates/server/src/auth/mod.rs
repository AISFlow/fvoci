//! Authentication: session tokens, passwords, and read tokens of shared boards.

pub mod api_token;
pub mod hash_queue;
pub mod limiter;
pub mod oidc;
pub mod password;
pub mod sealed;
pub mod secret;
pub mod token;

use std::sync::Arc;

use fvoci_model::{ContainerKind, Leaving, Session};
use fvoci_store::{SharedContainer, Store};

use crate::config::Settings;
use crate::error::AppError;
use api_token::TokenLimits;
use token::{Origins, SessionCookie};

/// How long after the last recorded use of a session its next use is recorded: ten
/// minutes. Requests in between do not write to the database, and the idle limit is
/// that much less exact.
const SESSION_USE_RECORDING_INTERVAL_MILLIS: i64 = 10 * 60 * 1000;

/// Looks up sessions and checks read tokens.
#[derive(Debug, Clone)]
pub struct Authenticator {
    /// A handle that names no organization: sessions are nobody's, and whose a shared
    /// board is, the store's directory finds out by the token.
    store: Store,
    session_idle_millis: i64,
    public_shared_containers: bool,
    cookie: SessionCookie,
    origins: Origins,
    /// How many requests personal API tokens sent this minute.
    token_limits: Arc<TokenLimits>,
}

/// A session a token was found to stand for.
#[derive(Debug, Clone)]
pub struct Authenticated {
    /// The session, as the store holds it.
    pub session: Session,
    /// Whether this lookup wrote down that the session was used, which it does when the
    /// last record is old enough. Then is also when a browser's cookie is renewed.
    pub use_recorded: bool,
}

impl Authenticator {
    /// Creates an authenticator with the session lifetime, the sharing switch and the
    /// origins of the configuration.
    pub fn new(store: Store, settings: &Settings) -> Self {
        let config = settings.config();
        let origins = settings.origins().clone();
        let idle_millis = config.session_idle_millis();
        Self {
            store,
            session_idle_millis: idle_millis,
            public_shared_containers: config.allow_public_links,
            cookie: SessionCookie::new(origins.https(), idle_millis / 1000),
            origins,
            token_limits: Arc::new(TokenLimits::new(
                limiter::Limits::from_config(config).address_attempts_per_minute,
            )),
        }
    }

    /// The session cookie of this server.
    pub fn session_cookie(&self) -> &SessionCookie {
        &self.cookie
    }

    /// The origins whose pages may send requests that change something.
    pub fn origins(&self) -> &Origins {
        &self.origins
    }

    /// How many requests personal API tokens sent this minute.
    pub fn token_limits(&self) -> &TokenLimits {
        &self.token_limits
    }

    /// Returns the session a token belongs to, and records that it was used unless that
    /// was recorded in the last ten minutes.
    ///
    /// The session is looked up by the digest of the token; the token itself is stored
    /// nowhere. A session has ended, and is not found, when it has not been used for the
    /// idle limit, when its absolute limit has passed, and when its user is deactivated.
    ///
    /// # Errors
    ///
    /// Returns an unauthorized error if the token is empty, unknown or of a session that
    /// has ended, 403 with the reason `token_forbidden_endpoint` for a personal API
    /// token, which is no session's and is not looked up as one, and a server error if
    /// the session cannot be looked up.
    pub async fn get_session(&self, token: &str) -> Result<Authenticated, AppError> {
        let now = now_millis();
        let mut session = self.session_at(token, now).await?;

        let mut use_recorded = false;
        if now.saturating_sub(session.last_used_at) >= SESSION_USE_RECORDING_INTERVAL_MILLIS {
            // A session that cannot be marked as used is still the caller's session.
            match self.store.touch_session(&session.id, now).await {
                Ok(()) => {
                    session.last_used_at = now;
                    use_recorded = true;
                }
                Err(error) => tracing::warn!(%error, "unable to record the use of the session"),
            }
        }
        Ok(Authenticated {
            session,
            use_recorded,
        })
    }

    /// Returns the session a token belongs to, without recording a use.
    ///
    /// This is for the websocket: the answer of a handshake cannot renew the cookie, so
    /// a use recorded there would be one whose renewal no request gets to send. A
    /// session lives by the requests of its user.
    ///
    /// # Errors
    ///
    /// Returns the errors of [`Authenticator::get_session`].
    pub async fn find_session(&self, token: &str) -> Result<Session, AppError> {
        self.session_at(token, now_millis()).await
    }

    /// The session of a token, if it has not ended by `now`.
    async fn session_at(&self, token: &str, now: i64) -> Result<Session, AppError> {
        if token.is_empty() {
            return Err(AppError::unauthorized("no session token"));
        }
        if api_token::is_api_token(token) {
            return Err(api_token::refused_as_session());
        }
        match self
            .store
            .get_session(
                &secret::token_digest(token),
                now,
                now.saturating_sub(self.session_idle_millis),
            )
            .await
        {
            Ok(session) => Ok(session),
            // An unknown or expired token is an everyday refusal; a store that fails is
            // the server's fault and is reported as such. The session of an account that
            // waits to be deleted is told so: it is good for the way back.
            Err(error) if error.is_not_found() => {
                Err(match self.leaving_session_at(token, now).await? {
                    Some(_) => AppError::account_deletion_pending(),
                    None => AppError::unauthorized("invalid session token"),
                })
            }
            Err(error) => Err(AppError::from(error).context("looking up the session")),
        }
    }

    /// The session a token belongs to, if it is one of an account that waits to be
    /// deleted, and until when the account waits: the session that opens the way back
    /// and nothing else. `None` for any other token, also one of an account in use.
    ///
    /// # Errors
    ///
    /// Returns a server error if the session cannot be looked up.
    pub async fn leaving_session(
        &self,
        token: &str,
    ) -> Result<Option<(Session, Leaving)>, AppError> {
        self.leaving_session_at(token, now_millis()).await
    }

    async fn leaving_session_at(
        &self,
        token: &str,
        now: i64,
    ) -> Result<Option<(Session, Leaving)>, AppError> {
        if api_token::is_api_token(token) {
            return Ok(None);
        }
        self.store
            .leaving_session(
                &secret::token_digest(token),
                now,
                now.saturating_sub(self.session_idle_millis),
            )
            .await
            .map_err(|error| AppError::from(error).context("looking up the session"))
    }

    /// Returns whether `read_token` grants read access to a shared board's blocks, which
    /// a websocket follows: a wiki's token reads its pages only, over HTTP.
    ///
    /// # Errors
    ///
    /// Returns the store's error if the lookup fails.
    pub async fn is_valid_read_token(
        &self,
        container_id: &str,
        read_token: &str,
    ) -> Result<bool, AppError> {
        Ok(self
            .shared_by_read_token(container_id, read_token)
            .await?
            .is_some_and(|shared| shared.kind == ContainerKind::Board))
    }

    /// The shared board or wiki that `read_token` grants read access to, with its
    /// organization: it is read there, whichever organization the request came in for.
    /// `None` if the token opens nothing.
    ///
    /// # Errors
    ///
    /// Returns the store's error if the lookup fails.
    pub async fn shared_by_read_token(
        &self,
        container_id: &str,
        read_token: &str,
    ) -> Result<Option<SharedContainer>, AppError> {
        // With sharing turned off in the configuration no read token opens anything,
        // whatever is stored: an everyday "no", and nothing for the log.
        if !self.public_shared_containers {
            return Ok(None);
        }
        // Whose board it is, is not known here, and need not be: the store hands the
        // organization of a shared board to whoever holds its token, and to nobody
        // else.
        Ok(self
            .store
            .shared_container(container_id, read_token)
            .await?)
    }
}

/// Returns the current time in milliseconds since the Unix epoch.
pub fn now_millis() -> i64 {
    let since_epoch = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default();
    i64::try_from(since_epoch.as_millis()).unwrap_or(i64::MAX)
}
