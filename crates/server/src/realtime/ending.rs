//! Ending what a connection follows without closing it: a workspace its user is no
//! member of any more, holds another role in, cannot open with this session, or that
//! went to the trash; a shared board whose read token stopped opening it. The
//! connection is told (`workspaceUnsubscribed`, `blocksUnsubscribed`) and keeps
//! what else it follows, and its rooms: the rooms of a workspace ask again about their
//! members themselves (`Collab::recheck_access`). A connection is closed only when its
//! session ends (`closing.rs`).

use fvoci_model::WorkspaceRole;

use super::revocations::Revocations;
use super::{
    Hub, Listener, ListenerId, Listeners, Outgoing, UnsubscribedBlocksMessage,
    UnsubscribedWorkspaceMessage, WorkspaceRoleChangedMessage, remove_from, text_of,
    unsubscribe_from_blocks,
};
use crate::metrics::Disconnect;

/// Why a subscription ended, as the connection is told it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Ended {
    /// Its user is no member of the workspace any more, holds another role in it now,
    /// or the workspace went to the trash. Following it again asks anew.
    Access,
    /// The read token it followed the board's blocks by stopped opening the board.
    Share,
    /// The workspace's organization takes only sessions made through its provider,
    /// and the connection's session was not.
    Sso,
    /// The workspace is the user's own space, which takes only sessions made with
    /// the account's password, and the connection's session was not.
    Password,
    /// The board has as many readers by its public link as it may (`readers.rs`): the
    /// subscription was not taken.
    Full,
}

/// Which subscription an ending is of: the one the connection follows now, whatever
/// it is, or the one it was found to follow with a role when access was checked, which
/// is not another one made since.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Of {
    /// Whatever it follows of the workspace now, or believes it does: a subscription
    /// being checked is told even if it is not registered.
    Any,
    /// Only if it still follows the workspace with this role.
    Role(WorkspaceRole),
}

impl Ended {
    /// The `reason` of the message, the words the connection used to be closed with.
    fn reason(self) -> &'static str {
        match self {
            Self::Access => "workspace access ended",
            Self::Share => "public link access ended",
            Self::Sso => "sso required",
            Self::Password => "password sign-in required",
            Self::Full => "public link connections full",
        }
    }
}

impl Listeners {
    /// Stops the connection following the workspace, if it does (as `of` says), and
    /// tells it so, with what its user is counted as having had taken back. Returns
    /// whether it was told: a connection that cannot take the message is closed as too
    /// slow.
    pub(super) fn end_workspace(
        &mut self,
        id: ListenerId,
        workspace_id: &str,
        ended: Ended,
        of: Of,
    ) -> bool {
        let Some(listener) = self.by_id.get_mut(&id) else {
            return false;
        };
        if let Of::Role(checked) = of
            && listener.workspace_roles.get(workspace_id) != Some(&checked)
        {
            // Ended already, or followed anew since, under what is current.
            return false;
        }
        listener
            .workspaces
            .retain(|followed| followed != workspace_id);
        listener.workspace_roles.remove(workspace_id);
        // A subscription of the same user to it being checked is checked again.
        self.revoked
            .user_in_workspace(&listener.user_id, workspace_id);
        remove_from(&mut self.by_workspace, workspace_id, id);
        self.tell(
            id,
            &UnsubscribedWorkspaceMessage {
                kind: "workspaceUnsubscribed",
                workspace_id,
                reason: ended.reason(),
            },
        )
    }

    /// Stops the connection following the blocks it named, of the workspace, by a
    /// read token that no longer opens their board, and tells it so.
    fn end_blocks(&mut self, id: ListenerId, workspace_id: &str, block_ids: &[String]) -> bool {
        unsubscribe_from_blocks(self, id, block_ids);
        self.tell(
            id,
            &UnsubscribedBlocksMessage {
                kind: "blocksUnsubscribed",
                workspace_id,
                block_ids,
                reason: Ended::Share.reason(),
            },
        )
    }

    /// Tells the connection that the blocks it asked to follow, of the workspace, by a
    /// public link were not taken: the board has as many readers by its link as it may.
    pub(super) fn refuse_blocks(
        &mut self,
        id: ListenerId,
        workspace_id: &str,
        block_ids: &[String],
    ) -> bool {
        self.tell(
            id,
            &UnsubscribedBlocksMessage {
                kind: "blocksUnsubscribed",
                workspace_id,
                block_ids,
                reason: Ended::Full.reason(),
            },
        )
    }

    /// Stops the connection following the board by its read token, with every block
    /// it followed of it, and tells it so.
    fn end_container(&mut self, id: ListenerId, container_id: &str) -> bool {
        let Some(permit) = self.by_id.get(&id).and_then(|listener| {
            listener
                .shared_containers
                .iter()
                .find(|permit| permit.container == container_id)
                .cloned()
        }) else {
            return false;
        };
        self.end_blocks(id, &permit.workspace, &permit.blocks)
    }

    /// Stops the connection following the board by its read token, with every block it
    /// followed of it and those it asked for now (`block_ids`, of `workspace_id`), and
    /// tells it so: for a subscription found ended as it was checked again.
    fn end_container_with(
        &mut self,
        id: ListenerId,
        container_id: &str,
        workspace_id: &str,
        block_ids: &[String],
    ) -> bool {
        let mut blocks: Vec<String> = self
            .by_id
            .get(&id)
            .and_then(|listener| {
                listener
                    .shared_containers
                    .iter()
                    .find(|permit| permit.container == container_id)
            })
            .map(|permit| permit.blocks.clone())
            .unwrap_or_default();
        for block in block_ids {
            if !blocks.contains(block) {
                blocks.push(block.clone());
            }
        }
        self.end_blocks(id, workspace_id, &blocks)
    }

    /// Takes the workspace out of what every listener follows, and tells each that
    /// followed it. Returns how many were told.
    pub(super) fn stop_following_workspace(&mut self, workspace_id: &str) -> usize {
        self.revoked.workspace(workspace_id);
        let ids = self
            .by_workspace
            .get(workspace_id)
            .cloned()
            .unwrap_or_default();
        ids.into_iter()
            .filter(|id| self.end_workspace(*id, workspace_id, Ended::Access, Of::Any))
            .count()
    }

    /// Queues `message` for the connection; one whose queue is full is closed.
    pub(super) fn tell(&mut self, id: ListenerId, message: &impl Outgoing) -> bool {
        let Some(text) = text_of(message) else {
            return false;
        };
        let Some(listener) = self.by_id.get(&id) else {
            return false;
        };
        if listener.outbox.try_send(text).is_ok() {
            return true;
        }
        tracing::warn!(
            listener = id,
            "websocket: dropping a connection that does not keep up"
        );
        listener.close(Disconnect::SlowConsumer);
        self.remove(id);
        false
    }
}

impl Hub {
    /// Stops the connection following the workspace, as `of` says, and tells it why.
    pub(super) fn end_workspace(&self, id: ListenerId, workspace_id: &str, ended: Ended, of: Of) {
        let told = self.listeners().end_workspace(id, workspace_id, ended, of);
        self.count_told("workspaceUnsubscribed", usize::from(told));
    }

    /// Stops the connection following the board by its read token, with every block it
    /// followed of it and those it asks for now, and tells it so.
    pub(super) fn end_container_with(
        &self,
        id: ListenerId,
        container_id: &str,
        workspace_id: &str,
        block_ids: &[String],
    ) {
        let told = self
            .listeners()
            .end_container_with(id, container_id, workspace_id, block_ids);
        self.count_told("blocksUnsubscribed", usize::from(told));
    }

    /// Stops the connection following the board by its read token, and tells it so.
    pub(super) fn end_container(&self, id: ListenerId, container_id: &str) {
        let told = self.listeners().end_container(id, container_id);
        self.count_told("blocksUnsubscribed", usize::from(told));
    }

    /// Counts what was taken back (`revoke`), then stops what `ends` picks of what the
    /// connections follow of workspaces, for `ended`, and tells each connection of
    /// each: in one hold of the listeners, so that no subscription registers between.
    fn end_workspaces_where(
        &self,
        ended: Ended,
        revoke: impl FnOnce(&mut Revocations),
        ends: impl Fn(&Listener, &str) -> bool,
    ) {
        let mut listeners = self.listeners();
        revoke(&mut listeners.revoked);
        let picked: Vec<(ListenerId, String)> = listeners
            .by_id
            .iter()
            .flat_map(|(id, listener)| {
                listener
                    .workspaces
                    .iter()
                    .filter(|workspace| ends(listener, workspace))
                    .map(|workspace| (*id, workspace.clone()))
            })
            .collect();
        let told = picked
            .into_iter()
            .filter(|(id, workspace)| listeners.end_workspace(*id, workspace, ended, Of::Any))
            .count();
        drop(listeners);
        self.count_told("workspaceUnsubscribed", told);
    }

    pub(super) fn count_told(&self, kind: &'static str, told: usize) {
        if told > 0 {
            self.inner.metrics.websocket_queued(kind, told);
        }
    }

    /// Stops the connections following blocks of a board by its read token. For when
    /// that token stops opening the board: sharing is turned off, the board is shared
    /// anew under another token, or the board is deleted. A reader with the new link
    /// follows it again.
    pub fn end_readers_of_container(&self, container_id: &str) {
        let mut listeners = self.listeners();
        listeners.revoked.container(container_id);
        listeners.end_shared_visits(container_id);
        let ids: Vec<ListenerId> = listeners
            .by_id
            .iter()
            .filter(|(_, listener)| {
                listener
                    .shared_containers
                    .iter()
                    .any(|permit| permit.container == container_id)
            })
            .map(|(id, _)| *id)
            .collect();
        let told = ids
            .into_iter()
            .filter(|id| listeners.end_container(*id, container_id))
            .count();
        drop(listeners);
        self.count_told("blocksUnsubscribed", told);
    }

    /// Stops the user's connections following the workspaces they are no member of
    /// now, or hold another role in. For when what they were in a workspace was taken
    /// back (an organization's administrator whose joining ended, who runs the server
    /// no more): following one again asks whether they are in it.
    pub async fn follow_only_own_workspaces(&self, user_id: &str) {
        let followed: Vec<(ListenerId, String, WorkspaceRole)> = {
            let mut listeners = self.listeners();
            // Taken back in this handle's organization; in which, if it names none, is
            // not known: in every one, then, as when the user's sessions end.
            match self.audience.org_id() {
                Some(org_id) => listeners.revoked.user_in_org(user_id, &org_id),
                None => listeners.revoked.user(user_id),
            }
            listeners
                .by_id
                .iter()
                .filter(|(_, listener)| listener.user_id == user_id)
                .flat_map(|(id, listener)| {
                    listener
                        .workspace_roles
                        .iter()
                        .map(move |(workspace, role)| (*id, workspace.clone(), *role))
                })
                .collect()
        };
        for (id, workspace_id, subscribed_role) in followed {
            match self
                .audience
                .current_workspace_access(user_id, &workspace_id)
                .await
            {
                Ok(Some(access)) if access.workspace_role == subscribed_role => {}
                Ok(_) => {
                    let of = Of::Role(subscribed_role);
                    self.end_workspace(id, &workspace_id, Ended::Access, of);
                }
                // Not known to have ended: closed as retryable, so the page asks
                // again as it connects.
                Err(error) => {
                    tracing::error!(%error, %workspace_id, "websocket: reading a workspace's membership failed");
                    self.close_unread(&[id]);
                }
            }
        }
    }

    /// Stops the user's connections following the workspace whose role of theirs
    /// changed; also a subscription of theirs being checked, which follows nothing
    /// yet, is checked again.
    pub fn workspace_role_changed(&self, workspace_id: &str, user_id: &str) {
        self.end_workspaces_where(
            Ended::Access,
            |revoked| revoked.user_in_workspace(user_id, workspace_id),
            |listener, workspace| listener.user_id == user_id && workspace == workspace_id,
        );
    }

    /// Announces the user's new `role` in `workspace_id` (`workspaceRoleChanged`) to
    /// their subscribed connections and ends the old-role subscription so they
    /// re-subscribe under `role` and refresh container permissions immediately.
    pub fn workspace_role_changed_to(
        &self,
        workspace_id: &str,
        user_id: &str,
        role: WorkspaceRole,
    ) {
        let mut listeners = self.listeners();
        listeners.revoked.user_in_workspace(user_id, workspace_id);
        let ids = listeners.of_user(workspace_id, user_id);
        let message = WorkspaceRoleChangedMessage {
            kind: "workspaceRoleChanged",
            workspace_id,
            user_id,
            role,
        };
        let role_told = ids
            .iter()
            .filter(|&&id| listeners.tell(id, &message))
            .count();
        let unsubscribed = ids
            .into_iter()
            .filter(|&id| listeners.end_workspace(id, workspace_id, Ended::Access, Of::Any))
            .count();
        drop(listeners);
        self.count_told("workspaceRoleChanged", role_told);
        self.count_told("workspaceUnsubscribed", unsubscribed);
    }

    /// Stops every connection following the workspace. For when it goes to the trash:
    /// it is there for nobody, and who follows it once it is restored asks again. A
    /// connection that follows it among others keeps the others.
    pub fn stop_following_workspace(&self, workspace_id: &str) {
        let told = self.listeners().stop_following_workspace(workspace_id);
        self.count_told("workspaceUnsubscribed", told);
    }

    /// Stops what the connections that an organization shuts out follow of its
    /// workspaces, `workspace_ids`, as it begins to take only sessions made through
    /// the provider `provider_id`: those of a session made otherwise. For after that is
    /// stored. A subscription of one of them being checked meanwhile is checked again,
    /// against access read anew.
    pub fn end_subscriptions_shut_out(&self, workspace_ids: &[String], provider_id: &str) {
        self.end_workspaces_where(
            Ended::Sso,
            |revoked| {
                for workspace_id in workspace_ids {
                    revoked.workspace(workspace_id);
                }
            },
            |listener, workspace| shuts_out(listener, workspace, workspace_ids, provider_id),
        );
    }

    /// Stops what the user's connections follow of `workspace_id`, the workspace of
    /// their own space, with a session that has not shown the account's password, as
    /// the account is given its first one: the space takes no other session from then
    /// on. For after that is stored. A subscription of theirs being checked meanwhile
    /// is checked again, against access read anew.
    pub fn end_subscriptions_without_password(&self, workspace_id: &str, user_id: &str) {
        self.end_workspaces_where(
            Ended::Password,
            |revoked| revoked.user_in_workspace(user_id, workspace_id),
            |listener, workspace| {
                listener.user_id == user_id
                    && workspace == workspace_id
                    && listener
                        .made()
                        .is_none_or(|made| !made.has_shown_password())
            },
        );
    }

    /// As [`Self::end_subscriptions_shut_out`] where the organization's workspaces are
    /// not known: stops every workspace followed with a session that was not made
    /// through the provider, whichever. Blocks followed by a read token carry no user's
    /// identifier, and stay.
    pub fn end_subscriptions_made_otherwise(&self, provider_id: &str) {
        self.end_workspaces_where(Ended::Sso, Revocations::everyone, |listener, _| {
            listener
                .made()
                .is_none_or(|made| made.is_made_otherwise_than(provider_id))
        });
    }
}

/// Whether the wall of an organization that now takes only sessions made through
/// `provider_id` shuts the connection out of `workspace`: one of the organization's,
/// `workspace_ids`, followed with a session made otherwise, as the one rule has it
/// (`SessionMade::is_made_otherwise_than`).
pub(super) fn shuts_out(
    listener: &Listener,
    workspace: &str,
    workspace_ids: &[String],
    provider_id: &str,
) -> bool {
    workspace_ids.iter().any(|id| id == workspace)
        && listener
            .made()
            .is_none_or(|made| made.is_made_otherwise_than(provider_id))
}
