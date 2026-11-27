//! Who is let into the room of a card's body, and how: asked when a connection
//! comes in, and again while it is there. A page of a wiki has a room as a card of a
//! board has (`BlockType::has_room`): what is said of a card and its board here is
//! said of a page and its wiki too.

use crate::app::{App, Asker};
use crate::error::AppError;
use crate::permissions::{DocMode, doc_mode};

/// A user let into the room of a card.
#[derive(Debug, Clone)]
pub(super) struct Admitted {
    /// The application as the card's organization reaches it, in the workspace the
    /// connection named.
    pub(super) app: App,
    /// The board the card is on, of that workspace.
    pub(super) container_id: String,
    /// How the user is in the room.
    pub(super) mode: DocMode,
}

/// Lets the user into the room of the card `card_id` of a board of the workspace
/// `workspace_id`, as what they may do on that board says ([`doc_mode`]); `None` for
/// who may not read the board, for a card that is not there, or not on a board of
/// that workspace, and for a connection without a user. `home` is the application as
/// the connection's handshake reached it.
///
/// A workspace is entered as a request enters it (`App::enter_workspace`): one whose
/// organization takes only sessions made through its provider lets the connection of
/// another session into no room, as it lets it follow none of its workspaces. Asked
/// again while the connection is there, it is put out once the organization takes
/// none.
///
/// Whoever is in the workspace is asked the same of the store whether the card is
/// there or not ([`App::container_access`]), and of the card only which board it is on
/// (`Store::board_of_card`), never its body: a card that is not there for the user
/// takes as long to refuse as one that does not exist, however large its body is.
///
/// # Errors
///
/// Returns the store's error if a fact cannot be read: that is no "no".
pub(super) async fn admit(
    home: &App,
    asker: Asker<'_>,
    workspace_id: &str,
    card_id: &str,
) -> Result<Option<Admitted>, AppError> {
    let user_id = asker.user_id;
    if user_id.is_empty() {
        return Ok(None);
    }
    let Some(app) = entered(home, asker, workspace_id).await? else {
        return Ok(None);
    };
    // The card's board and nothing else of the card: its body is read once the user
    // is let in, so that a refusal costs the same whatever the body is.
    let container_id = app.store().board_of_card(card_id).await.map_err(|error| {
        AppError::from(error).context("reading a card's board to enter its room")
    })?;
    let mode = mode_on(&app, user_id, container_id.as_deref().unwrap_or_default()).await?;
    Ok(container_id.zip(mode).map(|(container_id, mode)| Admitted {
        app,
        container_id,
        mode,
    }))
}

/// How a connection of `asker` is let into the room of a card ([`admit`]), refused as
/// for a card that is not there.
///
/// # Errors
///
/// As [`admit`], and a not-found error for who is not let in.
pub(super) async fn let_in(
    home: &App,
    asker: Asker<'_>,
    workspace_id: &str,
    card_id: &str,
) -> Result<Admitted, AppError> {
    admit(home, asker, workspace_id, card_id)
        .await?
        .ok_or_else(|| AppError::not_found("card"))
}

/// How `asker` is let into the rooms of the cards of the board `container_id` of the
/// workspace `workspace_id`, as [`admit`] lets them in, without reading which board a
/// card is on: who asks again of a room knows its board.
///
/// # Errors
///
/// As [`admit`].
pub(super) async fn container_mode(
    home: &App,
    asker: Asker<'_>,
    workspace_id: &str,
    container_id: &str,
) -> Result<Option<DocMode>, AppError> {
    match entered(home, asker, workspace_id).await? {
        Some(app) => mode_on(&app, asker.user_id, container_id).await,
        None => Ok(None),
    }
}

/// The application in the workspace `workspace_id` as `asker` enters it from `home`;
/// `None` if they are not let in.
async fn entered(
    home: &App,
    asker: Asker<'_>,
    workspace_id: &str,
) -> Result<Option<App>, AppError> {
    Ok(home
        .enter_workspace_quietly(asker, workspace_id)
        .await?
        .map(|entered| entered.app.in_workspace(workspace_id)))
}

/// How `user_id` is in the rooms of the board `container_id` as `app` reaches it: as what
/// they may do on it says; `None` for who may not read it. A template the server ships
/// with is every workspace's to read and nobody's to edit: it has no room.
async fn mode_on(
    app: &App,
    user_id: &str,
    container_id: &str,
) -> Result<Option<DocMode>, AppError> {
    Ok(app
        .container_access(user_id, container_id)
        .await?
        .filter(|access| !access.container.is_builtin)
        .and_then(|access| doc_mode(access.grant)))
}
