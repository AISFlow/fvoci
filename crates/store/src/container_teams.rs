//! Assigning workspace groups to boards, with authorization and audit in the transaction.

use fvoci_model::{
    AuditAction, Container, ContainerRole, ContainerTeam, ContainerVisibility, MemberRef,
    MinimumRole, WorkspaceRole,
};

use crate::audit::{self, AuditEntry};
use crate::db::{Id, Scope};
use crate::{
    Store, StoreError, container_team_access, containers, org_admin, teams, workspace_membership,
};

impl Store {
    /// Lists a board's assigned teams, including their current members.
    ///
    /// # Errors
    /// Returns the database error if the teams or their roles cannot be read.
    pub async fn container_teams(
        &self,
        workspace_id: &str,
        container_id: &str,
    ) -> Result<Vec<ContainerTeam>, StoreError> {
        self.with_conn(async |scope| {
            let groups = teams::list(scope, workspace_id).await?;
            let mut sql =
                scope.sql("SELECT team_id, role FROM container_teams WHERE container_id = ");
            sql.bind(Id(container_id))
                .push_bind(" AND workspace_id = ", Id(workspace_id))
                .push_bind(" AND org_id = ", scope.org()?);
            let rows = scope.fetch_all(sql).await?;
            let mut assigned = Vec::new();
            for team in groups {
                for row in &rows {
                    if row.id(0)? == team.id {
                        assigned.push(ContainerTeam {
                            team,
                            role: container_team_access::parse_role(&row.text(1)?)?,
                        });
                        break;
                    }
                }
            }
            Ok(assigned)
        })
        .await
    }

    /// Gives a team its board role, or removes its assignment with `None`.
    /// Direct roles are untouched; only a workspace admin or direct board admin acts.
    /// Returns the board with each of the team's people, or nothing when the team
    /// already had that role, or had none to take away.
    ///
    /// # Errors
    /// Returns not found for a missing or mismatched board/team or a revoked actor,
    /// or the database error. Audit records and the assignment commit together.
    pub async fn set_container_team(
        &self,
        workspace_id: &str,
        container_id: &str,
        team_id: &str,
        role: Option<MinimumRole>,
        by: &str,
    ) -> Result<Vec<MemberRef>, StoreError> {
        self.transaction(async |scope| {
            org_admin::hold_organization_row(scope).await?;
            if !workspace_membership::hold_workspace(scope, workspace_id).await? {
                return Err(StoreError::not_found("workspace"));
            }
            containers::hold_container(scope, container_id).await?;
            let container = containers::get_container(scope, container_id).await?;
            if container.workspace_id != workspace_id {
                return Err(StoreError::not_found("board"));
            }
            require_manager(scope, &container, by).await?;
            teams::require_team(scope, workspace_id, team_id).await?;
            // Giving the role it has, or taking away one it does not have, changes
            // nobody's access and is told to nobody.
            if assigned_role(scope, container_id, team_id).await? == role {
                return Ok(Vec::new());
            }
            let users = people(scope, team_id).await?;
            let changes: Vec<MemberRef> = users
                .iter()
                .map(|user_id| MemberRef::new(container_id, user_id))
                .collect();
            if let Some(role) = role {
                record_new_access(scope, &container, &users, by).await?;
                let mut sql = scope.sql(
                    "INSERT INTO container_teams (container_id, team_id, workspace_id, org_id, \
                     role) VALUES (",
                );
                sql.bind(Id(container_id))
                    .push_bind(", ", Id(team_id))
                    .push_bind(", ", Id(workspace_id))
                    .push_bind(", ", scope.org()?).push_bind(", ", role.as_str())
                    .push(") ON CONFLICT (container_id, team_id, org_id) DO UPDATE SET role = EXCLUDED.role");
                scope.execute(sql).await?;
            } else {
                let mut sql = scope.sql("DELETE FROM container_teams WHERE container_id = ");
                sql.bind(Id(container_id))
                    .push_bind(" AND team_id = ", Id(team_id))
                    .push_bind(" AND org_id = ", scope.org()?);
                scope.execute(sql).await?;
            }
            end_lost_subscriptions(scope, &changes, TemplateAccess::CurrentMembership).await?;
            Ok(changes)
        }).await
    }
}

async fn require_manager(
    scope: &mut Scope<'_>,
    container: &Container,
    by: &str,
) -> Result<(), StoreError> {
    let role = org_admin::membership(scope, &container.workspace_id, by)
        .await?
        .map(|(role, _)| role);
    let direct = containers::find_member(scope, &container.id, by).await?;
    if role == Some(WorkspaceRole::Admin)
        || (role == Some(WorkspaceRole::Member)
            && direct.is_some_and(|m| m.role == ContainerRole::Admin))
    {
        Ok(())
    } else {
        Err(StoreError::not_found("board"))
    }
}

/// The role the team `team_id` has on the board `container_id`; `None` if it has none.
async fn assigned_role(
    scope: &mut Scope<'_>,
    container_id: &str,
    team_id: &str,
) -> Result<Option<MinimumRole>, StoreError> {
    let mut sql = scope.sql("SELECT role FROM container_teams WHERE container_id = ");
    sql.bind(Id(container_id))
        .push_bind(" AND team_id = ", Id(team_id))
        .push_bind(" AND org_id = ", scope.org()?);
    scope
        .fetch_optional(sql)
        .await?
        .map(|row| container_team_access::parse_role(&row.text(0)?))
        .transpose()
}

pub(crate) async fn people(
    scope: &mut Scope<'_>,
    team_id: &str,
) -> Result<Vec<String>, StoreError> {
    let mut sql = scope.sql("SELECT tm.user_id FROM ");
    sql.push(&teams::present_members(scope))
        .push_bind(" tm WHERE tm.team_id = ", Id(team_id))
        .push_bind(" AND tm.org_id = ", scope.org()?)
        .push(" ORDER BY tm.user_id");
    scope.all(sql).await
}

async fn reads(
    scope: &mut Scope<'_>,
    container_id: &str,
    user_id: &str,
) -> Result<bool, StoreError> {
    let direct = containers::find_member(scope, container_id, user_id)
        .await?
        .map(|m| m.role);
    Ok(direct
        .max(container_team_access::role_for(scope, container_id, user_id).await?)
        .is_some())
}

/// Checked before the grant, while the organization/workspace/board are held. This
/// also covers an administrator joining a team already assigned to private boards.
pub(crate) async fn record_new_access(
    scope: &mut Scope<'_>,
    container: &Container,
    users: &[String],
    by: &str,
) -> Result<(), StoreError> {
    if container.visibility == ContainerVisibility::Open || reads(scope, &container.id, by).await? {
        return Ok(());
    }
    if org_admin::membership(scope, &container.workspace_id, by)
        .await?
        .map(|(role, _)| role)
        != Some(WorkspaceRole::Admin)
    {
        return Ok(());
    }
    for user_id in users {
        if reads(scope, &container.id, user_id).await? {
            continue;
        }
        let entry = if user_id == by {
            AuditEntry::new(
                &container.workspace_id,
                by,
                AuditAction::SelfAddedToContainer,
                &container.id,
            )
        } else {
            AuditEntry::new(
                &container.workspace_id,
                by,
                AuditAction::AddedToContainer,
                &container.id,
            )
            .with_subject(user_id)
        };
        audit::record_entry(scope, &entry).await?;
    }
    Ok(())
}

/// The people whose inherited roles can change, captured before a team is removed.
pub(crate) async fn affected(
    scope: &mut Scope<'_>,
    team_id: &str,
    only_user: Option<&str>,
) -> Result<Vec<MemberRef>, StoreError> {
    let mut sql = scope.sql("SELECT bt.container_id, tm.user_id FROM container_teams bt JOIN team_members tm ON tm.team_id = bt.team_id AND tm.org_id = bt.org_id WHERE bt.team_id = ");
    sql.bind(Id(team_id))
        .push_bind(" AND bt.org_id = ", scope.org()?);
    if let Some(user_id) = only_user {
        sql.push_bind(" AND tm.user_id = ", Id(user_id));
    }
    sql.push(" ORDER BY bt.container_id, tm.user_id");
    scope
        .fetch_all(sql)
        .await?
        .iter()
        .map(|row| Ok(MemberRef::new(row.id(0)?, row.id(1)?)))
        .collect()
}

/// Records the boards a newly added team member will read before adding the link.
pub(crate) async fn joining(
    scope: &mut Scope<'_>,
    team_id: &str,
    user_id: &str,
    by: &str,
) -> Result<Vec<MemberRef>, StoreError> {
    let mut sql = scope.sql("SELECT container_id FROM container_teams WHERE team_id = ");
    sql.bind(Id(team_id))
        .push_bind(" AND org_id = ", scope.org()?)
        .push(" ORDER BY container_id");
    let rows = scope.fetch_all(sql).await?;
    let mut changes = Vec::new();
    for row in rows {
        let container_id = row.id(0)?;
        containers::hold_container(scope, &container_id).await?;
        let container = containers::get_container(scope, &container_id).await?;
        record_new_access(scope, &container, &[user_id.to_owned()], by).await?;
        changes.push(MemberRef::new(container_id, user_id));
    }
    Ok(changes)
}

/// Whether workspace membership will remain after this transaction.
#[derive(Debug, Clone, Copy)]
pub(crate) enum TemplateAccess {
    CurrentMembership,
    Ending,
}

/// Removing one source preserves subscriptions while another reading permission remains.
pub(crate) async fn end_lost_subscriptions(
    scope: &mut Scope<'_>,
    changes: &[MemberRef],
    template_access: TemplateAccess,
) -> Result<(), StoreError> {
    for member in changes {
        if !reads(scope, &member.container_id, &member.user_id).await?
            && !reads_template(scope, member, template_access).await?
        {
            crate::subscriptions::end_subscriptions_to_container(
                scope,
                &member.container_id,
                &member.user_id,
            )
            .await?;
            crate::trash::forget_watcher_of_container(scope, &member.container_id, &member.user_id)
                .await?;
        }
    }
    Ok(())
}

async fn reads_template(
    scope: &mut Scope<'_>,
    member: &MemberRef,
    access: TemplateAccess,
) -> Result<bool, StoreError> {
    if matches!(access, TemplateAccess::Ending) {
        return Ok(false);
    }
    let container = containers::get_container(scope, &member.container_id).await?;
    let readable_open = container.visibility == ContainerVisibility::Open
        && (container.is_template
            || fvoci_model::ContainerKinds::OPEN_TO_READ.holds(container.kind));
    if !readable_open {
        return Ok(false);
    }
    Ok(
        org_admin::membership(scope, &container.workspace_id, &member.user_id)
            .await?
            .is_some_and(|(role, _)| role != WorkspaceRole::Guest),
    )
}
