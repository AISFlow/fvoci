//! What asks across all organizations: single-organization transactions open the wall
//! for one read and close it again before anything else runs ([`open_wall`],
//! [`close_wall`], [`is_the_instances_organization`], [`organizations_made_by`],
//! [`of_people`]), while [`organizations_of_user`] reads directly under a
//! `Tenant::System` scope. Nothing is written past the wall.
//!
//! The opening and the closing are here too ([`open_wall`], [`close_wall`]), for the
//! one other place that opens a transaction's wall: a registration's statement on the
//! one link it was made by (`signups.rs`).

use fvoci_model::{OrgKind, UserOrganization, Workspace, new_id};

use crate::db::{Dialect, Id, Row, Scope, Sql};
use crate::error::StoreError;
use crate::workspaces;

/// What the reach of a transaction was before its wall was opened.
#[must_use = "the wall is closed again with `close_wall`"]
pub(crate) struct OpenedWall {
    was_open: bool,
}

/// Opens the wall of the transaction of `scope`, on PostgreSQL: the rows of every
/// organization are in its reach until [`close_wall`] puts the reach back as it was,
/// which is not always closed: a transaction that opened it before keeps it open.
/// Only inside a transaction, whose setting the reach is.
///
/// # Errors
///
/// Returns [`StoreError::Database`] if the reach cannot be changed.
pub(crate) async fn open_wall(scope: &mut Scope<'_>) -> Result<OpenedWall, StoreError> {
    if scope.is_wall_open() {
        return Ok(OpenedWall { was_open: true });
    }
    set_reach(scope, "on").await?;
    scope.open_wall();
    Ok(OpenedWall { was_open: false })
}

/// Puts the reach of the transaction back as it was before [`open_wall`].
///
/// # Errors
///
/// Returns [`StoreError::Database`] if the reach cannot be changed.
pub(crate) async fn close_wall(scope: &mut Scope<'_>, wall: OpenedWall) -> Result<(), StoreError> {
    if wall.was_open {
        return Ok(());
    }
    set_reach(scope, "off").await?;
    scope.close_wall();
    Ok(())
}

async fn set_reach(scope: &mut Scope<'_>, reach: &str) -> Result<(), StoreError> {
    let mut sql = scope.sql("SELECT set_config('fvoci.system', ");
    sql.bind(reach).push(", true)");
    scope.fetch_optional(sql).await?;
    Ok(())
}

/// Runs the one read `read` with the rows of every organization in reach, in the
/// transaction of `scope`, which is one of an organization, and puts the reach back
/// as it was. On SQLite, which has no wall, `read` runs as it is. Only inside a
/// transaction, whose setting the reach is.
///
/// # Errors
///
/// Returns [`StoreError::NoOrganization`] for a scope that names no organization, on
/// either database: what such a scope reaches is not this function's to change, and
/// a read that it could not make would answer as if there were nothing to find.
/// Returns the error of `read`, and [`StoreError::Database`] if the reach cannot be
/// changed. It is put back whatever `read` answered; where that fails after `read`
/// failed, the error of `read` is the one returned, and the transaction is one
/// PostgreSQL takes nothing more in.
async fn past_the_wall<T>(
    scope: &mut Scope<'_>,
    read: impl AsyncFnOnce(&mut Scope<'_>) -> Result<T, StoreError>,
) -> Result<T, StoreError> {
    if !scope.has_org() {
        return Err(StoreError::NoOrganization);
    }
    if scope.dialect() != Dialect::Postgres {
        return read(scope).await;
    }
    let wall = open_wall(scope).await?;
    let result = read(scope).await;
    let closed = close_wall(scope, wall).await;
    match (result, closed) {
        (Ok(value), Ok(())) => Ok(value),
        (Err(error), _) | (Ok(_), Err(error)) => Err(error),
    }
}

/// Whether the organization of `scope` is the instance's own: the oldest one in use,
/// whose administrators are the instance's (`members::follow_instance_role`). Asked in
/// the caller's transaction, so that the answer holds for what it does; the
/// organization's wall hides the others, and is opened for this one read.
pub(crate) async fn is_the_instances_organization(
    scope: &mut Scope<'_>,
) -> Result<bool, StoreError> {
    let Some(org) = scope.org_id() else {
        return Ok(false);
    };
    let oldest = past_the_wall(scope, async |scope| oldest_organization(scope).await).await?;
    Ok(oldest.is_some_and(|id| id == org))
}

/// How many organizations in use the account `user_id` made, that people work in
/// together: by signing up with the name of one, or later. Of all, which the wall of
/// the caller's transaction hides and is opened for this one read. Not the ones it was
/// let into, and not its own space.
///
/// # Errors
///
/// Returns [`StoreError::NoOrganization`] for a scope that names no organization: the
/// count is asked by a transaction that makes one, and nothing is counted as none
/// for want of reach.
pub(crate) async fn organizations_made_by(
    scope: &mut Scope<'_>,
    user_id: &str,
) -> Result<i64, StoreError> {
    past_the_wall(scope, async |scope| {
        let mut sql = scope.sql("SELECT COUNT(*) FROM organizations WHERE created_by = ");
        sql.bind(Id(user_id))
            .push(" AND deleted_at IS NULL AND is_system = false")
            .push_bind(" AND kind = ", OrgKind::Shared.as_str());
        Ok(scope.first(sql).await?.unwrap_or(0))
    })
    .await
}

/// Every organization in use that people work in together of which `user_id` is a
/// member, oldest first: includes organizations where the account is in no workspace,
/// and leaves out personal spaces and organizations where it is only a workspace guest.
/// Called in a scope that reaches every organization.
pub(super) async fn organizations_of_user(
    scope: &mut Scope<'_>,
    user_id: &str,
) -> Result<Vec<UserOrganization>, StoreError> {
    let mut sql = scope.sql(
        "SELECT o.id, o.name, om.role FROM organization_members AS om \
         JOIN organizations AS o ON o.id = om.org_id WHERE om.user_id = ",
    );
    sql.bind(Id(user_id))
        .push(" AND o.deleted_at IS NULL AND o.is_system = false")
        .push_bind(" AND o.kind = ", OrgKind::Shared.as_str())
        .push(" ORDER BY o.created_at, o.id");
    scope
        .fetch_all(sql)
        .await?
        .iter()
        .map(|row| {
            Ok(UserOrganization {
                id: row.id(0)?,
                name: row.text(1)?,
                role: role(row, 2)?,
            })
        })
        .collect()
}

/// Appends the condition that `kind`, the column of an organization's kind, says one
/// that people work in together. Written into the statement and not bound: the index
/// of those organizations by age (`organizations_of_people_idx`) holds no account's own
/// space, and a database reads it only for a statement that says the kind itself. For the reads that look
/// for the oldest of them among however many spaces the accounts have.
pub(crate) fn of_people(sql: &mut Sql, kind: &'static str) {
    sql.push(" AND ")
        .push(kind)
        .push(" = '")
        .push(OrgKind::Shared.as_str())
        .push("'");
}

pub(super) async fn create_organization(
    scope: &mut Scope<'_>,
    name: &str,
    workspace: &Workspace,
) -> Result<String, StoreError> {
    let id = new_id();
    workspaces::insert_organization(scope, Id(&id), name, workspace).await?;
    Ok(id)
}

/// Reads a role from a column.
pub(super) fn role<R: std::str::FromStr<Err = fvoci_model::UnknownRole>>(
    row: &Row,
    index: usize,
) -> Result<R, StoreError> {
    row.text(index)?
        .parse()
        .map_err(|error: fvoci_model::UnknownRole| StoreError::Corrupt {
            column: "role",
            reason: error.to_string(),
        })
}

/// Reads the kind of an organization from a column.
pub(crate) fn org_kind(row: &Row, index: usize) -> Result<OrgKind, StoreError> {
    row.text(index)?
        .parse()
        .map_err(|error: fvoci_model::UnknownOrgKind| StoreError::Corrupt {
            column: "kind",
            reason: error.to_string(),
        })
}

/// The instance's own organization as `scope` reaches the organizations: the oldest
/// non-system one in use that people work in together.
pub(crate) async fn oldest_organization(
    scope: &mut Scope<'_>,
) -> Result<Option<String>, StoreError> {
    let mut sql = scope.sql("SELECT id FROM organizations WHERE is_system = ");
    sql.bind(false).push(" AND deleted_at IS NULL");
    of_people(&mut sql, "kind");
    sql.push(" ORDER BY created_at, id LIMIT 1");
    scope.first(sql).await
}

/// The organization that is the space of the account `user_id`, in use or not, by
/// who owns it: in a scope that reaches every organization.
pub(crate) async fn space_of(
    scope: &mut Scope<'_>,
    user_id: &str,
) -> Result<Option<String>, StoreError> {
    let mut sql = scope.sql("SELECT id FROM organizations WHERE owner_id = ");
    sql.bind(Id(user_id));
    scope.first(sql).await
}
