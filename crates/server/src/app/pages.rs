//! The pages of a wiki: their tree, making one, moving one, and the trash of them. A
//! page is a block, and is changed and deleted as one (`App::patch_block`,
//! `App::delete_block`).

use fvoci_model::{
    Block, BlockError, BlockType, Container, DeletedPage, NewPage, Page, PageDuplicate, PageMove,
    PageNode, PagePatch, PageTree,
};
use fvoci_store::{ByAgent, StoreError, Written};

use super::blocks::{ChangeAction, ChangeNotification, log_notification};
use super::changes::Changes;
use super::list_page::ListPage;
use super::{App, Reached};
use crate::error::{AppError, OrMissing, Reason};
use crate::permissions::ContainerPermission;

/// What a member of the wiki who may not edit it is refused making a page with.
pub const CREATE_PAGE_DENIED: &str = "access denied to create page";
/// What a member of the wiki who may not edit it is refused changing a page with.
pub const PATCH_PAGE_DENIED: &str = "access denied to patch page";
/// What a member of the wiki who may not edit it is refused moving a page with.
pub const MOVE_PAGE_DENIED: &str = "access denied to move page";
/// What a member of the wiki who may not edit it is refused copying a page with.
pub const DUPLICATE_PAGE_DENIED: &str = "access denied to duplicate page";
/// What a member of the wiki who may not edit it is refused deleting a page with.
pub const DELETE_PAGE_DENIED: &str = "access denied to delete page";
/// What a member of the wiki who may not edit it is refused its trash with.
pub const PAGE_TRASH_DENIED: &str = "access denied to the trash of the wiki";
/// What a member of the wiki who may not edit it is refused bringing a page back with.
pub const UNDELETE_PAGE_DENIED: &str = "access denied to undelete page";

/// Checks that the caller may edit the wiki `on` reached, refusing in `denial`, and
/// that `page`, which the request found by its identifier, is a page of it.
fn editing_page<'on>(
    on: &'on Reached,
    page: &Block,
    denial: &str,
) -> Result<&'on Container, AppError> {
    let wiki = on.require(ContainerPermission::ManageCards, denial)?;
    if on.holds(page) && page.block_type == BlockType::Page {
        Ok(wiki)
    } else {
        Err(AppError::not_found("page"))
    }
}

impl App {
    /// The page of the wiki, its block read once: a block that is no page of this wiki
    /// is no page, whatever else it is, and one in the trash is not there. Whether
    /// the asker may read the wiki is the caller's to have asked.
    ///
    /// # Errors
    ///
    /// Returns a not-found error for anything but a live page of the wiki, and the
    /// store's error if the block cannot be read.
    pub async fn page_of(&self, wiki_id: &str, page_id: &str) -> Result<Block, AppError> {
        const WHAT: &str = "page";
        let block = self.block(page_id).await.or_missing(WHAT)?;
        if block.container_id == wiki_id && block.block_type == BlockType::Page {
            Ok(block)
        } else {
            Err(AppError::not_found(WHAT))
        }
    }

    /// The pages of the wiki without their bodies, as their tree lists them.
    ///
    /// # Errors
    ///
    /// Returns the store's error if the pages cannot be read.
    pub async fn page_tree(&self, wiki_id: &str) -> Result<Vec<PageNode>, AppError> {
        Ok(self.store().get_page_tree(wiki_id).await?)
    }

    /// Makes a page of the wiki `on` reached as `new` says, for an editor of it, held to
    /// the organization's limits, and tells websocket listeners and webhooks in the
    /// background of it and of the pages numbered anew for it, as of any block that is
    /// written.
    ///
    /// # Errors
    ///
    /// Returns [`CREATE_PAGE_DENIED`] as [`Reached::require`] does, a not-found error if
    /// there is no such wiki, and a bad request if the request's shape is refused, the
    /// parent is no page of it, the page would be deeper or the wiki larger than the
    /// limits let them, the page to go after is no page of the same parent, or the
    /// title may not be stored.
    pub async fn create_page(&self, on: &Reached, new: &NewPage) -> Result<Page, AppError> {
        let wiki = on.require(ContainerPermission::ManageCards, CREATE_PAGE_DENIED)?;
        new.check_request()?;
        let (wiki_id, user_id) = (on.container_id(), on.user_id());
        let limits = self.limits().pages;
        let page = self
            .write_tree(wiki, ChangeAction::Add, async || {
                self.store()
                    .create_page(wiki_id, new, limits, user_id)
                    .await
            })
            .await?;
        Ok(Page::from(page))
    }

    /// Changes the page of the wiki `on` reached as `patch` says, for an editor of it:
    /// its title, its icon or its whole body, as the agent of `agent` if one writes.
    ///
    /// # Errors
    ///
    /// Returns [`PATCH_PAGE_DENIED`] as [`Reached::require`] does, a not-found error for
    /// a block that is no page of the wiki, a bad request if the patch's shape is refused
    /// or its body names a file the wiki does not have, and what `App::patch_block_as`
    /// returns.
    pub async fn patch_page(
        &self,
        on: &Reached,
        page: &Block,
        patch: PagePatch,
        agent: Option<ByAgent<'_>>,
    ) -> Result<Page, AppError> {
        editing_page(on, page, PATCH_PAGE_DENIED)?;
        patch.check_request()?;
        self.check_body_files(on.container_id(), patch.body.new_body())
            .await?;
        let patched = self
            .patch_block_as(
                &page.id,
                &patch.into_block_patch(),
                on.user_id(),
                ChangeNotification::Send,
                agent,
            )
            .await?;
        Ok(Page::from(patched))
    }

    /// Moves a page of the wiki `on` reached, with every page below it, where `to` says,
    /// for an editor of it, and tells websocket listeners and webhooks in the background
    /// of it and of the pages numbered anew for it. The pages below it are not told of:
    /// their parents stay.
    ///
    /// # Errors
    ///
    /// Returns [`MOVE_PAGE_DENIED`] as [`Reached::require`] does, a not-found error if
    /// the wiki has no such page, and a bad request if the parent is no page of the
    /// wiki, the page itself or one below it, or would leave a page deeper than the
    /// limit, or the page to go after is no other page there.
    pub async fn move_page(
        &self,
        on: &Reached,
        page: &Block,
        to: &PageMove,
    ) -> Result<Page, AppError> {
        let wiki = editing_page(on, page, MOVE_PAGE_DENIED)?;
        let (wiki_id, page_id, user_id) = (on.container_id(), page.id.as_str(), on.user_id());
        let limits = self.limits().pages;
        let page = self
            .write_tree(wiki, ChangeAction::Update, async || {
                self.store()
                    .move_page(wiki_id, page_id, to, limits, user_id)
                    .await
            })
            .await?;
        Ok(Page::from(page))
    }

    /// Copies a page of the wiki `on` reached with every page below it, for an editor of
    /// it, right after the page, the copy titled as `copy` says if it does; the bodies
    /// are copied, what is typed in them now included, and their images and file blocks
    /// get files of their own, as a card's duplicate's do. Their versions and comments
    /// are not copied. The wiki's members are told of each copy and each sibling
    /// numbered anew. Returns the copy of the page.
    ///
    /// # Errors
    ///
    /// Returns [`DUPLICATE_PAGE_DENIED`] as [`Reached::require`] does, a not-found error
    /// if the wiki has no such page, and a bad request if the title may not be stored or
    /// the copies would be deeper, or the wiki larger, than the organization's limits.
    pub async fn duplicate_page(
        &self,
        on: &Reached,
        page: &Block,
        copy: &PageDuplicate,
    ) -> Result<Page, AppError> {
        let wiki = editing_page(on, page, DUPLICATE_PAGE_DENIED)?;
        copy.check_request()?;
        let (wiki_id, page_id, user_id) = (on.container_id(), page.id.as_str(), on.user_id());
        let title = copy.title.as_deref();
        let limits = self.limits().pages;
        let family: Vec<String> = self
            .store()
            .get_page_tree(wiki_id)
            .await?
            .into_iter()
            .collect::<PageTree>()
            .below(page_id)
            .into_iter()
            .map(str::to_owned)
            .collect();
        // What is typed in the bodies now is copied too.
        self.collab().write_bodies(self, &family).await?;
        // Told of once the copies have their files, not as soon as they are stored.
        let Written {
            first,
            renumbered,
            below,
        } = {
            let _structure = self.collab().changing_structure(self, wiki_id).await;
            self.store()
                .duplicate_page(wiki_id, page_id, title, limits, user_id)
                .await
                .map_err(refused_without_parent)?
        };
        let mut copies: Vec<Block> = std::iter::once(first).chain(below).collect();
        self.give_copies_their_files(wiki_id, &mut copies).await;
        log_notification(ChangeNotification::Send, ChangeAction::Add, page_id);
        // The copy answered with the files it was given; one deleted meanwhile, with
        // every copy below it, is no longer there to answer.
        let page = copies
            .first()
            .cloned()
            .map(Page::from)
            .ok_or_else(|| AppError::not_found("page"))?;
        let told = renumbered.into_iter().chain(copies);
        self.tell(Changes::in_workspace(&wiki.workspace_id).changed(told));
        Ok(page)
    }

    /// Gives the images and file blocks of the copies files of their own, as a card's
    /// duplicate's are: the bodies pointed at them are part of the copy, not a change of
    /// it, so the copies begin without versions. The copies are stored whatever becomes of their files: a
    /// failure is logged, and the copies keep showing the files of the originals. Copies
    /// deleted meanwhile (with a page above them) are left out of `pages`, not to be told.
    async fn give_copies_their_files(&self, wiki_id: &str, pages: &mut Vec<Block>) {
        let mut showing: Vec<Block> = pages
            .iter()
            .filter(|page| {
                page.body
                    .as_ref()
                    .is_some_and(|body| !body.file_ids().is_empty())
            })
            .cloned()
            .collect();
        if showing.is_empty() {
            return;
        }
        self.copy_card_files_into(wiki_id, &mut showing).await;
        let bodies = showing
            .iter()
            .map(|page| (page.id.clone(), page.body.clone()))
            .collect();
        match self
            .store()
            .replace_bodies_as_written(wiki_id, bodies)
            .await
        {
            Ok(()) => {
                for page in pages.iter_mut() {
                    if let Some(renamed) = showing.iter_mut().find(|one| one.id == page.id) {
                        page.body = renamed.body.take();
                    }
                }
            }
            Err(error) => {
                tracing::error!(wiki_id, %error, "the copies of pages could not be given their files");
                if !error.is_not_found() {
                    return;
                }
                if let Ok(tree) = self.store().get_page_tree(wiki_id).await {
                    let live: PageTree = tree.into_iter().collect();
                    pages.retain(|page| live.contains(&page.id));
                }
            }
        }
    }

    /// Deletes a page of the wiki `on` reached, for an editor of it, with every page
    /// below it and the comments on them, as any block is deleted
    /// (`App::delete_block`).
    ///
    /// # Errors
    ///
    /// Returns [`DELETE_PAGE_DENIED`] as [`Reached::require`] does, a not-found error for
    /// a block that is no page of the wiki, and what `App::delete_block` returns.
    pub async fn delete_page(&self, on: &Reached, page: &Block) -> Result<(), AppError> {
        editing_page(on, page, DELETE_PAGE_DENIED)?;
        self.delete_block(&page.id, on.user_id(), ChangeNotification::Send)
            .await
    }

    /// The pages in the trash of the wiki `on` reached, for an editor of it, at the top
    /// of what went with them, one page of the list ([`ListPage`]).
    ///
    /// # Errors
    ///
    /// Returns [`PAGE_TRASH_DENIED`] as [`Reached::require`] does, and the store's error
    /// if the trash cannot be read.
    pub async fn deleted_pages(
        &self,
        on: &Reached,
        page: ListPage,
    ) -> Result<Vec<DeletedPage>, AppError> {
        on.require(ContainerPermission::ManageCards, PAGE_TRASH_DENIED)?;
        Ok(self
            .store()
            .get_deleted_pages(on.container_id(), Some(page.paging()))
            .await?)
    }

    /// Brings a page of the wiki `on` reached back from the trash, for an editor of it,
    /// with the pages that went with it, held to the organization's limits, and tells
    /// websocket listeners and webhooks in the background of each page that came back
    /// and each numbered anew for it.
    ///
    /// # Errors
    ///
    /// Returns [`UNDELETE_PAGE_DENIED`] as [`Reached::require`] does, a not-found error
    /// if the wiki's trash has no such page, and a bad request if the pages would be
    /// deeper, or the wiki larger, than the limits.
    pub async fn undelete_page(&self, on: &Reached, page_id: &str) -> Result<Page, AppError> {
        let wiki = on.require(ContainerPermission::ManageCards, UNDELETE_PAGE_DENIED)?;
        let (wiki_id, user_id) = (on.container_id(), on.user_id());
        let limits = self.limits().pages;
        let page = self
            .write_tree(wiki, ChangeAction::Add, async || {
                self.store()
                    .undelete_page(wiki_id, page_id, limits, user_id)
                    .await
            })
            .await
            .map_err(refused_without_parent)?;
        Ok(Page::from(page))
    }
}

/// What a write of pages whose request names no parent (a restore, a copy) is refused
/// with: a page that is not there is 404, and the rest as [`refused_without_member`].
fn refused_without_parent(error: StoreError) -> AppError {
    if error.is_not_found() {
        return AppError::not_found("page");
    }
    refused_without_member(error)
}

/// What a write of pages that names no parent (a copy of a page or a wiki, a restore)
/// is refused with when the store refuses it: pages that would be too deep name no
/// member of the request.
pub(super) fn refused_without_member(error: StoreError) -> AppError {
    match error {
        StoreError::InvalidBlock {
            source: source @ BlockError::PageTooDeep { limit },
            ..
        } => AppError::bad_request(source)
            .with_reason(Reason::PageTooDeep)
            .with_limit(u64::from(limit)),
        error => error.into(),
    }
}
