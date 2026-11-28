//! The message `reactionsChanged`: the reactions to a block, all of them as they are now,
//! told to the connections that are told the block's `blockChanged` as its board's
//! members (`contracts/websocket.md` 3). A reader by a public link is told nothing of
//! them, as `getSharedBlocks` gives no user identifier.

use fvoci_model::Reaction;
use serde::Serialize;

use super::delivery::{Access, Readers};
use super::{Hub, Outgoing};

/// `reactionsChanged`: the block, and its reactions as they are now; an empty list when
/// the last was taken off.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct ReactionsChangedMessage<'a> {
    #[serde(rename = "type")]
    kind: &'static str,
    workspace_id: &'a str,
    container_id: &'a str,
    block_id: &'a str,
    reactions: &'a [Reaction],
}

impl Outgoing for ReactionsChangedMessage<'_> {
    fn kind(&self) -> &'static str {
        self.kind
    }
}

impl Hub {
    /// Tells those who read the container `container_id` as its members, of the
    /// connections subscribed to the workspace, what the reactions to its block
    /// `block_id` are now.
    pub async fn broadcast_reactions_change(
        &self,
        workspace_id: &str,
        container_id: &str,
        block_id: &str,
        reactions: &[Reaction],
    ) {
        let recipients = self.workspace_listeners(workspace_id);
        if recipients.is_empty() {
            return;
        }
        let message = ReactionsChangedMessage {
            kind: "reactionsChanged",
            workspace_id,
            container_id,
            block_id,
            reactions,
        };
        self.send(
            &recipients,
            &message,
            Access::Members {
                workspace: workspace_id,
                readers: Readers::Now(container_id),
                also: None,
            },
        )
        .await;
    }
}
