//! Validating subscriptions, and registering them unless what concerns them was taken
//! back meanwhile (`revocations.rs`).

use std::collections::BTreeSet;

use fvoci_model::WorkspaceRole;

use super::ending::{Ended, Of};
use super::readers::ReaderLimits;
use super::revocations::Followed;
use super::{
    CHECKED_AT_MOST, Command, Hub, ListenerId, ReadPermit, SubscribedBlocksMessage,
    SubscribedWorkspaceMessage, remove_from, subscribe_to_blocks, subscribe_to_workspace,
    unsubscribe_from_blocks,
};
use crate::app::App;
use crate::error::AppError;
use crate::metrics::Disconnect;

/// Maximum distinct block IDs accepted in one `subscribeBlocks` command.
const MOST_BLOCKS_PER_COMMAND: usize = 200;

/// Truncates an untrusted command type to 64 characters for debug diagnostics.
fn truncated_command_kind(kind: &str) -> &str {
    match kind.char_indices().nth(64) {
        Some((idx, _)) => &kind[..idx],
        None => kind,
    }
}

/// Why a connection does not follow a workspace.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum NotOpened {
    /// Its access is not there: a subscription checked before ends, for this reason.
    Ended(Ended),
    /// Its access could not be read: nothing ends for it.
    Unreadable,
}

impl Hub {
    /// Handles one command. Returns `false` if the connection must be closed.
    pub(super) async fn handle(&self, app: &App, id: ListenerId, command: Command) -> bool {
        match command.kind.as_str() {
            "setPresence" => {
                self.set_presence(app, id, command).await;
                return true;
            }
            "subscribeBlocks" => {
                self.subscribe_blocks(id, &command, ReaderLimits::of(app))
                    .await;
                return true;
            }
            "unsubscribeBlocks" => {
                // Following less grants nothing: deduplicate IDs and drop local tracking
                // directly without unbounded DB lookups.
                let deduped: Vec<String> = command
                    .block_ids
                    .iter()
                    .cloned()
                    .collect::<BTreeSet<_>>()
                    .into_iter()
                    .collect();
                if !deduped.is_empty() && deduped.len() <= MOST_BLOCKS_PER_COMMAND {
                    match self
                        .container_of_read_token(
                            &command.workspace_id,
                            &command.read_token,
                            &deduped,
                        )
                        .await
                    {
                        Ok(Some(_)) => {
                            unsubscribe_from_blocks(&mut self.listeners(), id, &deduped);
                        }
                        Err(error) => {
                            tracing::error!(%error, workspace_id = %command.workspace_id, "websocket: checking a read token failed");
                            unsubscribe_from_blocks(&mut self.listeners(), id, &deduped);
                        }
                        Ok(None) => {}
                    }
                }
                return true;
            }
            _ => {}
        }
        let user_id = self
            .listeners()
            .by_id
            .get(&id)
            .map_or_default(|listener| listener.user_id.clone());
        if user_id.is_empty() {
            tracing::debug!(
                command_type = truncated_command_kind(&command.kind),
                "websocket: rejected an unauthenticated message"
            );
            return true;
        }
        match command.kind.as_str() {
            "subscribeWorkspace" => {
                self.subscribe_workspace(id, &user_id, &command.workspace_id)
                    .await;
            }
            "unsubscribeWorkspace" => {
                let mut listeners = self.listeners();
                if let Some(listener) = listeners.by_id.get_mut(&id) {
                    listener
                        .workspaces
                        .retain(|workspace| *workspace != command.workspace_id);
                    listener.workspace_roles.remove(&command.workspace_id);
                }
                remove_from(&mut listeners.by_workspace, &command.workspace_id, id);
            }
            other => tracing::debug!(
                command_type = truncated_command_kind(other),
                "websocket: invalid command type"
            ),
        }
        true
    }

    /// Follows the blocks of a shared board by its read token, if `limits` take one
    /// more reader of the board by its link (`readers.rs`); tells the connection
    /// `blocksUnsubscribed` otherwise, and `blocksSubscribed` once accepted.
    async fn subscribe_blocks(&self, id: ListenerId, command: &Command, limits: ReaderLimits) {
        let deduped_blocks: Vec<String> = command
            .block_ids
            .iter()
            .cloned()
            .collect::<BTreeSet<_>>()
            .into_iter()
            .collect();
        if deduped_blocks.is_empty() || deduped_blocks.len() > MOST_BLOCKS_PER_COMMAND {
            tracing::debug!(
                count = deduped_blocks.len(),
                "websocket: ignored subscribeBlocks with empty or excessive blockIds"
            );
            return;
        }
        // The board its token opened when it was first checked.
        let mut validated: Option<String> = None;
        for _ in 0..CHECKED_AT_MOST {
            let since = self.listeners().revoked.now();
            let container_id = match self
                .container_of_read_token(
                    &command.workspace_id,
                    &command.read_token,
                    &deduped_blocks,
                )
                .await
            {
                Ok(Some(container_id)) => container_id,
                Ok(None) => {
                    // The page believes it follows the blocks: it is told they ended,
                    // with what it followed of the board before.
                    if let Some(container_id) = &validated {
                        self.end_container_with(
                            id,
                            container_id,
                            &command.workspace_id,
                            &deduped_blocks,
                        );
                    }
                    return;
                }
                // The page believes it follows the blocks: it connects again.
                Err(error) => {
                    tracing::error!(%error, workspace_id = %command.workspace_id, "websocket: checking a read token failed");
                    self.close_unread(&[id]);
                    return;
                }
            };
            validated = Some(container_id.clone());
            #[cfg(feature = "test-util")]
            self.pause_subscription().await;
            let mut listeners = self.listeners();
            // Gone meanwhile: closed, with nothing to follow.
            let Some(listener) = listeners.by_id.get(&id) else {
                return;
            };
            // This board's link taken back meanwhile, or this user's sessions: checked
            // again.
            if listeners
                .revoked
                .of_container_since(&listener.user_id, &container_id, since)
            {
                continue;
            }
            if !listeners.admits_reader(id, &container_id, limits) {
                let told = listeners.refuse_blocks(id, &command.workspace_id, &deduped_blocks);
                drop(listeners);
                self.count_told("blocksUnsubscribed", usize::from(told));
                return;
            }
            subscribe_to_blocks(&mut listeners, id, &deduped_blocks);
            if let Some(listener) = listeners.by_id.get_mut(&id) {
                permit_blocks(
                    &mut listener.shared_containers,
                    container_id,
                    &command.workspace_id,
                    &command.read_token,
                    &deduped_blocks,
                );
            }
            let told = listeners.tell(
                id,
                &SubscribedBlocksMessage {
                    kind: "blocksSubscribed",
                    workspace_id: &command.workspace_id,
                    block_ids: &deduped_blocks,
                },
            );
            drop(listeners);
            self.count_told("blocksSubscribed", usize::from(told));
            return;
        }
        tracing::warn!(
            workspace_id = %command.workspace_id,
            "websocket: a block subscription was given up"
        );
        // The page believes it follows the blocks: it connects again, later.
        self.close_listener(id, Disconnect::ChecksOvertaken);
    }

    async fn subscribe_workspace(&self, id: ListenerId, user_id: &str, workspace: &str) {
        let mut validated_role = None;
        for _ in 0..CHECKED_AT_MOST {
            let since = self.listeners().revoked.now();
            let (role, org_id) = match self.opened_role(id, user_id, workspace).await {
                Ok(opened) => opened,
                Err(NotOpened::Ended(ended)) => {
                    // The page believes it follows the workspace: it is told it ended.
                    if validated_role.is_some() {
                        self.end_workspace(id, workspace, ended, Of::Any);
                    }
                    return;
                }
                // Access that cannot be read has not ended, but the page believes it
                // follows the workspace: it connects again and subscribes anew.
                Err(NotOpened::Unreadable) => {
                    self.close_unread(&[id]);
                    return;
                }
            };
            if validated_role.is_some_and(|previous| previous != role) {
                self.end_workspace(id, workspace, Ended::Access, Of::Any);
                return;
            }
            validated_role = Some(role);
            #[cfg(feature = "test-util")]
            self.pause_subscription().await;
            let mut listeners = self.listeners();
            if !listeners.by_id.contains_key(&id) {
                return;
            }
            // What concerns this user there, or this workspace, taken back meanwhile.
            let followed = Followed {
                user: user_id,
                workspace,
                org: &org_id,
            };
            if listeners.revoked.of_workspace_since(followed, since) {
                continue;
            }
            subscribe_to_workspace(&mut listeners, id, workspace);
            if let Some(listener) = listeners.by_id.get_mut(&id) {
                listener.workspace_roles.insert(workspace.to_owned(), role);
            }
            let told = listeners.tell(
                id,
                &SubscribedWorkspaceMessage {
                    kind: "workspaceSubscribed",
                    workspace_id: workspace,
                },
            );
            drop(listeners);
            self.count_told("workspaceSubscribed", usize::from(told));
            return;
        }
        tracing::warn!(
            workspace_id = workspace,
            user_id,
            "websocket: a workspace subscription was given up"
        );
        // The page believes it follows the workspace: it connects again, later.
        self.close_listener(id, Disconnect::ChecksOvertaken);
    }

    /// What the connection's user is in the workspace, and the workspace's
    /// organization, if they are a member of it and the connection's session opens that
    /// organization; otherwise why not.
    async fn opened_role(
        &self,
        id: ListenerId,
        user_id: &str,
        workspace: &str,
    ) -> Result<(WorkspaceRole, String), NotOpened> {
        let access = match self
            .audience
            .current_workspace_access(user_id, workspace)
            .await
        {
            Ok(Some(access)) => access,
            Ok(None) => return Err(NotOpened::Ended(Ended::Access)),
            Err(error) => {
                tracing::error!(%error, workspace_id = workspace, "websocket: reading a workspace's membership failed");
                return Err(NotOpened::Unreadable);
            }
        };
        let listeners = self.listeners();
        let listener = listeners
            .by_id
            .get(&id)
            .ok_or(NotOpened::Ended(Ended::Access))?;
        let made = listener.made().ok_or(NotOpened::Ended(Ended::Access))?;
        if access.takes.opened_by(made.making()) {
            Ok((access.workspace_role, access.org_id))
        } else if access.takes.asks_for_password() {
            Err(NotOpened::Ended(Ended::Password))
        } else {
            Err(NotOpened::Ended(Ended::Sso))
        }
    }

    /// A block subscription needs a workspace, blocks that all belong to one board, and that
    /// board's read token. Returns the board, or `None` if the subscription is refused.
    ///
    /// # Errors
    ///
    /// Returns the store's error if the blocks or the token cannot be checked: that is
    /// no "refused".
    async fn container_of_read_token(
        &self,
        workspace_id: &str,
        read_token: &str,
        block_ids: &[String],
    ) -> Result<Option<String>, AppError> {
        if workspace_id.is_empty()
            || block_ids.is_empty()
            || block_ids.len() > MOST_BLOCKS_PER_COMMAND
        {
            return Ok(None);
        }
        let mut container_id = String::new();
        for block_id in block_ids {
            let Some(of_block) = self
                .audience
                .board_of_shared_block(workspace_id, block_id, read_token)
                .await?
            else {
                return Ok(None);
            };
            if container_id.is_empty() {
                container_id = of_block;
            } else if container_id != of_block {
                return Ok(None);
            }
        }
        let valid = self
            .inner
            .auth
            .is_valid_read_token(&container_id, read_token)
            .await?;
        Ok(valid.then_some(container_id))
    }
}

/// Counts the blocks among those followed of the board by its read token: with the
/// board's permit if the connection follows it already, in a new one if not. The
/// permit keeps the token it was first given.
fn permit_blocks(
    permits: &mut Vec<ReadPermit>,
    container_id: String,
    workspace_id: &str,
    read_token: &str,
    block_ids: &[String],
) {
    let permit = if let Some(at) = permits
        .iter()
        .position(|permit| permit.container == container_id)
    {
        &mut permits[at]
    } else {
        permits.push(ReadPermit {
            container: container_id,
            token: read_token.to_owned(),
            workspace: workspace_id.to_owned(),
            blocks: Vec::new(),
        });
        permits.last_mut().expect("a permit was just pushed")
    };
    for block in block_ids {
        if !permit.blocks.contains(block) {
            permit.blocks.push(block.clone());
        }
    }
}
