//! The pages of a wiki: their tree, making one, moving one, and the trash of them. A
//! page is a block, and is changed and deleted as one ([`App::patch_block`],
//! [`App::delete_block`]).

use fvoci_model::{
    Block, BlockError, BlockType, DeletedPage, NewPage, Page, PageMove, PageNode, PageTree,
};
use fvoci_store::{PageCopies, PageRestore, PageWrite, StoreError};

use super::App;
use super::blocks::{ChangeAction, ChangeNotification, log_notification};
use super::list_page::ListPage;
use crate::error::{AppError, Reason};

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
        match self.block(page_id).await {
            Ok(block) if block.container_id == wiki_id && block.block_type == BlockType::Page => {
                Ok(block)
            }
            Ok(_) => Err(AppError::not_found(WHAT)),
            Err(error) if error.is_not_found() => Err(AppError::not_found(WHAT)),
            Err(error) => Err(error),
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

    /// Makes a page of the wiki as `new` says, held to the organization's limits, and
    /// tells websocket listeners and webhooks in the background of it and of the pages
    /// numbered anew for it, as of any block that is written.
    ///
    /// # Errors
    ///
    /// Returns a not-found error if there is no such wiki, and a bad request if the
    /// parent is no page of it, the page would be deeper or the wiki larger than the
    /// limits let them, the page to go after is no page of the same parent, or the
    /// title may not be stored.
    pub async fn create_page(
        &self,
        wiki_id: &str,
        new: &NewPage,
        user_id: &str,
    ) -> Result<Page, AppError> {
        let wiki = self.store().get_container(wiki_id).await?;
        let limits = self.limits().pages;
        let written = {
            let _structure = self.collab().changing_structure(self, wiki_id).await;
            self.store()
                .create_page(wiki_id, new, limits, user_id)
                .await?
        };
        log_notification(
            ChangeNotification::Send,
            ChangeAction::Add,
            &written.page.id,
        );
        Ok(self.announce_pages(&wiki.workspace_id, written))
    }

    /// Moves a page of the wiki, with every page below it, where `to` says, and tells
    /// websocket listeners and webhooks in the background of it and of the pages
    /// numbered anew for it. The pages below it are not told of: their parents stay.
    ///
    /// # Errors
    ///
    /// Returns a not-found error if the wiki has no such page, and a bad request if the
    /// parent is no page of the wiki, the page itself or one below it, or would leave a
    /// page deeper than the limit, or the page to go after is no other page there.
    pub async fn move_page(
        &self,
        wiki_id: &str,
        page_id: &str,
        to: &PageMove,
        user_id: &str,
    ) -> Result<Page, AppError> {
        let wiki = self.store().get_container(wiki_id).await?;
        let limits = self.limits().pages;
        let written = {
            let _structure = self.collab().changing_structure(self, wiki_id).await;
            self.store()
                .move_page(wiki_id, page_id, to, limits, user_id)
                .await?
        };
        log_notification(ChangeNotification::Send, ChangeAction::Update, page_id);
        Ok(self.announce_pages(&wiki.workspace_id, written))
    }

    /// Copies a page of the wiki with every page below it, right after the page, the copy
    /// titled `title` if given; the bodies are copied, what is typed in them now
    /// included, and their images and file blocks get files of their own, as a card's
    /// duplicate's do. Their versions and comments are not copied. The wiki's members are told of
    /// each copy and each sibling numbered anew. Returns the copy of the page.
    ///
    /// # Errors
    ///
    /// Returns a not-found error if the wiki has no such page, and a bad request if the
    /// copies would be deeper, or the wiki larger, than the organization's limits.
    pub async fn duplicate_page(
        &self,
        wiki_id: &str,
        page_id: &str,
        title: Option<&str>,
        user_id: &str,
    ) -> Result<Page, AppError> {
        let wiki = self.store().get_container(wiki_id).await?;
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
        let PageCopies {
            mut pages,
            renumbered,
        } = {
            let _structure = self.collab().changing_structure(self, wiki_id).await;
            self.store()
                .duplicate_page(wiki_id, page_id, title, limits, user_id)
                .await
                .map_err(refused_without_parent)?
        };
        self.give_copies_their_files(wiki_id, &mut pages).await;
        log_notification(ChangeNotification::Send, ChangeAction::Add, page_id);
        let page = pages
            .first()
            .cloned()
            .map(Page::from)
            .ok_or_else(|| AppError::not_found("page"))?;
        let mut told = renumbered;
        told.extend(pages);
        self.announce(&wiki.workspace_id, told);
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

    /// The wiki's pages in the trash at the top of what went with them, one page of the
    /// list ([`ListPage`]).
    ///
    /// # Errors
    ///
    /// Returns the store's error if the trash cannot be read.
    pub async fn deleted_pages(
        &self,
        wiki_id: &str,
        page: ListPage,
    ) -> Result<Vec<DeletedPage>, AppError> {
        Ok(self
            .store()
            .get_deleted_pages(wiki_id, Some(page.paging()))
            .await?)
    }

    /// Brings a page of the wiki back from the trash with the pages that went with it,
    /// held to the organization's limits, and tells websocket listeners and webhooks
    /// in the background of each page that came back and each numbered anew for it.
    ///
    /// # Errors
    ///
    /// Returns a not-found error if the wiki's trash has no such page, and a bad
    /// request if the pages would be deeper, or the wiki larger, than the limits.
    pub async fn undelete_page(
        &self,
        wiki_id: &str,
        page_id: &str,
        user_id: &str,
    ) -> Result<Page, AppError> {
        let wiki = self.store().get_container(wiki_id).await?;
        let limits = self.limits().pages;
        let restored = {
            let _structure = self.collab().changing_structure(self, wiki_id).await;
            self.store()
                .undelete_page(wiki_id, page_id, limits, user_id)
                .await
        };
        let PageRestore { pages, renumbered } = restored.map_err(refused_without_parent)?;
        log_notification(ChangeNotification::Send, ChangeAction::Add, page_id);
        let page = pages
            .first()
            .cloned()
            .map(Page::from)
            .ok_or_else(|| AppError::not_found("page"))?;
        let mut told = renumbered;
        told.extend(pages);
        self.announce(&wiki.workspace_id, told);
        Ok(page)
    }

    /// Tells websocket listeners and webhooks in the background of the pages written,
    /// the siblings numbered anew first, and returns the page the write was about.
    fn announce_pages(&self, workspace_id: &str, written: PageWrite) -> Page {
        let PageWrite { page, renumbered } = written;
        let mut told: Vec<Block> = renumbered;
        told.push(page.clone());
        self.announce(workspace_id, told);
        Page::from(page)
    }

    /// Tells websocket listeners and webhooks in the background of the pages, in turn.
    fn announce(&self, workspace_id: &str, told: Vec<Block>) {
        let app = self.clone();
        let workspace_id = workspace_id.to_owned();
        self.spawn_background(async move {
            for block in &told {
                app.ws().broadcast_block_change(&workspace_id, block).await;
                app.tell_webhooks(block);
            }
        });
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
