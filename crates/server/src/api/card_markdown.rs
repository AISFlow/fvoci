//! Markdown added after the last of a card's body, by its editors and their agents: the
//! way an agent writes into a body its people may be typing in.

use axum::body::Bytes;
use axum::extract::Path;
use serde::Deserialize;
use utoipa::ToSchema;
use utoipa_axum::router::OpenApiRouter;
use utoipa_axum::routes;

use super::cards::card_with_permission;
use super::contract::params::{CardId, MemberWorkspaceId};
use super::contract::tag;
use super::workspace::InWorkspace;
use super::{ApiResult, Delegated, Write, decode_object, json_response};
use fvoci_model::Card;

use crate::app::App;
use crate::error::AppError;
use crate::permissions::ContainerPermission;

pub(super) fn routes() -> OpenApiRouter<App> {
    OpenApiRouter::new().routes(routes!(handle_append_card_markdown))
}

/// Markdown to add after the last of a body.
#[derive(Debug, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct MarkdownAppend {
    /// The Markdown (`CommonMark` with the tables, strikethrough and task lists of
    /// GFM). A file of the card's board is named `fvoci-file://{fileId}`: an image as
    /// `![alt](fvoci-file://{fileId})`, any other file as `[name](fvoci-file://{fileId})`.
    pub(crate) markdown: String,
}

/// Add Markdown after the last of a card's body
///
/// For editors of the card's board. The Markdown is read into blocks and written into
/// the card's document after its last block, as one more editor writes: whoever edits
/// the card meanwhile keeps what they typed, sent or not, and is sent the blocks as
/// another editor's. The body is written to the card before the answer. 400 at
/// `/markdown` for Markdown that adds nothing or names a file the board does not have;
/// 413 (`document_too_large`) where the document would be larger than it may.
#[utoipa::path(
    post,
    path = "/w/{workspaceId}/cards/{cardId}/body/markdown",
    operation_id = "appendCardMarkdown",
    tag = tag::CARDS,
    params(MemberWorkspaceId, CardId),
    request_body = MarkdownAppend,
    responses((status = 200, description = "The card as it is now.", body = fvoci_model::Card)),
)]
async fn handle_append_card_markdown(
    Delegated(ws, ..): Delegated<Write, InWorkspace>,
    Path((_, card_id)): Path<(String, String)>,
    body: Bytes,
) -> ApiResult {
    let request: MarkdownAppend = decode_object(&body)?;
    let card = append_to_card(&ws, &card_id, &request.markdown).await?;
    Ok(json_response(&card))
}

/// Adds `markdown` after the last of the body of the card `card_id`, as
/// `appendCardMarkdown` does, and answers with the card as it is now.
///
/// # Errors
///
/// As `appendCardMarkdown` answers.
pub(super) async fn append_to_card(
    ws: &InWorkspace,
    card_id: &str,
    markdown: &str,
) -> Result<Card, AppError> {
    let card = card_with_permission(
        &ws.app,
        &ws.caller.user_id,
        card_id,
        ContainerPermission::ManageCards,
        "access denied to change card",
    )
    .await?;
    ws.app
        .append_markdown(&ws.caller, &card.id, &card.container_id, markdown)
        .await?;
    ws.app.card(&card.id).await
}
