//! Authorization at dispatch, backed by the store's committed visibility revision.

use fvoci_model::{ContainerMember, WorkspaceRole};

use super::ending::{Ended, Of};
use super::{CHECKED_AT_MOST, Hub, ListenerId, Outgoing, ReadPermit, UpdateBlockMessage, text_of};
use crate::app::SessionMade;
use crate::metrics::Disconnect;
use crate::permissions::{ContainerScope, told_without_a_role};

#[derive(Clone, Copy)]
pub(super) enum Access<'a> {
    Workspace(&'a str),
    /// Who follows the workspace and reads the board (`readers`), and `also`.
    Members {
        workspace: &'a str,
        readers: Readers<'a>,
        also: Option<&'a str>,
    },
    Public(&'a str),
    /// Who follows the workspace and sees `person`, a member of it, in it.
    Seers {
        workspace: &'a str,
        person: &'a str,
    },
    All,
}

/// Who reads a board, for what is told of it to its readers alone: its members, and of
/// an open wiki whoever works in its workspace ([`told_without_a_role`]).
#[derive(Clone, Copy)]
pub(super) enum Readers<'a> {
    /// The board as it is when the change is sent: its members and its scope are read
    /// then, the scope only if it is a board of the workspace the change is told in.
    Now(&'a str),
    /// A board that is gone, as it was: its members and its scope, of the workspace the
    /// change is told in.
    Former {
        members: &'a [ContainerMember],
        scope: ContainerScope,
    },
}

/// What [`Readers`] stand for when a change is checked: the members, and the scope,
/// `None` for a board that is not there (whose members alone are told).
#[derive(Default)]
struct Known {
    members: Vec<ContainerMember>,
    scope: Option<ContainerScope>,
}

impl Known {
    fn reads(&self, user: &str, role: WorkspaceRole) -> bool {
        self.members.iter().any(|member| member.user_id == user)
            || self
                .scope
                .is_some_and(|scope| told_without_a_role(scope, role))
    }
}

impl Hub {
    pub(super) async fn send(
        &self,
        ids: &[ListenerId],
        message: &impl Outgoing,
        access: Access<'_>,
    ) {
        if ids.is_empty() {
            return;
        }
        // Made once, before any revision is held: queueing under it only hands it on.
        let Some(text) = text_of(message) else {
            return;
        };
        let kind = message.kind();
        // What every connection is told depends on no one's access.
        if matches!(access, Access::All) {
            self.queue_text(ids, &text, kind);
            return;
        }
        let mut authorized = Vec::new();
        for _ in 0..CHECKED_AT_MOST {
            // Not cacheable for a while after an abandoned write: the checks below
            // then read the store anew.
            let revision = self.audience.version().await;
            authorized = self.authorized(ids, access).await;
            #[cfg(feature = "test-util")]
            self.pause_dispatch().await;
            if self.audience.if_current(revision, || {
                self.queue_text(&authorized, &text, kind);
            }) {
                return;
            }
        }
        // Who was to be told at the last check: ask them to resync without dropping the socket.
        self.give_up(&authorized, kind, access);
    }

    /// Tells who follows the block's board by its read token of the block: who sees
    /// the board as its member does ([`Self::split_by_sight`]) as a member is told it,
    /// whoever reads it by the link alone without the users it names, and without what
    /// its body names that is no card of the board. If what to leave out cannot be
    /// read, those readers are closed and not told.
    pub(super) async fn send_public_block(
        &self,
        ids: &[ListenerId],
        message: &UpdateBlockMessage<'_>,
    ) {
        if ids.is_empty() {
            return;
        }
        let workspace_id = message.workspace_id();
        // A card's, a page's or an entry's without its body: what that body names is not looked up.
        let block = message.block();
        let Some(whole) = text_of(message) else {
            return;
        };
        let mut authorized = Vec::new();
        for _ in 0..CHECKED_AT_MOST {
            // Not cacheable for a while after an abandoned write: the checks below
            // then read the store anew.
            let revision = self.audience.version().await;
            authorized = self
                .authorized(ids, Access::Public(&block.container_id))
                .await;
            if authorized.is_empty() {
                return;
            }
            let (members, mut readers) = self
                .split_by_sight(&authorized, workspace_id, &block.container_id)
                .await;
            let hidden = if readers.is_empty() {
                None
            } else {
                match self.audience.unsaid_by_link(block).await {
                    Ok(unsaid) => {
                        // A card's, a page's or an entry's carries no body to copy here.
                        let mut public = message.clone();
                        let shown = public.block_mut();
                        shown.hide_people(&unsaid.people);
                        shown.clear_references_but(&unsaid.shown);
                        text_of(&public)
                    }
                    Err(error) => {
                        tracing::error!(
                            %error,
                            container_id = %block.container_id,
                            "websocket: cannot redact public block"
                        );
                        self.close_unread(&readers);
                        readers.clear();
                        None
                    }
                }
            };
            #[cfg(feature = "test-util")]
            self.pause_dispatch().await;
            if self.audience.if_current(revision, || {
                self.queue_text(&members, &whole, "blockChanged");
                if let Some(hidden) = &hidden {
                    self.queue_text(&readers, hidden, "blockChanged");
                }
            }) {
                return;
            }
        }
        self.give_up(
            &authorized,
            "blockChanged",
            Access::Public(&block.container_id),
        );
    }

    /// Splits who follows the board by its read token into who sees it as its member
    /// does, and who reads it by the link alone. A member is a connection of a user who
    /// holds a role on the board, is in its workspace, and whose session opens the
    /// workspace's organization; anybody else, and a connection without a session,
    /// reads by the link. A connection with a session whose standing cannot be read now
    /// is closed as retryable: it reads anew rather than be told less than it may.
    async fn split_by_sight(
        &self,
        ids: &[ListenerId],
        workspace_id: &str,
        container_id: &str,
    ) -> (Vec<ListenerId>, Vec<ListenerId>) {
        let identities: Vec<(ListenerId, String, Option<SessionMade>)> = {
            let listeners = self.listeners();
            ids.iter()
                .filter_map(|id| {
                    let listener = listeners.by_id.get(id)?;
                    Some((*id, listener.user_id.clone(), listener.made()))
                })
                .collect()
        };
        let (mut members, mut readers) = (Vec::new(), Vec::new());
        let mut signed_in: Vec<(ListenerId, String, SessionMade)> = Vec::new();
        for (id, user, made) in identities {
            match made {
                Some(made) if !user.is_empty() => signed_in.push((id, user, made)),
                _ => readers.push(id),
            }
        }
        if signed_in.is_empty() {
            return (members, readers);
        }
        let container_members = match self.audience.members(container_id).await {
            Ok(container_members) => container_members,
            Err(error) => {
                tracing::error!(
                    %error,
                    container_id,
                    "websocket: reading the members of a shared board failed"
                );
                let unread: Vec<ListenerId> = signed_in.iter().map(|(id, _, _)| *id).collect();
                self.close_unread(&unread);
                return (members, readers);
            }
        };
        for (id, user, making) in signed_in {
            let on_container = container_members
                .iter()
                .any(|member| member.user_id == user);
            if !on_container {
                readers.push(id);
                continue;
            }
            match self.audience.workspace_access(&user, workspace_id).await {
                Ok(Some(access)) if access.takes.opened_by(making.making()) => {
                    members.push(id);
                }
                Ok(_) => readers.push(id),
                Err(error) => {
                    tracing::error!(%error, workspace_id, "websocket: reading a workspace's membership failed");
                    self.close_unread(&[id]);
                }
            }
        }
        (members, readers)
    }

    /// Ends the affected subscription on recipients whose check was overtaken by
    /// permission writes every time so their client re-subscribes and reloads without
    /// dropping the entire WebSocket connection.
    fn give_up(&self, ids: &[ListenerId], message_type: &str, access: Access<'_>) {
        tracing::warn!(
            message_type,
            recipients = ids.len(),
            "websocket: a change was given up, checked again too often"
        );
        for id in ids {
            match access {
                Access::Workspace(workspace)
                | Access::Members { workspace, .. }
                | Access::Seers { workspace, .. } => {
                    self.end_workspace(*id, workspace, Ended::Access, Of::Any);
                }
                Access::Public(container) => {
                    self.end_container(*id, container);
                }
                Access::All => {
                    self.close_listener(*id, Disconnect::ChecksOvertaken);
                }
            }
        }
    }

    /// Closes connections that a change could not be checked for, as retryable
    /// (`StoreUnavailable`): a page connects again and reads anew, rather than miss
    /// the change unknowingly. Their access did not end.
    pub(super) fn close_unread(&self, ids: &[ListenerId]) {
        for id in ids {
            self.close_listener(*id, Disconnect::StoreUnavailable);
        }
    }

    async fn authorized(&self, ids: &[ListenerId], access: Access<'_>) -> Vec<ListenerId> {
        let known = if let Access::Members {
            workspace, readers, ..
        } = access
        {
            let Some(known) = self.known(workspace, readers).await else {
                self.close_unread(ids);
                return Vec::new();
            };
            known
        } else {
            Known::default()
        };
        let mut authorized = Vec::new();
        for id in ids {
            if self.authorizes(*id, access, &known).await {
                authorized.push(*id);
            }
        }
        authorized
    }

    /// What `readers` stand for now in `workspace`; `None`, logged, if it cannot be read.
    /// A board of another workspace is told to its members alone: what it is there for
    /// besides them is of its own workspace.
    async fn known(&self, workspace: &str, readers: Readers<'_>) -> Option<Known> {
        match readers {
            Readers::Former { members, scope } => Some(Known {
                members: members.to_vec(),
                scope: Some(scope),
            }),
            Readers::Now(container) => {
                let members = self
                    .audience
                    .members(container)
                    .await
                    .inspect_err(|error| unread(container, error))
                    .ok()?;
                let scope = self
                    .audience
                    .container_scope(workspace, container)
                    .await
                    .inspect_err(|error| unread(container, error))
                    .ok()?;
                Some(Known { members, scope })
            }
        }
    }

    async fn authorizes(&self, id: ListenerId, access: Access<'_>, known: &Known) -> bool {
        let following = {
            let listeners = self.listeners();
            let Some(listener) = listeners.by_id.get(&id) else {
                return false;
            };
            match access {
                Access::All => return true,
                Access::Public(container) => listener
                    .shared_containers
                    .iter()
                    .find(|permit| permit.container == container)
                    .map(|permit| Following::Link(permit.clone())),
                Access::Workspace(workspace) => following_workspace(listener, workspace, Told::All),
                Access::Members {
                    workspace, also, ..
                } => following_workspace(listener, workspace, Told::Readers { also }),
                Access::Seers { workspace, person } => {
                    following_workspace(listener, workspace, Told::Seers { person })
                }
            }
        };
        match following {
            None => false,
            Some(Following::Link(permit)) => self.link_opens(id, &permit).await,
            Some(Following::Workspace {
                workspace,
                user,
                role,
                making,
                told,
            }) => {
                if !self
                    .still_in_workspace(id, workspace, &user, role, &making)
                    .await
                {
                    return false;
                }
                match told {
                    Told::All => true,
                    Told::Readers { also } => {
                        also == Some(user.as_str()) || known.reads(&user, role)
                    }
                    Told::Seers { person } => self.sees(id, &user, workspace, role, person).await,
                }
            }
        }
    }

    /// Whether the board's read token still opens it for the connection; one whose
    /// link opens nothing now is ended and told.
    async fn link_opens(&self, id: ListenerId, permit: &ReadPermit) -> bool {
        match self
            .audience
            .shared_board(&permit.container, &permit.token)
            .await
        {
            Ok(true) => true,
            Ok(false) => {
                self.end_container(id, &permit.container);
                false
            }
            // The link may still open: not "ended", but not to miss the change.
            Err(error) => {
                tracing::error!(
                    %error,
                    container_id = %permit.container,
                    "websocket: reading a board's sharing failed"
                );
                self.close_unread(&[id]);
                false
            }
        }
    }

    /// Whether `user`, in `role` in the workspace, sees `person`, a member of it, by name;
    /// one whose sight cannot be read now is closed as retryable, and reads anew.
    async fn sees(
        &self,
        id: ListenerId,
        user: &str,
        workspace: &str,
        role: WorkspaceRole,
        person: &str,
    ) -> bool {
        match self
            .audience
            .sees_member(user, workspace, role, person)
            .await
        {
            Ok(sees) => sees,
            Err(error) => {
                tracing::error!(%error, workspace_id = workspace, "websocket: reading whom a user sees failed");
                self.close_unread(&[id]);
                false
            }
        }
    }

    /// Whether `user` is still in the workspace with the role the connection follows
    /// it with, by a session that opens its organization. What ended is ended only if
    /// the connection still follows it with that role (a subscription made since is
    /// not), and told; what cannot be read closes the connection as retryable.
    async fn still_in_workspace(
        &self,
        id: ListenerId,
        workspace: &str,
        user: &str,
        role: WorkspaceRole,
        making: &SessionMade,
    ) -> bool {
        let checked = Of::Role(role);
        let current = match self.audience.workspace_access(user, workspace).await {
            Ok(Some(current)) => current,
            Ok(None) => {
                self.end_workspace(id, workspace, Ended::Access, checked);
                return false;
            }
            // Access that cannot be read has not ended, but the change is not to be
            // missed either.
            Err(error) => {
                tracing::error!(%error, workspace_id = workspace, "websocket: reading a workspace's membership failed");
                self.close_unread(&[id]);
                return false;
            }
        };
        if current.workspace_role != role {
            self.end_workspace(id, workspace, Ended::Access, checked);
            return false;
        }
        if !current.takes.opened_by(making.making()) {
            let ended = if current.takes.asks_for_password() {
                Ended::Password
            } else {
                Ended::Sso
            };
            self.end_workspace(id, workspace, ended, checked);
            return false;
        }
        true
    }
}

/// What a connection follows that a change is checked against.
enum Following<'a> {
    /// The board, by its read token.
    Link(ReadPermit),
    /// The workspace, as `user` with `role`, by a session of that `making`.
    Workspace {
        workspace: &'a str,
        user: String,
        role: WorkspaceRole,
        making: SessionMade,
        told: Told<'a>,
    },
}

/// Who of those that follow a workspace is told a change of it.
#[derive(Clone, Copy)]
enum Told<'a> {
    All,
    /// Only the board's readers ([`Readers`]), and `also` (who made it).
    Readers {
        also: Option<&'a str>,
    },
    /// Only who sees `person`, a member of the workspace, in it.
    Seers {
        person: &'a str,
    },
}

/// Logs that who reads `container` could not be read for a change of it.
fn unread(container: &str, error: &impl std::fmt::Display) {
    tracing::error!(%error, container_id = container, "websocket: cannot read dispatch membership");
}

/// What `listener` follows of `workspace`, if it does.
fn following_workspace<'a>(
    listener: &super::Listener,
    workspace: &'a str,
    told: Told<'a>,
) -> Option<Following<'a>> {
    let role = listener.workspace_roles.get(workspace)?;
    let making = listener.made()?;
    Some(Following::Workspace {
        workspace,
        user: listener.user_id.clone(),
        role: *role,
        making,
        told,
    })
}
