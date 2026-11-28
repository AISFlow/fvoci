//! An account leaving an organization that people work in together, at its own
//! asking or taken out by an administrator: what it has there goes, and what it wrote
//! stays.
//!
//! It is what erasing an account does in an organization that stays
//! (`account_erasure.rs`), but for two things: the organization keeps the account as
//! who made it, which is counted still (`organizations_made_by`), and a workspace the
//! account joined as the organization's administrator records the joining's end
//! (`OrgAdminLeft` when leaving on its own, `OrgAdminRevoked` when taken out by an
//! administrator). Who administers what is asked and the account leaves in one
//! transaction, under the instance's row and then the organization's, as every change
//! of who administers the organization holds them: of two administrators who leave or
//! take each other out at once, the second is asked once the first is out.

use fvoci_model::{AuditAction, OrgKind, OrgRole};

use crate::Store;
use crate::account_deletion::{self, Without};
use crate::account_erasure::{self, LeftFrom};
use crate::audit::{self, AuditActor, AuditEntry};
use crate::db::{OrganizationHeld, Scope};
use crate::error::StoreError;
use crate::{directory, org_admin, users};

/// What became of an account's asking to leave the handle's organization.
#[derive(Debug, PartialEq, Eq)]
pub enum Departure {
    /// It is out of the organization: what it left there.
    Left(LeftFrom),
    /// It is no member of the organization, or the organization is none in use that
    /// people work in together: nothing changed.
    NotAMember,
    /// Something stands in the way of leaving: nothing changed.
    Refused(DepartureRefused),
}

/// What stands in the way of an account's leaving an organization.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DepartureRefused {
    /// The account has no password to sign in with (made through an organization's
    /// provider and given none yet): leaving would cut off its way in to its own space.
    PasswordRequired,
    /// The organization is the instance's own, and the account is the only
    /// administrator of the instance in use.
    LastInstanceAdmin,
    /// The account is the only administrator of the organization whose account is in
    /// use, and other people are in it.
    SoleAdmin,
    /// Nobody else is in the organization: it would be left empty.
    Alone,
}

/// What became of an administrator's taking a member out of the handle's organization.
#[derive(Debug, PartialEq, Eq)]
pub enum MemberRemoval {
    /// The member is out of the organization: what they left there.
    Removed(LeftFrom),
    /// The caller does not administer the organization, or the organization is none in
    /// use that people work in together: nothing changed.
    NotAnAdmin,
    /// The target is no member of the organization: nothing changed.
    NotAMember,
    /// The administrator named themselves: leaving is done through
    /// [`Store::leave_organization`]. Nothing changed.
    CannotRemoveSelf,
    /// Something would be left without anybody to run it, or empty: nothing changed.
    Refused(RemovalRefused),
}

/// What stands in the way of taking a member out of an organization.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RemovalRefused {
    /// The organization is the instance's own, and the member is the only administrator
    /// of the instance in use.
    LastInstanceAdmin,
    /// The member is the only administrator of the organization whose account is in
    /// use, and other people are in it.
    SoleAdmin,
    /// Nobody else is in the organization: it would be left empty.
    Alone,
}

impl From<RemovalRefused> for DepartureRefused {
    fn from(refused: RemovalRefused) -> Self {
        match refused {
            RemovalRefused::LastInstanceAdmin => Self::LastInstanceAdmin,
            RemovalRefused::SoleAdmin => Self::SoleAdmin,
            RemovalRefused::Alone => Self::Alone,
        }
    }
}

/// Whether the organization is the instance's own first organization or another.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum InstanceStanding {
    InstanceOrg,
    OtherOrg,
}

impl InstanceStanding {
    async fn of(scope: &mut Scope<'_>) -> Result<Self, StoreError> {
        if directory::is_the_instances_organization(scope).await? {
            Ok(Self::InstanceOrg)
        } else {
            Ok(Self::OtherOrg)
        }
    }
}

/// How an account's joinings of workspaces as organization administrator are ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum JoiningsEnding {
    /// The account leaves of its own accord (`OrgAdminLeft`).
    Left,
    /// Another administrator takes the account out (`OrgAdminRevoked`).
    Revoked,
}

impl Store {
    /// Takes the account `user_id` out of the handle's organization at its own asking,
    /// unless something stands in the way ([`DepartureRefused`], asked first in that
    /// order: `PasswordRequired`, `LastInstanceAdmin`, `SoleAdmin`, `Alone`): its
    /// joinings of workspaces as the organization's administrator end, each recorded in
    /// its workspace; then it leaves each workspace of the organization, in use or in
    /// the trash, as an erased account does, a board it was the only admin of passing
    /// to the oldest other administrator of the workspace on their own account
    /// (recorded as the account's doing); then its roles on boards left from before,
    /// what it follows, its membership of the organization and the links it made there
    /// that nobody used go. Who leaves the server's own organization runs the server no
    /// more, as one made its member does not (`users::follow_org_role`). Its sessions,
    /// its identities, and what it wrote stay, and so does the organization's word of
    /// who made it. All of it happens or none.
    ///
    /// # Errors
    ///
    /// Returns [`StoreError::NoOrganization`] if the handle names none, and
    /// [`StoreError::Database`] if a statement fails; nothing is changed then.
    pub async fn leave_organization(&self, user_id: &str) -> Result<Departure, StoreError> {
        self.transaction(async |scope| {
            // The instance's row first, as a change of who administers the instance
            // takes it, then the organization's, as a change of its roles does.
            let held_instance = users::lock_instance(scope).await?;
            let held_org = org_admin::hold_organization_row(scope).await?;
            if !is_shared_and_in_use(scope).await? {
                return Ok(Departure::NotAMember);
            }
            let Some(role) = org_admin::org_role_of(scope, user_id).await? else {
                return Ok(Departure::NotAMember);
            };
            if !users::has_password(scope, user_id).await? {
                return Ok(Departure::Refused(DepartureRefused::PasswordRequired));
            }
            let standing = InstanceStanding::of(scope).await?;
            if let Some(refused) = standing_refusal(scope, user_id, role, standing).await? {
                return Ok(Departure::Refused(refused.into()));
            }
            // The administrators of the server's own organization are the server's for
            // now (`members::follow_instance_role`): who leaves it, as who is made its
            // member, does not run the server, or they would make anybody its
            // administrator again from outside it.
            if standing == InstanceStanding::InstanceOrg
                && !users::follow_org_role(scope, held_instance, user_id, OrgRole::Member).await?
            {
                return Ok(Departure::Refused(DepartureRefused::LastInstanceAdmin));
            }
            let actor = AuditActor::Person(user_id.to_owned());
            let left =
                evict_from_organization(scope, held_org, user_id, &actor, JoiningsEnding::Left)
                    .await?;
            Ok(Departure::Left(left))
        })
        .await
    }

    /// Takes the member `user_id` out of the handle's organization on behalf of the
    /// administrator `actor_id`: the same removal as [`Store::leave_organization`],
    /// without requiring `user_id` to have a password (an administrator may remove an
    /// account that signs in only through a provider), with every board handover and
    /// revoked workspace joining recorded under `actor_id`, and an
    /// [`AuditAction::OrgMemberRemoved`] record written to the organization's audit log.
    /// An administrator cannot take themselves out here ([`MemberRemoval::CannotRemoveSelf`]).
    /// All of it happens or none.
    ///
    /// # Errors
    ///
    /// Returns [`StoreError::NoOrganization`] if the handle names none, and
    /// [`StoreError::Database`] if a statement fails; nothing is changed then.
    pub async fn remove_org_member(
        &self,
        user_id: &str,
        actor_id: &str,
    ) -> Result<MemberRemoval, StoreError> {
        self.transaction(async |scope| {
            let held_instance = users::lock_instance(scope).await?;
            let held_org = org_admin::hold_organization_row(scope).await?;
            if !is_shared_and_in_use(scope).await? {
                return Ok(MemberRemoval::NotAnAdmin);
            }
            if org_admin::org_role_of(scope, actor_id).await? != Some(OrgRole::Admin) {
                return Ok(MemberRemoval::NotAnAdmin);
            }
            if user_id == actor_id {
                return Ok(MemberRemoval::CannotRemoveSelf);
            }
            let Some(role) = org_admin::org_role_of(scope, user_id).await? else {
                return Ok(MemberRemoval::NotAMember);
            };
            let standing = InstanceStanding::of(scope).await?;
            if let Some(refused) = standing_refusal(scope, user_id, role, standing).await? {
                return Ok(MemberRemoval::Refused(refused));
            }
            if standing == InstanceStanding::InstanceOrg
                && !users::follow_org_role(scope, held_instance, user_id, OrgRole::Member).await?
            {
                return Ok(MemberRemoval::Refused(RemovalRefused::LastInstanceAdmin));
            }
            let actor = AuditActor::Person(actor_id.to_owned());
            let left =
                evict_from_organization(scope, held_org, user_id, &actor, JoiningsEnding::Revoked)
                    .await?;
            let org_id = scope.org_id().ok_or(StoreError::NoOrganization)?;
            let entry = AuditEntry::new("", actor_id, AuditAction::OrgMemberRemoved, &org_id)
                .with_subject(user_id);
            audit::record_entry(scope, &entry).await?;
            Ok(MemberRemoval::Removed(left))
        })
        .await
    }
}

/// Removes `user_id` from every workspace, board, group, subscription, unused link, and
/// membership of the scope's organization, ending admin joinings as `joinings` says and
/// recording handovers and revoked joinings under `actor`.
async fn evict_from_organization<'c>(
    scope: &mut Scope<'c>,
    held_org: OrganizationHeld<'c>,
    user_id: &str,
    actor: &AuditActor,
    joinings: JoiningsEnding,
) -> Result<LeftFrom, StoreError> {
    let joined = match joinings {
        JoiningsEnding::Left => org_admin::leave_joinings(scope, user_id).await?,
        JoiningsEnding::Revoked => org_admin::revoke_joinings(scope, user_id, actor).await?,
    };
    let mut left = LeftFrom::default();
    account_erasure::leave_every_workspace(scope, user_id, actor, &mut left).await?;
    account_erasure::leave_organization(scope, held_org, user_id).await?;
    crate::favorites::forget_user_in_organization(scope, user_id).await?;
    // A workspace they were in by a joining alone is one they left too.
    left.left.extend(joined);
    left.left.sort_unstable();
    left.left.dedup();
    Ok(left)
}

/// Whether the scope's organization is in use and one that people work in together:
/// of a user's own space, and of the system's, nobody is a member to leave.
async fn is_shared_and_in_use(scope: &mut Scope<'_>) -> Result<bool, StoreError> {
    let mut sql = scope.sql("SELECT 1 FROM organizations WHERE id = ");
    sql.bind(scope.org()?)
        .push_bind(" AND kind = ", OrgKind::Shared.as_str())
        .push(" AND deleted_at IS NULL AND is_system = false");
    scope.exists(sql).await
}

/// What of the instance or the organization stands in the way of `user_id` (in `role`)
/// leaving or being removed, in the order a refused deletion is told in:
/// `LastInstanceAdmin`, then `SoleAdmin`, then `Alone`.
async fn standing_refusal(
    scope: &mut Scope<'_>,
    user_id: &str,
    role: OrgRole,
    standing: InstanceStanding,
) -> Result<Option<RemovalRefused>, StoreError> {
    // The instance comes first, as a refused deletion is told.
    if standing == InstanceStanding::InstanceOrg
        && account_deletion::is_last_instance_admin(scope, user_id).await?
    {
        return Ok(Some(RemovalRefused::LastInstanceAdmin));
    }
    let org_id = scope.org_id().ok_or(StoreError::NoOrganization)?;
    Ok(
        match (
            account_deletion::without(scope, &org_id, user_id).await?,
            role,
        ) {
            (Without::Nobody, _) => Some(RemovalRefused::Alone),
            (Without::NoAdministrator, OrgRole::Admin) => Some(RemovalRefused::SoleAdmin),
            // A member leaves an organization nobody administers as it is.
            (Without::NoAdministrator, OrgRole::Member) | (Without::Administered, _) => None,
        },
    )
}
