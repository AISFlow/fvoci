//! A workspace in one answer, as an agent reads it first (`getWorkspaceOverview`): the
//! workspace, who reads it, and every container they read, with what they are on each
//! and the properties of a board's cards.

use std::collections::HashMap;

use fvoci_model::{
    ContainerKind, ContainerRole, OrgKind, OverviewContainer, OverviewPerson, OverviewWorkspace,
    WorkspaceOverview, WorkspaceRole,
};

use super::App;
use crate::error::AppError;

/// What the reader of an overview is in its workspace, as the workspace was entered.
#[derive(Debug, Clone, Copy)]
pub struct OverviewReader<'a> {
    /// The account of the reader.
    pub user_id: &'a str,
    /// What they are in the workspace.
    pub role: WorkspaceRole,
    /// What the workspace's organization is.
    pub kind: OrgKind,
}

impl App {
    /// The overview of the workspace `workspace_id` for `reader`, on the handle they
    /// entered it with: the containers of the kinds it reaches that they read
    /// ([`App::readable_workspace_containers`]: theirs, and the open ones unless they
    /// are a guest), oldest first. An administrator is not given those they only
    /// manage: an overview is of what is read.
    ///
    /// # Errors
    ///
    /// Returns 404 if the workspace is not there, and the store's error.
    pub async fn workspace_overview(
        &self,
        workspace_id: &str,
        reader: OverviewReader<'_>,
    ) -> Result<WorkspaceOverview, AppError> {
        let workspace = self.workspace(workspace_id).await?;
        let user = self.user(reader.user_id).await?;
        let containers = self
            .readable_workspace_containers(reader.user_id, workspace_id, reader.role)
            .await?;
        let roles: HashMap<String, ContainerRole> = self
            .store()
            .get_members_for_user(reader.user_id)
            .await?
            .into_iter()
            .map(|member| {
                let role = member.effective_role();
                (member.container_id, role)
            })
            .collect();
        let name = if user.display_name.is_empty() {
            user.username.clone()
        } else {
            user.display_name
        };
        Ok(WorkspaceOverview {
            workspace: OverviewWorkspace {
                id: workspace.id,
                org_id: self.store().tenant().org_id().unwrap_or_default(),
                name: workspace.title,
                kind: reader.kind,
                my_workspace_role: reader.role,
            },
            me: OverviewPerson {
                id: user.id,
                username: user.username,
                name,
            },
            containers: containers
                .into_iter()
                .map(|container| {
                    let my_role = roles.get(&container.id).copied();
                    OverviewContainer {
                        card_properties: (container.kind == ContainerKind::Board)
                            .then_some(container.card_properties),
                        id: container.id,
                        kind: container.kind,
                        title: container.title,
                        icon: container.icon,
                        visibility: container.visibility,
                        joined: my_role.is_some(),
                        my_role,
                        updated_at: container.updated_at,
                    }
                })
                .collect(),
        })
    }
}
