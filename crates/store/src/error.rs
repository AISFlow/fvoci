//! The error type of the store.

use fvoci_model::{
    BlockError, CategoryError, ContainerError, ContainersAndBlocksError, Refusals,
    SubscriptionError,
};
use thiserror::Error;

/// An error from a [`Store`](crate::Store) operation.
#[derive(Debug, Error)]
#[non_exhaustive]
pub enum StoreError {
    /// A temporary organization administrator cannot add themselves to a team.
    #[error("cannot add yourself to a team while joined as organization administrator")]
    TeamSelfJoining,

    /// A workspace guest cannot be added to a team.
    #[error("a workspace guest cannot be added to a team")]
    GuestCannotJoinTeam,

    /// A reorder request listed the same identifier more than once.
    #[error("reorder list contains duplicate identifier {0}")]
    DuplicateReorderId(String),

    /// A record that the operation needs does not exist.
    #[error("{{{0}}} not found")]
    NotFound(String),

    /// A change would leave a user with more entries of client state than there may be.
    #[error("a user keeps at most {limit} entries of client state")]
    TooManyClientStateEntries {
        /// The most entries a user may keep.
        limit: usize,
    },

    /// A user tried to register without the signup token although the instance has
    /// its first user already.
    #[error("the first user has registered already")]
    NotFirstUser,

    /// An account was to be made that starts no organization, while the instance has
    /// no first user: the first account starts the instance's own organization.
    #[error("the instance has no first user yet, who starts its organization")]
    NoFirstUser,

    /// An account that made as many organizations as one may, which are in use, asked
    /// to start another.
    #[error("an account starts at most {limit} organizations")]
    TooManyOrganizations {
        /// How many organizations in use one account may have made.
        limit: u32,
    },

    /// A lookup of a single session or public link record matched no row.
    ///
    /// The message stays in the log: the HTTP layer answers "not found" without it.
    #[error("sql: no rows in result set")]
    NoRows,

    /// An update addressed a user that does not exist, by identifier or by name.
    ///
    /// The HTTP layer answers it as "user not found" (404).
    #[error("user not found ({0})")]
    UserNotFound(String),

    /// Fewer records exist than the operation asked for.
    #[error("not all instances of {{{entity}}} in {{{}}} found", .ids.join(", "))]
    NotAllFound {
        /// The kind of record, such as `block`.
        entity: &'static str,
        /// The identifiers that were asked for.
        ids: Vec<String>,
    },

    /// A block failed validation before it was written.
    #[error("error validating block {id}: {source}")]
    InvalidBlock {
        /// The identifier of the block.
        id: String,
        /// What is wrong with it.
        source: BlockError,
    },

    /// A block operation was given unusable arguments.
    #[error(transparent)]
    Block(#[from] BlockError),

    /// A body checked or composed inside the store failed body validation (for example,
    /// appending to an existing diary entry exceeded `BODY_MAX_BYTES` or
    /// `BODY_MAX_NODES`).
    #[error("{source}")]
    InvalidBody {
        /// The request field name that carried the body.
        field: &'static str,
        /// Why the body was refused.
        source: fvoci_model::BodyError,
    },

    /// A board failed validation before it was written.
    #[error(transparent)]
    InvalidContainer(#[from] ContainerError),

    /// A boards-and-blocks request failed validation.
    #[error(transparent)]
    InvalidContainersAndBlocks(#[from] ContainersAndBlocksError),

    /// A block listed for deletion belongs to none of the boards listed with it.
    #[error("block {block_id} doesn't belong to any of the boards in the delete request")]
    BlockNotInDeletedContainers {
        /// The identifier of the block.
        block_id: String,
    },

    /// A category failed validation before it was written.
    #[error(transparent)]
    InvalidCategory(#[from] CategoryError),

    /// A subscription failed validation before it was written.
    #[error(transparent)]
    InvalidSubscription(#[from] SubscriptionError),

    /// Request parameters failed domain validation inside a store transaction.
    #[error(transparent)]
    Refused(#[from] Refusals),

    /// A statement that writes was given text that is not an identifier where it needs
    /// one. The HTTP layer answers it as a bad request.
    #[error("{0:?} is not an identifier")]
    InvalidId(String),

    /// A place of the file storage given to a record, a mark of writing or an upload
    /// session is not spelled the one way ([`fvoci_model::is_storage_path`]). The
    /// server makes its places; it is the server's error.
    #[error("{0:?} is not a place of the file storage")]
    InvalidPath(String),

    /// A row cannot be stored because another one has what must be one row's alone.
    #[error("{0} is taken")]
    Conflict(Taken),

    /// A personal API token was to reach a workspace its maker no longer works in (left
    /// it, was made its guest, or it went to the trash): nothing of the token is stored.
    #[error("the maker of the token no longer works in workspace {0}")]
    NoLongerWorksIn(String),

    /// A block of the trash cannot come back where it was: what it was on has since
    /// become a block it may not be on (a comment is on a card, nothing is below one).
    #[error("block {id} cannot come back where it was: {source}")]
    NotRestorable {
        /// The block at the top of what would come back.
        id: String,
        /// What it would break.
        source: BlockError,
    },

    /// A patch was made against a block as it was before its last change.
    #[error("block {id} changed at {updated_at}, after it was read at {read_at}")]
    Stale {
        /// The block.
        id: String,
        /// The `updatedAt` the patch was made against.
        read_at: i64,
        /// The block's `updatedAt`.
        updated_at: i64,
    },

    /// A block's document was written since whoever writes it now read it: it logged
    /// an update, was folded, was made again, or its body was written to the block.
    #[error("the document of block {block_id} was written since it was read")]
    DocumentWritten {
        /// The block whose document it is.
        block_id: String,
    },

    /// The operation is about one organization and the store's handle names none
    /// ([`Tenant`](crate::Tenant)). A mistake of the caller: the HTTP layer answers it
    /// as a server error.
    #[error("the operation needs an organization, and the store was given none")]
    NoOrganization,

    /// The database configuration cannot be used.
    #[error("invalid database configuration: {0}")]
    Config(String),

    /// The database holds a schema this server cannot use.
    #[error("unsupported database schema: {0}")]
    Schema(String),

    /// The database's schema is to be brought up to date, and the role the store
    /// connects as may not: it does not own the tables. Nothing was changed.
    #[error(
        "the database's schema is not this server's ({reason}), and the role {role} does not \
         own the tables of the schema {schema}: bring it up to date as the role that owns \
         them, with the same search_path"
    )]
    SchemaNotOwned {
        /// The role the store connects as.
        role: String,
        /// The schema the role works in.
        schema: String,
        /// What is to be done to the schema.
        reason: String,
    },

    /// A stored value cannot be decoded.
    #[error("corrupt value in column {column}: {reason}")]
    Corrupt {
        /// The column, as named in the query.
        column: &'static str,
        /// Why the value was rejected.
        reason: String,
    },

    /// The database reported an error.
    #[error("database error: {0}")]
    Database(#[from] sqlx::Error),
}

/// What of a user is another user's already.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Taken {
    /// The user name, in any letter case.
    Username,
    /// The email address, in any letter case.
    Email,
    /// The subject of an issuer: another account is the one its provider vouches for.
    Identity,
    /// The name of a workspace: another workspace in use of the organization has it.
    WorkspaceName,
    /// The name of a team: another team in the workspace has it.
    TeamName,
    /// The last use of a link of an invitation: it was used as often as it may be.
    LinkUse,
}

impl std::fmt::Display for Taken {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(match self {
            Self::Username => "the user name",
            Self::Email => "the email address",
            Self::Identity => "the identity at the provider",
            Self::WorkspaceName => "the name of the workspace",
            Self::TeamName => "the name of the team",
            Self::LinkUse => "the last use of the invitation",
        })
    }
}

impl StoreError {
    /// Reads the violation of a foreign key as "`what` not found": the row that was to
    /// be stored refers to something that does not exist, or no longer does. Any other
    /// error is returned as it is.
    #[must_use]
    pub(crate) fn missing_reference(self, what: &'static str) -> Self {
        match self {
            Self::Database(error)
                if error
                    .as_database_error()
                    .is_some_and(sqlx::error::DatabaseError::is_foreign_key_violation) =>
            {
                Self::not_found(what)
            }
            other => other,
        }
    }

    /// Returns `true` if the pool of connections gave none in its own time.
    #[must_use]
    pub fn is_pool_timeout(&self) -> bool {
        matches!(self, Self::Database(sqlx::Error::PoolTimedOut))
    }

    /// Returns `true` if the database did not take the statement in the time it had: no
    /// connection came free, or an SQLite file was held by another writer for longer
    /// than the busy timeout (`SQLITE_BUSY` and its extended codes). The statement did
    /// nothing, and may do when it is tried again.
    #[must_use]
    pub fn is_busy(&self) -> bool {
        /// `SQLITE_BUSY`, the low byte of its extended codes too.
        const SQLITE_BUSY: i32 = 5;
        let Self::Database(error) = self else {
            return false;
        };
        let held = error
            .as_database_error()
            .and_then(|error| error.try_downcast_ref::<sqlx::sqlite::SqliteError>())
            .and_then(sqlx::error::DatabaseError::code)
            .and_then(|code| code.parse::<i32>().ok())
            .is_some_and(|code| code & 0xff == SQLITE_BUSY);
        held || self.is_pool_timeout()
    }

    /// Returns `true` if the error means a record does not exist, which the HTTP layer
    /// reports as "not found" rather than as a server error.
    pub fn is_not_found(&self) -> bool {
        matches!(
            self,
            Self::NotFound(_) | Self::NotAllFound { .. } | Self::NoRows
        )
    }

    pub(crate) fn not_found(what: impl std::fmt::Display) -> Self {
        Self::NotFound(what.to_string())
    }
}
