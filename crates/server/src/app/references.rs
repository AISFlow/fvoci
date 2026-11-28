//! References: the card or the page a body names, as each reader sees it, and the view
//! of a board a board view of a body shows.
//!
//! A body holds the identifier of what it names and nothing else, and whoever wrote it
//! was asked nothing about it. So everything is decided here, for the one who reads and
//! as things are now: what a reference names is told to whoever reads the board or the
//! wiki it is on ([`Grant::reads`](crate::permissions::Grant::reads), asked of
//! [`App::container_access`] as every route to a board asks), in the workspace of the
//! request. To anybody else it is what an identifier of nothing is: left out, with the
//! same reads made on the way.

use std::collections::{BTreeMap, HashMap, HashSet};

use fvoci_model::{
    Block, BlockType, Body, Container, LinkedBlock, LinkedKind, PublicContainers, is_id,
};

use super::App;
use super::list_page::ListPage;
use crate::error::AppError;

/// The most references one request resolves: each is a question of access of its own.
pub const MAX_REFERENCE_IDS: usize = 100;

/// The most cards and pages a search for what a reference may name finds.
const FOUND_AT_MOST: usize = 20;

/// The longest term a search for what a reference may name takes, in characters: far
/// past a title typed after `[[`, and short enough that a term that finds nothing does
/// not make the reading of every title the work of many words.
const MAX_TITLE_TERM_CHARS: usize = 200;

impl App {
    /// What the references to `ids` show the user: the cards and pages among them that
    /// are on a board or a wiki of this handle's workspace ([`App::in_workspace`]) the
    /// user reads, each once, in the order they were first asked for; and the views of
    /// those boards, as a board view of a body shows one ([`LinkedBlock::view_of`]).
    ///
    /// Whatever else an identifier names is left out and nothing says why: no block,
    /// one in the trash, one of another workspace or organization, one of a board the
    /// user does not read, or a block that is neither a card, a page, a unit nor a
    /// view. A private board's view is told to its members alone, as its cards are. Access is
    /// asked once per distinct container (or once for an unknown identifier), caching
    /// the answer within this call so resolving many blocks on the same board does not
    /// repeat identical container access lookups.
    ///
    /// # Errors
    ///
    /// Returns 400 for more than [`MAX_REFERENCE_IDS`] identifiers and for an entry
    /// that has not the form of one (naming its place in the list), and the store's
    /// error if a read fails.
    pub async fn resolve_references(
        &self,
        user_id: &str,
        ids: &[String],
    ) -> Result<Vec<LinkedBlock>, AppError> {
        if ids.len() > MAX_REFERENCE_IDS {
            return Err(AppError::bad_request(format_args!(
                "at most {MAX_REFERENCE_IDS} references are resolved at once"
            )));
        }
        if let Some(index) = ids.iter().position(|id| !is_id(id)) {
            return Err(
                AppError::bad_request("a reference names a block by its identifier")
                    .with_pointer(format!("/{index}")),
            );
        }
        let mut asked: Vec<&str> = Vec::with_capacity(ids.len());
        for id in ids {
            if !asked.contains(&id.as_str()) {
                asked.push(id);
            }
        }
        let blocks = self.store().named_blocks(&asked).await?;
        let named: HashMap<&str, &Block> = blocks
            .iter()
            .map(|block| (block.id.as_str(), block))
            .collect();
        let mut readable_containers: HashMap<&str, Container> = HashMap::new();
        let mut told = Vec::new();
        for id in asked {
            let block = named.get(id);
            let container_id = block.map_or(id, |block| block.container_id.as_str());
            if !readable_containers.contains_key(container_id)
                && let Some(resolved) = self.container_read(user_id, container_id).await?
            {
                readable_containers.insert(container_id, resolved);
            }
            if let (Some(block), Some(container)) = (block, readable_containers.get(container_id))
                && let Some(linked) = LinkedBlock::of(block, container)
                    .or_else(|| LinkedBlock::view_of(block, container))
            {
                told.push(linked);
            }
        }
        Ok(told)
    }

    /// One page of the cards and pages whose body names the block, or which have a
    /// comment whose body does, on the boards and wikis of the workspace the user
    /// reads: the oldest first. What names it from anywhere else takes no place in a
    /// page and is not counted: the answer does not say whether there is any.
    ///
    /// `None` unless the block is a card or a page the user is told of
    /// ([`App::resolve_references`]): what names something is told to who reads that.
    ///
    /// # Errors
    ///
    /// Returns the store's error if a read fails.
    pub async fn references_to(
        &self,
        user_id: &str,
        workspace_id: &str,
        block_id: &str,
        asked: ListPage,
    ) -> Result<Option<Vec<LinkedBlock>>, AppError> {
        let told = self
            .resolve_references(user_id, &[block_id.to_owned()])
            .await?;
        // What names a view is no reference: a view has no backlinks to show.
        if told.iter().all(|linked| linked.kind == LinkedKind::View) {
            return Ok(None);
        }
        let paging = asked.paging();
        let mut containers = self.containers_held(user_id, workspace_id).await?;
        // A template of the workspace is read by who works there, and what is on it
        // names as anything does.
        containers.extend(
            self.store()
                .get_template_boards(workspace_id, user_id)
                .await?,
        );
        let read = self.containers_read(user_id, containers).await?;
        let container_ids: Vec<&str> = read.keys().map(String::as_str).collect();
        let found = self
            .store()
            .blocks_naming(block_id, &container_ids, paging)
            .await?;
        Ok(Some(linked(&found, &read)))
    }

    /// The cards and pages a reference of the user may name, by their titles: those on
    /// the boards and wikis of the workspace the user reads whose title has every word
    /// of `term`, at most 20, the one changed last first; for
    /// an empty term the ones changed last. Bodies are not looked into, and what is on
    /// a template is not found.
    ///
    /// # Errors
    ///
    /// Returns 400 for a term of more than 200 characters, and the
    /// store's error if a read fails.
    pub async fn search_references(
        &self,
        user_id: &str,
        workspace_id: &str,
        term: &str,
    ) -> Result<Vec<LinkedBlock>, AppError> {
        if term.chars().count() > MAX_TITLE_TERM_CHARS {
            return Err(AppError::bad_request(format_args!(
                "a search for what a reference may name is at most {MAX_TITLE_TERM_CHARS} characters"
            )));
        }
        let containers = self.containers_held(user_id, workspace_id).await?;
        let read = self.containers_read(user_id, containers).await?;
        let container_ids: Vec<&str> = read.keys().map(String::as_str).collect();
        let found = self
            .store()
            .titled_blocks(&container_ids, term, FOUND_AT_MOST)
            .await?;
        Ok(linked(&found, &read))
    }

    /// The cards of the board among what the bodies name: all that whoever reads the
    /// board by its link is shown of the references of its bodies
    /// ([`Block::clear_references_but`]). That reader is told what is on the board and
    /// nothing else of the workspace: a reference to a page, to a card of another
    /// board, to what is in the trash or to nothing goes out without its identifier,
    /// each like the others.
    ///
    /// # Errors
    ///
    /// Returns the store's error if the cards cannot be read; nothing is sent then.
    pub async fn cards_named_on(
        &self,
        container_id: &str,
        bodies: impl IntoIterator<Item = &Body>,
    ) -> Result<HashSet<String>, AppError> {
        self.named_on(container_id, BlockType::Card, bodies).await
    }

    /// The pages of the wiki among what the bodies name: all that whoever reads the
    /// wiki by its link is shown of the references of its pages, as
    /// [`App::cards_named_on`] is of a board's. A reference to a card, to a page of
    /// another wiki, to what is in the trash or to nothing goes out without its
    /// identifier.
    ///
    /// # Errors
    ///
    /// Returns the store's error if the pages cannot be read; nothing is sent then.
    pub async fn pages_named_on(
        &self,
        wiki_id: &str,
        bodies: impl IntoIterator<Item = &Body>,
    ) -> Result<HashSet<String>, AppError> {
        self.named_on(wiki_id, BlockType::Page, bodies).await
    }

    /// The units of the course among what the bodies name: all that whoever reads the
    /// course by its link is shown of the references of its units, as
    /// [`App::pages_named_on`] is of a wiki's. A reference to a chapter, a card, a page,
    /// a unit of another course, what is in the trash or nothing goes out without its
    /// identifier.
    ///
    /// # Errors
    ///
    /// Returns the store's error if the units cannot be read; nothing is sent then.
    pub async fn units_named_on(
        &self,
        course_id: &str,
        bodies: impl IntoIterator<Item = &Body>,
    ) -> Result<HashSet<String>, AppError> {
        self.named_on(course_id, BlockType::Unit, bodies).await
    }

    /// The blocks of the type on the board among what the bodies name.
    async fn named_on(
        &self,
        container_id: &str,
        block_type: BlockType,
        bodies: impl IntoIterator<Item = &Body>,
    ) -> Result<HashSet<String>, AppError> {
        let mut named: Vec<&str> = bodies.into_iter().flat_map(Body::reference_ids).collect();
        named.sort_unstable();
        named.dedup();
        Ok(self
            .store()
            .blocks_on_container_among(container_id, block_type, &named)
            .await?)
    }

    /// The boards, wikis and courses of the workspace the user holds a role on, and its
    /// open boards, wikis and courses, which whoever works there reads without joining;
    /// templates left out: where to look for what the user reads. Whether the user does
    /// read one is not said here ([`App::containers_read`] asks).
    async fn containers_held(
        &self,
        user_id: &str,
        workspace_id: &str,
    ) -> Result<Vec<Container>, AppError> {
        self.containers_for_user_and_workspace(user_id, workspace_id, PublicContainers::OpenToRead)
            .await
    }

    /// Those of `containers` the user reads ([`App::container_read`]), by their identifiers.
    ///
    /// `containers` is where the store says the user holds a role, or what it lists as a
    /// template, which is no answer of its own: every one is asked of
    /// [`App::container_access`] before the store is asked what is on any. So the store
    /// looks only where the user reads, what is found there is the answer as it is, a
    /// page is cut from that alone, and the reads a question takes are those of the
    /// boards the user holds a role on and of the open wikis, whatever is on them. They
    /// are asked one after another: a request holds no more of the store's connections
    /// than any other does.
    async fn containers_read(
        &self,
        user_id: &str,
        containers: Vec<Container>,
    ) -> Result<BTreeMap<String, Container>, AppError> {
        let mut read = BTreeMap::new();
        for container in containers {
            if let Some(container) = self.container_read(user_id, &container.id).await? {
                read.insert(container.id.clone(), container);
            }
        }
        Ok(read)
    }

    /// The board or wiki, if it is of this handle's workspace and the user reads it:
    /// what [`App::container_access`] grants, as every route to a board asks.
    ///
    /// A template the server ships with is every workspace's to read and of none: what
    /// is on it is not of this workspace, and no reference of this workspace shows it.
    async fn container_read(
        &self,
        user_id: &str,
        container_id: &str,
    ) -> Result<Option<Container>, AppError> {
        let access = self.container_access(user_id, container_id).await?;
        Ok(access
            .filter(|access| {
                access.grant.reads() && self.reaches_workspace(&access.container.workspace_id)
            })
            .map(|access| access.container))
    }
}

/// The blocks as a reference shows them, each with the board it is on.
fn linked(blocks: &[Block], containers: &BTreeMap<String, Container>) -> Vec<LinkedBlock> {
    blocks
        .iter()
        .filter_map(|block| LinkedBlock::of(block, containers.get(&block.container_id)?))
        .collect()
}
