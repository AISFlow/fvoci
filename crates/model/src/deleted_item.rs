//! What a list of the trash says about a thing in it.

use serde::Serialize;

use crate::ContainerKind;

/// A card or a board in the trash, with as much as it takes to decide whether to bring
/// it back (`restoreBlock`, `restoreContainer`).
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, utoipa::ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct DeletedItem {
    /// The identifier of the card or the board.
    pub id: String,
    /// Its title when it was deleted.
    pub title: String,
    /// When it was deleted, in milliseconds since the Unix epoch.
    pub deleted_at: i64,
    /// The identifier of who deleted it, who may no longer exist; empty if not known.
    pub deleted_by: String,
    /// For a board, its kind: the trash of boards lists boards and wikis together. Left
    /// out for a card.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub kind: Option<ContainerKind>,
}
