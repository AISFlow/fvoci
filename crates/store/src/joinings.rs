//! The roles on the boards of a workspace that a user held when they joined it as the
//! organization's administrator: written when they join, and given back when the
//! joining ends, whatever they did with their roles meanwhile. Who joined a workspace
//! was its administrator for that time only; what they made themselves on its boards
//! (a member of a board they could not read, an admin of one they read, a member of an
//! open board, the maker of a new one) goes with the joining.

use fvoci_model::{ContainerMember, ContainerRole};
use serde::{Deserialize, Serialize};

use crate::containers;
use crate::db::{Id, Param, Scope};
use crate::error::StoreError;
use crate::trash;

/// One role on a board as it was held: the board, and the membership's direct role.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct HeldRole(String, ContainerRole);

/// The user's memberships of the boards of the workspace, those in the trash among them,
/// as the text the joining keeps (`workspace_members.prior_container_roles`), by board.
pub(crate) async fn held_roles(
    scope: &mut Scope<'_>,
    workspace_id: &str,
    user_id: &str,
) -> Result<String, StoreError> {
    let mut sql = scope.sql(
        "SELECT bm.container_id, bm.role \
         FROM container_members AS bm JOIN containers AS b ON b.id = bm.container_id \
         WHERE b.workspace_id = ",
    );
    sql.bind(Id(workspace_id))
        .push_bind(" AND bm.user_id = ", Id(user_id))
        .push(" ORDER BY bm.container_id");
    let mut held: Vec<HeldRole> = scope
        .fetch_all(sql)
        .await?
        .iter()
        .map(|row| {
            Ok(HeldRole(
                row.id(0)?,
                containers::role_from_text(&row.text(1)?)?,
            ))
        })
        .collect::<Result<_, StoreError>>()?;
    for (container_id, members) in trashed_members(scope, workspace_id).await? {
        if let Some(member) = members.iter().find(|member| member.user_id == user_id) {
            held.push(HeldRole(container_id, member.role));
        }
    }
    serde_json::to_string(&held).map_err(|error| StoreError::Corrupt {
        column: "prior_container_roles",
        reason: error.to_string(),
    })
}

/// Gives the user back the memberships of the boards of the workspace they held when
/// they joined it (`held`, as [`held_roles`] wrote it): every other one of theirs there
/// goes, and one of a board that is gone meanwhile is not made again.
pub(crate) async fn give_back_roles(
    scope: &mut Scope<'_>,
    workspace_id: &str,
    user_id: &str,
    held: &str,
) -> Result<(), StoreError> {
    let held: Vec<HeldRole> = if held.is_empty() {
        Vec::new()
    } else {
        serde_json::from_str(held).map_err(|error| StoreError::Corrupt {
            column: "prior_container_roles",
            reason: error.to_string(),
        })?
    };
    let mut sql = scope.sql("DELETE FROM container_members WHERE user_id = ");
    sql.bind(Id(user_id))
        .push(" AND container_id IN (SELECT id FROM containers WHERE workspace_id = ")
        .bind(Id(workspace_id))
        .push(")");
    scope.execute(sql).await?;
    give_back_roles_in_the_trash(scope, workspace_id, user_id, &held).await?;
    for HeldRole(container_id, role) in held {
        let mut sql =
            scope.sql("INSERT INTO container_members (container_id, user_id, role) SELECT id, ");
        sql.bind(Id(user_id))
            .push_bind(", ", role.as_str())
            .push_bind(" FROM containers WHERE id = ", Id(&container_id))
            .push_bind(" AND workspace_id = ", Id(workspace_id));
        scope.execute(sql).await?;
    }
    Ok(())
}

/// Gives the user back, on each board of the workspace in the trash, the membership
/// they held when they joined, and no other: a board deleted while they were joined
/// comes back without what they made themselves on it.
async fn give_back_roles_in_the_trash(
    scope: &mut Scope<'_>,
    workspace_id: &str,
    user_id: &str,
    held: &[HeldRole],
) -> Result<(), StoreError> {
    for (container_id, mut members) in trashed_members(scope, workspace_id).await? {
        let theirs = members.iter().find(|member| member.user_id == user_id);
        let given_back = held.iter().find(|role| role.0 == container_id);
        if theirs.map(|member| member.role) == given_back.map(|role| role.1) {
            continue;
        }
        members.retain(|member| member.user_id != user_id);
        if let Some(HeldRole(_, role)) = given_back {
            members.push(ContainerMember::new(&container_id, user_id, *role));
        }
        let mut sql = scope.sql("UPDATE deleted_containers SET members = ");
        sql.bind(Param::json(&members)?)
            .push_bind(" WHERE id = ", Id(&container_id));
        scope.execute(sql).await?;
    }
    Ok(())
}

/// The memberships kept with each board of the workspace in the trash, by board.
async fn trashed_members(
    scope: &mut Scope<'_>,
    workspace_id: &str,
) -> Result<Vec<(String, Vec<ContainerMember>)>, StoreError> {
    let mut sql = scope.sql("SELECT id, members FROM deleted_containers WHERE workspace_id = ");
    sql.bind(Id(workspace_id)).push(" ORDER BY id");
    scope
        .fetch_all(sql)
        .await?
        .iter()
        .map(|row| Ok((row.id(0)?, trash::from_json(row, 1, "members")?)))
        .collect()
}
