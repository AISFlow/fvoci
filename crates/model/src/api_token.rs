//! Personal API tokens: what a user gives an agent to read or write the content of
//! some of their workspaces on their behalf.

use std::fmt;
use std::str::FromStr;

use serde::{Deserialize, Serialize};
use thiserror::Error;

/// The text every personal API token begins with, by which a request's credential is
/// told from a session token before anything is looked up.
pub const API_TOKEN_PREFIX: &str = "fvoci_pat_";

/// How many characters of a token, after [`API_TOKEN_PREFIX`], are kept to tell it by:
/// shown in lists, never enough to sign in with.
pub const API_TOKEN_SHOWN_CHARS: usize = 6;

/// The most characters a token's name has.
pub const API_TOKEN_NAME_MAX_CHARS: usize = 80;

/// How long a token lasts when its maker does not say, in days.
pub const API_TOKEN_DEFAULT_DAYS: u32 = 90;

/// The longest a token may last, in days.
pub const API_TOKEN_MAX_DAYS: u32 = 365;

/// How many tokens one account holds at most that open anything (not taken back, not
/// past their end): one agent's token each, for as many agents as a person runs.
pub const API_TOKEN_MAX_LIVE: usize = 25;

/// What a token may do with the content it reaches.
#[derive(
    Debug,
    Clone,
    Copy,
    PartialEq,
    Eq,
    Hash,
    PartialOrd,
    Ord,
    Serialize,
    Deserialize,
    utoipa::ToSchema,
)]
#[serde(rename_all = "lowercase")]
pub enum ApiTokenScope {
    /// Read cards, pages, units, their comments and files.
    Read,
    /// Read, and write their content: titles, bodies, property values, comments and
    /// attachments. Never who is on what, what is shared, or anything of the account.
    Write,
}

impl ApiTokenScope {
    /// The wire and database form.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Read => "read",
            Self::Write => "write",
        }
    }

    /// Whether a token of this scope may do what `needed` asks: a write token reads
    /// too.
    #[must_use]
    pub const fn covers(self, needed: Self) -> bool {
        matches!((self, needed), (Self::Write, _) | (Self::Read, Self::Read))
    }
}

impl fmt::Display for ApiTokenScope {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// Error returned when a text names no [`ApiTokenScope`].
#[derive(Debug, Clone, PartialEq, Eq, Error)]
#[error("{0:?} is no token scope")]
pub struct ParseApiTokenScopeError(String);

impl FromStr for ApiTokenScope {
    type Err = ParseApiTokenScopeError;

    fn from_str(text: &str) -> Result<Self, Self::Err> {
        match text {
            "read" => Ok(Self::Read),
            "write" => Ok(Self::Write),
            other => Err(ParseApiTokenScopeError(other.to_owned())),
        }
    }
}

/// A workspace a token reaches, as its list shows it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, utoipa::ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct ApiTokenWorkspace {
    /// The identifier of the workspace, as its routes take it.
    pub workspace_id: String,
    /// Its organization.
    pub org_id: String,
    /// Its name.
    pub name: String,
}

/// A personal API token as its owner sees it: never the token itself, which is shown
/// once, when it is made.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, utoipa::ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct ApiToken {
    /// The token's identifier.
    pub id: String,
    /// Whose it is.
    pub user_id: String,
    /// The name its owner gave it.
    pub name: String,
    /// The first characters of the token (`fvoci_pat_` and six more): enough to tell
    /// which token a client holds, never to sign in.
    pub token_prefix: String,
    /// What it may do.
    pub scope: ApiTokenScope,
    /// The workspaces of organizations it reaches.
    pub workspaces: Vec<ApiTokenWorkspace>,
    /// Whether it reaches the owner's own space too (never its diaries).
    pub allow_personal_space: bool,
    /// Whether its owner is told of what it does, as of anybody's doing: of a comment
    /// it writes on what they watch, of a mention of them it writes (in a comment, or in
    /// a body), of a card it gives them. The notification names the owner as its doer
    /// and the token by `agentTokenName`. Nobody is told anything in the owner's own
    /// space, whatever this says.
    pub notify_self: bool,
    /// When it was last used, in milliseconds since the Unix epoch, to within five
    /// minutes; absent if never.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_used_at: Option<i64>,
    /// The address it was last used from; absent if never, or unknown.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_used_address: Option<String>,
    /// When it was made.
    pub created_at: i64,
    /// When it stops opening anything.
    pub expires_at: i64,
}

/// A personal API token as an administrator of an organization it reaches sees it:
/// whose it is, what it may do there, and when it was made, last used and ends. What
/// it opens elsewhere (other organizations' workspaces, its owner's own space), where
/// it was used from and what its owner asked of it are its owner's alone.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, utoipa::ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct MemberApiToken {
    /// The token's identifier.
    pub id: String,
    /// Whose it is.
    pub user_id: String,
    /// The name its owner gave it.
    pub name: String,
    /// The first characters of the token (`fvoci_pat_` and six more).
    pub token_prefix: String,
    /// What it may do.
    pub scope: ApiTokenScope,
    /// The workspaces of the administrator's organization it reaches.
    pub workspaces: Vec<ApiTokenWorkspace>,
    /// When it was last used, wherever, in milliseconds since the Unix epoch, to within
    /// five minutes; absent if never.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_used_at: Option<i64>,
    /// When it was made.
    pub created_at: i64,
    /// When it stops opening anything.
    pub expires_at: i64,
}

impl From<ApiToken> for MemberApiToken {
    /// Leaves out what is its owner's alone.
    fn from(token: ApiToken) -> Self {
        Self {
            id: token.id,
            user_id: token.user_id,
            name: token.name,
            token_prefix: token.token_prefix,
            scope: token.scope,
            workspaces: token.workspaces,
            last_used_at: token.last_used_at,
            created_at: token.created_at,
            expires_at: token.expires_at,
        }
    }
}

/// What the owner of an account asks a new token to be (`createApiToken`).
#[derive(Clone, PartialEq, Eq, Deserialize, utoipa::ToSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct NewApiToken {
    /// Its name, 1 to 80 characters after trimming.
    pub name: String,
    /// What it may do.
    pub scope: ApiTokenScope,
    /// The workspaces of organizations it reaches: at most 50, each one the owner works
    /// in (no guest's), and none of the owner's own space, which `allowPersonalSpace`
    /// says.
    #[serde(default)]
    pub workspace_ids: Vec<String>,
    /// Whether it reaches the owner's own space too. An account with a password gives
    /// it in `password`.
    #[serde(default)]
    pub allow_personal_space: bool,
    /// Whether the owner asks to be told of what it does (`ApiToken.notifySelf`). On
    /// unless given.
    #[serde(default = "notify_self_by_default")]
    pub notify_self: bool,
    /// How many days it lasts, 1 to 365; 90 if absent. A token that reaches an
    /// organization that takes only sessions made through its provider lasts no longer
    /// than such a session may: what is asked beyond that is cut to it.
    #[serde(default)]
    pub expires_in_days: Option<u32>,
    /// The account's password, for a token that reaches the owner's own space.
    #[serde(default)]
    pub password: Option<String>,
}

const fn notify_self_by_default() -> bool {
    true
}

impl fmt::Debug for NewApiToken {
    /// Leaves out the password, saying only whether one was given.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("NewApiToken")
            .field("name", &self.name)
            .field("scope", &self.scope)
            .field("workspace_ids", &self.workspace_ids)
            .field("allow_personal_space", &self.allow_personal_space)
            .field("notify_self", &self.notify_self)
            .field("expires_in_days", &self.expires_in_days)
            .field("password_given", &self.password.is_some())
            .finish()
    }
}

/// A token as it is made: the only time the token itself is told.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize, utoipa::ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct IssuedApiToken {
    /// The token, to be sent as `Authorization: Bearer <token>`. It is not stored, and
    /// not shown again.
    pub token: String,
    /// The token as its list shows it.
    pub api_token: ApiToken,
}

impl fmt::Debug for IssuedApiToken {
    /// Leaves out the token: its list's form (`tokenPrefix`) tells which it is.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("IssuedApiToken")
            .field("api_token", &self.api_token)
            .finish_non_exhaustive()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_write_token_reads_and_a_read_token_does_not_write() {
        assert!(ApiTokenScope::Write.covers(ApiTokenScope::Read));
        assert!(ApiTokenScope::Write.covers(ApiTokenScope::Write));
        assert!(ApiTokenScope::Read.covers(ApiTokenScope::Read));
        assert!(!ApiTokenScope::Read.covers(ApiTokenScope::Write));
    }

    #[test]
    fn what_holds_a_secret_does_not_print_it() {
        let new: NewApiToken = serde_json::from_str(
            r#"{"name": "agent", "scope": "read", "password": "hunter2-secret"}"#,
        )
        .expect("a new token");
        let printed = format!("{new:?}");
        assert!(!printed.contains("hunter2-secret"), "{printed}");
        assert!(printed.contains("password_given: true"), "{printed}");
        let issued = IssuedApiToken {
            token: "fvoci_pat_the-secret-itself".to_owned(),
            api_token: ApiToken {
                id: "t".to_owned(),
                user_id: "u".to_owned(),
                name: "agent".to_owned(),
                token_prefix: "fvoci_pat_the-se".to_owned(),
                scope: ApiTokenScope::Read,
                workspaces: Vec::new(),
                allow_personal_space: false,
                notify_self: true,
                last_used_at: None,
                last_used_address: None,
                created_at: 0,
                expires_at: 1,
            },
        };
        let printed = format!("{issued:?}");
        assert!(!printed.contains("the-secret-itself"), "{printed}");
        assert!(printed.contains("fvoci_pat_the-se"), "{printed}");
    }

    #[test]
    fn scopes_round_trip_through_their_text() {
        for scope in [ApiTokenScope::Read, ApiTokenScope::Write] {
            assert_eq!(scope.as_str().parse::<ApiTokenScope>(), Ok(scope));
        }
        assert!("admin".parse::<ApiTokenScope>().is_err());
    }
}
