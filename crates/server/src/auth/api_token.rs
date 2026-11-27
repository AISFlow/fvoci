//! Personal API tokens as a request presents them: told from a session token by their
//! prefix before anything is looked up, made of as many random bytes as a session
//! token, and stored by their digest alone.
//!
//! A token is a credential of its own kind. The lookups of sessions refuse it before
//! they look anything up ([`refused_as_session`]), so that a route that takes a session
//! takes no token: only the routes that say they do (`Delegated` of `crate::api`, and
//! the MCP server, `TokenCaller` of `crate::api::mcp`, which takes nothing else) take
//! one, and only from the `Authorization` header.
//!
//! How many requests a token sends is counted in memory ([`TokenLimits`]): of each
//! token, and of the tokens that reach one organization together.

use std::collections::HashMap;
use std::net::IpAddr;
use std::sync::Mutex;
use std::time::{Duration, Instant};

use fvoci_model::{API_TOKEN_PREFIX, API_TOKEN_SHOWN_CHARS, ApiTokenScope};
use fvoci_store::{ByAgent, LiveApiToken};

use super::limiter::AddressKey;
use super::secret;
use crate::error::{AppError, Reason};

/// How many requests one token sends in a minute at most.
pub const TOKEN_REQUESTS_PER_MINUTE: u32 = 120;

/// How many requests the tokens that reach one organization send together in a minute
/// at most: five tokens' worth, so that one person's agents do not crowd out the rest.
pub const ORGANIZATION_TOKEN_REQUESTS_PER_MINUTE: u32 = 5 * TOKEN_REQUESTS_PER_MINUTE;

/// The window the requests are counted in.
const WINDOW: Duration = Duration::from_secs(60);

/// How many windows are kept at most; past that, those that ended are forgotten, and
/// while none has, a request that would need a new one is refused (429) until one has.
const MAX_WINDOWS: usize = 100_000;

/// How seldom the windows that ended are looked for while there are as many as are
/// kept: the look goes over all of them.
const SWEEP_INTERVAL: Duration = Duration::from_secs(1);

/// How seldom a token's use is written down: its list tells when it was last used to
/// within this much.
pub const USE_RECORDING_INTERVAL_MILLIS: i64 = 5 * 60 * 1000;

/// A token as it is made: the token itself, which is told once, the characters it is
/// shown by, and the digest it is stored under.
pub struct MadeToken {
    /// `fvoci_pat_` and 43 characters of the URL-safe base64 alphabet: 256 random bits.
    pub token: String,
    /// `fvoci_pat_` and the first characters after it.
    pub prefix: String,
    /// The digest it is stored and found under.
    pub digest: String,
}

impl std::fmt::Debug for MadeToken {
    /// Leaves out the token.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("MadeToken")
            .field("prefix", &self.prefix)
            .finish_non_exhaustive()
    }
}

/// Makes a new token.
///
/// # Panics
///
/// Panics if the operating system cannot supply random bytes ([`secret::new_secret`]).
#[must_use]
pub fn make_token() -> MadeToken {
    let token = format!("{API_TOKEN_PREFIX}{}", secret::new_secret());
    let prefix = token[..API_TOKEN_PREFIX.len() + API_TOKEN_SHOWN_CHARS].to_owned();
    let digest = secret::token_digest(&token);
    MadeToken {
        token,
        prefix,
        digest,
    }
}

/// Whether a credential is a personal API token, by its prefix alone.
#[must_use]
pub fn is_api_token(credential: &str) -> bool {
    credential.starts_with(API_TOKEN_PREFIX)
}

/// Whether a personal API token is of the form every token made is
/// ([`make_token`]): one that is not stands for nobody, and is not looked up.
#[must_use]
pub fn is_well_formed(token: &str) -> bool {
    token.strip_prefix(API_TOKEN_PREFIX).is_some_and(|secret| {
        secret.len() == secret::SECRET_LENGTH
            && secret
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-' || byte == b'_')
    })
}

/// The refusal of a personal API token where a session is looked up: a route that is
/// not open to tokens. A 403, so that a route that serves whoever has no session does
/// not take the token for nobody's.
#[must_use]
pub fn refused_as_session() -> AppError {
    AppError::forbidden("a personal API token does not open this")
        .with_reason(Reason::TokenForbiddenEndpoint)
}

/// The refusal of a token that reads only, sent to a route that writes.
#[must_use]
pub fn reads_only() -> AppError {
    AppError::forbidden("the personal API token reads only")
        .with_reason(Reason::TokenScopeInsufficient)
}

/// A day in milliseconds.
pub(crate) const DAY_MILLIS: i64 = 24 * 60 * 60 * 1000;

/// The refusal of a token older than an organization that takes only sessions made
/// through its provider lets one be ([`Delegation::ends_within`]): its owner makes
/// another.
#[must_use]
pub fn past_its_days_there() -> AppError {
    AppError::unauthorized(
        "the personal API token is older than the organization lets one be: make another",
    )
    .with_reason(Reason::TokenExpiredForOrganization)
}

/// The refusal of a token, or of its making, in a workspace of an organization that
/// takes only sessions made through its provider and gives its workspaces to no token.
#[must_use]
pub fn disallowed_by_organization() -> AppError {
    AppError::forbidden("the organization gives its workspaces to no personal API token")
        .with_reason(Reason::ApiTokensDisallowed)
}

/// What a token gives an agent: to do as its owner may, with the content of the
/// workspaces it names, reading or writing as its scope says.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Delegation {
    /// The token that gives it, which what the agent writes is put down to.
    pub token_id: String,
    /// Its name, which what it writes is told and versioned under.
    pub name: String,
    /// What it may do.
    pub scope: ApiTokenScope,
    /// Whether it reaches its owner's own space (never its diaries).
    pub allow_personal_space: bool,
    /// Whether its owner is told of what it does.
    pub notify_self: bool,
    /// The provider of the session that made it, if it was made through one.
    pub origin_provider_id: Option<String>,
    /// When it was made, in milliseconds since the Unix epoch.
    pub created_at: i64,
    /// When it ends, in milliseconds since the Unix epoch.
    pub expires_at: i64,
}

impl Delegation {
    /// The agent of the token, as what it writes names it.
    #[must_use]
    pub fn by_agent(&self) -> ByAgent<'_> {
        ByAgent {
            token_id: &self.token_id,
            token_name: &self.name,
            notify_self: self.notify_self,
        }
    }

    /// When the token ends in an organization that takes only sessions made through its
    /// provider: `days` after it was made at the latest, as such a session would, also
    /// for a token made to last longer before the organization began to take only those.
    #[must_use]
    pub fn ends_within(&self, days: u32) -> i64 {
        let most = i64::from(days).saturating_mul(DAY_MILLIS);
        self.expires_at.min(self.created_at.saturating_add(most))
    }
}

impl From<LiveApiToken> for Delegation {
    fn from(token: LiveApiToken) -> Self {
        Self {
            token_id: token.id,
            name: token.name,
            scope: token.scope,
            allow_personal_space: token.allow_personal_space,
            notify_self: token.notify_self,
            origin_provider_id: token.origin_provider_id,
            created_at: token.created_at,
            expires_at: token.expires_at,
        }
    }
}

/// What the requests of tokens are counted by.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
enum Counted {
    Token(String),
    Organization(String),
    /// The tokens that stood for nobody, by the address they came from.
    Guesses(AddressKey),
}

/// The requests of one key in the current window.
#[derive(Debug, Clone, Copy)]
struct Window {
    started: Instant,
    requests: u32,
}

/// The windows of the counts, and when those that ended were last looked for.
#[derive(Debug, Default)]
struct Windows {
    windows: HashMap<Counted, Window>,
    swept: Option<Instant>,
}

/// The counts of the requests of tokens, by token and by organization, and of the
/// tokens that stood for nobody, by address.
///
/// The tokens that stand for nobody are counted apart from the attempts to sign in
/// (`LoginLimiter`): a client that sends a token that ended does not keep the people
/// of its address from signing in, nor do they keep it from being told so.
#[derive(Debug, Default)]
pub struct TokenLimits {
    windows: Mutex<Windows>,
    /// How many tokens that stand for nobody an address sends in a minute at most;
    /// `None` for no count by address (`login_address_attempts_per_minute` 0).
    guesses_per_minute: Option<u32>,
}

impl TokenLimits {
    /// Counts that let an address send `guesses_per_minute` tokens that stand for
    /// nobody in a minute; `None` for no count by address.
    #[must_use]
    pub fn new(guesses_per_minute: Option<u32>) -> Self {
        Self {
            windows: Mutex::default(),
            guesses_per_minute,
        }
    }

    /// Refuses a token from `address` at `now` before it is looked up, if the address
    /// sent as many tokens that stand for nobody as it may this minute.
    ///
    /// # Errors
    ///
    /// Returns 429 with how long to wait.
    pub fn admit_address(&self, address: Option<IpAddr>, now: Instant) -> Result<(), AppError> {
        let (Some(address), Some(limit)) = (address, self.guesses_per_minute) else {
            return Ok(());
        };
        let windows = self.windows();
        match windows
            .windows
            .get(&Counted::Guesses(AddressKey::from(address)))
        {
            Some(window) if window.holds(now) && window.requests >= limit => {
                Err(window.refusal(now))
            }
            _ => Ok(()),
        }
    }

    /// Counts a token from `address` at `now` that stands for nobody: once a request.
    pub fn count_guess(&self, address: Option<IpAddr>, now: Instant) {
        let (Some(address), Some(limit)) = (address, self.guesses_per_minute) else {
            return;
        };
        // The request is answered 401 whatever this says; the next one is refused.
        let _counted = self.admit(Counted::Guesses(AddressKey::from(address)), limit, now);
    }

    /// Counts a request of the token at `now`.
    ///
    /// # Errors
    ///
    /// Returns 429 with the reason `token_rate_limited` and how long to wait, if the
    /// token sent as many as it may this minute.
    pub fn admit_token(&self, token_id: &str, now: Instant) -> Result<(), AppError> {
        self.admit(
            Counted::Token(token_id.to_owned()),
            TOKEN_REQUESTS_PER_MINUTE,
            now,
        )
    }

    /// Counts a request of a token to a workspace of the organization at `now`.
    ///
    /// # Errors
    ///
    /// Returns 429 as [`TokenLimits::admit_token`] does, if the organization's tokens
    /// sent as many as they may this minute.
    pub fn admit_organization(&self, org_id: &str, now: Instant) -> Result<(), AppError> {
        self.admit(
            Counted::Organization(org_id.to_owned()),
            ORGANIZATION_TOKEN_REQUESTS_PER_MINUTE,
            now,
        )
    }

    fn windows(&self) -> std::sync::MutexGuard<'_, Windows> {
        self.windows
            .lock()
            .expect("the windows are never held across a panic")
    }

    fn admit(&self, key: Counted, limit: u32, now: Instant) -> Result<(), AppError> {
        let mut windows = self.windows();
        if windows.windows.len() >= MAX_WINDOWS && !windows.windows.contains_key(&key) {
            let due = windows
                .swept
                .is_none_or(|swept| now.saturating_duration_since(swept) >= SWEEP_INTERVAL);
            if due {
                windows.windows.retain(|_, window| window.holds(now));
                windows.swept = Some(now);
            }
            if windows.windows.len() >= MAX_WINDOWS {
                // What is not counted is not let through: the soonest a window ends.
                return Err(AppError::quota_reached(
                    "too many requests of personal API tokens; try again later",
                    Reason::TokenRateLimited,
                    SWEEP_INTERVAL,
                ));
            }
        }
        let window = windows.windows.entry(key).or_insert(Window {
            started: now,
            requests: 0,
        });
        if !window.holds(now) {
            *window = Window {
                started: now,
                requests: 0,
            };
        }
        if window.requests >= limit {
            return Err(window.refusal(now));
        }
        window.requests += 1;
        Ok(())
    }
}

impl Window {
    /// Whether the window still counts at `now`.
    fn holds(self, now: Instant) -> bool {
        now.saturating_duration_since(self.started) < WINDOW
    }

    /// The refusal of a request past the window's limit at `now`, with the wait until
    /// the window ends.
    fn refusal(self, now: Instant) -> AppError {
        let wait = WINDOW.saturating_sub(now.saturating_duration_since(self.started));
        AppError::quota_reached(
            "too many requests of personal API tokens; try again later",
            Reason::TokenRateLimited,
            wait,
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::error::ErrorKind;

    #[test]
    fn a_token_is_256_random_bits_behind_its_prefix_and_shown_by_six_more() {
        let made = make_token();
        assert!(is_api_token(&made.token));
        assert_eq!(
            made.token.len(),
            API_TOKEN_PREFIX.len() + secret::SECRET_LENGTH
        );
        assert_eq!(
            made.prefix.len(),
            API_TOKEN_PREFIX.len() + API_TOKEN_SHOWN_CHARS
        );
        assert!(made.token.starts_with(&made.prefix));
        assert_eq!(made.digest, secret::token_digest(&made.token));
        assert_ne!(make_token().token, made.token, "each is new");
        assert!(
            !format!("{made:?}").contains(&made.token),
            "nor is it printed"
        );
        assert!(!is_api_token("a-session-token"));
        assert!(is_well_formed(&made.token));
        assert!(!is_well_formed(&made.token[..made.token.len() - 1]));
        assert!(!is_well_formed(&format!(
            "{}!",
            &made.token[..made.token.len() - 1]
        )));
        assert!(!is_well_formed(&made.token[API_TOKEN_PREFIX.len()..]));
    }

    #[test]
    fn an_address_is_held_to_its_tokens_that_stand_for_nobody() {
        let limits = TokenLimits::new(Some(2));
        let (now, address) = (Instant::now(), Some(IpAddr::from([192, 0, 2, 1])));
        let other = Some(IpAddr::from([192, 0, 2, 2]));
        for _ in 0..2 {
            limits.admit_address(address, now).expect("admitted");
            limits.count_guess(address, now);
        }
        let refused = limits.admit_address(address, now).expect_err("held");
        assert_eq!(refused.reason(), Some(Reason::TokenRateLimited));
        limits.admit_address(other, now).expect("another address");
        limits
            .admit_token("t", now)
            .expect("a token's own count is apart");
        limits
            .admit_address(address, now + WINDOW)
            .expect("the next minute");
        let uncounted = TokenLimits::new(None);
        for _ in 0..3 {
            uncounted.count_guess(address, now);
        }
        uncounted
            .admit_address(address, now)
            .expect("not counted by address");
    }

    #[test]
    fn a_count_that_has_no_room_is_refused() {
        let limits = TokenLimits::default();
        let now = Instant::now();
        {
            let mut windows = limits.windows();
            for index in 0..MAX_WINDOWS {
                windows.windows.insert(
                    Counted::Token(index.to_string()),
                    Window {
                        started: now,
                        requests: 1,
                    },
                );
            }
        }
        let refused = limits.admit_token("new", now).expect_err("no room");
        assert_eq!(refused.reason(), Some(Reason::TokenRateLimited));
        limits
            .admit_token("0", now)
            .expect("a token counted already goes on");
        limits
            .admit_token("new", now + WINDOW)
            .expect("once the windows end, there is room");
    }

    #[test]
    fn a_token_sends_its_requests_of_a_minute_and_waits_for_the_next() {
        let limits = TokenLimits::default();
        let start = Instant::now();
        for _ in 0..TOKEN_REQUESTS_PER_MINUTE {
            limits.admit_token("t", start).expect("admitted");
        }
        let refused = limits
            .admit_token("t", start + Duration::from_secs(20))
            .expect_err("one too many");
        assert_eq!(refused.kind(), ErrorKind::RateLimited);
        assert_eq!(refused.reason(), Some(Reason::TokenRateLimited));
        limits
            .admit_token("other", start)
            .expect("another token counts for itself");
        limits
            .admit_token("t", start + WINDOW)
            .expect("the next minute");
    }

    #[test]
    fn the_tokens_of_an_organization_send_five_tokens_worth_together() {
        let limits = TokenLimits::default();
        let now = Instant::now();
        for _ in 0..ORGANIZATION_TOKEN_REQUESTS_PER_MINUTE {
            limits.admit_organization("o", now).expect("admitted");
        }
        assert!(limits.admit_organization("o", now).is_err());
        assert!(limits.admit_organization("p", now).is_ok());
    }
}
