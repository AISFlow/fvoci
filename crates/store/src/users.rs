//! Users, their settings, and their client state.
//!
//! Accounts belong to the service. Workspace people are selected by membership and
//! visibility in `directory.rs`; these account operations do not choose a workspace.
//!
//! An account is made in `accounts.rs`.

use std::collections::BTreeMap;

use fvoci_model::{
    CLIENT_STATE_MAX_ENTRIES, ClientStatePatch, InstanceRole, OrgRole, User, UserSettings,
    UserSettingsPatch,
};

use crate::Store;
use crate::audit::AuditActor;
use crate::db::{FromRow, Id, InstanceHeld, LockRank, Param, Row, RowLock, Scope, Sql};
use crate::error::{StoreError, Taken};
use crate::password_reset;
use crate::person_names;
use crate::sessions::{self, PasswordSet, SessionsEnded};
use crate::{directory, members};

/// The columns of a user, in the order [`User::from_row`] reads them.
pub(crate) const USER_COLUMNS: &str = concat!(
    "id, username, email, password_hash, password_locked_at, created_at, updated_at, deleted_at, \
     is_admin, display_name, email_verified_at, ",
    person_names::name_columns!()
);

/// What became of a change to who is an administrator of the instance.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AdminChange {
    /// The user is, or is no longer, an administrator: the user as stored now.
    Done(Box<User>),
    /// There is no such user, or the user is deactivated; nothing changed.
    NoSuchUser,
    /// The user is the only administrator and the change would have ended that;
    /// nothing changed.
    LastAdmin,
    /// Who asked is no administrator of the instance, or no longer one by the time the
    /// change was to be made; nothing changed.
    NotAnAdministrator,
}

/// Who asks for a change of the instance's administrators.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Asker {
    /// An administrator of the instance, who still is one when the change is made.
    Administrator,
    /// The operator, through the administration socket, whom no account stands for.
    Operator,
}

/// The most users a search returns.
const SEARCH_LIMIT: usize = 10;

/// The order of a list of users: by when their accounts were made, and by identifier
/// where that is the same.
const OLDEST_FIRST: &str = " ORDER BY created_at, id";

/// The condition that an account waits to be deleted: its person asked, and may still
/// take the asking back (`account_deletion.rs`). Not once its erasing began.
pub(crate) const WAITS: &str = "delete_after > 0 AND erase_started_at = 0";

/// The columns of `user_settings` read for [`UserSettings`].
const USER_SETTINGS_COLUMNS: &str = "language, theme, density, card_open, haptics, week_start, \
                                     hour_cycle, time_zone, notify_mentions, notify_assignments, \
                                     notify_comments, notify_due";

impl Store {
    /// Returns the number of users that are not deleted.
    ///
    /// # Errors
    ///
    /// Returns [`StoreError::Database`] if the query fails.
    pub async fn get_registered_user_count(&self) -> Result<i64, StoreError> {
        self.with_conn(async |scope| {
            let sql = scope.sql("SELECT COUNT(*) FROM users WHERE deleted_at IS NULL");
            Ok(scope.first(sql).await?.unwrap_or(0))
        })
        .await
    }

    /// Returns the user with the given identifier, unless deleted.
    ///
    /// # Errors
    ///
    /// Returns [`StoreError::NotFound`] if there is no such user.
    pub async fn get_user_by_id(&self, user_id: &str) -> Result<User, StoreError> {
        self.with_conn(async |scope| get_user_by_id(scope, user_id).await)
            .await
    }

    /// The identifier of the account with the login name, in any letter case, whatever
    /// its state: in use, out of use, waiting to be deleted or being erased. `None` if
    /// there is none: an account that was erased has no name. For the server's
    /// operator, who erases an account by its name.
    ///
    /// # Errors
    ///
    /// Returns [`StoreError::Database`] if the query fails.
    pub async fn account_id_by_username(
        &self,
        username: &str,
    ) -> Result<Option<String>, StoreError> {
        self.with_conn(async |scope| {
            let mut sql = scope.sql("SELECT id FROM users WHERE erased_at = 0 AND ");
            Lookup::Username(username).push_condition(&mut sql);
            scope.first(sql).await
        })
        .await
    }

    /// Returns the users with the given identifiers, oldest first.
    ///
    /// # Errors
    ///
    /// Returns [`StoreError::NotFound`] if none of the users exists and
    /// [`StoreError::NotAllFound`] unless exactly one user is found per identifier.
    #[cfg(any(test, feature = "test-util"))]
    pub async fn get_users_list(&self, user_ids: &[String]) -> Result<Vec<User>, StoreError> {
        self.with_conn(async |scope| {
            crate::users_by_id::users_by_ids(scope, user_ids, |_, _| ()).await
        })
        .await
    }

    /// Returns the user with the given email address, unless deleted.
    ///
    /// The address is compared without regard to case, as the unique index on it is:
    /// all letters on PostgreSQL, the ASCII letters on SQLite. It is the account's own
    /// address: an account made through a provider has none, and the address the
    /// provider gave for it finds nothing here.
    ///
    /// # Errors
    ///
    /// Returns [`StoreError::NotFound`] if there is no such user.
    pub async fn get_user_by_email(&self, email: &str) -> Result<User, StoreError> {
        self.with_conn(async |scope| find_user(scope, Lookup::Email(email)).await)
            .await
    }

    /// Returns the user with the given name, unless deleted.
    ///
    /// The name is compared without regard to case, as the unique index on it is: all
    /// letters on PostgreSQL, the ASCII letters on SQLite.
    ///
    /// # Errors
    ///
    /// Returns [`StoreError::NotFound`] if there is no such user.
    pub async fn get_user_by_username(&self, username: &str) -> Result<User, StoreError> {
        self.with_conn(async |scope| find_user(scope, Lookup::Username(username)).await)
            .await
    }

    /// Returns the account with the given identifier that somebody can sign in to: one
    /// in use, or one that waits to be deleted (its `deleted_at` is set then),
    /// whose session opens the way back and nothing else.
    ///
    /// # Errors
    ///
    /// Returns [`StoreError::NotFound`] if there is no such account.
    pub async fn account_by_id(&self, user_id: &str) -> Result<User, StoreError> {
        self.with_conn(async |scope| account_by_id(scope, user_id).await)
            .await
    }

    /// Returns the account with the given name, in any letter case, that somebody can
    /// sign in to, as [`Store::account_by_id`] has it.
    ///
    /// # Errors
    ///
    /// Returns [`StoreError::NotFound`] if there is no such account.
    pub async fn account_by_username(&self, username: &str) -> Result<User, StoreError> {
        self.with_conn(async |scope| find_account(scope, Lookup::Username(username)).await)
            .await
    }

    /// Returns the account with the given email address as its own, in any letter case,
    /// that somebody can sign in to, as [`Store::account_by_id`] has it.
    ///
    /// # Errors
    ///
    /// Returns [`StoreError::NotFound`] if there is no such account.
    pub async fn account_by_email(&self, email: &str) -> Result<User, StoreError> {
        self.with_conn(async |scope| find_account(scope, Lookup::Email(email)).await)
            .await
    }

    /// Makes a user an administrator of the instance, or ends that, unless it would
    /// leave the instance without an administrator. Deactivated users do not count as
    /// administrators.
    ///
    /// The check and the write are one step: of two administrators who give up the role
    /// at once, one is refused.
    ///
    /// Through a handle that names an organization the user's role in it follows, by
    /// the organization's own way (`org_admin::change_org_role`): the instance's
    /// administrators are the organization's, for now. No role in a workspace changes.
    /// Then the organization's last administrator stays one too, and one who stops being
    /// its administrator is taken out of the workspaces they joined as one, on behalf of
    /// `actor_id`, who is recorded doing so.
    ///
    /// `actor_id` asks, and must still administer the instance once the instance's row
    /// is held: a request let in just before they stopped being one changes nothing.
    ///
    /// # Errors
    ///
    /// Returns [`StoreError::Database`] if a statement fails.
    pub async fn set_user_admin(
        &self,
        user_id: &str,
        role: InstanceRole,
        actor_id: &str,
    ) -> Result<AdminChange, StoreError> {
        self.change_instance_role(user_id, role, actor_id, Asker::Administrator)
            .await
    }

    /// Makes the user an administrator of the instance on the operator's word, as
    /// [`Store::set_user_admin`] does but asked by no administrator: for the
    /// administration socket, the way back in when no administrator can sign in. The
    /// change is recorded as the server operator's.
    ///
    /// # Errors
    ///
    /// Returns [`StoreError::Database`] if a statement fails.
    pub async fn make_admin_by_operator(&self, user_id: &str) -> Result<AdminChange, StoreError> {
        let role = InstanceRole::Administrator;
        self.change_instance_role(user_id, role, user_id, Asker::Operator)
            .await
    }

    async fn change_instance_role(
        &self,
        user_id: &str,
        role: InstanceRole,
        actor_id: &str,
        asker: Asker,
    ) -> Result<AdminChange, StoreError> {
        let is_admin = role == InstanceRole::Administrator;
        let actor = match asker {
            Asker::Administrator => AuditActor::Person(actor_id.to_owned()),
            Asker::Operator => AuditActor::ServerOperator,
        };
        // The instance's organization as it is now, whose administrators are the
        // instance's; asked again in the transaction, after the instance's row is held:
        // were it another by then, nothing is followed into this one. (It cannot be
        // another today: the oldest organization goes with no account, the only way
        // one goes, and a new one is the youngest.)
        let store = match self.instance_organization().await? {
            Some(org_id) => self.for_org(&org_id)?,
            None => self.scoped(crate::Tenant::Unset),
        };
        store
            .transaction(async |scope| {
                lock_instance(scope).await?;
                let follows = directory::is_the_instances_organization(scope).await?;

                let mut sql = select_users(scope);
                sql.push_bind(" AND id = ", Id(user_id));
                let Some(current) = fetch_users(scope, sql).await?.into_iter().next() else {
                    return Ok(AdminChange::NoSuchUser);
                };
                if asker == Asker::Administrator {
                    let mut sql =
                        scope.sql("SELECT 1 FROM users WHERE deleted_at IS NULL AND is_admin = ");
                    sql.bind(true).push_bind(" AND id = ", Id(actor_id));
                    if !scope.exists(sql).await? {
                        return Ok(AdminChange::NotAnAdministrator);
                    }
                }
                if current.is_admin && !is_admin {
                    let mut sql =
                        scope.sql("SELECT 1 FROM users WHERE deleted_at IS NULL AND is_admin = ");
                    sql.bind(true)
                        .push_bind(" AND id <> ", Id(user_id))
                        .push(" LIMIT 1");
                    if !scope.exists(sql).await? {
                        return Ok(AdminChange::LastAdmin);
                    }
                    if follows && crate::org_admin::is_last_org_admin(scope, user_id).await? {
                        return Ok(AdminChange::LastAdmin);
                    }
                }
                if current.is_admin != is_admin {
                    if follows {
                        crate::org_admin::lock_organization(scope).await?;
                    }
                    let now = scope.now_millis();
                    let mut sql = scope.sql("UPDATE users SET is_admin = ");
                    sql.bind(is_admin)
                        .push_bind(", updated_at = ", now)
                        .push_bind(" WHERE id = ", Id(user_id));
                    scope.execute(sql).await?;
                    if follows {
                        members::follow_instance_role(scope, user_id, role, &actor).await?;
                    }
                }
                let user = get_user_by_id(scope, user_id).await?;
                Ok(AdminChange::Done(Box::new(user)))
            })
            .await
    }

    /// Sets the password hash of the user with the given identifier and deletes their
    /// sessions, except the one named: the session the password was changed through
    /// stays, every other device has to sign in again with the new password. The
    /// session that stays is found under `kept_session_token_digest` from then on: what
    /// it could do has changed, so its old token is no longer accepted.
    ///
    /// The hash is replaced only while it is still `seen_password_hash`, the hash the
    /// old password was checked against, and is not locked (`lock_password`). Returns
    /// whether it was: `false` if the password was set anew or locked in the meantime,
    /// and nothing is changed then.
    ///
    /// All of it happens or none.
    ///
    /// # Errors
    ///
    /// Returns [`StoreError::Database`] if a statement fails.
    pub async fn change_user_password(
        &self,
        user_id: &str,
        seen_password_hash: &str,
        password_hash: &str,
        kept_session_id: &str,
        kept_session_token_digest: &str,
    ) -> Result<Option<SessionsEnded>, StoreError> {
        // A first password changes which sessions the account's own space takes
        // (`SessionTaken::of_space`): what was read of that is read again.
        let space = if seen_password_hash.is_empty() {
            self.space_of(user_id).await?
        } else {
            None
        };
        self.transaction(async |scope| {
            let now = scope.now_millis();
            let mut sql = scope.sql("UPDATE users SET ");
            sql.push_bind("password_hash = ", password_hash)
                .push_bind(", updated_at = ", now)
                .push_bind(" WHERE deleted_at IS NULL AND id = ", Id(user_id))
                .push_bind(" AND password_hash = ", seen_password_hash)
                .push_bind(" AND password_locked_at = ", 0_i64);
            if scope.execute(sql).await? == 0 {
                return Ok(None);
            }
            if let Some(space) = &space {
                scope.invalidate_visibility_of(space)?;
            }
            let ended =
                sessions::delete_other_sessions_of_user(scope, user_id, kept_session_id).await?;
            sessions::replace_token_digest(scope, kept_session_id, kept_session_token_digest)
                .await?;
            password_reset::end_links_of_user(scope, user_id).await?;
            Ok(Some(ended))
        })
        .await
    }

    /// Replaces a user's password hash by one made from the same password with other
    /// parameters, provided the stored hash is still `old_hash`. Sessions stay: the
    /// password did not change. Returns whether the hash was replaced; it is not if the
    /// password was changed in the meantime.
    ///
    /// # Errors
    ///
    /// Returns [`StoreError::Database`] if the statement fails.
    pub async fn replace_password_hash(
        &self,
        user_id: &str,
        old_hash: &str,
        new_hash: &str,
    ) -> Result<bool, StoreError> {
        self.with_write_conn(async |scope| {
            let mut sql = scope.sql("UPDATE users SET ");
            sql.push_bind("password_hash = ", new_hash)
                .push_bind(" WHERE id = ", Id(user_id))
                .push_bind(" AND password_hash = ", old_hash);
            Ok(scope.execute(sql).await? > 0)
        })
        .await
    }

    /// Returns every user that is not deleted, oldest first.
    ///
    /// # Errors
    ///
    /// Returns [`StoreError::Database`] if the query fails.
    #[cfg(any(test, feature = "test-util"))]
    pub async fn get_users_by_workspace(&self) -> Result<Vec<User>, StoreError> {
        self.with_conn(async |scope| {
            let mut sql = select_users(scope);
            sql.push(OLDEST_FIRST);
            fetch_users(scope, sql).await
        })
        .await
    }

    /// Sets and removes entries of the bag of client state of a user and returns the
    /// bag as it is afterwards, keyed by entry name.
    ///
    /// A name that is both set and removed ends up removed. Either all changes are
    /// stored or none. Whether the names and values are ones the bag takes is the
    /// caller's to check; how many there are is checked here, with the change.
    ///
    /// # Errors
    ///
    /// Returns [`StoreError::TooManyClientStateEntries`] if the bag would hold more than
    /// `fvoci_model::CLIENT_STATE_MAX_ENTRIES` entries, and [`StoreError::UserNotFound`]
    /// if there is no such user; nothing is stored then.
    pub async fn patch_client_state(
        &self,
        user_id: &str,
        patch: &ClientStatePatch,
    ) -> Result<BTreeMap<String, String>, StoreError> {
        self.transaction(async |scope| patch_client_state(scope, user_id, patch).await)
            .await
    }

    /// Returns what the user has set about how things are shown to them; what was never
    /// set, and what this build cannot read, is at its default.
    ///
    /// # Errors
    ///
    /// Returns [`StoreError::Database`] if the query fails.
    pub async fn get_user_settings(&self, user_id: &str) -> Result<UserSettings, StoreError> {
        self.with_conn(async |scope| stored_settings(scope, user_id).await)
            .await
    }

    /// Changes the settings `patch` names and returns all of them as they are
    /// afterwards. All or nothing.
    ///
    /// # Errors
    ///
    /// Returns [`StoreError::UserNotFound`] if there is no such user.
    pub async fn patch_user_settings(
        &self,
        user_id: &str,
        patch: &UserSettingsPatch,
    ) -> Result<UserSettings, StoreError> {
        self.transaction(async |scope| {
            lock_user(scope, user_id).await?;
            upsert_user_settings(scope, user_id, patch).await?;
            stored_settings(scope, user_id).await
        })
        .await
    }

    /// Returns the bag of client state of a user, keyed by entry name.
    ///
    /// # Errors
    ///
    /// Returns [`StoreError::Database`] if the query fails.
    pub async fn get_client_state(
        &self,
        user_id: &str,
    ) -> Result<BTreeMap<String, String>, StoreError> {
        self.with_conn(async |scope| get_client_state(scope, user_id).await)
            .await
    }
}

/// Starts the query for the users that are not deleted; conditions follow with `AND`.
fn select_users(scope: &Scope<'_>) -> Sql {
    scope.sql([
        "SELECT ",
        USER_COLUMNS,
        " FROM users WHERE deleted_at IS NULL",
    ])
}

/// The ten oldest users among those `narrow` keeps, oldest first, whose login name or
/// name (`PersonName::contains`) contains `search_query`, ignoring case; the ten oldest
/// of them for an empty query.
///
/// Compared here rather than in SQL, so that both databases fold every letter the same
/// way and `%` and `_` are treated as literal characters.
///
/// # Errors
///
/// Returns [`StoreError::Database`] if a query fails.
pub(crate) async fn search_users(
    scope: &mut Scope<'_>,
    search_query: &str,
    narrow: impl Fn(&mut Sql, &Scope<'_>),
) -> Result<Vec<User>, StoreError> {
    let limit = i64::try_from(SEARCH_LIMIT).unwrap_or(i64::MAX);
    if search_query.is_empty() {
        let mut sql = select_users(scope);
        narrow(&mut sql, scope);
        sql.push(OLDEST_FIRST).push_bind(" LIMIT ", limit);
        return fetch_users(scope, sql).await;
    }
    // The names only, matched here; then the users that matched, read whole.
    let wanted = search_query.to_lowercase();
    let mut sql = scope.sql(concat!(
        "SELECT id, username, ",
        person_names::name_columns!(),
        " FROM users WHERE deleted_at IS NULL"
    ));
    narrow(&mut sql, scope);
    sql.push(OLDEST_FIRST);
    let mut matched = Vec::with_capacity(SEARCH_LIMIT);
    for row in scope.fetch_all(sql).await? {
        let found = row.text(1)?.to_lowercase().contains(&wanted)
            || person_names::name_from_row(&row, 2)?.contains(&wanted);
        if found {
            matched.push(row.id(0)?);
            if matched.len() == SEARCH_LIMIT {
                break;
            }
        }
    }
    if matched.is_empty() {
        return Ok(Vec::new());
    }
    let mut sql = select_users(scope);
    sql.push(" AND id IN (")
        .bind_list(matched.iter().map(|id| Id(id)))
        .push(")")
        .push(OLDEST_FIRST);
    fetch_users(scope, sql).await
}

pub(crate) async fn fetch_users(scope: &mut Scope<'_>, sql: Sql) -> Result<Vec<User>, StoreError> {
    scope.all(sql).await
}

impl FromRow for User {
    fn from_row(row: &Row) -> Result<Self, StoreError> {
        Ok(User {
            id: row.id(0)?,
            username: row.text(1)?,
            email: row.text(2)?,
            password_hash: row.text(3)?,
            password_locked_at: row.int(4)?,
            created_at: row.int(5)?,
            updated_at: row.int(6)?,
            deleted_at: row.opt_int(7)?,
            is_admin: row.bool(8)?,
            display_name: row.text(9)?,
            email_verified_at: row.int(10)?,
            name: person_names::name_from_row(row, 11)?,
        })
    }
}

/// The error for a user that a read did not find (`user not found`).
pub(crate) fn user_not_found() -> StoreError {
    StoreError::not_found("user")
}

/// What a user is looked up by.
#[derive(Debug, Clone, Copy)]
enum Lookup<'a> {
    Id(&'a str),
    Username(&'a str),
    Email(&'a str),
}

impl Lookup<'_> {
    /// Appends the condition that selects the user.
    fn push_condition(self, sql: &mut Sql) {
        match self {
            Self::Id(id) => sql.push_bind("id = ", Id(id)),
            // An erased account has no name, and is found by none.
            Self::Username(name) => sql
                .push_bind("username <> '' AND lower(username) = lower(", name)
                .push(")"),
            // An account with no address of its own (one made through a provider) is
            // found by none, not even by an empty one.
            Self::Email(email) => sql
                .push_bind("email <> '' AND lower(email) = lower(", email)
                .push(")"),
        };
    }
}

/// Returns the user that is not deleted and that `lookup` selects.
async fn find_user(scope: &mut Scope<'_>, lookup: Lookup<'_>) -> Result<User, StoreError> {
    let mut sql = select_users(scope);
    sql.push(" AND ");
    lookup.push_condition(&mut sql);
    fetch_users(scope, sql)
        .await?
        .into_iter()
        .next()
        .ok_or_else(user_not_found)
}

/// The account with the identifier that somebody can sign in to, as
/// [`Store::account_by_id`] has it.
pub(crate) async fn account_by_id(
    scope: &mut Scope<'_>,
    user_id: &str,
) -> Result<User, StoreError> {
    find_account(scope, Lookup::Id(user_id)).await
}

/// Returns the account `lookup` selects that somebody can sign in to: in use, or
/// waiting to be deleted ([`WAITS`]).
async fn find_account(scope: &mut Scope<'_>, lookup: Lookup<'_>) -> Result<User, StoreError> {
    let mut sql = scope.sql([
        "SELECT ",
        USER_COLUMNS,
        " FROM users WHERE (deleted_at IS NULL OR (",
        WAITS,
        ")) AND ",
    ]);
    lookup.push_condition(&mut sql);
    fetch_users(scope, sql)
        .await?
        .into_iter()
        .next()
        .ok_or_else(user_not_found)
}

/// Whether an account in use, one that waits to be deleted, or one that is being
/// erased (its row has the address until the erasing ends) has the verified address
/// `email` as its own, in any letter case. Unverified addresses entered while mail is
/// disabled (`email_verified_at = 0`) do not reserve the address against verified OIDC
/// sign-in (`H-3`).
pub(crate) async fn has_address(scope: &mut Scope<'_>, email: &str) -> Result<bool, StoreError> {
    let mut sql = scope.sql([
        "SELECT 1 FROM users \
         WHERE email_verified_at > 0 \
         AND (deleted_at IS NULL OR (",
        WAITS,
        ") OR (erase_started_at > 0 AND erased_at = 0)) AND ",
    ]);
    Lookup::Email(email).push_condition(&mut sql);
    scope.exists(sql).await
}

/// Whether there is a user row with the identifier, deactivated or not: whether a row
/// that refers to the user can be stored.
pub(crate) async fn user_row_exists(
    scope: &mut Scope<'_>,
    user_id: &str,
) -> Result<bool, StoreError> {
    let mut sql = scope.sql("SELECT 1 FROM users WHERE id = ");
    sql.bind(Id(user_id));
    scope.exists(sql).await
}

pub(crate) async fn get_user_by_id(
    scope: &mut Scope<'_>,
    user_id: &str,
) -> Result<User, StoreError> {
    find_user(scope, Lookup::Id(user_id)).await
}

/// Inserts the row of a user as it is, shown by the name its parts make
/// (`PersonName::shown`) whatever its `display_name` says; a name or an address that is
/// taken is a conflict.
pub(crate) async fn insert_user_row(scope: &mut Scope<'_>, user: &User) -> Result<(), StoreError> {
    let mut sql = scope.sql(["INSERT INTO users (", USER_COLUMNS, ") VALUES ("]);
    sql.bind(Id(&user.id))
        .push_bind(", ", &user.username)
        .push_bind(", ", &user.email)
        .push_bind(", ", &user.password_hash)
        .push_bind(", ", user.password_locked_at)
        .push_bind(", ", user.created_at)
        .push_bind(", ", user.updated_at)
        .push_bind(", ", Param::OptInt(user.deleted_at))
        .push_bind(", ", user.is_admin)
        .push_bind(", ", user.name.shown())
        .push_bind(", ", user.email_verified_at);
    person_names::push_name_values(&mut sql, &user.name);
    sql.push(")");
    match scope.execute(sql).await {
        Ok(_) => Ok(()),
        Err(StoreError::Database(error)) => Err(unique_violation(error)),
        Err(error) => Err(error),
    }
}

/// Reads the violation of one of the two unique indexes of the users, or of the key of
/// their identities, as a conflict.
pub(crate) fn unique_violation(error: sqlx::Error) -> StoreError {
    let Some(database_error) = error.as_database_error() else {
        return StoreError::Database(error);
    };
    if !database_error.is_unique_violation() {
        return StoreError::Database(error);
    }
    // PostgreSQL names the index; SQLite names the indexed expression.
    let message = database_error.message();
    let of_index = database_error.constraint().unwrap_or(message);
    if of_index.contains("user_identities") {
        StoreError::Conflict(Taken::Identity)
    } else if of_index.contains("email") {
        StoreError::Conflict(Taken::Email)
    } else if of_index.contains("username") {
        StoreError::Conflict(Taken::Username)
    } else {
        StoreError::Database(error)
    }
}

/// Sets the password hash of the user that `lookup` selects, deleted or not, and ends
/// a lock on the password: the operator set it.
async fn update_password(
    scope: &mut Scope<'_>,
    lookup: Lookup<'_>,
    password_hash: &str,
) -> Result<(), StoreError> {
    let now = scope.now_millis();
    let mut sql = scope.sql("UPDATE users SET ");
    sql.push_bind("password_hash = ", password_hash)
        .push_bind(", password_locked_at = ", 0_i64)
        .push_bind(", updated_at = ", now)
        .push(" WHERE ");
    lookup.push_condition(&mut sql);
    if scope.execute(sql).await? == 0 {
        let (Lookup::Id(user) | Lookup::Username(user) | Lookup::Email(user)) = lookup;
        return Err(StoreError::UserNotFound(user.to_owned()));
    }
    Ok(())
}

mod preferences;
pub(crate) use preferences::{get_client_state, patch_client_state};
use preferences::{stored_settings, upsert_user_settings};

/// Locks the password of the account at `now`, if it has one: it is taken nowhere, as
/// the proof of a login or of anything a session asks, until a new one is set through a
/// reset link or by the operator. The hash stays, so the account still has a password:
/// a session made through a provider opens no more of it than it did.
pub(crate) async fn lock_password(
    scope: &mut Scope<'_>,
    user_id: &str,
    now: i64,
) -> Result<(), StoreError> {
    let mut sql = scope.sql("UPDATE users SET password_locked_at = ");
    sql.bind(now)
        .push_bind(", updated_at = ", now)
        .push_bind(" WHERE password_hash <> '' AND id = ", Id(user_id));
    scope.execute(sql).await?;
    Ok(())
}

/// Holds a user's row against other changes of what is the user's until the
/// transaction ends.
///
/// # Errors
///
/// Returns [`StoreError::UserNotFound`] if there is no such user.
pub(crate) async fn lock_user(scope: &mut Scope<'_>, user_id: &str) -> Result<(), StoreError> {
    let mut sql = scope.sql("SELECT id FROM users WHERE deleted_at IS NULL AND id = ");
    sql.bind(Id(user_id));
    sql.push_lock(scope.row_lock(LockRank::Users, user_id, RowLock::NoKeyUpdate));
    match scope.fetch_optional(sql).await? {
        Some(_) => Ok(()),
        None => Err(StoreError::UserNotFound(user_id.to_owned())),
    }
}

/// Holds the user's row (`FOR NO KEY UPDATE` on PostgreSQL) regardless of `deleted_at`,
/// serializing cleanup on organization departure or guest removal with concurrent
/// per-user writes.
pub(crate) async fn hold_user_for_cleanup(
    scope: &mut Scope<'_>,
    user_id: &str,
) -> Result<(), StoreError> {
    let mut sql = scope.sql("SELECT id FROM users WHERE id = ");
    sql.bind(Id(user_id));
    scope
        .hold(LockRank::Users, user_id, sql, RowLock::NoKeyUpdate)
        .await?;
    Ok(())
}

/// Holds the row of the account against being put out of use until the transaction
/// ends, and says whether it is in use: an asking to delete it waits for this, or
/// this finds it asked.
pub(crate) async fn hold_in_use(scope: &mut Scope<'_>, user_id: &str) -> Result<bool, StoreError> {
    let mut sql = scope.sql("SELECT id FROM users WHERE deleted_at IS NULL AND id = ");
    sql.bind(Id(user_id));
    sql.push_lock(scope.row_lock(LockRank::Users, user_id, RowLock::Share));
    scope.exists(sql).await
}

/// Whether the account `user_id` has a password to sign in with, by the same rule as
/// [`User::has_password`].
pub(crate) async fn has_password(scope: &mut Scope<'_>, user_id: &str) -> Result<bool, StoreError> {
    let mut sql = scope.sql("SELECT password_hash FROM users WHERE id = ");
    sql.bind(Id(user_id));
    match scope.fetch_optional(sql).await? {
        Some(row) => Ok(User::has_password_hash(&row.text(0)?)),
        None => Ok(false),
    }
}

/// Holds the instance's row to the end of the transaction: the changes of who
/// administers the instance are made one at a time. On SQLite a transaction that writes
/// holds the whole database.
pub(crate) async fn lock_instance<'c>(
    scope: &mut Scope<'c>,
) -> Result<InstanceHeld<'c>, StoreError> {
    let mut sql = scope.sql("SELECT id FROM instance WHERE id = 1");
    sql.push_lock(scope.row_lock(LockRank::Instance, "1", RowLock::Update));
    scope.fetch_optional(sql).await?;
    Ok(InstanceHeld::new())
}

/// Makes the user an administrator of the instance if `role` is the organization's
/// administrator, and ends that otherwise, unless they are its last administrator: then
/// nothing changes and the answer is `false`.
pub(crate) async fn follow_org_role<'c>(
    scope: &mut Scope<'c>,
    _held: InstanceHeld<'c>,
    user_id: &str,
    role: OrgRole,
) -> Result<bool, StoreError> {
    let is_admin = role == OrgRole::Admin;
    if !is_admin {
        let mut sql = scope.sql("SELECT 1 FROM users WHERE is_admin = ");
        sql.bind(true).push_bind(" AND id = ", Id(user_id));
        if !scope.exists(sql).await? {
            return Ok(true);
        }
        let mut sql = scope.sql("SELECT 1 FROM users WHERE deleted_at IS NULL AND is_admin = ");
        sql.bind(true)
            .push_bind(" AND id <> ", Id(user_id))
            .push(" LIMIT 1");
        if !scope.exists(sql).await? {
            return Ok(false);
        }
    }
    let now = scope.now_millis();
    let mut sql = scope.sql("UPDATE users SET is_admin = ");
    sql.bind(is_admin)
        .push_bind(", updated_at = ", now)
        .push_bind(" WHERE id = ", Id(user_id))
        .push_bind(" AND is_admin <> ", is_admin);
    scope.execute(sql).await?;
    Ok(true)
}

/// Sets the password hash of the user with the given name, in any letter case, and
/// deletes their sessions; returns that user and the sessions that ended.
///
/// # Errors
///
/// Returns [`StoreError::UserNotFound`] if no user has that name.
pub(crate) async fn reset_password(
    scope: &mut Scope<'_>,
    username: &str,
    password_hash: &str,
) -> Result<PasswordSet, StoreError> {
    let lookup = Lookup::Username(username);
    update_password(scope, lookup, password_hash).await?;
    let mut sql = scope.sql("SELECT id FROM users WHERE ");
    lookup.push_condition(&mut sql);
    let row = scope
        .fetch_optional(sql)
        .await?
        .ok_or_else(|| StoreError::UserNotFound(username.to_owned()))?;
    let user_id = row.id(0)?;
    let ended = sessions::delete_sessions_of_user(scope, &user_id).await?;
    password_reset::end_links_of_user(scope, &user_id).await?;
    Ok(PasswordSet { user_id, ended })
}
