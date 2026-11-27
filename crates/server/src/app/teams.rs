//! Workspace teams. Authorization is checked at the route and again by writes under
//! the workspace lock; names are trimmed here before they reach storage.

use std::collections::BTreeSet;

use super::App;
use crate::collab::AccessScope;
use crate::error::{AppError, Reason};
use fvoci_model::{ContainerMember, MemberRef, Team, is_id, team_name};
use fvoci_store::{StoreError, Taken};

/// A store's refusal of a team name another team of the workspace has, as the problem
/// `team_name_taken` about `/name`.
fn name_taken(error: StoreError) -> AppError {
    match error {
        StoreError::Conflict(Taken::TeamName) => {
            AppError::conflict("another team of the workspace has the name in some letter case")
                .with_reason(Reason::TeamNameTaken)
                .with_field("name")
        }
        other => AppError::from(other),
    }
}

impl App {
    /// Assigned groups of the board, in workspace name order.
    ///
    /// # Errors
    /// Returns the store error if the list cannot be read.
    pub async fn container_teams(
        &self,
        workspace_id: &str,
        container_id: &str,
    ) -> Result<Vec<fvoci_model::ContainerTeam>, AppError> {
        Ok(self
            .store()
            .container_teams(workspace_id, container_id)
            .await?)
    }

    /// Current people on the board, with direct and inherited roles kept separate.
    ///
    /// # Errors
    /// Returns the store error if either membership source cannot be read.
    pub async fn effective_container_members(
        &self,
        container_id: &str,
    ) -> Result<Vec<ContainerMember>, AppError> {
        Ok(self
            .store()
            .effective_container_members(container_id)
            .await?)
    }

    /// Lists teams, including their nonguest members.
    ///
    /// # Errors
    /// Returns the store error if the list cannot be read.
    pub async fn teams(&self, workspace_id: &str) -> Result<Vec<Team>, AppError> {
        Ok(self.store().teams(workspace_id).await?)
    }

    /// Reads a team within its workspace.
    ///
    /// # Errors
    /// Returns 404 if the team is absent or belongs to a different workspace.
    pub async fn team(&self, workspace_id: &str, team_id: &str) -> Result<Team, AppError> {
        if !is_id(team_id) {
            return Err(AppError::not_found("team"));
        }
        self.teams(workspace_id)
            .await?
            .into_iter()
            .find(|team| team.id == team_id)
            .ok_or_else(|| AppError::not_found("team"))
    }

    /// Creates an empty team for the workspace's administrator.
    ///
    /// # Errors
    /// Returns 400 for a name [`fvoci_model::team_name`] refuses, 409 with the reason
    /// `team_name_taken` for the name of another team in any letter case, or the store
    /// error.
    pub async fn create_team(
        &self,
        workspace_id: &str,
        text: &str,
        by: &str,
    ) -> Result<Team, AppError> {
        let id = self
            .store()
            .create_team(workspace_id, &team_name(text)?, by)
            .await
            .map_err(name_taken)?;
        self.team(workspace_id, &id).await
    }

    /// Renames a team for the workspace's administrator.
    ///
    /// # Errors
    /// As [`App::create_team`], and 404 if the team is absent.
    pub async fn rename_team(
        &self,
        workspace_id: &str,
        team_id: &str,
        text: &str,
        by: &str,
    ) -> Result<Team, AppError> {
        self.store()
            .rename_team(workspace_id, team_id, &team_name(text)?, by)
            .await
            .map_err(name_taken)?;
        self.team(workspace_id, team_id).await
    }

    /// Deletes a team and all its board roles.
    ///
    /// # Errors
    /// Returns 404 for an absent team or the store error.
    pub async fn delete_team(
        &self,
        workspace_id: &str,
        team_id: &str,
        by: &str,
    ) -> Result<(), AppError> {
        self.collab()
            .narrowing_access(self, &AccessScope::workspace(workspace_id));
        let changes = self.store().delete_team(workspace_id, team_id, by).await?;
        self.tell_team_changes(workspace_id, changes).await;
        Ok(())
    }

    /// Adds a nonguest workspace member to a team.
    ///
    /// # Errors
    /// Returns 404 for an absent team or a person who is no nonguest workspace member.
    pub async fn add_team_member(
        &self,
        workspace_id: &str,
        team_id: &str,
        user_id: &str,
        by: &str,
    ) -> Result<(), AppError> {
        if !is_id(user_id) {
            return Err(AppError::not_found("workspace member"));
        }
        let changes = self
            .store()
            .add_team_member(workspace_id, team_id, user_id, by)
            .await?;
        self.tell_team_changes(workspace_id, changes).await;
        Ok(())
    }

    /// Removes a person from a team.
    ///
    /// # Errors
    /// Returns 404 for an absent team or the store error.
    pub async fn remove_team_member(
        &self,
        workspace_id: &str,
        team_id: &str,
        user_id: &str,
        by: &str,
    ) -> Result<(), AppError> {
        if !is_id(user_id) {
            return Err(AppError::not_found("workspace member"));
        }
        self.collab()
            .narrowing_access(self, &AccessScope::workspace(workspace_id).of_user(user_id));
        let changes = self
            .store()
            .remove_team_member(workspace_id, team_id, user_id, by)
            .await?;
        self.tell_team_changes(workspace_id, changes).await;
        Ok(())
    }
}

impl App {
    /// Gives or removes a board's team role and asks affected clients to read again.
    ///
    /// # Errors
    /// Returns the store's refusal or database error.
    pub async fn set_container_team(
        &self,
        workspace_id: &str,
        container_id: &str,
        team_id: &str,
        role: Option<fvoci_model::MinimumRole>,
        by: &str,
    ) -> Result<(), AppError> {
        self.collab()
            .narrowing_access(self, &AccessScope::container(container_id));
        let changes = self
            .store()
            .set_container_team(workspace_id, container_id, team_id, role, by)
            .await?;
        self.tell_team_changes(workspace_id, changes).await;
        Ok(())
    }

    /// Rechecks the card rooms of the boards whose team roles changed, for the people
    /// those roles reach, before the change is answered, then asks their clients to
    /// read the boards' members again, in the background.
    async fn tell_team_changes(&self, workspace_id: &str, changes: Vec<MemberRef>) {
        if changes.is_empty() {
            return;
        }
        let containers: BTreeSet<&String> =
            changes.iter().map(|change| &change.container_id).collect();
        let people: BTreeSet<&str> = changes
            .iter()
            .map(|change| change.user_id.as_str())
            .collect();
        // Every change of a team is the team's people on the team's boards, so the
        // boards and people together name no pair the change did not reach.
        self.collab()
            .recheck_access(self, AccessScope::containers(containers).of_users(people))
            .await;
        let (app, workspace_id) = (self.clone(), workspace_id.to_owned());
        self.spawn_background(async move {
            for member in changes {
                app.ws()
                    .broadcast_member_delete(&workspace_id, &member.container_id, &member.user_id)
                    .await;
            }
        });
    }
}
