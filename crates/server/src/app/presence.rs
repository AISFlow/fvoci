//! The authority behind a board visit, and the people its reader may identify.

use fvoci_model::WorkspaceRole;

use super::{App, Asker};
use crate::collab::Wall;
use crate::error::AppError;

/// A visit checked behind its organization's wall. A token reader has no named role.
#[derive(Debug)]
pub(crate) struct PresenceAccess {
    pub(crate) app: App,
    pub(crate) workspace_id: String,
    pub(crate) role: Option<WorkspaceRole>,
    /// The board's organization, as its access changes are told.
    pub(crate) wall: Wall,
    /// The card asked for if it is one of the board's, not deleted; else empty: the
    /// visit is then on the board with no card.
    pub(crate) card_id: String,
}

impl App {
    /// Checks a visit with the same board authority as HTTP. A supplied read token
    /// always means an anonymous visit, even on a signed-in connection.
    pub(crate) async fn presence_access(
        &self,
        asker: Option<Asker<'_>>,
        workspace_id: &str,
        container_id: &str,
        card_id: &str,
        read_token: &str,
    ) -> Result<Option<PresenceAccess>, AppError> {
        let (app, role, user_id) = if read_token.is_empty() {
            let Some(asker) = asker else {
                return Ok(None);
            };
            let Some(entered) = self.enter_workspace_quietly(asker, workspace_id).await? else {
                return Ok(None);
            };
            (
                entered.app.in_workspace(workspace_id),
                Some(entered.role),
                asker.user_id,
            )
        } else {
            let Some(app) = self
                .enter_shared_container(container_id, read_token)
                .await?
            else {
                return Ok(None);
            };
            (app, None, "")
        };
        let Some(access) = app.container_access(user_id, container_id).await? else {
            return Ok(None);
        };
        if access.container.deleted_at.is_some()
            || (role.is_some() && !access.grant.reads())
            || (!workspace_id.is_empty() && workspace_id != access.container.workspace_id)
        {
            return Ok(None);
        }
        let card_id = if card_id.is_empty() || !is_open_card(&app, container_id, card_id).await? {
            String::new()
        } else {
            card_id.to_owned()
        };
        Ok(Some(PresenceAccess {
            wall: Wall::of(&app),
            app,
            workspace_id: access.container.workspace_id,
            role,
            card_id,
        }))
    }
}

/// Whether `card_id` is a card of the board `container_id`, or a page of the wiki of that
/// identifier, not deleted: a block whose body has a room, on the board the visit was
/// let onto. Its body is not read.
async fn is_open_card(app: &App, container_id: &str, card_id: &str) -> Result<bool, AppError> {
    let on = app.store().board_of_card(card_id).await?;
    Ok(on.as_deref() == Some(container_id))
}
