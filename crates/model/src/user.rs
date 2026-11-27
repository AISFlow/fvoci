//! Users, their settings and client state, and their login sessions.

use std::collections::BTreeMap;
use std::fmt;

use serde::{Deserialize, Serialize};

use crate::person_name::PersonName;
use crate::refusal::{RefusalReason, Refusals};
use crate::settings::{
    CLIENT_STATE_MAX_NAME_LENGTH, ClientStateError, check_client_state_name,
    check_client_state_value,
};

/// What a user is to the instance.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InstanceRole {
    /// An administrator of the instance.
    Administrator,
    /// A user like any other.
    User,
}

/// The user of a session, as their own account is answered to them: the user, and what
/// nobody else is told of the account.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize, utoipa::ToSchema)]
#[serde(default, rename_all = "camelCase")]
pub struct Me {
    /// The user, as anybody who sees them is answered them.
    #[serde(flatten)]
    pub user: User,
    /// Whether the account has a password to sign in with. An account made by a
    /// sign-in through an organization's provider has none until its person sets one
    /// (`setPassword`), and signs in through the provider only until then.
    pub has_password: bool,
    /// The email address of the account, which nobody else is answered; empty for an
    /// account that has none.
    pub email: String,
    /// The address the account waits to have confirmed (`requestEmailChange`), which
    /// becomes `email` by the link mailed to it; left out when it waits for none.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub pending_email: Option<String>,
}

impl Me {
    /// The user as their own account is answered to them, waiting to have
    /// `pending_email` confirmed if any. Reads whether there is a password before the
    /// hash is blanked: `user` is as the store has it.
    #[must_use]
    pub fn of(mut user: User, pending_email: Option<String>) -> Self {
        let has_password = user.has_password();
        user.password_hash.clear();
        let email = user.email.clone();
        Self {
            user,
            has_password,
            email,
            pending_email,
        }
    }
}

/// A user account.
///
/// The JSON form carries only the public members. The email address, password hash and
/// the other authentication members are never serialized and are ignored when decoding.
#[derive(Clone, PartialEq, Eq, Default, Serialize, Deserialize, utoipa::ToSchema)]
#[serde(default, rename_all = "camelCase")]
pub struct User {
    /// The user's identifier.
    pub id: String,
    /// The login name.
    pub username: String,
    /// The name the user is shown by, by the rule of [`PersonName::shown`]: empty when
    /// that is the login name, which is shown then. Kept with the account and made anew
    /// whenever its name changes; what a request says of it is not taken over. At most
    /// 129 UTF-16 code units (a name's two parts and a space), not unique.
    pub display_name: String,
    /// The parts of the user's name, which of them the user is shown by, and their
    /// title and credentials. Whoever sees the user's name sees these.
    #[serde(flatten)]
    pub name: PersonName,
    /// The email address.
    #[serde(skip)]
    pub email: String,
    /// When `email` was verified (by confirmation link or by the operator), in
    /// milliseconds since the Unix epoch; zero when `email` is empty or was entered
    /// without mail verification.
    #[serde(skip)]
    pub email_verified_at: i64,
    /// The hash of the password, as a PHC string.
    #[serde(skip)]
    pub password_hash: String,
    /// Since when the password is taken nowhere, in milliseconds since the Unix epoch;
    /// zero while it is taken. A link mailed to an address the account had gave that
    /// address back then, and a new password set through a reset link or by the
    /// operator ends it. The account still has a password.
    #[serde(skip)]
    pub password_locked_at: i64,
    /// Creation time in milliseconds since the Unix epoch; zero for a deleted account
    /// (an empty `username`).
    pub created_at: i64,
    /// Last modification time in milliseconds since the Unix epoch; zero for a deleted
    /// account.
    pub updated_at: i64,
    /// When the account was put out of use (deactivated, or its deletion asked), in
    /// milliseconds since the Unix epoch; absent for one in use, and for a deleted
    /// account, which an empty `username` tells.
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "crate::serde_util::positive_millis"
    )]
    #[schema(nullable = false)]
    pub deleted_at: Option<i64>,
    /// Whether the user is an administrator of the instance. Decided by the server;
    /// nothing a request says about it is taken over.
    pub is_admin: bool,
}

impl User {
    /// Whether a stored password hash means the account has a password to sign in with.
    #[must_use]
    pub fn has_password_hash(password_hash: &str) -> bool {
        !password_hash.is_empty()
    }

    /// Whether the account has a password to sign in with.
    #[must_use]
    pub fn has_password(&self) -> bool {
        Self::has_password_hash(&self.password_hash)
    }

    /// Whether the password is taken nowhere ([`Self::password_locked_at`]).
    #[must_use]
    pub fn password_is_locked(&self) -> bool {
        self.password_locked_at != 0
    }
}

impl fmt::Debug for User {
    /// Leaves out the password hash.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("User")
            .field("id", &self.id)
            .field("username", &self.username)
            .field("display_name", &self.display_name)
            .field("email", &self.email)
            .field("email_verified_at", &self.email_verified_at)
            .field("password_locked_at", &self.password_locked_at)
            .field("name", &self.name)
            .field("created_at", &self.created_at)
            .field("updated_at", &self.updated_at)
            .field("deleted_at", &self.deleted_at)
            .field("is_admin", &self.is_admin)
            .finish_non_exhaustive()
    }
}

/// A change to the bag of state a client keeps with the account: entries the server
/// stores without knowing what they mean.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize, utoipa::ToSchema)]
#[serde(default, rename_all = "camelCase")]
pub struct ClientStatePatch {
    /// Entries to set, by name.
    pub updated_fields: BTreeMap<String, String>,
    /// Names of entries to remove.
    pub deleted_fields: Vec<String>,
}

impl ClientStatePatch {
    /// Checks that every name the change sets and every value it sets it to is one the
    /// bag takes. Names to remove are not checked: removing what cannot be there
    /// removes nothing.
    ///
    /// # Errors
    ///
    /// Returns every entry, in the order of the names, that the bag does not take: each
    /// at `/updatedFields/<name>`, with what is wrong with its name or its value.
    pub fn validate(&self) -> Result<(), Refusals> {
        let mut refused = Refusals::new();
        for (name, value) in &self.updated_fields {
            let checked =
                check_client_state_name(name).and_then(|()| check_client_state_value(value));
            if let Err(error) = checked {
                let reason = match error {
                    ClientStateError::InvalidName if name.is_empty() => RefusalReason::Required,
                    ClientStateError::InvalidName if name.len() > CLIENT_STATE_MAX_NAME_LENGTH => {
                        RefusalReason::TooLong
                    }
                    ClientStateError::InvalidName => RefusalReason::InvalidCharacters,
                    ClientStateError::ValueTooLong => RefusalReason::TooLong,
                };
                refused.refuse_within("updatedFields", name, reason, error.to_string());
            }
        }
        refused.into_result()
    }
}

/// Which sessions an operation of the store ended: the connections made with them are
/// to be closed. The store tells it by the witness it returns (`SessionsEnded`).
#[derive(Debug, PartialEq, Eq)]
pub enum EndedSessionScope {
    /// No sessions ended.
    None,
    /// One session, by identifier.
    Session(String),
    /// Specific sessions, by identifier.
    Sessions(Vec<String>),
    /// Every session of this user.
    User(String),
    /// Every session of `user_id` except `kept_session_id`.
    UserExcept {
        /// The user whose other sessions ended.
        user_id: String,
        /// The session kept open.
        kept_session_id: String,
    },
    /// Every session created through a provider of `org_id` with `issuer`.
    Origin {
        /// Organization of the provider.
        org_id: String,
        /// Issuer of the provider.
        issuer: String,
    },
    /// Every session of `user_id` created for `(issuer, subject)` (or without subject).
    Identity {
        /// The user who unlinked the identity.
        user_id: String,
        /// Issuer of the unlinked identity.
        issuer: String,
        /// Subject of the unlinked identity.
        subject: String,
    },
}

/// A login session, as the server keeps it. The token the client presents is not part
/// of it: only a digest of the token is stored, and that stays in the store.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Session {
    /// The session's identifier.
    pub id: String,
    /// The identifier of the signed-in user.
    pub user_id: String,
    /// When the session was made, in milliseconds since the Unix epoch.
    pub created_at: i64,
    /// When the session was last seen in use, in milliseconds since the Unix epoch. It
    /// is not written on every request, so it may be a little behind.
    pub last_used_at: i64,
    /// When the session ends whatever its use, in milliseconds since the Unix epoch.
    pub expires_at: i64,
    /// The `User-Agent` the session was made by, for telling a user's sessions apart.
    pub user_agent: String,
    /// The `OpenID Connect` provider the session was made through; `None` for a session
    /// made with a password.
    pub login: Option<LoginOrigin>,
    /// When the session confirmed the account's password, in milliseconds since the
    /// Unix epoch; `None` for one that has not. It stays for as long as the session
    /// does.
    pub password_confirmed_at: Option<i64>,
}

impl Session {
    /// The identifier of the provider the session was made through, if any.
    #[must_use]
    pub fn login_provider_id(&self) -> Option<&str> {
        self.login.as_ref().map(|login| login.provider_id.as_str())
    }
}

/// Where a session made through a provider came from. Kept as it was when the session
/// was made: removing the provider or changing its issuer ends the sessions of the
/// organization and issuer, and unlinking an identity those made for it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LoginOrigin {
    /// The provider row the session was made through; a replaced row gets a new one.
    pub provider_id: String,
    /// The organization whose provider it was.
    pub org_id: String,
    /// The provider's issuer at the time.
    pub issuer: String,
    /// The subject the provider gave: with the issuer, the identity the session was
    /// made for. `None` for a session made before that was kept, which is taken for
    /// one of every identity of its issuer.
    pub subject: Option<String>,
    /// The revision of the provider's sign-in settings it was made at: it proves that
    /// revision only. `None` for a session made before that was kept, which proves
    /// none.
    pub revision: Option<i64>,
}

impl LoginOrigin {
    /// Whether the session was made for the identity `subject` of `issuer`, or may have
    /// been: one that knows its issuer only answers for every subject of it.
    #[must_use]
    pub fn is_of_identity(&self, issuer: &str, subject: &str) -> bool {
        self.issuer == issuer
            && self
                .subject
                .as_deref()
                .is_none_or(|made_for| made_for == subject)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn made_for(subject: Option<&str>) -> LoginOrigin {
        LoginOrigin {
            provider_id: "provider".to_owned(),
            org_id: "org".to_owned(),
            issuer: "https://one.example".to_owned(),
            subject: subject.map(str::to_owned),
            revision: Some(1),
        }
    }

    /// A session is of the identity it was made for, and of no other; one that knows
    /// no subject is of every identity of its issuer, and of none of another issuer.
    #[test]
    fn a_session_is_of_the_identity_it_was_made_for() {
        let cases = [
            (Some("first"), "https://one.example", "first", true),
            (Some("first"), "https://one.example", "second", false),
            (None, "https://one.example", "second", true),
            (Some("first"), "https://two.example", "first", false),
            (None, "https://two.example", "first", false),
        ];
        for (subject, issuer, asked, answer) in cases {
            assert_eq!(
                made_for(subject).is_of_identity(issuer, asked),
                answer,
                "made for {subject:?}, asked of {asked} at {issuer}"
            );
        }
    }
}
