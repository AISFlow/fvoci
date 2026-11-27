//! The order in which a transaction holds the links of invitations and what they hang
//! from, on PostgreSQL. Two transactions that hold what they share in one order wait
//! for each other; in two orders, each can hold what the other waits for, and
//! PostgreSQL ends one of them (`40P01`).
//!
//! 1. The turn of several links ([`take_turn`]): first, by a transaction that may hold
//!    the links of more than one organization. Under it, the steps below go per
//!    organization, and one organization's after another's in any order: two
//!    transactions that span organizations take turns.
//! 2. The organization's row ([`org_admin::hold_organization_row`], or
//!    [`hold_organization`] of another), before any link of it that is held with
//!    another, or with anything a deletion holds or deletes. Two transactions that hold
//!    several links of one organization wait for each other here.
//! 3. The workspace. A transaction that holds a single member's link, which only the
//!    workspace's purge deletes, and after it no organization's row, may hold the
//!    workspace instead of the organization's row ([`hold_for_letting_in`]).
//! 4. The board.
//! 5. The links, a link before the links made like it for the recipients of its mail,
//!    and then the registrations made by them.
//!
//! A step may be left out, never taken after a later one of the same organization: a
//! new link holds its workspace, its board and the link it is made like
//! ([`hold_for_new_link`]). Deleting a board or a workspace holds the organization's
//! row and then the board or the workspace, and the schema's cascades then delete the
//! links and the registrations made by them in an order of the database's own:
//! whoever holds more than one link, or a link and then anything such a deletion holds
//! or deletes, holds the organization's row first. A transaction that holds a single
//! link, and after it nothing a deletion holds or deletes but what it reaches through
//! that link (a registration made by it), may hold it alone: a deletion waits for
//! it.
//!
//! SQLite's writers wait for each other already: nothing here holds anything there.

use fvoci_model::Invitation;

use crate::NewLink;
use crate::db::{Dialect, Id, LockRank, OrganizationHeld, Scope};
use crate::error::StoreError;
use crate::{containers, org_admin};

/// The key of the advisory lock of the turn of several links: `fvoclnks`.
const SEVERAL_LINKS_LOCK: i64 = 0x6676_6f63_6c6e_6b73;

/// That the transaction holds the turn of several links, until it ends.
#[derive(Debug, Clone, Copy)]
pub(crate) struct Turn(());

/// Takes the turn of the transactions that may hold the links of more than one
/// organization, until this one ends: a registration, or an acceptance, that replaces
/// one made by another link holds both, and the clean-up of registrations that ended
/// holds the links of any.
pub(crate) async fn take_turn(scope: &mut Scope<'_>) -> Result<Turn, StoreError> {
    scope.record_lock(LockRank::LinkTurn, "links");
    if scope.dialect() == Dialect::Postgres {
        let mut sql = scope.sql("SELECT pg_advisory_xact_lock(");
        sql.bind(SEVERAL_LINKS_LOCK).push(")");
        scope.fetch_optional(sql).await?;
    }
    Ok(Turn(()))
}

/// Holds the handle's organization's row, for a transaction that writes several links
/// of the organization and no other's, which a deletion of their board or workspace
/// takes along: the revocation of a link with its children, and turning a
/// workspace's guest links off. No turn: what holds the links of several
/// organizations holds this row before any link of it.
pub(crate) async fn hold_links_of_organization<'c>(
    scope: &mut Scope<'c>,
) -> Result<OrganizationHeld<'c>, StoreError> {
    org_admin::hold_organization_row(scope).await
}

/// Holds the row of the organization `org_id` before a link of it, as
/// [`org_admin::hold_organization_row`] holds the handle's, whichever organization the
/// handle names, if any: under the turn, or as the one organization whose links the
/// transaction holds. The row is reached past the wall, which the caller opens for
/// another organization's.
pub(crate) async fn hold_organization<'c>(
    scope: &mut Scope<'c>,
    org_id: &str,
) -> Result<OrganizationHeld<'c>, StoreError> {
    scope.record_lock(LockRank::Organization, org_id);
    if scope.dialect() == Dialect::Postgres {
        let mut sql = scope.sql("SELECT id FROM organizations WHERE id = ");
        sql.bind(Id(org_id)).push(" FOR NO KEY UPDATE");
        scope.fetch_optional(sql).await?;
    }
    Ok(OrganizationHeld::new())
}

/// Holds what a new link hangs from, in the order above: its workspace of the handle's
/// organization in use, and for a guest's link the workspace inviting guests and its
/// board there that is no template; for a link made like another, that link, of the
/// same workspace and board, while it is neither revoked nor ended at `now`.
///
/// No organization's row: a link being made is nothing a deletion waits for. A guest's
/// link holds the workspace against turning its guest links off, which then waits and
/// revokes the new link with the others, or comes first and is found; the board, as
/// its reference does, against its deletion, which comes first and is found, or
/// waits; and the link it is made like against its revocation, likewise.
///
/// # Errors
///
/// Returns [`StoreError::NotFound`], naming which, if one of them does not let the link
/// be made; [`StoreError::NoOrganization`] if the handle names none; and
/// [`StoreError::Database`] if a statement fails.
pub(crate) async fn hold_for_new_link(
    scope: &mut Scope<'_>,
    link: &NewLink<'_>,
) -> Result<(), StoreError> {
    let postgres = scope.dialect() == Dialect::Postgres;
    if link.guest_of.is_some() {
        scope.record_lock(LockRank::Workspace, link.workspace_id);
    }
    let mut sql = scope.sql("SELECT id FROM workspaces WHERE id = ");
    sql.bind(Id(link.workspace_id))
        .push_bind(" AND org_id = ", scope.org()?)
        .push(" AND deleted_at IS NULL");
    if link.guest_of.is_some() {
        sql.push_bind(" AND guest_invitations = ", true);
        if postgres {
            sql.push(" FOR SHARE");
        }
    }
    if scope.fetch_optional(sql).await?.is_none() {
        return Err(StoreError::not_found("workspace"));
    }
    if let Some((container_id, _)) = link.guest_of {
        scope.record_lock(LockRank::Container, container_id);
        let mut sql = scope.sql("SELECT id FROM containers WHERE id = ");
        sql.bind(Id(container_id))
            .push_bind(" AND workspace_id = ", Id(link.workspace_id))
            .push_bind(" AND is_template = ", false);
        // What the new link's reference takes anyway: only a deletion waits for it.
        if postgres {
            sql.push(" FOR KEY SHARE");
        }
        if scope.fetch_optional(sql).await?.is_none() {
            return Err(StoreError::not_found("board"));
        }
    }
    if let Some(mailed) = link.mailed {
        let mut sql = scope.sql("SELECT id FROM invitations WHERE id = ");
        sql.bind(Id(mailed.parent_id))
            .push_bind(" AND org_id = ", scope.org()?)
            .push_bind(" AND workspace_id = ", Id(link.workspace_id));
        // Of the board held above: a link of another board is not held after its own.
        match link.guest_of {
            Some((container_id, _)) => sql.push_bind(" AND container_id = ", Id(container_id)),
            None => sql.push(" AND container_id IS NULL"),
        };
        sql.push_bind(" AND revoked_at = ", 0_i64)
            .push_bind(" AND expires_at > ", link.now);
        if postgres {
            sql.push(" FOR SHARE");
        }
        if scope.fetch_optional(sql).await?.is_none() {
            return Err(StoreError::not_found("invitation"));
        }
    }
    Ok(())
}

/// Holds what a link that lets somebody in hangs from, before the link, and says
/// whether it is there still: for a guest's link the organization's row and then its
/// board, as its deletion holds them, and as team changes hold them, which keeps
/// the access the board gives as it is through the grant; for a member's link its workspace
/// in use, `FOR KEY SHARE`, against its purge from the trash, which holds it before the
/// links it deletes. Nothing else waits for that, so taking a maker out of the
/// workspace meanwhile does not wait for the link, and is seen once the link is held.
/// Under the `turn` the transaction holds another link after this one, after that
/// link's organization's row: then the organization's row comes first for a member's
/// link too.
pub(crate) async fn hold_for_letting_in(
    scope: &mut Scope<'_>,
    link: &Invitation,
    turn: Option<Turn>,
) -> Result<bool, StoreError> {
    if link.container_id.is_some() || turn.is_some() {
        org_admin::hold_organization_row(scope).await?;
    }
    if let Some(container_id) = &link.container_id {
        return containers::lock_container(scope, container_id).await;
    }
    scope.record_lock(LockRank::Workspace, &link.workspace_id);
    let mut sql = scope.sql("SELECT id FROM workspaces WHERE id = ");
    sql.bind(Id(&link.workspace_id))
        .push_bind(" AND org_id = ", scope.org()?)
        .push(" AND deleted_at IS NULL");
    if scope.dialect() == Dialect::Postgres {
        sql.push(" FOR KEY SHARE");
    }
    Ok(scope.fetch_optional(sql).await?.is_some())
}
