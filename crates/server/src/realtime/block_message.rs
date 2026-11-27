//! The message `blockChanged`: a block that changed. A card's, a page's or an entry's
//! carries a summary of its body in place of the body, which its room and HTTP give
//! (`contracts/websocket.md` 3); every other block goes whole.

use std::borrow::Cow;

use fvoci_model::{Block, Body, BodySummary, CardPreview};
use serde::Serialize;

use super::Outgoing;

/// What `blockChanged` says of a card's, a page's or an entry's body in its place, as a
/// list of cards says it (`getCards`, and its `preview`).
#[derive(Debug, Clone, Default, Serialize)]
#[serde(rename_all = "camelCase")]
struct Summary {
    has_body: bool,
    tasks_total: u32,
    tasks_done: u32,
    preview: CardPreview,
}

impl Summary {
    fn of(body: Option<&Body>) -> Self {
        let Some(body) = body else {
            return Self::default();
        };
        let BodySummary { has_body, tasks } = body.summary();
        Self {
            has_body,
            tasks_total: tasks.total,
            tasks_done: tasks.done,
            preview: CardPreview::of(body),
        }
    }
}

/// `blockChanged`: the block, without its body and with its summary if it is a card, a
/// page or an entry.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct UpdateBlockMessage<'a> {
    #[serde(rename = "type")]
    kind: &'static str,
    workspace_id: &'a str,
    block: Cow<'a, Block>,
    #[serde(skip_serializing_if = "Option::is_none")]
    summary: Option<Summary>,
}

impl<'a> UpdateBlockMessage<'a> {
    pub(super) fn new(workspace_id: &'a str, block: &'a Block) -> Self {
        let (block, summary) = if block.deleted_at.is_none() && block.block_type.has_room() {
            let summary = Summary::of(block.body.as_ref());
            (Cow::Owned(without_body(block)), Some(summary))
        } else if !block.takes_number() && block.number.is_some() {
            let mut hidden = block.clone();
            hidden.hide_inactive_number();
            (Cow::Owned(hidden), None)
        } else {
            (Cow::Borrowed(block), None)
        };
        Self {
            kind: "blockChanged",
            workspace_id,
            block,
            summary,
        }
    }

    pub(super) fn workspace_id(&self) -> &'a str {
        self.workspace_id
    }

    /// The block the message carries: a card's, a page's or an entry's without its body.
    pub(super) fn block(&self) -> &Block {
        &self.block
    }

    /// The block the message carries, to change for whoever is told less of it. The
    /// summary names nobody: its preview's text has nothing of a mention or a reference.
    pub(super) fn block_mut(&mut self) -> &mut Block {
        self.block.to_mut()
    }
}

impl Outgoing for UpdateBlockMessage<'_> {
    fn kind(&self) -> &'static str {
        self.kind
    }
}

/// `block` without its body, which is not copied.
fn without_body(block: &Block) -> Block {
    let Block {
        id,
        parent_id,
        created_by,
        modified_by,
        block_type,
        title,
        fields,
        body: _,
        created_at,
        updated_at,
        deleted_at,
        container_id,
        edited_at,
        number: _,
    } = block;
    Block {
        id: id.clone(),
        parent_id: parent_id.clone(),
        created_by: created_by.clone(),
        modified_by: modified_by.clone(),
        block_type: *block_type,
        title: title.clone(),
        fields: fields.clone(),
        body: None,
        created_at: *created_at,
        updated_at: *updated_at,
        deleted_at: *deleted_at,
        container_id: container_id.clone(),
        edited_at: *edited_at,
        number: block.shown_number(),
    }
}
