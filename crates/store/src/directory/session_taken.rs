//! Which sessions of a user open a workspace: the one rule of it, what it reads of a
//! session, and how it is read of a workspace's organization and its user.

use fvoci_model::{OrgKind, Session, UserWorkspace, WorkspaceRole};

use super::{IN_USE, org_kind, role};
use crate::db::{Id, Scope};
use crate::error::StoreError;

/// How a session was made, and what it has confirmed since: all that the rule reads
/// of a session ([`SessionTaken::opened_by`]). It is read of a [`Session`] and of
/// nothing less, so that whoever asks the rule for a session asks it with all of it.
///
/// `P` is how the provider's identifier is held: `&str` as read of a session for one
/// request, `String` as a connection keeps it for as long as it lasts
/// ([`Making::into_owned`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Making<P = String> {
    /// The provider the session was made through; `None` for one made with the
    /// account's password.
    through: Option<P>,
    /// Whether the session gave the account's password after it was made
    /// (`Store::confirm_session_password`).
    confirmed_password: bool,
}

impl<P> Making<P> {
    /// The making of a session made with the account's password, through no provider.
    #[must_use]
    pub const fn with_password() -> Self {
        Self {
            through: None,
            confirmed_password: false,
        }
    }

    /// The making of a session made through `provider` that has confirmed nothing.
    #[cfg(any(test, feature = "test-util"))]
    #[must_use]
    pub const fn through(provider: P) -> Self {
        Self {
            through: Some(provider),
            confirmed_password: false,
        }
    }

    /// The making a personal API token carries: that of the session it was made by,
    /// through `origin`, its provider. A token is no session: what it opens is asked of
    /// it as of the session that made it. One that reaches the account's own space was
    /// made with the account's password ([`Self::having_confirmed_password`]).
    #[must_use]
    pub const fn of_api_token(origin: Option<P>) -> Self {
        Self {
            through: origin,
            confirmed_password: false,
        }
    }

    /// The same making, of a session that has confirmed the account's password since:
    /// for a connection that kept the making of its handshake and whose session the
    /// store recorded as having confirmed it meanwhile
    /// (`Store::confirm_session_password`).
    #[must_use]
    pub fn having_confirmed_password(self) -> Self {
        Self {
            confirmed_password: true,
            ..self
        }
    }
}

impl Making<&str> {
    /// The same making, for what outlives the session it was read of.
    #[must_use]
    pub fn into_owned(self) -> Making {
        Making {
            through: self.through.map(str::to_owned),
            confirmed_password: self.confirmed_password,
        }
    }
}

impl Making {
    /// The same making, as the rule reads it.
    #[must_use]
    pub fn as_deref(&self) -> Making<&str> {
        Making {
            through: self.through.as_deref(),
            confirmed_password: self.confirmed_password,
        }
    }
}

impl<'a> From<&'a Session> for Making<&'a str> {
    fn from(session: &'a Session) -> Self {
        Self {
            through: session.login_provider_id(),
            confirmed_password: session.password_confirmed_at.is_some(),
        }
    }
}

/// Which sessions of a user open what is a workspace's or its organization's: the one
/// rule of it, for a request, a websocket connection and a room alike.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SessionTaken {
    /// Any session of the user.
    Any,
    /// Only a session made through this provider: the organization takes no password
    /// (`sso_required`), and none that a session confirmed either.
    Through(String),
    /// Only a session that has shown the account's password, by being made with it or
    /// by confirming it since: the user's own space, once the account has a password.
    /// A provider is an organization's, and what a session made through one opens is
    /// what that organization's people may reach; the space is the person's alone.
    WithPassword,
}

impl SessionTaken {
    /// What an organization that people work in takes: sessions made through
    /// `required` and no other, or any if it takes a password too (`None`).
    #[must_use]
    pub fn of_organization(required: Option<String>) -> Self {
        required.map_or(Self::Any, Self::Through)
    }

    /// What the space of an account takes: a session that has shown its password once
    /// it has one, and any until then, as an account without a password has no other.
    /// `password` is the account's stored password hash, or any text that is empty
    /// where that is (`HAS_PASSWORD`): nothing is read of it but that.
    #[must_use]
    pub fn of_space(password: &str) -> Self {
        if password.is_empty() {
            Self::Any
        } else {
            Self::WithPassword
        }
    }

    /// Whether what is refused of a session here is for want of the account's
    /// password, and not of a provider: what the session is told, so that its person
    /// knows which way in to take.
    #[must_use]
    pub fn asks_for_password(&self) -> bool {
        matches!(self, Self::WithPassword)
    }

    /// Whether a session of that `making` opens it. A session made through another
    /// provider opens nothing of an organization that takes one: not one of another
    /// organization, and not the one the organization had before; and the password it
    /// confirmed opens nothing of such an organization, whose the password is not.
    #[must_use]
    pub fn opened_by(&self, making: Making<&str>) -> bool {
        match self {
            Self::Any => true,
            Self::Through(required) => making.through == Some(required.as_str()),
            Self::WithPassword => making.through.is_none() || making.confirmed_password,
        }
    }
}

/// What a workspace takes, from what was read of it: its organization's kind, the
/// provider that organization takes sessions of and no other (empty for none), and
/// whether the user, who is a member of it, has a password ([`HAS_PASSWORD`]).
pub(crate) fn taken(kind: OrgKind, required_provider: String, password: &str) -> SessionTaken {
    match kind {
        OrgKind::Personal => SessionTaken::of_space(password),
        OrgKind::Shared => {
            SessionTaken::of_organization(Some(required_provider).filter(|id| !id.is_empty()))
        }
    }
}

/// A workspace a user is a member of, as it was read, and which sessions it takes.
#[derive(Debug)]
pub(crate) struct Membership {
    pub(crate) id: String,
    org_id: String,
    kind: OrgKind,
    name: String,
    role: WorkspaceRole,
    pub(crate) takes: SessionTaken,
}

impl Membership {
    /// The workspace as a session of that `making` finds it among the user's: listed
    /// whether the session opens it or not, and saying which of the two keeps it
    /// closed.
    pub(super) fn listed_for(self, making: Making<&str>) -> UserWorkspace {
        let closed = !self.takes.opened_by(making);
        UserWorkspace {
            id: self.id,
            org_id: self.org_id,
            kind: self.kind,
            name: self.name,
            role: self.role,
            sso_required: closed && self.kind == OrgKind::Shared,
            password_required: closed && self.kind == OrgKind::Personal,
        }
    }
}

/// The workspaces in use the user is a member of, in every organization, each with
/// the sessions it takes: by organization, the oldest first, and in each the oldest
/// workspace first; the workspace of the user's own space last, whenever it was made.
/// Read through `scope`, which reaches every organization.
pub(crate) async fn memberships_of(
    scope: &mut Scope<'_>,
    user_id: &str,
) -> Result<Vec<Membership>, StoreError> {
    let mut sql = scope.sql([
        "SELECT t.id, t.org_id, t.name, wm.role, p.id, o.kind, ",
        HAS_PASSWORD,
        " FROM workspace_members AS wm \
         JOIN users AS u ON u.id = wm.user_id \
         JOIN workspaces AS t ON t.id = wm.workspace_id \
         JOIN organizations AS o ON o.id = t.org_id \
         LEFT JOIN oidc_providers AS p ON p.org_id = t.org_id AND p.sso_required = ",
    ]);
    sql.bind(true)
        .push_bind(" WHERE wm.user_id = ", Id(user_id))
        .push(IN_USE)
        .push(&scope.same_organization(&[("wm", "t")]))
        .push_bind(" ORDER BY CASE WHEN o.kind = ", OrgKind::Personal.as_str())
        .push(" THEN 1 ELSE 0 END, o.created_at, o.id, t.created_at, t.id");
    scope
        .fetch_all(sql)
        .await?
        .iter()
        .map(|row| {
            let kind = org_kind(row, 5)?;
            Ok(Membership {
                id: row.id(0)?,
                org_id: row.id(1)?,
                kind,
                name: row.text(2)?,
                role: role(row, 3)?,
                takes: taken(kind, row.id(4)?, &row.text(6)?),
            })
        })
        .collect()
}

/// The column that says whether the user `u` has a password, and nothing of it: empty
/// for none.
pub(crate) const HAS_PASSWORD: &str = "CASE WHEN u.password_hash = '' THEN '' ELSE 'set' END";

#[cfg(test)]
mod tests {
    use super::*;

    /// One case of which session opens what.
    struct Row {
        case: &'static str,
        takes: SessionTaken,
        /// How the session was made, and what it confirmed since.
        making: Making<&'static str>,
        opens: bool,
    }

    fn through(provider: &str) -> SessionTaken {
        SessionTaken::of_organization(Some(provider.to_owned()))
    }

    /// The table of which session opens what: the rule of an organization that takes
    /// no password and of a space whose account has one, row by row, for a session as
    /// it was made and for one that confirmed the account's password since.
    #[test]
    fn which_session_opens_what() {
        let rows = [
            Row {
                case: "a password opens an organization that takes one",
                takes: SessionTaken::of_organization(None),
                making: Making::with_password(),
                opens: true,
            },
            Row {
                case: "a provider's session opens an organization that takes a password too",
                takes: SessionTaken::of_organization(None),
                making: Making::through("ours"),
                opens: true,
            },
            Row {
                case: "a password opens nothing of an organization that takes none",
                takes: through("ours"),
                making: Making::with_password(),
                opens: false,
            },
            Row {
                case: "the session of the organization's provider opens it",
                takes: through("ours"),
                making: Making::through("ours"),
                opens: true,
            },
            Row {
                case: "the session of another organization's provider opens nothing",
                takes: through("ours"),
                making: Making::through("theirs"),
                opens: false,
            },
            Row {
                case: "the session of the provider the organization had before opens nothing",
                takes: through("ours-now"),
                making: Making::through("ours-before"),
                opens: false,
            },
            Row {
                case: "any session opens the space of an account without a password",
                takes: SessionTaken::of_space(""),
                making: Making::through("ours"),
                opens: true,
            },
            Row {
                case: "a password opens the space of an account that has one",
                takes: SessionTaken::of_space("a hash"),
                making: Making::with_password(),
                opens: true,
            },
            Row {
                case: "a provider's session opens nothing of the space of an account with a password",
                takes: SessionTaken::of_space("a hash"),
                making: Making::through("ours"),
                opens: false,
            },
            Row {
                case: "a provider's session that confirmed the password opens the space",
                takes: SessionTaken::of_space("a hash"),
                making: Making::through("ours").having_confirmed_password(),
                opens: true,
            },
            Row {
                case: "a confirmed password opens nothing of an organization that takes none",
                takes: through("ours"),
                making: Making::through("theirs").having_confirmed_password(),
                opens: false,
            },
            Row {
                case: "the organization's provider opens it whatever the session confirmed",
                takes: through("ours"),
                making: Making::through("ours").having_confirmed_password(),
                opens: true,
            },
            Row {
                case: "a session made with the password that confirmed it opens what it did",
                takes: SessionTaken::of_space("a hash"),
                making: Making::with_password().having_confirmed_password(),
                opens: true,
            },
            Row {
                case: "a password given twice opens nothing of an organization that takes none",
                takes: through("ours"),
                making: Making::with_password().having_confirmed_password(),
                opens: false,
            },
        ];
        for row in rows {
            assert_eq!(row.takes.opened_by(row.making), row.opens, "{}", row.case);
        }
    }

    /// What the rule reads of a session is read of the session itself: the provider it
    /// was made through and whether it confirmed the password, and kept as it was
    /// read by what outlives the request.
    #[test]
    fn the_making_of_a_session_is_read_of_it() {
        let login = fvoci_model::LoginOrigin {
            provider_id: "ours".to_owned(),
            org_id: "org".to_owned(),
            issuer: "https://one.example".to_owned(),
            subject: None,
            revision: None,
        };
        let made = Session {
            login: Some(login),
            ..Session::default()
        };
        assert_eq!(Making::from(&made), Making::through("ours"));
        let confirmed = Session {
            password_confirmed_at: Some(1),
            ..made
        };
        let making = Making::from(&confirmed);
        assert_eq!(making, Making::through("ours").having_confirmed_password());
        assert_eq!(making.into_owned().as_deref(), making);
        assert_eq!(Making::from(&Session::default()), Making::with_password());
    }

    #[test]
    fn a_space_takes_no_provider_and_an_organization_no_password() {
        assert_eq!(
            taken(OrgKind::Personal, "a-provider".to_owned(), "set"),
            SessionTaken::WithPassword
        );
        assert_eq!(
            taken(OrgKind::Personal, String::new(), ""),
            SessionTaken::Any
        );
        assert_eq!(
            taken(OrgKind::Shared, "a-provider".to_owned(), "set"),
            through("a-provider")
        );
        assert_eq!(
            taken(OrgKind::Shared, String::new(), "set"),
            SessionTaken::Any
        );
    }
}
