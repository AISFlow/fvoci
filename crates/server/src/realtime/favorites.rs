//! The stars of a person (`favoritesChanged`): each of their connections that follows
//! the workspace is told that their stars there changed, put on, taken off or put in
//! another order, in whichever tab. The message says nothing of what changed: the
//! client reads the stars again, which asks of each whether the session reads what it
//! is on.

use serde::Serialize;

use super::{Hub, Outgoing};

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct FavoritesChangedMessage<'a> {
    #[serde(rename = "type")]
    kind: &'static str,
    workspace_id: &'a str,
}

impl Outgoing for FavoritesChangedMessage<'_> {
    fn kind(&self) -> &'static str {
        self.kind
    }
}

impl Hub {
    /// Tells the connections of `user_id` that follow the workspace that their stars
    /// there changed.
    pub async fn favorites_changed(&self, workspace_id: &str, user_id: &str) {
        let message = FavoritesChangedMessage {
            kind: "favoritesChanged",
            workspace_id,
        };
        self.send_to_user(workspace_id, user_id, &message).await;
    }
}
