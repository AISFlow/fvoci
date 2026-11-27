//! Keep board assignments in trash without keeping a snapshot of a team's people.

use fvoci_model::{Container, MinimumRole};

use crate::db::{Id, Param, Scope};
use crate::{StoreError, container_team_access, container_teams, trash};

#[derive(Debug, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
struct StoredRole {
    team_id: String,
    role: MinimumRole,
}

pub(crate) async fn keep(scope: &mut Scope<'_>, container_id: &str) -> Result<(), StoreError> {
    let mut sql = scope.sql("SELECT team_id, role FROM container_teams WHERE container_id = ");
    sql.bind(Id(container_id))
        .push_bind(" AND org_id = ", scope.org()?)
        .push(" ORDER BY team_id");
    let roles = scope
        .fetch_all(sql)
        .await?
        .iter()
        .map(|row| {
            Ok(StoredRole {
                team_id: row.id(0)?,
                role: container_team_access::parse_role(&row.text(1)?)?,
            })
        })
        .collect::<Result<Vec<_>, StoreError>>()?;
    let mut sql = scope.sql("UPDATE deleted_containers SET team_roles = ");
    sql.bind(Param::json(&roles)?)
        .push_bind(" WHERE id = ", Id(container_id));
    scope.execute(sql).await?;
    Ok(())
}

/// Gives the surviving teams their roles again, after the board's people have theirs,
/// and with the people the teams have now. Restoring is a grant like an assignment:
/// when `by` restores the board as a manager who does not read it, whoever comes to read
/// it only through its teams is put on the record, the manager too.
pub(crate) async fn restore(
    scope: &mut Scope<'_>,
    container: &Container,
    by: &str,
) -> Result<(), StoreError> {
    let mut sql = scope.sql("SELECT team_roles FROM deleted_containers WHERE id = ");
    sql.bind(Id(&container.id));
    let row = scope
        .fetch_optional(sql)
        .await?
        .ok_or_else(|| StoreError::not_found("board"))?;
    let roles: Vec<StoredRole> = trash::from_json(&row, 0, "team_roles")?;
    // The organization lock covers team deletion and membership changes too. Everyone
    // the teams bring is looked at before any of their roles is back, so what is
    // recorded does not depend on the order of the teams.
    let mut people = Vec::new();
    for stored in &roles {
        people.extend(container_teams::people(scope, &stored.team_id).await?);
    }
    people.sort_unstable();
    people.dedup();
    container_teams::record_new_access(scope, container, &people, by).await?;
    for stored in roles {
        // Only a surviving team in this workspace can regain the assignment.
        let mut sql = scope
            .sql("INSERT INTO container_teams (container_id, team_id, workspace_id, org_id, role) SELECT ");
        sql.bind(Id(&container.id))
            .push(", id, workspace_id, org_id, ")
            .bind(stored.role.as_str())
            .push_bind(" FROM teams WHERE id = ", Id(&stored.team_id))
            .push_bind(" AND workspace_id = ", Id(&container.workspace_id))
            .push_bind(" AND org_id = ", scope.org()?);
        scope.execute(sql).await?;
    }
    Ok(())
}
