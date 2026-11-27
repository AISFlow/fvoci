//! An account its person asks to delete: the asking, the wait, and taking it back.
//!
//! From the asking the account is out of use, as a deactivated one is (`deleted_at`),
//! and waits until `delete_after`: its sessions are ended, and one made by signing in
//! meanwhile opens nothing but the way back ([`Store::leaving_session`],
//! [`Store::cancel_account_deletion`]). Its memberships and what it wrote stay as
//! they are until it is erased (`account_erasure.rs`).
//!
//! An account does not go while something would be left without anybody to run it:
//! the instance without an administrator, or an organization that has other people in
//! it without one. An organization that has nobody but the account goes with it, but
//! the instance's own, which stays whoever is in it, and its only administrator. Who
//! administers what is asked and the account is put out of use in one transaction,
//! under the instance's row and then the rows of the organizations the account
//! administers, as every change of who administers what holds them: of two
//! administrators who ask at once, one is refused. Who comes into an organization is
//! not held off by that: what was read here is read again when the erasing begins.
//!
//! The account's organizations are read with the system's reach: a person's
//! memberships are of several, and the wall hides the others from any one of them.
//! Nothing here writes a row of an organization.

use fvoci_model::{
    ContainerKind, Leaving, LeavingOrganization, OrgKind, OrgRole, OrganizationName, Session,
};

use crate::db::{Id, InstanceHeld, Scope, Sql};
use crate::error::StoreError;
use crate::sessions::{self, SessionsEnded};
use crate::{Store, container_kinds, directory, link_locks, password_reset, users};

/// What stands in the way of deleting an account, and what goes with it.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct DeletionStanding {
    /// Whether the account is the only administrator of the instance in use.
    pub last_instance_admin: bool,
    /// The organizations in use it is the only administrator of that stay when it
    /// goes: those that have other people, and the instance's own, which never goes
    /// with an account (the organization after it would be the instance's then, and
    /// its administrators the instance's).
    pub sole_admin_of: Vec<OrganizationName>,
    /// The organizations in use that have nobody but the account, the instance's own
    /// excepted: they go with it.
    pub alone_in: Vec<OrganizationName>,
    /// The account's own space, which goes with it always: nothing is asked of who
    /// else is in it. `None` only for an account whose space is gone already.
    pub own_space: Option<OrganizationName>,
}

impl DeletionStanding {
    /// Whether nothing stands in the way.
    #[must_use]
    pub fn is_open(&self) -> bool {
        !self.last_instance_admin && self.sole_admin_of.is_empty()
    }
}

/// What became of a request to delete an account.
#[derive(Debug, PartialEq, Eq)]
pub enum DeletionAsking {
    /// The account is out of use and waits: its sessions are ended.
    Asked(Leaving, SessionsEnded),
    /// Something would be left without anybody to run it: nothing changed.
    Refused(DeletionStanding),
    /// The account is not in use, or its password was set anew since it was checked:
    /// nothing changed.
    Changed,
}

impl Store {
    /// What stands in the way of deleting the account, and which organizations would
    /// go with it, as things are now; the request decides anew.
    ///
    /// # Errors
    ///
    /// Returns [`StoreError::Database`] if a query fails.
    pub async fn account_deletion_standing(
        &self,
        user_id: &str,
    ) -> Result<DeletionStanding, StoreError> {
        self.system()
            .with_conn(async |scope| standing(scope, user_id, Held::No).await)
            .await
    }

    /// How much is in the handle's organization, which goes with its only member, and
    /// what kind it is of: the boards and the wikis of its workspaces in use (templates
    /// not counted), and the files they hold. Returns `Ok(None)` if the organization
    /// row is no longer there (for example, erased concurrently).
    ///
    /// # Errors
    ///
    /// Returns [`StoreError::NoOrganization`] if the handle names none,
    /// [`StoreError::Corrupt`] if a stored kind is unknown, and
    /// [`StoreError::Database`] if a query fails.
    pub async fn leaving_organization(
        &self,
        organization: OrganizationName,
    ) -> Result<Option<LeavingOrganization>, StoreError> {
        self.with_conn(async |scope| {
            let mut sql = scope.sql("SELECT kind FROM organizations WHERE id = ");
            sql.bind(scope.org()?);
            let Some(row) = scope.fetch_optional(sql).await? else {
                return Ok(None);
            };
            let kind = directory::org_kind(&row, 0)?;
            let (mut boards, mut wikis, mut diaries, mut courses) = (0, 0, 0, 0);
            let mut sql = scope.sql(
                "SELECT c.kind, CAST(COUNT(*) AS BIGINT) FROM containers AS c \
                 JOIN workspaces AS w ON w.id = c.workspace_id \
                 WHERE w.deleted_at IS NULL AND w.org_id = ",
            );
            sql.bind(scope.org()?)
                .push_bind(" AND c.is_template = ", false)
                .push(&scope.same_organization(&[("c", "w")]))
                .push(" GROUP BY c.kind");
            for row in scope.fetch_all(sql).await? {
                match container_kinds::kind_from_text(&row.text(0)?)? {
                    ContainerKind::Board => boards = row.int(1)?,
                    ContainerKind::Wiki => wikis = row.int(1)?,
                    ContainerKind::Diary => diaries = row.int(1)?,
                    ContainerKind::Course => courses = row.int(1)?,
                }
            }
            let mut sql = scope.sql(
                "SELECT CAST(COUNT(*) AS BIGINT) FROM files AS f \
                 JOIN containers AS c ON c.id = f.container_id \
                 JOIN workspaces AS w ON w.id = c.workspace_id \
                 WHERE w.deleted_at IS NULL AND w.org_id = ",
            );
            sql.bind(scope.org()?)
                .push_bind(" AND c.is_template = ", false)
                .push(&scope.same_organization(&[("f", "c"), ("c", "w")]));
            let files = match scope.fetch_optional(sql).await? {
                Some(row) => row.int(0)?,
                None => 0,
            };
            Ok(Some(LeavingOrganization {
                id: organization.id,
                kind,
                name: organization.name,
                boards: Some(boards),
                wikis: Some(wikis),
                diaries: Some(diaries),
                courses: Some(courses),
                files: Some(files),
            }))
        })
        .await
    }

    /// Puts the account out of use at `now`, to be erased after `delete_after`, and
    /// ends its sessions and the credentials to set its password anew: provided nothing
    /// stands in the way ([`DeletionStanding`]), the account is in use, and its
    /// password hash is still `seen_password_hash`, the one its password was checked
    /// against (empty for an account that has none). All of it happens or none.
    ///
    /// # Errors
    ///
    /// Returns [`StoreError::Database`] if a statement fails.
    pub async fn request_account_deletion(
        &self,
        user_id: &str,
        seen_password_hash: &str,
        now: i64,
        delete_after: i64,
    ) -> Result<DeletionAsking, StoreError> {
        self.system()
            .transaction(async |scope| {
                let held_instance = users::lock_instance(scope).await?;
                let standing = standing(scope, user_id, Held::Yes(held_instance)).await?;
                if !standing.is_open() {
                    return Ok(DeletionAsking::Refused(standing));
                }
                let mut sql = scope.sql("UPDATE users SET deleted_at = ");
                sql.bind(now)
                    .push_bind(", delete_after = ", delete_after)
                    .push_bind(", updated_at = ", now)
                    .push_bind(" WHERE deleted_at IS NULL AND id = ", Id(user_id))
                    .push_bind(" AND password_hash = ", seen_password_hash)
                    .push_bind(" AND password_locked_at = ", 0_i64);
                if scope.execute(sql).await? == 0 {
                    return Ok(DeletionAsking::Changed);
                }
                // The links of the account's own space open nothing while it waits:
                // what was checked of a link before is checked again, of that space
                // and of no other organization.
                if let Some(space) = &standing.own_space {
                    scope.invalidate_visibility_of(&space.id)?;
                }
                let ended = sessions::delete_sessions_of_user(scope, user_id).await?;
                password_reset::end_links_of_user(scope, user_id).await?;
                Ok(DeletionAsking::Asked(
                    Leaving {
                        requested_at: now,
                        delete_after,
                    },
                    ended,
                ))
            })
            .await
    }

    /// Whether the address is an account's own that no new account may take: one in
    /// use, one that waits to be deleted, or one being erased, in any letter case.
    ///
    /// # Errors
    ///
    /// Returns [`StoreError::Database`] if the query fails.
    pub async fn is_address_held(&self, email: &str) -> Result<bool, StoreError> {
        self.with_conn(async |scope| users::has_address(scope, email).await)
            .await
    }

    /// Takes back the asking to delete the account: it is in use again, as it was, with
    /// its memberships and what it wrote. Says whether it was taken back: not for an
    /// account that does not wait, and not once its erasing began.
    ///
    /// # Errors
    ///
    /// Returns [`StoreError::Database`] if the statement fails.
    pub async fn cancel_account_deletion(
        &self,
        user_id: &str,
        now: i64,
    ) -> Result<bool, StoreError> {
        // Through the reach of every organization, whatever handle asks: the space
        // whose links open again is found by its owner.
        self.system()
            .transaction(async |scope| {
                let mut sql = scope
                    .sql("UPDATE users SET deleted_at = NULL, delete_after = 0, updated_at = ");
                sql.bind(now)
                    .push_bind(" WHERE id = ", Id(user_id))
                    .push(" AND ")
                    .push(users::WAITS);
                let back = scope.execute(sql).await? > 0;
                if back && let Some(space) = own_space(scope, user_id).await? {
                    // The links of its own space open again.
                    scope.invalidate_visibility_of(&space.id)?;
                }
                Ok(back)
            })
            .await
    }

    /// The session with the token digest, if it is one of an account that waits to be
    /// deleted, and until when the account waits: valid at `now` as
    /// [`Store::get_session`] has it, but for its account being out of use. Such a
    /// session opens the way back and nothing else.
    ///
    /// # Errors
    ///
    /// Returns [`StoreError::Database`] if the query fails.
    pub async fn leaving_session(
        &self,
        token_digest: &str,
        now: i64,
        idle_since: i64,
    ) -> Result<Option<(Session, Leaving)>, StoreError> {
        self.with_conn(async |scope| {
            let mut sql = scope.sql(
                "SELECT u.deleted_at, u.delete_after FROM sessions AS s \
                 JOIN users AS u ON u.id = s.user_id WHERE s.token_hash = ",
            );
            sql.bind(token_digest)
                .push_bind(" AND s.expires_at > ", now)
                .push_bind(" AND s.last_used_at > ", idle_since)
                .push(" AND u.deleted_at IS NOT NULL AND u.delete_after > 0 AND u.erase_started_at = 0");
            let Some(row) = scope.fetch_optional(sql).await? else {
                return Ok(None);
            };
            let leaving = Leaving {
                requested_at: row.int(0)?,
                delete_after: row.int(1)?,
            };
            let session = sessions::session_by_digest(scope, token_digest).await?;
            Ok(session.map(|session| (session, leaving)))
        })
        .await
    }
}

/// Whether the rows that decide are held to the end of the transaction.
#[derive(Debug, Clone, Copy)]
pub(crate) enum Held<'c> {
    Yes(InstanceHeld<'c>),
    No,
}

/// What stands in the way of deleting the account `user_id`, read through a scope that
/// reaches every organization. [`Held::Yes`] holds the row of each organization the
/// account administers in ascending identifier order (under [`link_locks::take_turn`]
/// when there is more than one).
pub(crate) async fn standing<'c>(
    scope: &mut Scope<'c>,
    user_id: &str,
    held: Held<'c>,
) -> Result<DeletionStanding, StoreError> {
    let lock_orgs = matches!(held, Held::Yes(_));
    let mut standing = DeletionStanding {
        last_instance_admin: is_last_instance_admin(scope, user_id).await?,
        ..DeletionStanding::default()
    };
    standing.own_space = own_space(scope, user_id).await?;
    let mut sql = scope.sql(
        "SELECT o.id, o.name FROM organization_members AS m \
         JOIN organizations AS o ON o.id = m.org_id WHERE m.user_id = ",
    );
    sql.bind(Id(user_id))
        .push_bind(" AND m.role = ", OrgRole::Admin.as_str())
        .push_bind(" AND o.kind = ", OrgKind::Shared.as_str())
        .push(" AND o.deleted_at IS NULL AND o.is_system = false ORDER BY o.id");
    let administered: Vec<OrganizationName> = scope
        .fetch_all(sql)
        .await?
        .iter()
        .map(|row| {
            Ok(OrganizationName {
                id: row.id(0)?,
                name: row.text(1)?,
            })
        })
        .collect::<Result<_, StoreError>>()?;
    // The rows of several organizations are held under the turn, as every
    // transaction that spans organizations holds them (`link_locks`): the caller holds
    // the instance's row already (`users::lock_instance`), which nothing takes after the
    // turn.
    if lock_orgs && administered.len() > 1 {
        link_locks::take_turn(scope).await?;
    }
    let the_instances = directory::oldest_organization(scope).await?;
    for organization in administered {
        if lock_orgs {
            link_locks::hold_organization(scope, &organization.id).await?;
        }
        match without(scope, &organization.id, user_id).await? {
            Without::Administered => {}
            Without::NoAdministrator => standing.sole_admin_of.push(organization),
            // The instance's own stays, whoever is in it.
            Without::Nobody if the_instances.as_deref() == Some(organization.id.as_str()) => {
                standing.sole_admin_of.push(organization);
            }
            Without::Nobody => standing.alone_in.push(organization),
        }
    }
    Ok(standing)
}

/// What an organization is without the user: what deleting the account asks of each
/// organization it administers, and leaving one asks of the organization it leaves.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Without {
    /// Somebody else whose account is in use administers it.
    Administered,
    /// Other people are in it, and nobody else administers it.
    NoAdministrator,
    /// Nobody else is in it.
    Nobody,
}

/// What the organization `org_id` is without the user, read through a scope that
/// reaches it.
pub(crate) async fn without(
    scope: &mut Scope<'_>,
    org_id: &str,
    user_id: &str,
) -> Result<Without, StoreError> {
    if has_another_admin(scope, org_id, user_id).await? {
        Ok(Without::Administered)
    } else if has_other_people(scope, org_id, user_id).await? {
        Ok(Without::NoAdministrator)
    } else {
        Ok(Without::Nobody)
    }
}

/// The account's own space in use, by who owns it: whatever its members are.
async fn own_space(
    scope: &mut Scope<'_>,
    user_id: &str,
) -> Result<Option<OrganizationName>, StoreError> {
    let mut sql = scope.sql("SELECT id, name FROM organizations WHERE owner_id = ");
    sql.bind(Id(user_id))
        .push_bind(" AND kind = ", OrgKind::Personal.as_str())
        .push(" AND deleted_at IS NULL");
    scope
        .fetch_optional(sql)
        .await?
        .map(|row| {
            Ok(OrganizationName {
                id: row.id(0)?,
                name: row.text(1)?,
            })
        })
        .transpose()
}

/// Whether the account administers the instance and no other account in use does.
pub(crate) async fn is_last_instance_admin(
    scope: &mut Scope<'_>,
    user_id: &str,
) -> Result<bool, StoreError> {
    let mut sql = scope.sql("SELECT 1 FROM users WHERE is_admin = ");
    sql.bind(true).push_bind(" AND id = ", Id(user_id));
    if scope.fetch_optional(sql).await?.is_none() {
        return Ok(false);
    }
    let mut sql = scope.sql("SELECT 1 FROM users WHERE deleted_at IS NULL AND is_admin = ");
    sql.bind(true)
        .push_bind(" AND id <> ", Id(user_id))
        .push(" LIMIT 1");
    Ok(scope.fetch_optional(sql).await?.is_none())
}

/// Whether the organization has an administrator besides the user whose account is in
/// use: one that waits to be deleted is none.
async fn has_another_admin(
    scope: &mut Scope<'_>,
    org_id: &str,
    user_id: &str,
) -> Result<bool, StoreError> {
    let mut sql = scope.sql(
        "SELECT 1 FROM organization_members AS om JOIN users AS u ON u.id = om.user_id \
         WHERE u.deleted_at IS NULL AND om.org_id = ",
    );
    sql.bind(Id(org_id))
        .push_bind(" AND om.role = ", OrgRole::Admin.as_str())
        .push_bind(" AND om.user_id <> ", Id(user_id))
        .push(" LIMIT 1");
    Ok(scope.fetch_optional(sql).await?.is_some())
}

/// Whether anybody but the user is in the organization: a member of it, or a member or
/// a guest of one of its workspaces, in use or in the trash, whatever their account's
/// state.
pub(crate) async fn has_other_people(
    scope: &mut Scope<'_>,
    org_id: &str,
    user_id: &str,
) -> Result<bool, StoreError> {
    let mut sql = scope.sql("SELECT 1 FROM organization_members WHERE org_id = ");
    sql.bind(Id(org_id))
        .push_bind(" AND user_id <> ", Id(user_id))
        .push(" LIMIT 1");
    if scope.fetch_optional(sql).await?.is_some() {
        return Ok(true);
    }
    let mut sql: Sql = scope.sql(
        "SELECT 1 FROM workspace_members AS wm JOIN workspaces AS w ON w.id = wm.workspace_id \
         WHERE w.org_id = ",
    );
    sql.bind(Id(org_id))
        .push_bind(" AND wm.user_id <> ", Id(user_id))
        .push(&scope.same_organization(&[("wm", "w")]))
        .push(" LIMIT 1");
    Ok(scope.fetch_optional(sql).await?.is_some())
}
