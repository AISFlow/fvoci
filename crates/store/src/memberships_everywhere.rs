//! A user's board memberships in every organization, as the session that asks opens
//! them: of the workspaces the one rule says it opens, and of no other.

use fvoci_model::{ContainerKinds, ContainerMember};

use crate::db::{Id, Scope, Sql};
use crate::directory::{IN_USE, Making, memberships_of};
use crate::error::StoreError;
use crate::{container_kinds, container_team_access, containers};

/// The user's memberships of boards of `kinds` in live workspaces of every
/// organization, direct and through a team (`inherited_role`), ordered by board
/// identifier, read through `scope`, which reaches every organization
/// (`Store::container_memberships_of_user`).
pub(crate) async fn of_user(
    scope: &mut Scope<'_>,
    user_id: &str,
    making: Making<&str>,
    kinds: ContainerKinds,
) -> Result<Vec<ContainerMember>, StoreError> {
    let opened = opened_workspaces(scope, user_id, making).await?;
    // A list of none is no SQL, and there is nothing in no workspace.
    if opened.is_empty() {
        return Ok(Vec::new());
    }
    let mut sql = containers::select_members(scope);
    sql.push(
        " JOIN containers AS b ON b.id = bm.container_id \
         JOIN workspaces AS t ON t.id = b.workspace_id \
         JOIN organizations AS o ON o.id = t.org_id \
         JOIN workspace_members AS wm ON wm.workspace_id = t.id AND wm.user_id = bm.user_id \
         WHERE bm.user_id = ",
    )
    .bind(Id(user_id))
    .push(IN_USE)
    .push(&scope.same_organization(&[("bm", "b"), ("b", "t"), ("wm", "t")]));
    container_kinds::push_kinds(&mut sql, "b.kind", kinds);
    push_among(&mut sql, &opened);
    sql.push(" ORDER BY bm.container_id");
    let mut members = containers::fetch_members(scope, sql).await?;
    extend_across_organizations(scope, user_id, &opened, kinds, &mut members).await?;
    Ok(members)
}

/// Adds to `members` the boards `user_id` has a role on through a team, of every
/// organization, as `Store::container_memberships_of_user` lists the direct ones: of a
/// workspace and an organization in use, among the workspaces `opened`. Sorted by
/// board.
async fn extend_across_organizations(
    scope: &mut Scope<'_>,
    user_id: &str,
    opened: &[String],
    kinds: ContainerKinds,
    members: &mut Vec<ContainerMember>,
) -> Result<(), StoreError> {
    let mut sql = scope.sql(
        "SELECT bt.container_id, tm.user_id, bt.role FROM container_teams AS bt \
         JOIN ",
    );
    sql.push(&crate::teams::present_members(scope))
        .push(
            " tm ON tm.team_id = bt.team_id AND tm.org_id = bt.org_id \
             AND tm.workspace_id = bt.workspace_id \
             JOIN containers AS b ON b.id = bt.container_id \
             JOIN workspaces AS t ON t.id = b.workspace_id \
             JOIN organizations AS o ON o.id = t.org_id WHERE tm.user_id = ",
        )
        .bind(Id(user_id))
        .push(IN_USE)
        .push(&scope.same_organization(&[("bt", "b"), ("b", "t")]));
    container_kinds::push_kinds(&mut sql, "b.kind", kinds);
    push_among(&mut sql, opened);
    container_team_access::extend(scope, sql, members).await?;
    members.sort_by(|a, b| a.container_id.cmp(&b.container_id));
    Ok(())
}

/// The workspaces the user is a member of that a session of that `making` opens, by
/// the one rule of it ([`crate::SessionTaken::opened_by`]): what a workspace takes is
/// read as an entry into it reads it, and nothing here says it again.
async fn opened_workspaces(
    scope: &mut Scope<'_>,
    user_id: &str,
    making: Making<&str>,
) -> Result<Vec<String>, StoreError> {
    Ok(memberships_of(scope, user_id)
        .await?
        .into_iter()
        .filter(|membership| membership.takes.opened_by(making))
        .map(|membership| membership.id)
        .collect())
}

/// Keeps the workspaces `t` that are among `opened`, which names one at least: a
/// workspace that is not known to be opened is left out.
fn push_among(sql: &mut Sql, opened: &[String]) {
    let mut separator = " AND t.id IN (";
    for workspace in opened {
        sql.push_bind(separator, Id(workspace));
        separator = ", ";
    }
    sql.push(")");
}
