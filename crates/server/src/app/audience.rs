//! What the websocket hub asks about boards, to know whom a change is told to and how
//! much of it.

mod cache;

use std::collections::HashSet;
use std::sync::Arc;

use cache::Cache;
use fvoci_model::{
    Block, BlockType, Body, ContainerKind, ContainerMember, PersonProperties, WorkspaceRole,
};
use fvoci_store::{
    Looking, Making, NotificationReader, Sight, Store, StoreError, Tenant, VisibilityVersion,
    WorkspaceAccess,
};

use crate::permissions::{ContainerScope, people_reach};

/// The boards as the websocket hub asks about them: who the members of a board are,
/// which of its properties name users, and which board a block is of. The hub is made
/// before the [`App`](super::App) that holds it, and so asks here.
///
/// An error is the store's own, for the hub's log; none of it is sent to a client.
#[derive(Debug, Clone)]
pub struct Audience {
    store: Store,
    cache: Arc<Caches>,
}

#[derive(Debug, Default)]
struct Caches {
    workspaces: Cache<(String, String), Option<WorkspaceAccess>>,
    members: Cache<(Tenant, String), Vec<ContainerMember>>,
    people: Cache<(Tenant, String), PersonProperties>,
    scopes: Cache<(Tenant, String), Option<(String, ContainerScope)>>,
    shares: Cache<(String, String), bool>,
}

impl Audience {
    /// The audience of the boards in `store`.
    pub fn new(store: Store) -> Self {
        Self {
            store,
            cache: Arc::default(),
        }
    }

    pub(super) fn with_store(&self, store: Store) -> Self {
        Self {
            store,
            cache: Arc::clone(&self.cache),
        }
    }

    /// The organization this audience asks about, if its handle names one.
    pub fn org_id(&self) -> Option<String> {
        self.store.tenant().org_id()
    }

    /// The committed revision used to validate cached dispatch permissions.
    pub async fn version(&self) -> VisibilityVersion {
        self.store.visibility_version().await
    }

    /// Queues a checked dispatch atomically with respect to authorization writes.
    pub fn if_current(&self, version: VisibilityVersion, send: impl FnOnce()) -> bool {
        self.store.if_visibility_current(version, send)
    }

    /// Whether a previously admitted public subscription still has a valid token: the
    /// subscriptions of a link follow a board's blocks, which a wiki's token does not
    /// open.
    ///
    /// # Errors
    ///
    /// Returns the store's error if the sharing cannot be read: that is no "no".
    pub async fn shared_board(&self, board_id: &str, read_token: &str) -> Result<bool, StoreError> {
        self.cache
            .shares
            .read(
                &self.store,
                (board_id.to_owned(), read_token.to_owned()),
                || async {
                    Ok(self
                        .store
                        .shared_container(board_id, read_token)
                        .await?
                        .is_some_and(|shared| shared.kind == ContainerKind::Board))
                },
            )
            .await
    }

    /// The board of a block in the named workspace, if its public read token opens it.
    ///
    /// # Errors
    ///
    /// Returns the store's error if either lookup fails.
    pub async fn board_of_shared_block(
        &self,
        workspace_id: &str,
        block_id: &str,
        read_token: &str,
    ) -> Result<Option<String>, StoreError> {
        self.store
            .board_of_shared_block(workspace_id, block_id, read_token)
            .await
    }

    /// What the user is in the workspace, a guest too (`App::actor`), with the provider
    /// a session must be made through to open the workspace's organization; `None` for
    /// who is no member. What a subscription of the workspace is told of a board goes
    /// to the board's members only, so a guest hears the boards they were invited to
    /// and no other.
    ///
    /// Cached at the visibility revision of this audience's organization, which a
    /// change of a membership or of the organization's provider moves on. So the
    /// workspace must be of that organization: the one of the change being sent,
    /// which is written through this audience's store. A workspace of any organization
    /// is asked with [`Self::current_workspace_access`].
    ///
    /// # Errors
    ///
    /// Returns the store's error if the membership cannot be read: that is no "not a
    /// member".
    pub async fn workspace_access(
        &self,
        user_id: &str,
        workspace_id: &str,
    ) -> Result<Option<WorkspaceAccess>, StoreError> {
        self.cache
            .workspaces
            .read(
                &self.store,
                (user_id.to_owned(), workspace_id.to_owned()),
                || async { self.store.resolve_workspace(user_id, workspace_id).await },
            )
            .await
    }

    /// As [`Self::workspace_access`], read from the store and not cached: for what is
    /// asked of a workspace of any organization (a subscription, what is left of a
    /// user's after their roles change), which no one organization's revision covers.
    ///
    /// # Errors
    ///
    /// As [`Self::workspace_access`].
    pub async fn current_workspace_access(
        &self,
        user_id: &str,
        workspace_id: &str,
    ) -> Result<Option<WorkspaceAccess>, StoreError> {
        self.store.resolve_workspace(user_id, workspace_id).await
    }

    /// Whether the user `seer_id`, in `role` in the workspace, sees `person_id`, a
    /// member of it, by name (`permissions::sees_person`). Whoever works there sees
    /// each of its members; a guest is asked of the store, as for any other look at
    /// the person, and not cached, for a change of a name is rare.
    ///
    /// # Errors
    ///
    /// Returns the store's error if whom the guest sees cannot be read.
    pub async fn sees_member(
        &self,
        seer_id: &str,
        workspace_id: &str,
        role: WorkspaceRole,
        person_id: &str,
    ) -> Result<bool, StoreError> {
        if role != WorkspaceRole::Guest || seer_id == person_id {
            return Ok(true);
        }
        let sight = Sight::InWorkspace {
            seer_id,
            workspace_id,
            reach: people_reach(role),
            looking: Looking::ByName,
        };
        let seen = self
            .store
            .get_users_in_sight(sight, &[person_id.to_owned()])
            .await?;
        Ok(!seen.is_empty())
    }

    /// The members of the board.
    ///
    /// # Errors
    ///
    /// Returns the store's error if the query fails.
    pub async fn members(&self, container_id: &str) -> Result<Vec<ContainerMember>, StoreError> {
        self.cache
            .members
            .read(
                &self.store,
                (self.store.tenant(), container_id.to_owned()),
                || async { self.store.effective_container_members(container_id).await },
            )
            .await
    }

    /// Who the board is there for besides its members ([`ContainerScope::of`]), if it is a
    /// board of the workspace `workspace_id`; `None` for a board of another workspace,
    /// and for one that is not there, in the trash among them.
    ///
    /// # Errors
    ///
    /// Returns the store's error if the board cannot be read.
    pub async fn container_scope(
        &self,
        workspace_id: &str,
        container_id: &str,
    ) -> Result<Option<ContainerScope>, StoreError> {
        let placed = self
            .cache
            .scopes
            .read(
                &self.store,
                (self.store.tenant(), container_id.to_owned()),
                || async {
                    match self.store.get_container(container_id).await {
                        Ok(container) => Ok(Some((
                            container.workspace_id.clone(),
                            ContainerScope::of(&container),
                        ))),
                        Err(error) if error.is_not_found() => Ok(None),
                        Err(error) => Err(error),
                    }
                },
            )
            .await?;
        Ok(placed
            .filter(|(of, _)| of == workspace_id)
            .map(|(_, scope)| scope))
    }

    /// The properties of the board's cards whose values name users.
    ///
    /// # Errors
    ///
    /// Returns the store's error if there is no such board, or it cannot be read.
    pub async fn person_properties(
        &self,
        container_id: &str,
    ) -> Result<PersonProperties, StoreError> {
        self.cache
            .people
            .read(
                &self.store,
                (self.store.tenant(), container_id.to_owned()),
                || async {
                    Ok(self
                        .store
                        .get_container(container_id)
                        .await?
                        .person_properties())
                },
            )
            .await
    }

    /// What whoever reads the block's board by its link is not told of the block: the
    /// person properties of the board ([`Self::person_properties`]), and the cards of
    /// the board among what the block's body names, which are all that reader is shown
    /// of the references of the body ([`Block::clear_references_but`]). The cards are read
    /// anew each time, and only for a body that names something: a card that comes or
    /// goes changes no version of the audience.
    ///
    /// # Errors
    ///
    /// Returns the store's error if either cannot be read: the block is not sent then.
    pub async fn unsaid_by_link(&self, block: &Block) -> Result<UnsaidByLink, StoreError> {
        let people = self.person_properties(&block.container_id).await?;
        let named = block
            .body
            .as_ref()
            .map(Body::reference_ids)
            .unwrap_or_default();
        let shown = self
            .store
            .blocks_on_container_among(&block.container_id, BlockType::Card, &named)
            .await?;
        Ok(UnsaidByLink { people, shown })
    }

    /// Whether what the organization `org_id` told `user_id` is kept from a session
    /// of theirs made as `making` says: as the list of their notifications keeps it.
    ///
    /// # Errors
    ///
    /// Returns the store's error if the query fails.
    pub async fn notifications_kept_from(
        &self,
        user_id: &str,
        making: Making<&str>,
        org_id: &str,
    ) -> Result<bool, StoreError> {
        self.store
            .notifications_kept_from(NotificationReader { user_id, making }, org_id)
            .await
    }
}

/// Who read a board that is gone: its members when it went, and who it was there for
/// besides them (an open wiki is read by whoever works in its workspace), of the
/// workspace the change is told in.
#[derive(Debug, Clone, Copy)]
pub struct Former<'a> {
    /// The board's members, as they were.
    pub members: &'a [ContainerMember],
    /// Who the board was there for besides its members.
    pub scope: ContainerScope,
}

/// What a block is sent without to whoever reads its board by the link alone
/// ([`Audience::unsaid_by_link`]).
#[derive(Debug)]
pub struct UnsaidByLink {
    /// The person properties of the board, whose values go out empty.
    pub people: PersonProperties,
    /// The cards of the board among what the body names: every other reference goes
    /// out without its identifier.
    pub shown: HashSet<String>,
}
