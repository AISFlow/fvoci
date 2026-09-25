//! Shared SQL fragments for group-principal membership grants.

pub(crate) fn group_members_join_sql(members_alias: &str, group_members_alias: &str) -> String {
    format!(
        "INNER JOIN fvoci.group_members {group_members_alias}
            ON {group_members_alias}.workspace_id = {members_alias}.workspace_id
           AND {group_members_alias}.group_id = {members_alias}.group_id"
    )
}

/// `EXISTS`: the actor holds a group grant on the project row aliased by `project_alias`.
pub(crate) fn group_project_grant_exists_sql(project_alias: &str, actor_param: u32) -> String {
    let join = group_members_join_sql("pm", "gm");
    format!(
        "EXISTS (
            SELECT 1
            FROM fvoci.project_members pm
            {join}
            WHERE pm.workspace_id = {project_alias}.workspace_id
              AND pm.project_id = {project_alias}.id
              AND gm.user_id = ${actor_param}
              AND pm.group_id IS NOT NULL
        )"
    )
}

/// Roles granted to the actor through group principals on a project.
pub(crate) fn group_project_grant_roles_select_sql(
    workspace_param: u32,
    project_param: u32,
    user_param: u32,
) -> String {
    let join = group_members_join_sql("pm", "gm");
    format!(
        "SELECT pm.role
        FROM fvoci.project_members pm
        {join}
        WHERE pm.workspace_id = ${workspace_param}
          AND pm.project_id = ${project_param}
          AND gm.user_id = ${user_param}
          AND pm.group_id IS NOT NULL"
    )
}

/// Roles granted to the actor through group principals on a wiki document.
pub(crate) fn group_document_grant_roles_select_sql(
    workspace_param: u32,
    document_param: u32,
    user_param: u32,
) -> String {
    let join = group_members_join_sql("dm", "gm");
    format!(
        "SELECT dm.role
        FROM fvoci.document_members dm
        {join}
        WHERE dm.workspace_id = ${workspace_param}
          AND dm.document_id = ${document_param}
          AND gm.user_id = ${user_param}
          AND dm.group_id IS NOT NULL"
    )
}

/// Wiki document ids reachable by a guest through group grants.
pub(crate) fn guest_wiki_document_ids_select_sql(workspace_param: u32, user_param: u32) -> String {
    let join = group_members_join_sql("dm", "gm");
    format!(
        "SELECT DISTINCT dm.document_id
        FROM fvoci.document_members dm
        {join}
        INNER JOIN fvoci.documents d
            ON d.workspace_id = dm.workspace_id AND d.id = dm.document_id
        WHERE dm.workspace_id = ${workspace_param}
          AND gm.user_id = ${user_param}
          AND dm.group_id IS NOT NULL
          AND d.project_id IS NULL
          AND d.deleted_at IS NULL"
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn project_visibility_and_role_queries_share_group_join() {
        let join = group_members_join_sql("pm", "gm");
        let exists = group_project_grant_exists_sql("p", 3);
        let roles = group_project_grant_roles_select_sql(1, 2, 3);
        assert!(exists.contains(&join));
        assert!(roles.contains(&join));
    }

    #[test]
    fn document_and_guest_wiki_queries_share_group_join() {
        let join = group_members_join_sql("dm", "gm");
        let roles = group_document_grant_roles_select_sql(1, 2, 3);
        let wiki = guest_wiki_document_ids_select_sql(1, 2);
        assert!(roles.contains(&join));
        assert!(wiki.contains(&join));
    }
}
