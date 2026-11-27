//! Making an account and letting it in: the one place where a user's row is stored and
//! where a user becomes a member of an organization and of a workspace of it.
//!
//! Whoever signs up with the name of an organization, whoever accepts an invitation without an
//! account, and whoever a provider vouches for the first time all get their account
//! here (`insert_account`) and are let in here (`join_organization`,
//! `join_workspace`). An account's own space is made with its row, in the same
//! transaction: no account is without one. Whoever signs up without the name of an
//! organization gets that and nothing else (`alone`), and starts an organization
//! later, as an account that is there ([`Store::start_organization`]).

use fvoci_model::{InstanceRole, OrgRole, User, Workspace, WorkspaceRole, new_id};

use crate::db::{Id, Scope, Sql};
use crate::error::{StoreError, Taken};
use crate::users;
use crate::{Store, Tenant};
use crate::{directory, signups, workspaces};

/// The organization somebody who signs up with the name of an organization starts in.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Founding {
    /// The first organization, created with the first account. If the database already
    /// holds an organization of people with no account (one from before organizations
    /// had accounts, in a database re-anchored at the baseline), the first user takes it.
    First,
    /// A new organization, under an identifier the store gives it, whatever
    /// organization the handle names.
    New,
}

/// Single-transaction policy deciding how [`Store::sign_up_with_policy`] and
/// [`Store::confirm_signup_with_policy`] found an organization.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FoundingPolicy {
    /// Require the registrant to be the instance's very first user, returning
    /// [`StoreError::NotFirstUser`] otherwise (single-organization servers).
    FirstOnly,
    /// Atomically claim the first-user slot if still open, or found a new organization
    /// in the same transaction when another account already claimed it
    /// (multi-organization servers).
    FirstOrNew,
}

/// The space of an account: the organization that is the account's own, and its one
/// workspace.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OwnSpace {
    /// The organization, of the kind `personal`.
    pub org_id: String,
    /// The one workspace of that organization.
    pub workspace_id: String,
}

/// An organization an account started, and its first workspace.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StartedOrganization {
    /// The organization.
    pub org_id: String,
    /// Its first workspace, which is its default one.
    pub workspace_id: String,
}

/// A workspace of the scope's organization that a user is let into.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Which<'a> {
    /// The workspace with this identifier.
    Workspace(&'a str),
    /// The organization's default workspace.
    DefaultWorkspace,
}

impl Store {
    /// Stores the account of somebody who signs up, and what they start: the
    /// organization `founding` says and its first workspace, both under
    /// `organization_name`, with the user as the administrator of both. Returns the
    /// identifier of the workspace. All of it happens or none.
    ///
    /// `user` is updated to what was stored (its times, and whether an administrator,
    /// whatever the caller put there); whoever is the
    /// first user of the instance is its administrator, however they found their
    /// organization. Who is first is decided with the insertion: of two who sign up at
    /// once into an empty instance, one is.
    ///
    /// # Errors
    ///
    /// Returns [`StoreError::NotFirstUser`] if [`Founding::First`] is asked and the
    /// instance has had its first user; [`StoreError::Conflict`] if another user,
    /// deleted or not, has the name or the email address in any letter case, or
    /// (`Taken::WorkspaceName`) another workspace in use of the organization there is
    /// has the organization's name already;
    /// and [`StoreError::Database`] if a statement fails, as it
    /// does for [`Founding::New`] on an SQLite database, which holds one organization.
    pub async fn sign_up(
        &self,
        user: &mut User,
        organization_name: &str,
        founding: Founding,
    ) -> Result<String, StoreError> {
        // The first-user flag is claimed in the same transaction as the organization:
        // a competing first signup rolls back without leaving an empty organization.
        let existing = match founding {
            Founding::First => self.instance_organization().await?,
            Founding::New => None,
        };
        let place = OrgPlace::of(existing.as_deref());
        let org_id = existing.unwrap_or_else(new_id);
        let store = self.for_org(&org_id)?;
        store
            .transaction(async |scope| found(scope, user, organization_name, founding, place).await)
            .await
    }

    /// Stores the account of somebody who signs up and the organization `policy` chooses
    /// in a single transaction on fresh and already-bootstrapped instances.
    ///
    /// # Errors
    ///
    /// Returns [`StoreError::NotFirstUser`] when `policy` is
    /// [`FoundingPolicy::FirstOnly`] and the instance already has its first user, and
    /// otherwise the same errors as [`Store::sign_up`].
    pub async fn sign_up_with_policy(
        &self,
        user: &mut User,
        organization_name: &str,
        policy: FoundingPolicy,
    ) -> Result<String, StoreError> {
        let existing = self.unclaimed_instance_organization().await?;
        match policy {
            FoundingPolicy::FirstOnly => {
                let place = OrgPlace::of(existing.as_deref());
                let org_id = existing.unwrap_or_else(new_id);
                self.for_org(&org_id)?
                    .transaction(async |scope| {
                        found(scope, user, organization_name, Founding::First, place).await
                    })
                    .await
            }
            FoundingPolicy::FirstOrNew => {
                if let Some(existing_org_id) = existing {
                    let result = self
                        .for_org(&existing_org_id)?
                        .transaction(async |scope| {
                            found(
                                scope,
                                user,
                                organization_name,
                                Founding::First,
                                OrgPlace::Existing,
                            )
                            .await
                        })
                        .await;
                    if !matches!(result, Err(StoreError::NotFirstUser)) {
                        return result;
                    }
                }
                let org_id = new_id();
                self.for_org(&org_id)?
                    .transaction(async |scope| {
                        found_first_or_new(scope, user, organization_name).await
                    })
                    .await
            }
        }
    }

    /// Stores the account of somebody who signs up without the name of an
    /// organization: the account and its own space, and nothing besides. The account
    /// is in no organization that people work in, and administers nothing. Returns
    /// its own space. All of it happens or none.
    ///
    /// `user` is updated to what was stored, as [`Store::sign_up`] updates it.
    ///
    /// # Errors
    ///
    /// Returns [`StoreError::NoFirstUser`] while the instance has had no first user:
    /// the first account starts the instance's organization, whose administrators
    /// follow who administers the instance, and this one starts none. Returns
    /// [`StoreError::Conflict`] for a name or an address that is taken, as
    /// [`Store::sign_up`] does.
    pub async fn sign_up_alone(&self, user: &mut User) -> Result<OwnSpace, StoreError> {
        self.scoped(Tenant::Unset)
            .transaction(async |scope| alone(scope, user).await)
            .await
    }

    /// Starts an organization that people work in together for the account `user_id`:
    /// the organization and its first workspace, both under `name`, with the account
    /// as the administrator of both, as [`Store::sign_up`] makes them for
    /// [`Founding::New`]. An account makes `limit` organizations and no more: the ones
    /// in use that it made are counted, by signing up or here, and not the ones it was
    /// let into. All of it happens or none; of several at once for one account, no
    /// more are made than the limit leaves room for.
    ///
    /// # Errors
    ///
    /// Returns [`StoreError::TooManyOrganizations`] if the account made `limit`
    /// organizations that are in use; [`StoreError::UserNotFound`] if no account in
    /// use has the identifier; and [`StoreError::Database`] if a statement fails, as
    /// it does on an SQLite database that holds its one organization.
    pub async fn start_organization(
        &self,
        user_id: &str,
        name: &str,
        limit: u32,
    ) -> Result<StartedOrganization, StoreError> {
        let org_id = new_id();
        self.for_org(&org_id)?
            .transaction(async |scope| {
                // Held against another start by the same account, which waits here
                // and then counts the organization this one stores.
                users::lock_user(scope, user_id).await?;
                let made = directory::organizations_made_by(scope, user_id).await?;
                if made >= i64::from(limit) {
                    return Err(StoreError::TooManyOrganizations { limit });
                }
                let workspace_id = new_id();
                found_organization(scope, name, &workspace_id).await?;
                made_by(scope, user_id).await?;
                join_organization(scope, user_id, OrgRole::Admin).await?;
                let which = Which::Workspace(&workspace_id);
                join_workspace(scope, which, user_id, WorkspaceRole::Admin).await?;
                Ok(StartedOrganization {
                    org_id: org_id.clone(),
                    workspace_id,
                })
            })
            .await
    }

    /// Whether the instance has had its first user.
    ///
    /// # Errors
    ///
    /// Returns [`StoreError::Database`] if the query fails.
    pub async fn is_bootstrapped(&self) -> Result<bool, StoreError> {
        self.with_conn(async |scope| has_first_user(scope).await)
            .await
    }

    /// Stores a user past the ways in, for a test: the first of the instance as its
    /// administrator, and, through a handle that names an organization, as a member of
    /// it and of its default workspace (an administrator of both if of the instance).
    ///
    /// # Errors
    ///
    /// As [`Store::sign_up`] for the account.
    #[cfg(any(test, feature = "test-util"))]
    pub async fn create_user(&self, user: &mut User) -> Result<(), StoreError> {
        self.transaction(async |scope| {
            let role = become_first(scope).await?;
            insert_account(scope, user, role).await?;
            if scope.has_org() {
                let (org_role, workspace_role) = match role {
                    InstanceRole::Administrator => (OrgRole::Admin, WorkspaceRole::Admin),
                    InstanceRole::User => (OrgRole::Member, WorkspaceRole::Member),
                };
                join_organization(scope, &user.id, org_role).await?;
                join_workspace(scope, Which::DefaultWorkspace, &user.id, workspace_role).await?;
            }
            Ok(())
        })
        .await
    }
}

/// Whether the organization the scope names is there already, for the first user to
/// take, or is to be made.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum OrgPlace {
    /// An organization of people with no account yet. The baseline seeds none (only
    /// the system's organization); a database re-anchored at the baseline may hold one
    /// from before its organizations had accounts.
    Existing,
    /// None yet: it is made under the scope's identifier.
    ToMake,
}

impl OrgPlace {
    /// The place of `existing`, the organization of the instance if it has one.
    pub(crate) fn of(existing: Option<&str>) -> Self {
        if existing.is_some() {
            Self::Existing
        } else {
            Self::ToMake
        }
    }
}

/// Stores `user` and the scope's organization `founding` says, as [`Store::sign_up`]
/// does, taking the organization the scope names if it is `place`d there already.
/// Returns the identifier of the organization's first workspace.
pub(crate) async fn found(
    scope: &mut Scope<'_>,
    user: &mut User,
    organization_name: &str,
    founding: Founding,
    place: OrgPlace,
) -> Result<String, StoreError> {
    let role = become_first(scope).await?;
    let workspace_id = match founding {
        Founding::First if role == InstanceRole::User => {
            return Err(StoreError::NotFirstUser);
        }
        Founding::First if place == OrgPlace::Existing => {
            name_organization(scope, organization_name).await?
        }
        Founding::First | Founding::New => {
            let workspace_id = new_id();
            found_organization(scope, organization_name, &workspace_id).await?;
            workspace_id
        }
    };
    insert_account(scope, user, role).await?;
    made_by(scope, &user.id).await?;
    join_organization(scope, &user.id, OrgRole::Admin).await?;
    join_workspace(
        scope,
        Which::Workspace(&workspace_id),
        &user.id,
        WorkspaceRole::Admin,
    )
    .await?;
    Ok(workspace_id)
}

/// Atomically claims the first-user slot if still open or founds a new organization in
/// the same transaction when `place` is [`OrgPlace::ToMake`].
pub(crate) async fn found_first_or_new(
    scope: &mut Scope<'_>,
    user: &mut User,
    organization_name: &str,
) -> Result<String, StoreError> {
    let role = become_first(scope).await?;
    let workspace_id = new_id();
    found_organization(scope, organization_name, &workspace_id).await?;
    insert_account(scope, user, role).await?;
    made_by(scope, &user.id).await?;
    join_organization(scope, &user.id, OrgRole::Admin).await?;
    join_workspace(
        scope,
        Which::Workspace(&workspace_id),
        &user.id,
        WorkspaceRole::Admin,
    )
    .await?;
    Ok(workspace_id)
}

/// Writes down that the account `user_id` made the scope's organization: the one it
/// started, or, on a database that had an organization before it had a user, the one
/// the first account took, which nobody made before.
async fn made_by(scope: &mut Scope<'_>, user_id: &str) -> Result<(), StoreError> {
    let mut sql = scope.sql("UPDATE organizations SET created_by = ");
    sql.bind(Id(user_id))
        .push_bind(" WHERE id = ", scope.org()?);
    scope.execute(sql).await?;
    Ok(())
}

/// Stores `user` as an account that starts no organization, as
/// [`Store::sign_up_alone`] does, in whatever organization the scope reaches: the
/// account joins none. Returns its own space.
///
/// # Errors
///
/// Returns [`StoreError::NoFirstUser`] while the instance has had no first user.
pub(crate) async fn alone(scope: &mut Scope<'_>, user: &mut User) -> Result<OwnSpace, StoreError> {
    // Read, not claimed: the flag never goes off again, so an account that finds it
    // on comes after the first user, and one that finds it off starts nothing.
    if !has_first_user(scope).await? {
        return Err(StoreError::NoFirstUser);
    }
    insert_account(scope, user, InstanceRole::User).await
}

/// Whether the instance has had its first user, as the scope's transaction sees it.
async fn has_first_user(scope: &mut Scope<'_>) -> Result<bool, StoreError> {
    let sql = scope.sql("SELECT bootstrapped FROM instance WHERE id = 1");
    match scope.fetch_optional(sql).await? {
        Some(row) => row.bool(0),
        None => Ok(false),
    }
}

/// Turns on the flag that the instance has had its first user, and says what whoever
/// this transaction stores is to the instance: who turns the flag on is the first
/// user, its administrator. A second transaction that tries at the same moment waits
/// for this one and then finds the flag on.
async fn become_first(scope: &mut Scope<'_>) -> Result<InstanceRole, StoreError> {
    let mut sql = scope.sql("UPDATE instance SET bootstrapped = ");
    sql.bind(true)
        .push_bind(" WHERE id = 1 AND bootstrapped = ", false);
    Ok(if scope.execute(sql).await? > 0 {
        InstanceRole::Administrator
    } else {
        InstanceRole::User
    })
}

/// Stores the row of a new account: `user` with the current time as creation and
/// modification time, not deleted, and an administrator of the instance if `role` says
/// so, whatever the caller put there. Of its other members the identifier, the names, the
/// email address and the password hash are stored. And the account's own space
/// ([`make_own_space`]), whatever organization the scope reaches, which is returned.
///
/// # Errors
///
/// Returns [`StoreError::Conflict`] if another user, deleted or not, or a registration
/// whose address is not confirmed yet and that has not ended, has the name or the email
/// address in any letter case, or a link that gives an account its address back holds
/// the address; and [`StoreError::InvalidId`] if the identifier is not one.
pub(crate) async fn insert_account(
    scope: &mut Scope<'_>,
    user: &mut User,
    role: InstanceRole,
) -> Result<OwnSpace, StoreError> {
    let now = scope.now_millis();
    signups::claim_account_names(scope).await?;
    if let Some(taken) = signups::signup_has_names(scope, &user.username, &user.email, now).await? {
        return Err(StoreError::Conflict(taken));
    }
    // An address a link may give back to the account that had it is that account's.
    if crate::email_revert::holds(scope, &user.email, None, now).await? {
        return Err(StoreError::Conflict(Taken::Email));
    }
    user.created_at = now;
    user.updated_at = now;
    user.deleted_at = None;
    user.is_admin = role == InstanceRole::Administrator;
    user.display_name = user.name.shown();
    users::insert_user_row(scope, user).await?;
    make_own_space(scope, &user.id).await
}

/// Makes the space of the account `user_id`, whose row this transaction stored: an
/// organization of the kind `personal` and its one workspace, neither with a name,
/// and the account the administrator of both, as who owns them. The workspace invites
/// no guests. One account owns one organization, which the schema holds.
///
/// The space is another organization than the one the scope reaches (the one the
/// account signs up for, is invited into or signs in to): it is stored as itself
/// ([`Scope::in_new_organization`]), and nothing of it is the scope's organization's.
async fn make_own_space(scope: &mut Scope<'_>, user_id: &str) -> Result<OwnSpace, StoreError> {
    let org_id = new_id();
    let workspace = Workspace {
        id: new_id(),
        guest_invitations: false,
        ..Workspace::default()
    };
    scope
        .in_new_organization(&org_id, async |scope| {
            let org = scope.org()?;
            workspaces::insert_own_space(scope, org, user_id, &workspace).await?;
            join_organization(scope, user_id, OrgRole::Admin).await?;
            let which = Which::Workspace(&workspace.id);
            join_workspace(scope, which, user_id, WorkspaceRole::Admin).await?;
            Ok(())
        })
        .await?;
    Ok(OwnSpace {
        org_id,
        workspace_id: workspace.id,
    })
}

/// Removes the registration that waits at `email`, the address the organization's
/// provider vouches for as it signs somebody in, if it was made by a link of the
/// organization: its name is free, and the use it took of the link given back.
pub(crate) async fn discard_signup_at(
    scope: &mut Scope<'_>,
    email: &str,
) -> Result<(), StoreError> {
    if let Some(signup_id) = signups::discard_at_address(scope, email).await? {
        tracing::info!(
            signup_id,
            org_id = scope.org_id(),
            "a registration that waited went with a sign-in through the provider"
        );
    }
    Ok(())
}

/// Makes way for the account of a person the organization's provider vouches for at
/// `email`: the registration that waits at the address by a link of the organization
/// goes, and the address is asked of the accounts once the names are held, where a
/// registration confirmed a moment ago is one. The registration before the names, as
/// its confirmation takes them.
///
/// # Errors
///
/// Returns [`StoreError::Conflict`] with [`Taken::Email`] if an account has the
/// address as its own.
pub(crate) async fn take_address_from_signup(
    scope: &mut Scope<'_>,
    email: &str,
) -> Result<(), StoreError> {
    discard_signup_at(scope, email).await?;
    signups::claim_account_names(scope).await?;
    if users::has_address(scope, email).await? {
        return Err(StoreError::Conflict(Taken::Email));
    }
    Ok(())
}

/// Makes the user a member of the scope's organization in `role`, unless they are one,
/// whatever as; says whether they joined. Two at once for the same user both succeed,
/// and one of them joins.
pub(crate) async fn join_organization(
    scope: &mut Scope<'_>,
    user_id: &str,
    role: OrgRole,
) -> Result<bool, StoreError> {
    let now = scope.now_millis();
    let mut sql =
        scope.sql("INSERT INTO organization_members (org_id, user_id, role, created_at) VALUES (");
    sql.bind(scope.org()?)
        .push_bind(", ", Id(user_id))
        .push_bind(", ", role.as_str())
        .push_bind(", ", now)
        .push(") ON CONFLICT DO NOTHING");
    Ok(scope.execute(sql).await? > 0)
}

/// Makes the user a member, in `role`, of a workspace in use of the scope's
/// organization, unless they are a member of it, whatever as; says whether they joined.
///
/// # Errors
///
/// Returns [`StoreError::NotFound`] if the organization has no such workspace in use,
/// and [`StoreError::Corrupt`] if it has no default workspace in use, which is broken:
/// nobody is to end up in an organization and in no workspace of it.
pub(crate) async fn join_workspace(
    scope: &mut Scope<'_>,
    which: Which<'_>,
    user_id: &str,
    role: WorkspaceRole,
) -> Result<bool, StoreError> {
    let now = scope.now_millis();
    let mut sql = scope.sql(
        "INSERT INTO workspace_members (workspace_id, user_id, role, created_at) SELECT t.id, ",
    );
    sql.bind(Id(user_id))
        .push_bind(", ", role.as_str())
        .push_bind(", ", now)
        .push(" FROM ");
    push_workspace(&mut sql, scope, which)?;
    sql.push(" ON CONFLICT DO NOTHING");
    if scope.execute(sql).await? > 0 {
        return Ok(true);
    }
    // A member already, or no such workspace to be let into.
    let mut sql = scope.sql("SELECT 1 FROM ");
    push_workspace(&mut sql, scope, which)?;
    if scope.fetch_optional(sql).await?.is_some() {
        return Ok(false);
    }
    Err(match which {
        Which::Workspace(_) => StoreError::not_found("workspace"),
        Which::DefaultWorkspace => StoreError::Corrupt {
            column: "default_workspace_id",
            reason: "the organization has no default workspace in use".to_owned(),
        },
    })
}

/// Pushes the workspace `t` that `which` names, of the scope's organization and in use:
/// what follows `FROM`, with its conditions.
fn push_workspace(sql: &mut Sql, scope: &Scope<'_>, which: Which<'_>) -> Result<(), StoreError> {
    match which {
        Which::Workspace(workspace_id) => {
            sql.push_bind("workspaces AS t WHERE t.id = ", Id(workspace_id))
                .push_bind(" AND t.org_id = ", scope.org()?);
        }
        Which::DefaultWorkspace => {
            sql.push_bind(
                "organizations AS o JOIN workspaces AS t ON t.id = o.default_workspace_id \
                 WHERE o.id = ",
                scope.org()?,
            );
        }
    }
    sql.push(" AND t.deleted_at IS NULL");
    Ok(())
}

/// Gives the scope's organization, which is there and nobody's, the name of the organization
/// that takes it, and its default workspace the same; returns that workspace.
async fn name_organization(
    scope: &mut Scope<'_>,
    organization_name: &str,
) -> Result<String, StoreError> {
    let now = scope.now_millis();
    let mut sql = scope.sql("UPDATE organizations SET name = ");
    sql.bind(organization_name)
        .push_bind(", updated_at = ", now)
        .push_bind(" WHERE id = ", scope.org()?)
        .push(" AND deleted_at IS NULL");
    if scope.execute(sql).await? == 0 {
        return Err(StoreError::not_found("organization"));
    }
    let mut sql = scope.sql("UPDATE workspaces SET name = ");
    sql.bind(organization_name)
        .push_bind(", updated_at = ", now)
        .push_bind(
            " WHERE id = (SELECT default_workspace_id FROM organizations WHERE id = ",
            scope.org()?,
        )
        .push_bind(") AND org_id = ", scope.org()?)
        .push(" AND deleted_at IS NULL")
        .push(" RETURNING id");
    // A name is one workspace's among those the organization has in use: an
    // organization that was given another workspace of that name before anybody signed
    // up keeps it, and the name is to be another.
    match scope
        .fetch_optional(sql)
        .await
        .map_err(crate::org_admin::name_taken)?
    {
        Some(row) => row.id(0),
        None => Err(StoreError::Corrupt {
            column: "default_workspace_id",
            reason: "the organization has no default workspace in use".to_owned(),
        }),
    }
}

/// Makes the scope's organization, under the scope's identifier and the given name,
/// with its first workspace, which is its default one, under the same name.
async fn found_organization(
    scope: &mut Scope<'_>,
    organization_name: &str,
    workspace_id: &str,
) -> Result<(), StoreError> {
    let workspace = Workspace {
        id: workspace_id.to_owned(),
        title: organization_name.to_owned(),
        ..Workspace::default()
    };
    let org = scope.org()?;
    workspaces::insert_organization(scope, org, organization_name, &workspace).await
}
