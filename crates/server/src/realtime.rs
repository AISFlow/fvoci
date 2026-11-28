//! What the open websocket connections follow and are told: the table of the
//! connections, what each subscribed to, the visits of boards, and the changes sent to
//! them as they happen. Clients subscribe to a workspace or to blocks; the transport
//! that reads and writes their frames is `crate::ws`, which hands every command here.
//!
//! Whose connection it is is settled at the handshake, from the session the request
//! carries; there is no message to sign in with. A user's connection may subscribe to
//! workspaces. `subscribeBlocks` works without a session but needs the read token of the
//! shared board the blocks belong to.
//!
//! The commands and the binary frames of the bodies edited together are handed to the
//! connection's seat among their rooms (`crate::collab::Seat`).

mod block_message;
mod connection;
mod delivery;
mod favorites;
mod notifications;
mod presence;
pub(crate) mod quiet;
mod reactions;
mod readers;
mod subscriptions;

#[cfg(feature = "test-util")]
mod testing;

use block_message::UpdateBlockMessage;
pub(crate) use connection::{Connection, Opened};
use delivery::{Access, Readers};
use quiet::QuietLimit;
#[cfg(feature = "test-util")]
pub use testing::SubscriptionPause;

use std::collections::HashMap;
use std::sync::{Arc, Mutex, OnceLock};
use std::time::Duration;

use bytes::Bytes;
use fvoci_model::{
    Block, BlockType, Category, ClientConfig, Container, ContainerCategoryWebsocketData,
    ContainerMember, LoginOrigin, Session,
};
use serde::{Deserialize, Serialize};
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

use crate::app::{Audience, Former, SessionKept, SessionMade};
use crate::auth::limiter::AddressKey;
use crate::auth::{Authenticator, now_millis};
use crate::metrics::{Disconnect, Metrics};
use crate::permissions::ContainerScope;

/// Whose connection a websocket is: the session its handshake came with, and the token
/// that session was found under.
#[derive(Clone)]
pub struct Identity {
    /// The session the handshake came with.
    pub session: Session,
    /// The session token of the handshake.
    pub token: String,
}

impl std::fmt::Debug for Identity {
    /// Leaves out the token.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Identity")
            .field("session", &self.session)
            .finish_non_exhaustive()
    }
}

/// A command a client sends.
#[derive(Debug, Default, Deserialize)]
#[serde(default, rename_all = "camelCase")]
struct Command {
    #[serde(rename = "type")]
    kind: String,
    workspace_id: String,
    container_id: String,
    card_id: String,
    read_token: String,
    block_ids: Vec<String>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct UpdateContainerMessage<'a> {
    #[serde(rename = "type")]
    kind: &'static str,
    workspace_id: &'a str,
    container: &'a Container,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct UpdateMemberMessage<'a> {
    #[serde(rename = "type")]
    kind: &'static str,
    workspace_id: &'a str,
    member: &'a ContainerMember,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct RemovedMemberRef<'a> {
    container_id: &'a str,
    user_id: &'a str,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct RemoveMemberMessage<'a> {
    #[serde(rename = "type")]
    kind: &'static str,
    workspace_id: &'a str,
    member: RemovedMemberRef<'a>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct UpdateCategoryMessage<'a> {
    #[serde(rename = "type")]
    kind: &'static str,
    workspace_id: &'a str,
    #[serde(skip_serializing_if = "Option::is_none")]
    category: Option<&'a Category>,
    #[serde(skip_serializing_if = "<[_]>::is_empty")]
    container_categories: &'a [ContainerCategoryWebsocketData],
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct CategoryReorderMessage<'a> {
    #[serde(rename = "type")]
    kind: &'static str,
    category_order: &'a [String],
    workspace_id: &'a str,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct CategoryContainersReorderMessage<'a> {
    #[serde(rename = "type")]
    kind: &'static str,
    category_id: &'a str,
    container_order: &'a [String],
    workspace_id: &'a str,
}

/// The server accepted a workspace subscription (`subscriptions.rs`).
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct SubscribedWorkspaceMessage<'a> {
    #[serde(rename = "type")]
    kind: &'static str,
    workspace_id: &'a str,
}

/// The server accepted a shared-board block subscription (`subscriptions.rs`).
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct SubscribedBlocksMessage<'a> {
    #[serde(rename = "type")]
    kind: &'static str,
    workspace_id: &'a str,
    block_ids: &'a [String],
}

/// The server stopped what a connection followed of a workspace (`ending.rs`).
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct UnsubscribedWorkspaceMessage<'a> {
    #[serde(rename = "type")]
    kind: &'static str,
    workspace_id: &'a str,
    reason: &'static str,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct WorkspaceRoleChangedMessage<'a> {
    #[serde(rename = "type")]
    kind: &'static str,
    workspace_id: &'a str,
    user_id: &'a str,
    role: fvoci_model::WorkspaceRole,
}

/// The server stopped what a connection followed of a shared board's blocks.
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct UnsubscribedBlocksMessage<'a> {
    #[serde(rename = "type")]
    kind: &'static str,
    workspace_id: &'a str,
    block_ids: &'a [String],
    reason: &'static str,
}

/// That a person's name changed: the client reads the person again, through the same
/// route and rule as any other reading of them.
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct UpdateUserMessage<'a> {
    #[serde(rename = "type")]
    kind: &'static str,
    workspace_id: &'a str,
    user_id: &'a str,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct UpdateClientConfigMessage<'a> {
    #[serde(rename = "type")]
    kind: &'static str,
    client_config: &'a ClientConfig,
}

/// A message the hub sends, with the `type` it names: the label it is counted under.
trait Outgoing: Serialize {
    fn kind(&self) -> &'static str;
}

macro_rules! outgoing {
    ($($message:ident),* $(,)?) => {
        $(impl Outgoing for $message<'_> {
            fn kind(&self) -> &'static str {
                self.kind
            }
        })*
    };
}

outgoing!(
    UpdateContainerMessage,
    UpdateMemberMessage,
    RemoveMemberMessage,
    UpdateUserMessage,
    UpdateCategoryMessage,
    CategoryReorderMessage,
    CategoryContainersReorderMessage,
    UpdateClientConfigMessage,
    SubscribedWorkspaceMessage,
    SubscribedBlocksMessage,
    UnsubscribedWorkspaceMessage,
    WorkspaceRoleChangedMessage,
    UnsubscribedBlocksMessage,
);

type ListenerId = u64;

/// How many times a subscription or a change is checked again because access was
/// taken back meanwhile, before it is given up and its connection told to read anew
/// (1013): a check is a few reads of the store, and revocations elsewhere can keep
/// overtaking it.
const CHECKED_AT_MOST: usize = 8;

#[derive(Debug)]
struct Listener {
    /// The user of the session the handshake came with; empty for a connection without
    /// one, which stays so: no message signs a connection in.
    user_id: String,
    /// The session the handshake came with; empty for a connection without one.
    session_id: String,
    /// Where that session came from when it was made through a provider; `None` for a
    /// password, and without a session.
    login: Option<LoginOrigin>,
    /// How that session was made, and what it confirmed at the handshake or since, as
    /// the rule of which session opens what reads it: `None` for a connection without a
    /// session.
    making: Option<SessionKept>,
    /// The token the session is looked up by while the connection lasts
    /// (`Hub::session_ends`); `None` for a connection without a session.
    token: Option<Arc<SessionToken>>,
    /// Where the connection comes from, as readers by a public link are counted
    /// (`readers.rs`); `None` through the administration socket.
    address: Option<AddressKey>,
    outbox: mpsc::Sender<Bytes>,
    /// Cancelled when the hub gives the connection up; ends its reading side.
    closed: CancellationToken,
    /// Why the hub gave the connection up, set before `closed` is cancelled.
    closed_because: Arc<OnceLock<Disconnect>>,
    workspaces: Vec<String>,
    workspace_roles: HashMap<String, fvoci_model::WorkspaceRole>,
    blocks: Vec<String>,
    /// The shared boards whose read token this connection subscribed to blocks with.
    /// When such a board stops being shared under that token, they stop being followed.
    shared_containers: Vec<ReadPermit>,
    presence: presence::State,
}

/// The token a connection's session is found under now: the one of its handshake,
/// until the session goes on under another and its connections with it.
struct SessionToken(Mutex<String>);

impl SessionToken {
    /// The token of a handshake, for the connection and for the hub that hands it the
    /// next one.
    fn shared(token: String) -> Arc<Self> {
        Arc::new(Self(Mutex::new(token)))
    }

    /// The token as it is now.
    fn current(&self) -> String {
        self.0.lock().expect("the session token's lock").clone()
    }

    fn replace(&self, token: &str) {
        token.clone_into(&mut self.0.lock().expect("the session token's lock"));
    }
}

impl std::fmt::Debug for SessionToken {
    /// Leaves out the token.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SessionToken").finish_non_exhaustive()
    }
}

/// A shared board followed by its read token: the workspace and the blocks the
/// connection named, which it is told of when the token stops opening the board.
#[derive(Clone)]
struct ReadPermit {
    container: String,
    token: String,
    workspace: String,
    blocks: Vec<String>,
}

impl std::fmt::Debug for ReadPermit {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ReadPermit")
            .field("container", &self.container)
            .field("workspace", &self.workspace)
            .field("blocks", &self.blocks)
            .finish_non_exhaustive()
    }
}

#[derive(Debug, Default)]
struct Listeners {
    next_id: ListenerId,
    /// What was taken back, for the subscriptions being checked (`revocations.rs`).
    revoked: revocations::Revocations,
    by_id: HashMap<ListenerId, Listener>,
    by_workspace: HashMap<String, Vec<ListenerId>>,
    by_block: HashMap<String, Vec<ListenerId>>,
    /// The connections with a session, by its user.
    by_user: HashMap<String, Vec<ListenerId>>,
    /// The board visits of the connections, by board.
    visits: presence::Containers,
}

impl Listener {
    /// How the connection's session was made, and what it has confirmed by now; `None`
    /// for an anonymous connection without a session.
    fn made(&self) -> Option<SessionMade> {
        self.making.as_ref().map(SessionKept::made)
    }

    /// Gives the connection up, for `reason`; the first reason given is the one counted.
    fn close(&self, reason: Disconnect) {
        let _ = self.closed_because.set(reason);
        self.closed.cancel();
    }
}

impl Listeners {
    /// The listeners of a user that are subscribed to the workspace: every tab of the user.
    fn of_user(&self, workspace_id: &str, user_id: &str) -> Vec<ListenerId> {
        self.by_workspace.get(workspace_id).map_or_default(|ids| {
            ids.iter()
                .copied()
                .filter(|id| {
                    self.by_id
                        .get(id)
                        .is_some_and(|listener| listener.user_id == user_id)
                })
                .collect()
        })
    }

    /// Queues `text` for each of `recipients`, and closes and takes out one that does
    /// not keep up. How many it was queued for.
    fn queue(&mut self, recipients: &[ListenerId], text: &Bytes) -> usize {
        let mut queued = 0;
        for id in recipients {
            let Some(listener) = self.by_id.get(id) else {
                continue;
            };
            if listener.outbox.try_send(text.clone()).is_ok() {
                queued += 1;
            } else {
                tracing::warn!(
                    listener = id,
                    "websocket: dropping a connection that does not keep up"
                );
                listener.close(Disconnect::SlowConsumer);
                self.remove(*id);
            }
        }
        queued
    }

    fn remove(&mut self, id: ListenerId) {
        let Some(listener) = self.by_id.remove(&id) else {
            return;
        };
        for workspace in &listener.workspaces {
            remove_from(&mut self.by_workspace, workspace, id);
        }
        for block in &listener.blocks {
            remove_from(&mut self.by_block, block, id);
        }
        remove_from(&mut self.by_user, &listener.user_id, id);
        self.visits.forget(id, &listener.presence);
    }
}

fn remove_from(index: &mut HashMap<String, Vec<ListenerId>>, key: &str, id: ListenerId) {
    if let Some(ids) = index.get_mut(key) {
        ids.retain(|other| *other != id);
        if ids.is_empty() {
            index.remove(key);
        }
    }
}

/// The set of open websocket connections and what each is subscribed to.
#[derive(Debug, Clone)]
pub struct Hub {
    inner: Arc<HubInner>,
    pub(crate) audience: Audience,
}

#[derive(Debug)]
struct HubInner {
    listeners: Mutex<Listeners>,
    auth: Authenticator,
    ping_interval: Option<Duration>,
    metrics: Metrics,
    /// Whether the task that tells visits of access changes started (`presence`).
    watching_access: std::sync::Once,
    #[cfg(feature = "test-util")]
    subscription_pause: Mutex<Option<SubscriptionPause>>,
    #[cfg(feature = "test-util")]
    dispatch_pause: Mutex<Option<SubscriptionPause>>,
    /// The connections that read a command they have not carried out yet.
    #[cfg(feature = "test-util")]
    at_work: testing::AtWork,
    /// How long its connections may send nothing ([`quiet::QUIET_LIMIT`]).
    quiet_limit: QuietLimit,
}

impl Hub {
    /// Creates a hub. `ping_interval` is how often each connection is pinged, if at all
    /// (the configuration keeps it within the quiet limit of a connection, and only the
    /// tests leave it out); the connections, the messages and why connections end are
    /// counted in `metrics`.
    pub fn new(
        audience: Audience,
        auth: Authenticator,
        ping_interval: Option<Duration>,
        metrics: Metrics,
    ) -> Self {
        Self {
            inner: Arc::new(HubInner {
                listeners: Mutex::new(Listeners::default()),
                auth,
                ping_interval,
                metrics,
                watching_access: std::sync::Once::new(),
                #[cfg(feature = "test-util")]
                subscription_pause: Mutex::default(),
                #[cfg(feature = "test-util")]
                dispatch_pause: Mutex::default(),
                #[cfg(feature = "test-util")]
                at_work: testing::AtWork::default(),
                quiet_limit: QuietLimit::default(),
            }),
            audience,
        }
    }

    fn listeners(&self) -> std::sync::MutexGuard<'_, Listeners> {
        self.inner
            .listeners
            .lock()
            .expect("the listener table lock is never held across a panic")
    }

    /// Closes every connection. For shutdown: the connections are not part of the HTTP
    /// server's own connection handling once they are upgraded.
    pub fn shutdown(&self) {
        self.close_where(Disconnect::Shutdown, |_| true);
    }

    /// Returns whether any WebSocket connection is currently listening to `workspace_id`.
    #[must_use]
    pub fn has_workspace_listeners(&self, workspace_id: &str) -> bool {
        self.listeners().by_workspace.contains_key(workspace_id)
    }

    // -- broadcasts -------------------------------------------------------------------

    /// Tells the board's members in the workspace, and the listeners of the block and of its
    /// parent, that a block changed.
    ///
    /// A read-token subscription receives no user identifiers ([`Block::hide_people`])
    /// and no identifier of what a body names outside the board
    /// ([`Block::clear_references_but`]), as over public HTTP, unless its connection's
    /// user sees the board as its member.
    pub async fn broadcast_block_change(&self, workspace_id: &str, block: &Block) {
        let followers = self.block_followers(block);
        // Most changes happen with nobody listening; do not ask the store then.
        if followers.is_empty() && !self.listeners().by_workspace.contains_key(workspace_id) {
            return;
        }
        let recipients = self.workspace_listeners(workspace_id);
        // On the heap: a card's message holds a copy of its block, which would make
        // every future that tells of a block the larger.
        let message = Box::new(UpdateBlockMessage::new(workspace_id, block));
        self.send(
            &recipients,
            &*message,
            Access::Members {
                workspace: workspace_id,
                readers: Readers::Now(&block.container_id),
                also: None,
            },
        )
        .await;
        let by_token: Vec<_> = followers.into_iter().map(|(id, _)| id).collect();
        self.send_public_block(&by_token, &message).await;
    }

    /// The connections that follow the block or its parent by `subscribeBlocks`, with
    /// their users (empty for a connection without a session).
    fn block_followers(&self, block: &Block) -> Vec<(ListenerId, String)> {
        let listeners = self.listeners();
        [&block.id, &block.parent_id]
            .into_iter()
            .flat_map(|block_id| listeners.by_block.get(block_id).into_iter().flatten())
            .filter_map(|id| {
                listeners
                    .by_id
                    .get(id)
                    .map(|listener| (*id, listener.user_id.clone()))
            })
            .collect()
    }

    /// Tells the same listeners that a block was deleted. The message is a block change
    /// whose block has its identifiers, its type, and the deletion time.
    pub async fn broadcast_block_delete(
        &self,
        workspace_id: &str,
        block_id: &str,
        container_id: &str,
        block_type: BlockType,
    ) {
        let now = now_millis();
        let block = Block {
            id: block_id.to_owned(),
            container_id: container_id.to_owned(),
            block_type,
            updated_at: now,
            deleted_at: Some(now),
            ..Block::default()
        };
        self.broadcast_block_change(workspace_id, &block).await;
    }

    /// Tells those who read a board that is deleted by now that one of its blocks was
    /// deleted, with the message of [`Self::broadcast_block_delete`].
    ///
    /// The memberships went with the board, so the caller says who read it.
    pub async fn broadcast_block_delete_with_container(
        &self,
        workspace_id: &str,
        block_id: &str,
        container_id: &str,
        block_type: BlockType,
        former: Former<'_>,
    ) {
        let now = now_millis();
        let block = Block {
            id: block_id.to_owned(),
            container_id: container_id.to_owned(),
            block_type,
            updated_at: now,
            deleted_at: Some(now),
            ..Block::default()
        };
        let message = UpdateBlockMessage::new(workspace_id, &block);
        self.send_to_former(workspace_id, &message, former).await;
    }

    /// Tells the board's members in the workspace that the board changed.
    pub async fn broadcast_container_change(&self, workspace_id: &str, container: &Container) {
        let message = UpdateContainerMessage {
            kind: "containerChanged",
            workspace_id,
            container,
        };
        let recipients = self.workspace_listeners(workspace_id);
        self.send(
            &recipients,
            &message,
            Access::Members {
                workspace: workspace_id,
                readers: Readers::Now(&container.id),
                also: None,
            },
        )
        .await;
    }

    /// Tells those who read the board that it was deleted. The message is a board change
    /// whose board has only its identifiers and the deletion time.
    ///
    /// The memberships went with the board, so the caller says who read it. It is told
    /// so as well to those who read an open wiki that was made private, as they read it
    /// no longer: the caller says who read it before.
    pub async fn broadcast_container_delete(
        &self,
        workspace_id: &str,
        container_id: &str,
        former: Former<'_>,
    ) {
        let container = gone_container(workspace_id, container_id);
        let message = UpdateContainerMessage {
            kind: "containerChanged",
            workspace_id,
            container: &container,
        };
        self.send_to_former(workspace_id, &message, former).await;
    }

    /// Tells who read the board as it was, `scope`, without holding a role on it, that
    /// it is gone for them, with the message of [`Self::broadcast_container_delete`]: an
    /// open wiki made private. Its `members` read it still, and are not told.
    pub async fn broadcast_closed_to_others(
        &self,
        workspace_id: &str,
        container_id: &str,
        members: &[ContainerMember],
        scope: ContainerScope,
    ) {
        let container = gone_container(workspace_id, container_id);
        let message = UpdateContainerMessage {
            kind: "containerChanged",
            workspace_id,
            container: &container,
        };
        let in_workspace = self.workspace_listeners(workspace_id);
        let recipients: Vec<ListenerId> = {
            let listeners = self.listeners();
            in_workspace
                .into_iter()
                .filter(|id| {
                    listeners.by_id.get(id).is_some_and(|listener| {
                        !members
                            .iter()
                            .any(|member| member.user_id == listener.user_id)
                    })
                })
                .collect()
        };
        let readers = Readers::Former {
            members: &[],
            scope,
        };
        self.send(
            &recipients,
            &message,
            Access::Members {
                workspace: workspace_id,
                readers,
                also: None,
            },
        )
        .await;
    }

    /// Tells who read a board that is gone of `message`: its members, and of an open
    /// wiki whoever works in the workspace, as they are in it now.
    async fn send_to_former(
        &self,
        workspace_id: &str,
        message: &impl Outgoing,
        former: Former<'_>,
    ) {
        let recipients = self.workspace_listeners(workspace_id);
        let readers = Readers::Former {
            members: former.members,
            scope: former.scope,
        };
        self.send(
            &recipients,
            message,
            Access::Members {
                workspace: workspace_id,
                readers,
                also: None,
            },
        )
        .await;
    }

    /// Tells the board's members in the workspace that a membership changed.
    pub async fn broadcast_member_change(
        &self,
        workspace_id: &str,
        container_id: &str,
        member: &ContainerMember,
    ) {
        let message = UpdateMemberMessage {
            kind: "containerMemberChanged",
            workspace_id,
            member,
        };
        let recipients = self.workspace_listeners(workspace_id);
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

    /// Tells the board's members in the workspace, and the removed user, that a membership
    /// ended.
    pub async fn broadcast_member_delete(
        &self,
        workspace_id: &str,
        container_id: &str,
        user_id: &str,
    ) {
        let message = RemoveMemberMessage {
            kind: "containerMemberRemoved",
            workspace_id,
            member: RemovedMemberRef {
                container_id,
                user_id,
            },
        };
        let recipients = self.workspace_listeners(workspace_id);
        self.send(
            &recipients,
            &message,
            Access::Members {
                workspace: workspace_id,
                readers: Readers::Now(container_id),
                also: Some(user_id),
            },
        )
        .await;
    }

    /// Tells whoever follows the workspace and sees the person, a member of it
    /// (`permissions::sees_person`, by name), that the person's name changed.
    pub async fn broadcast_person_change(&self, workspace_id: &str, user_id: &str) {
        let message = UpdateUserMessage {
            kind: "userChanged",
            workspace_id,
            user_id,
        };
        let recipients = self.workspace_listeners(workspace_id);
        self.send(
            &recipients,
            &message,
            Access::Seers {
                workspace: workspace_id,
                person: user_id,
            },
        )
        .await;
    }

    /// Tells every connection that the client configuration changed.
    pub async fn broadcast_config_change(&self, client_config: &ClientConfig) {
        let message = UpdateClientConfigMessage {
            kind: "clientConfigChanged",
            client_config,
        };
        let recipients: Vec<ListenerId> = self.listeners().by_id.keys().copied().collect();
        self.send(&recipients, &message, Access::All).await;
    }

    /// Tells the category's owner that it changed.
    pub async fn broadcast_category_change(&self, category: &Category) {
        let message = UpdateCategoryMessage {
            kind: "categoryChanged",
            workspace_id: &category.workspace_id,
            category: Some(category),
            container_categories: &[],
        };
        self.send_to_user(&category.workspace_id, &category.user_id, &message)
            .await;
    }

    /// Tells a user that boards moved between their categories or were hidden.
    pub async fn broadcast_category_container_change(
        &self,
        workspace_id: &str,
        user_id: &str,
        container_categories: &[ContainerCategoryWebsocketData],
    ) {
        let message = UpdateCategoryMessage {
            kind: "containerCategoriesChanged",
            workspace_id,
            category: None,
            container_categories,
        };
        self.send_to_user(workspace_id, user_id, &message).await;
    }

    /// Tells a user the new order of their categories.
    pub async fn broadcast_category_reorder(
        &self,
        workspace_id: &str,
        user_id: &str,
        category_order: &[String],
    ) {
        let message = CategoryReorderMessage {
            kind: "categoriesReordered",
            category_order,
            workspace_id,
        };
        self.send_to_user(workspace_id, user_id, &message).await;
    }

    /// Tells a user the new order of the boards in one of their categories.
    pub async fn broadcast_category_containers_reorder(
        &self,
        workspace_id: &str,
        user_id: &str,
        category_id: &str,
        container_order: &[String],
    ) {
        let message = CategoryContainersReorderMessage {
            kind: "categoryContainersReordered",
            category_id,
            container_order,
            workspace_id,
        };
        self.send_to_user(workspace_id, user_id, &message).await;
    }

    fn workspace_listeners(&self, workspace_id: &str) -> Vec<ListenerId> {
        self.listeners()
            .by_workspace
            .get(workspace_id)
            .cloned()
            .unwrap_or_default()
    }

    /// Sends a message to every connection of the user that is subscribed to the workspace,
    /// so every open tab of the user receives it.
    async fn send_to_user(&self, workspace_id: &str, user_id: &str, message: &impl Outgoing) {
        let recipients = self.listeners().of_user(workspace_id, user_id);
        self.send(&recipients, message, Access::Workspace(workspace_id))
            .await;
    }

    /// Queues the text of a message whose `type` is `kind` for the given listeners.
    /// A listener whose queue is full is dropped and its connection closed. Cloning the
    /// text copies none of it.
    fn queue_text(&self, recipients: &[ListenerId], text: &Bytes, kind: &'static str) {
        if recipients.is_empty() {
            return;
        }
        let queued = self.listeners().queue(recipients, text);
        self.inner.metrics.websocket_queued(kind, queued);
    }
}

/// A message as the text frame it is sent in; `None`, logged, if it cannot be written.
fn text_of(message: &impl Outgoing) -> Option<Bytes> {
    match serde_json::to_string(message) {
        Ok(text) => Some(text.into()),
        Err(error) => {
            tracing::error!(%error, message_type = message.kind(), "websocket: a message could not be serialized");
            None
        }
    }
}

fn subscribe_to_workspace(listeners: &mut Listeners, id: ListenerId, workspace_id: &str) {
    let Some(listener) = listeners.by_id.get_mut(&id) else {
        return;
    };
    if listener
        .workspaces
        .iter()
        .any(|workspace| workspace == workspace_id)
    {
        return;
    }
    listener.workspaces.push(workspace_id.to_owned());
    listeners
        .by_workspace
        .entry(workspace_id.to_owned())
        .or_default()
        .push(id);
}

fn subscribe_to_blocks(listeners: &mut Listeners, id: ListenerId, block_ids: &[String]) {
    let Some(listener) = listeners.by_id.get_mut(&id) else {
        return;
    };
    for block_id in block_ids {
        if listener.blocks.contains(block_id) {
            continue;
        }
        listener.blocks.push(block_id.clone());
        listeners
            .by_block
            .entry(block_id.clone())
            .or_default()
            .push(id);
    }
}

fn unsubscribe_from_blocks(listeners: &mut Listeners, id: ListenerId, block_ids: &[String]) {
    if let Some(listener) = listeners.by_id.get_mut(&id) {
        listener.blocks.retain(|block| !block_ids.contains(block));
        for permit in &mut listener.shared_containers {
            permit.blocks.retain(|block| !block_ids.contains(block));
        }
        // A board none of whose blocks is followed is not followed.
        listener
            .shared_containers
            .retain(|permit| !permit.blocks.is_empty());
    }
    for block_id in block_ids {
        remove_from(&mut listeners.by_block, block_id, id);
    }
}

/// A board as it is told once gone: only its identifiers and the time it went, as
/// its deletion and the closing of an open wiki to who did not join it are told.
fn gone_container(workspace_id: &str, container_id: &str) -> Container {
    let now = now_millis();
    Container {
        id: container_id.to_owned(),
        workspace_id: workspace_id.to_owned(),
        updated_at: now,
        deleted_at: Some(now),
        ..Container::default()
    }
}

mod closing;
mod ending;
mod revocations;
mod subscribe;

/// Unit tests for the listener table and command decoding.
#[cfg(test)]
mod tests;
