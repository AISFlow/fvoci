//! Effective board memberships, without turning inherited roles into stored direct roles.

use fvoci_model::{Container, ContainerMember, ContainerRole, MinimumRole};

use crate::db::{Id, Scope, Sql, SqlFragment};
use crate::{Store, StoreError, containers};

/// All container-level facts needed to resolve container access (`containers` row,
/// direct `container_members` row, and highest inherited `container_teams` role), read
/// in a single roundtrip on one database connection.
#[derive(Debug, Clone, PartialEq)]
pub struct ContainerAccessFacts {
    /// The container, if it exists in this handle's reach.
    pub container: Option<Container>,
    /// The user's direct membership on the container, if any.
    pub member: Option<ContainerMember>,
    /// The highest inherited team role for a nonguest workspace member on the container.
    pub team_role: Option<ContainerRole>,
}

impl Store {
    /// Reads the container, the caller's direct membership, and the caller's highest
    /// active team role in one query on a single connection. Always probes all three
    /// tables whether or not the container exists so missing and unauthorized containers
    /// follow the same read path.
    ///
    /// # Errors
    ///
    /// Returns [`StoreError::NoOrganization`] for an unscoped handle, [`StoreError::Corrupt`]
    /// if a stored enum or JSON column cannot be decoded, or [`StoreError::Database`] if
    /// the query fails.
    pub async fn container_access_facts(
        &self,
        container_id: &str,
        user_id: &str,
    ) -> Result<ContainerAccessFacts, StoreError> {
        self.with_conn(async |scope| {
            let org = scope.org()?;
            let mut sql = scope.sql([
                "SELECT b.id IS NOT NULL, ",
                containers::CONTAINER_SELECT_B,
                ", bm.role, (\
                 SELECT CAST(MAX(CASE bt.role \
                   WHEN 'editor' THEN 3 \
                   WHEN 'commenter' THEN 2 \
                   WHEN 'viewer' THEN 1 \
                   ELSE 99 END) AS BIGINT) \
                 FROM container_teams bt JOIN ",
            ]);
            sql.push(&crate::teams::present_members(scope))
                .push(
                    " tm ON tm.team_id = bt.team_id \
                     AND tm.workspace_id = bt.workspace_id \
                     AND tm.org_id = bt.org_id \
                     WHERE bt.org_id = ",
                )
                .bind(org)
                .push_bind(" AND bt.container_id = ", Id(container_id))
                .push_bind(" AND tm.user_id = ", Id(user_id))
                .push(") FROM (SELECT 1) AS probe LEFT JOIN containers AS b ON b.id = ")
                .bind(Id(container_id))
                .push_bind(
                    " LEFT JOIN container_members AS bm ON bm.container_id = ",
                    Id(container_id),
                )
                .push_bind(" AND bm.user_id = ", Id(user_id));
            let Some(row) = scope.fetch_optional(sql).await? else {
                return Ok(ContainerAccessFacts {
                    container: None,
                    member: None,
                    team_role: None,
                });
            };
            let container = if row.bool(0)? {
                Some(containers::container_from_row_at(&row, 1)?)
            } else {
                None
            };
            let member_col = 1 + containers::CONTAINER_COLUMN_COUNT;
            let member = match row.opt_text(member_col)? {
                Some(raw_role) => Some(ContainerMember {
                    container_id: container_id.to_owned(),
                    user_id: user_id.to_owned(),
                    role: raw_role.parse().map_err(|_| StoreError::Corrupt {
                        column: "role",
                        reason: format!("unknown member role {raw_role:?}"),
                    })?,
                    synthetic: false,
                    inherited_role: None,
                }),
                None => None,
            };
            let team_role = match row.opt_int(member_col + 1)? {
                None => None,
                Some(3) => Some(ContainerRole::Editor),
                Some(2) => Some(ContainerRole::Commenter),
                Some(1) => Some(ContainerRole::Viewer),
                Some(other) => {
                    return Err(StoreError::Corrupt {
                        column: "role",
                        reason: format!("invalid team board role rank {other}"),
                    });
                }
            };
            Ok(ContainerAccessFacts {
                container,
                member,
                team_role,
            })
        })
        .await
    }
    /// The highest team role of a current nonguest member on this board.
    ///
    /// # Errors
    /// Returns the database error, including an unreadable stored role.
    #[cfg(any(test, feature = "test-util"))]
    pub async fn container_team_role(
        &self,
        container_id: &str,
        user_id: &str,
    ) -> Result<Option<ContainerRole>, StoreError> {
        self.with_conn(async |scope| role_for(scope, container_id, user_id).await)
            .await
    }

    /// Current board members, with inherited roles separate from their direct `role`:
    /// the direct members who are in the board's workspace now and the people of its
    /// teams, as `Store::get_present_members_for_container` lists the direct ones, of a
    /// workspace and an organization that are not deleted.
    ///
    /// # Errors
    /// Returns the database error if either membership source cannot be read.
    pub async fn effective_container_members(
        &self,
        container_id: &str,
    ) -> Result<Vec<ContainerMember>, StoreError> {
        self.with_conn(async |scope| {
            let mut sql = containers::select_members(scope);
            sql.push(
                " JOIN containers b ON b.id = bm.container_id JOIN workspace_members wm \
                 ON wm.workspace_id = b.workspace_id AND wm.user_id = bm.user_id \
                 JOIN workspaces w ON w.id = b.workspace_id \
                 JOIN organizations o ON o.id = w.org_id",
            )
            .push_bind(" WHERE bm.container_id = ", Id(container_id))
            .push(" AND w.deleted_at IS NULL AND o.deleted_at IS NULL")
            .push(&scope.same_organization(&[("bm", "b"), ("wm", "b"), ("w", "b")]));
            let mut members = containers::fetch_members(scope, sql).await?;
            let mut sql = inherited(scope)?;
            sql.push_bind(" AND bt.container_id = ", Id(container_id))
                .push(
                    " AND EXISTS (SELECT 1 FROM organizations o \
                 WHERE o.id = bt.org_id AND o.deleted_at IS NULL)",
                );
            extend(scope, sql, &mut members).await?;
            members.sort_by(|a, b| a.user_id.cmp(&b.user_id));
            Ok(members)
        })
        .await
    }
}

pub(crate) async fn role_for(
    scope: &mut Scope<'_>,
    container_id: &str,
    user_id: &str,
) -> Result<Option<ContainerRole>, StoreError> {
    let mut sql = inherited(scope)?;
    sql.push_bind(" AND bt.container_id = ", Id(container_id))
        .push_bind(" AND tm.user_id = ", Id(user_id));
    let mut highest = None;
    for row in scope.fetch_all(sql).await? {
        highest = highest.max(Some(ContainerRole::from(parse_role(&row.text(2)?)?)));
    }
    Ok(highest)
}

pub(crate) fn parse_role(value: &str) -> Result<MinimumRole, StoreError> {
    MinimumRole::ALL
        .into_iter()
        .find(|role| role.as_str() == value)
        .ok_or_else(|| StoreError::Corrupt {
            column: "role",
            reason: format!("invalid team board role: {value}"),
        })
}

fn inherited(scope: &Scope<'_>) -> Result<Sql, StoreError> {
    let mut sql =
        scope.sql("SELECT bt.container_id, tm.user_id, bt.role FROM container_teams bt JOIN ");
    sql.push(&crate::teams::present_members(scope))
        .push(" tm ON tm.team_id = bt.team_id AND tm.workspace_id = bt.workspace_id AND tm.org_id = bt.org_id WHERE true")
        .push_bind(" AND bt.org_id = ", scope.org()?);
    Ok(sql)
}

async fn extend_for_user(
    scope: &mut Scope<'_>,
    user_id: &str,
    members: &mut Vec<ContainerMember>,
) -> Result<(), StoreError> {
    let mut sql = inherited(scope)?;
    sql.push_bind(" AND tm.user_id = ", Id(user_id));
    extend(scope, sql, members).await?;
    members.sort_by(|a, b| a.container_id.cmp(&b.container_id));
    Ok(())
}

pub(crate) async fn extend(
    scope: &mut Scope<'_>,
    sql: Sql,
    members: &mut Vec<ContainerMember>,
) -> Result<(), StoreError> {
    for row in scope.fetch_all(sql).await? {
        let container_id = row.id(0)?;
        let user_id = row.id(1)?;
        let role = parse_role(&row.text(2)?)?;
        if let Some(member) = members
            .iter_mut()
            .find(|m| m.container_id == container_id && m.user_id == user_id)
        {
            if member
                .inherited_role
                .is_none_or(|old| ContainerRole::from(old) < ContainerRole::from(role))
            {
                member.inherited_role = Some(role);
                if member.synthetic {
                    member.role = ContainerRole::from(role);
                }
            }
        } else {
            members.push(ContainerMember {
                container_id,
                user_id,
                role: ContainerRole::from(role),
                inherited_role: Some(role),
                synthetic: true,
            });
        }
    }
    Ok(())
}

/// A role from a team on board `b`, for the named user expression. This is also used
/// by the system directory, so the organization is bound through the board's workspace.
pub(crate) fn push_has_team_role(sql: &mut Sql, scope: &Scope<'_>, user_id: &str) {
    sql.push("EXISTS (SELECT 1 FROM container_teams bt JOIN ")
        .push(&crate::teams::present_members(scope))
        .push(
            " tm ON tm.team_id = bt.team_id AND tm.org_id = bt.org_id \
             AND tm.workspace_id = bt.workspace_id \
             WHERE bt.container_id = b.id AND bt.workspace_id = b.workspace_id \
             AND tm.user_id = ",
        )
        .bind(Id(user_id))
        .push(&scope.same_organization(&[("bt", "b")]))
        .push(")");
}

/// Directory membership rows `(container_id, user_id, org_id)`, direct and inherited.
pub(crate) fn directory_members(scope: &Scope<'_>) -> SqlFragment {
    let mut frag = SqlFragment::of(&[
        "(SELECT dm.container_id, dm.user_id, dw.org_id FROM container_members dm \
         JOIN containers db ON db.id = dm.container_id \
         JOIN workspaces dw ON dw.id = db.workspace_id WHERE true",
    ]);
    frag.push_fragment(&scope.same_organization(&[("dm", "db"), ("db", "dw")]))
        .push(" UNION ALL SELECT bt.container_id, tm.user_id, bt.org_id FROM container_teams bt JOIN ")
        .push_fragment(&crate::teams::present_members(scope))
        .push(" tm ON tm.team_id = bt.team_id AND tm.org_id = bt.org_id AND tm.workspace_id = bt.workspace_id)");
    frag
}

/// Active inherited memberships `(container_id, user_id, org_id)` for containers in live
/// workspaces and organizations.
pub(crate) fn inherited_members_source(scope: &Scope<'_>) -> SqlFragment {
    let mut frag = SqlFragment::of(&[
        "(SELECT bt.container_id, tm.user_id, bt.org_id FROM container_teams AS bt JOIN ",
    ]);
    frag.push_fragment(&crate::teams::present_members(scope))
        .push(
            " AS tm ON tm.team_id = bt.team_id \
             AND tm.workspace_id = bt.workspace_id \
             AND tm.org_id = bt.org_id \
             JOIN organizations AS o ON o.id = bt.org_id \
             WHERE o.deleted_at IS NULL)",
        );
    frag
}

pub(crate) async fn get_members_for_user(
    scope: &mut Scope<'_>,
    user_id: &str,
) -> Result<Vec<ContainerMember>, StoreError> {
    let mut sql = containers::select_members(scope);
    sql.push(" JOIN containers AS b ON b.id = bm.container_id WHERE ");
    crate::members::push_own_membership_in_workspace(&mut sql, user_id)
        .push(" ORDER BY bm.container_id");
    let mut members = containers::fetch_members(scope, sql).await?;
    extend_for_user(scope, user_id, &mut members).await?;
    Ok(members)
}
