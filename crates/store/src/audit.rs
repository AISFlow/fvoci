//! What was done that is to be accounted for: the audit log of an organization.
//!
//! A record is written in the transaction of what it records, and never changed: the
//! schema refuses a change, and a removal but with its workspace when that goes for
//! good. A record is a workspace's (read by its members) or the organization's (read by
//! its administrators).

use std::str::FromStr;

use fvoci_model::{AuditAction, AuditPerson, AuditRecord, AuditRecordActor, new_id};

use crate::db::{Bound, Id, OptId, Param, Row, Scope};
use crate::error::StoreError;
use crate::{Paging, Store};

/// Who did what a record says: a person by their account, the server's operator
/// (through the local administration socket), who has none, or the server itself, for
/// what its timers do. Chosen by the entry point of the change, never taken from what
/// a request says.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AuditActor {
    /// The user with this identifier, kept when the account goes.
    Person(String),
    /// Who runs the server.
    ServerOperator,
    /// The server itself: what ends when its time runs out.
    Server,
}

impl AuditActor {
    /// The identifier of the person, or `None` for the server and its operator.
    #[must_use]
    pub fn person_id(&self) -> Option<&str> {
        match self {
            Self::Person(id) => Some(id),
            Self::ServerOperator | Self::Server => None,
        }
    }

    /// Which of them it is, as the log stores it.
    const fn kind(&self) -> ActorKind {
        match self {
            Self::Person(_) => ActorKind::Person,
            Self::ServerOperator => ActorKind::ServerOperator,
            Self::Server => ActorKind::Server,
        }
    }
}

/// Which of them an actor is, as the column `actor_kind` of the log spells it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ActorKind {
    Person,
    ServerOperator,
    Server,
}

impl ActorKind {
    const ALL: [Self; 3] = [Self::Person, Self::ServerOperator, Self::Server];

    const fn as_str(self) -> &'static str {
        match self {
            Self::Person => "person",
            Self::ServerOperator => "server_operator",
            Self::Server => "server",
        }
    }
}

impl FromStr for ActorKind {
    type Err = StoreError;

    fn from_str(text: &str) -> Result<Self, Self::Err> {
        Self::ALL
            .into_iter()
            .find(|kind| kind.as_str() == text)
            .ok_or_else(|| StoreError::Corrupt {
                column: "actor_kind",
                reason: format!("{text:?} is no audit actor kind"),
            })
    }
}

/// One record of the audit log as it is written: who did what to what (and whom), in
/// which workspace, and when. The organization is the one of the store's handle, or
/// the one named to `record_entry_in`.
#[derive(Clone, PartialEq, Eq)]
pub struct AuditEntry {
    /// The workspace it was done in; empty for what was done to the organization.
    pub workspace_id: String,
    /// Who did it.
    pub actor: AuditActor,
    /// What was done.
    pub action: AuditAction,
    /// What it was done to: a board, a workspace or the organization.
    pub target_id: String,
    /// Whom it was done to, a user; empty for an action that names nobody.
    pub subject_id: String,
    /// When, in milliseconds since the epoch; set when the record is written.
    pub created_at: i64,
    /// The issuer a record of a provider's move names; empty for the others.
    pub detail: String,
    /// Who an identity a record of a move tied is: the email address its provider
    /// gave, or its subject; empty for the others.
    pub identity: String,
}

impl std::fmt::Debug for AuditEntry {
    /// Leaves out who the identity is.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AuditEntry")
            .field("workspace_id", &self.workspace_id)
            .field("actor", &self.actor)
            .field("action", &self.action)
            .field("target_id", &self.target_id)
            .field("subject_id", &self.subject_id)
            .field("created_at", &self.created_at)
            .field("detail", &self.detail)
            .finish_non_exhaustive()
    }
}

impl AuditEntry {
    /// A record of `actor_id` doing `action` to `target_id` in the workspace, to nobody.
    #[must_use]
    pub fn new(workspace_id: &str, actor_id: &str, action: AuditAction, target_id: &str) -> Self {
        Self::by(
            &AuditActor::Person(actor_id.to_owned()),
            workspace_id,
            action,
            target_id,
        )
    }

    /// A record of `actor` doing `action` to `target_id` in the workspace, to nobody.
    pub(crate) fn by(
        actor: &AuditActor,
        workspace_id: &str,
        action: AuditAction,
        target_id: &str,
    ) -> Self {
        Self {
            workspace_id: workspace_id.to_owned(),
            actor: actor.clone(),
            action,
            target_id: target_id.to_owned(),
            subject_id: String::new(),
            created_at: 0,
            detail: String::new(),
            identity: String::new(),
        }
    }

    /// The record with whom it was done to.
    #[must_use]
    pub fn with_subject(mut self, subject_id: &str) -> Self {
        subject_id.clone_into(&mut self.subject_id);
        self
    }

    /// The record with the issuer it names.
    #[must_use]
    pub fn with_detail(mut self, detail: &str) -> Self {
        detail.clone_into(&mut self.detail);
        self
    }

    /// The record with who the identity it tied is: `email`, the address its provider
    /// gave, without the blanks around it, or `oidc_subject`, the identity's subject at
    /// its issuer, where that leaves nothing.
    #[must_use]
    pub(crate) fn with_identity(mut self, email: &str, oidc_subject: &str) -> Self {
        let email = email.trim();
        let shown = if email.is_empty() {
            oidc_subject
        } else {
            email
        };
        shown.clone_into(&mut self.identity);
        self
    }
}

/// Whose records are read: a workspace's, or those of the organization itself.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AuditPlace<'a> {
    /// The records of the workspace.
    Workspace(&'a str),
    /// The records of the organization of the handle, of no workspace.
    Organization,
}

impl Store {
    /// The records of the audit log of a workspace of the handle's organization, the
    /// oldest first.
    ///
    /// # Errors
    ///
    /// Returns [`StoreError::Database`] if the query fails, and [`StoreError::Corrupt`]
    /// if a stored action is none.
    pub async fn get_audit_log(&self, workspace_id: &str) -> Result<Vec<AuditEntry>, StoreError> {
        self.with_conn(async |scope| {
            let mut sql = scope.sql(
                "SELECT workspace_id, actor_id, action, target_id, subject_id, created_at, \
                 actor_kind, detail FROM audit_log WHERE workspace_id = ",
            );
            sql.bind(Id(workspace_id)).push(" ORDER BY created_at, id");
            scope
                .fetch_all(sql)
                .await?
                .iter()
                .map(|row| {
                    Ok(AuditEntry {
                        workspace_id: row.id(0)?,
                        actor: actor(row, 6, 1)?,
                        action: action(row, 2)?,
                        target_id: row.id(3)?,
                        subject_id: row.id(4)?,
                        created_at: row.int(5)?,
                        detail: row.text(7)?,
                        // Of the organization's records alone, which are no workspace's.
                        identity: String::new(),
                    })
                })
                .collect()
        })
        .await
    }

    /// One page of the records of a workspace, or of the organization, the newest first
    /// (and by identifier), with the names of who did it and whom it was done to as
    /// their accounts have them now. The target's name is the workspace's for what was
    /// done to a workspace, and none otherwise: what else may be told of a board is the
    /// caller's to say.
    ///
    /// # Errors
    ///
    /// Returns [`StoreError::Database`] if the query fails, and [`StoreError::Corrupt`]
    /// if a stored action is none.
    pub async fn audit_records(
        &self,
        place: AuditPlace<'_>,
        paging: Paging,
    ) -> Result<Vec<AuditRecord>, StoreError> {
        self.with_conn(async |scope| {
            let mut sql = scope.sql(
                "SELECT a.id, a.action, a.actor_id, actor.username, actor.display_name, \
                 a.subject_id, subject.username, subject.display_name, a.target_id, w.name, \
                 a.created_at, a.actor_kind, a.detail, a.identity FROM audit_log AS a \
                 LEFT JOIN users AS actor ON actor.id = a.actor_id \
                 LEFT JOIN users AS subject ON subject.id = a.subject_id \
                 LEFT JOIN workspaces AS w ON w.id = a.target_id AND w.org_id = a.org_id \
                 WHERE a.org_id = ",
            );
            sql.bind(scope.org()?);
            let identities = Identities::of(place);
            match place {
                AuditPlace::Workspace(workspace_id) => {
                    sql.push_bind(" AND a.workspace_id = ", Id(workspace_id));
                }
                AuditPlace::Organization => {
                    sql.push(" AND a.workspace_id IS NULL");
                }
            }
            sql.push(" ORDER BY a.created_at DESC, a.id DESC")
                .push_page(paging);
            scope
                .fetch_all(sql)
                .await?
                .iter()
                .map(|row| record(row, identities))
                .collect()
        })
        .await
    }
}

fn action(row: &Row, index: usize) -> Result<AuditAction, StoreError> {
    row.text(index)?.parse().map_err(
        |error: fvoci_model::UnknownAuditAction| StoreError::Corrupt {
            column: "action",
            reason: error.to_string(),
        },
    )
}

/// The actor of a row, from its kind at `kind` and the person's identifier at `id`.
fn actor(row: &Row, kind: usize, id: usize) -> Result<AuditActor, StoreError> {
    Ok(match row.text(kind)?.parse()? {
        ActorKind::Person => AuditActor::Person(row.id(id)?),
        ActorKind::ServerOperator => AuditActor::ServerOperator,
        ActorKind::Server => AuditActor::Server,
    })
}

/// Whether the records read tell who an identity they tied is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Identities {
    /// Told: the organization's records, which its administrators alone read.
    Told,
    /// Not told: a workspace's, read by its members. No such record is a
    /// workspace's; it would not be told if one were.
    Withheld,
}

impl Identities {
    const fn of(place: AuditPlace<'_>) -> Self {
        match place {
            AuditPlace::Organization => Self::Told,
            AuditPlace::Workspace(_) => Self::Withheld,
        }
    }
}

fn record(row: &Row, identities: Identities) -> Result<AuditRecord, StoreError> {
    let subject_id = row.id(5)?;
    let target_name = row.text(9)?;
    let action = action(row, 1)?;
    Ok(AuditRecord {
        id: row.id(0)?,
        action,
        actor: match actor(row, 11, 2)? {
            AuditActor::Person(id) => AuditRecordActor::Person(AuditPerson {
                id,
                username: row.text(3)?,
                display_name: row.text(4)?,
            }),
            AuditActor::ServerOperator => AuditRecordActor::ServerOperator,
            AuditActor::Server => AuditRecordActor::Server,
        },
        subject: if subject_id.is_empty() {
            None
        } else {
            Some(AuditPerson {
                id: subject_id,
                username: row.text(6)?,
                display_name: row.text(7)?,
            })
        },
        target_id: row.id(8)?,
        // A board's identifier is no workspace's: the join finds none for it.
        target_name: (!target_name.is_empty()).then_some(target_name),
        created_at: row.int(10)?,
        detail: row.opt_text(12)?,
        identity: match identities {
            Identities::Told => row.opt_text(13)?,
            Identities::Withheld => None,
        },
    })
}

/// Writes a record, in the handle's organization, at this moment.
pub(crate) async fn record_entry(
    scope: &mut Scope<'_>,
    entry: &AuditEntry,
) -> Result<(), StoreError> {
    let org = scope.org()?;
    insert_entry(scope, org, entry).await
}

/// Writes a record in the organization `org_id`, at this moment: for a handle with the
/// system's reach, which names no organization of its own. Nothing here checks that
/// the caller may write there (on SQLite no wall would); the caller has chosen
/// `org_id` by what it read in the same transaction.
pub(crate) async fn record_entry_in(
    scope: &mut Scope<'_>,
    org_id: &str,
    entry: &AuditEntry,
) -> Result<(), StoreError> {
    insert_entry(scope, Id(org_id), entry).await
}

async fn insert_entry(
    scope: &mut Scope<'_>,
    org: impl Into<Bound>,
    entry: &AuditEntry,
) -> Result<(), StoreError> {
    let mut sql = scope.sql(
        "INSERT INTO audit_log (id, org_id, workspace_id, actor_id, action, target_id, \
         subject_id, created_at, actor_kind, detail, identity) VALUES (",
    );
    sql.bind(Id(&new_id()))
        .push_bind(", ", org)
        .push_bind(", ", OptId(&entry.workspace_id))
        .push_bind(", ", OptId(entry.actor.person_id().unwrap_or_default()))
        .push_bind(", ", entry.action.as_str())
        .push_bind(", ", Id(&entry.target_id))
        .push_bind(", ", OptId(&entry.subject_id))
        .push_bind(", ", scope.now_millis())
        .push_bind(", ", entry.actor.kind().as_str())
        .push_bind(
            ", ",
            Param::OptText((!entry.detail.is_empty()).then(|| entry.detail.clone())),
        )
        .push_bind(
            ", ",
            Param::OptText((!entry.identity.is_empty()).then(|| entry.identity.clone())),
        )
        .push(")");
    scope.execute(sql).await?;
    Ok(())
}
