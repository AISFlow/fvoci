//! A person's stars in a workspace, as they read what the stars are on now.
//!
//! A star is put on what its person reads in the workspace (a board, a wiki or a course,
//! or a card, a page or a unit of one), and is listed while they read it: the boards
//! as every route to a board asks ([`App::container_access`]), the cards, pages and
//! units as a reference resolves them ([`App::resolve_references`]). The handle reaches
//! no diary, and an entry is none of what a reference names: no star is put on either,
//! so that what a diary holds is not shown at the head of the sidebar or of the search.
//! What is not read, or is in the trash, is left out of the list without a word, and is
//! there again once it is read again. Each of the person's connections to the workspace
//! is told when their stars change (`favoritesChanged`).

use std::collections::HashMap;

use fvoci_model::{Container, Favorite, FavoriteKind, MAX_FAVORITES, is_id};
use fvoci_store::{FavoriteAdded, StoredFavorite};

use super::App;
use super::references::MAX_REFERENCE_IDS;
use crate::auth::now_millis;
use crate::error::{AppError, Reason};

impl App {
    /// The stars of `user_id` in the workspace, in their order, as the user reads what
    /// they are on now: what they do not read is left out, and so is what a star is put
    /// on no more: a card of a board made a template since, say.
    ///
    /// # Errors
    ///
    /// Returns the store's error if a read fails.
    pub async fn favorites(
        &self,
        user_id: &str,
        workspace_id: &str,
    ) -> Result<Vec<Favorite>, AppError> {
        let stored = self.store().favorites_of(user_id, workspace_id).await?;
        let block_ids: Vec<String> = stored
            .iter()
            .filter(|favorite| favorite.kind != FavoriteKind::Container)
            .map(|favorite| favorite.target_id.clone())
            .collect();
        // Whether each container is one a star is kept on, or under: asked once each.
        let mut containers: HashMap<String, Option<Container>> = HashMap::new();
        let mut blocks: HashMap<String, Favorite> = HashMap::new();
        // A list past the stars a workspace keeps (two put at once before they were
        // held one after the other) is still read whole, a resolver's worth at a time.
        let mut linked_blocks = Vec::with_capacity(block_ids.len());
        for some in block_ids.chunks(MAX_REFERENCE_IDS) {
            linked_blocks.extend(self.resolve_references(user_id, some).await?);
        }
        for linked in linked_blocks {
            if !containers.contains_key(&linked.container_id) {
                let container = self
                    .favorite_container(user_id, &linked.container_id)
                    .await?;
                containers.insert(linked.container_id.clone(), container);
            }
            if containers
                .get(&linked.container_id)
                .is_some_and(Option::is_some)
                && let Some(favorite) = Favorite::of_block(linked)
            {
                blocks.insert(favorite.target_id.clone(), favorite);
            }
        }
        let mut shown = Vec::with_capacity(stored.len());
        for favorite in stored {
            let found = if favorite.kind == FavoriteKind::Container {
                let container = match containers.remove(&favorite.target_id) {
                    Some(known) => known,
                    None => {
                        self.favorite_container(user_id, &favorite.target_id)
                            .await?
                    }
                };
                container.map(|container| Favorite::of_container(&container))
            } else {
                blocks.remove(&favorite.target_id)
            };
            shown.extend(found);
        }
        Ok(shown)
    }

    /// Puts a star of `user_id` on `target_id`, after their others; one that is there
    /// stays where it is. The server finds what it is.
    ///
    /// # Errors
    ///
    /// Returns 404 for what the user does not read in the workspace, a diary or an
    /// entry of one, and an identifier of nothing or of no form of one, all alike; 400
    /// with the reason `too_many_favorites` past [`MAX_FAVORITES`]; and the store's
    /// error if a statement fails.
    pub async fn add_favorite(
        &self,
        user_id: &str,
        workspace_id: &str,
        target_id: &str,
    ) -> Result<(), AppError> {
        let Some(kind) = self.favorite_kind_of(user_id, target_id).await? else {
            return Err(AppError::not_found("favorite target"));
        };
        let favorite = StoredFavorite {
            target_id: target_id.to_owned(),
            kind,
        };
        match self
            .store()
            .add_favorite(workspace_id, user_id, &favorite, now_millis())
            .await?
        {
            FavoriteAdded::Added => {
                self.ws().favorites_changed(workspace_id, user_id).await;
                Ok(())
            }
            FavoriteAdded::AlreadyThere => Ok(()),
            FavoriteAdded::Full => Err(AppError::bad_request(format_args!(
                "a user keeps at most {MAX_FAVORITES} favorites in a workspace"
            ))
            .with_reason(Reason::TooManyFavorites)
            .with_limit(u64::try_from(MAX_FAVORITES).unwrap_or(u64::MAX))),
        }
    }

    /// Takes the star of `user_id` off `target_id`.
    ///
    /// # Errors
    ///
    /// Returns a not-found error where the user has no star on it in the workspace,
    /// whatever the identifier names, and the store's error if the statement fails.
    pub async fn remove_favorite(
        &self,
        user_id: &str,
        workspace_id: &str,
        target_id: &str,
    ) -> Result<(), AppError> {
        if !is_id(target_id)
            || !self
                .store()
                .remove_favorite(workspace_id, user_id, target_id)
                .await?
        {
            return Err(AppError::not_found("favorite"));
        }
        self.ws().favorites_changed(workspace_id, user_id).await;
        Ok(())
    }

    /// Puts the stars of `user_id` in the order of `target_ids`, the others after them.
    ///
    /// # Errors
    ///
    /// Returns 400 for more than [`MAX_FAVORITES`] identifiers and for an entry that has
    /// not the form of one (naming its place), and the store's error if the statement
    /// fails.
    pub async fn reorder_favorites(
        &self,
        user_id: &str,
        workspace_id: &str,
        target_ids: &[String],
    ) -> Result<(), AppError> {
        if target_ids.len() > MAX_FAVORITES {
            return Err(AppError::bad_request(format_args!(
                "at most {MAX_FAVORITES} favorites are put in order"
            )));
        }
        if let Some(index) = target_ids.iter().position(|id| !is_id(id)) {
            return Err(
                AppError::bad_request("a favorite is named by its identifier")
                    .with_pointer(format!("/{index}")),
            );
        }
        self.store()
            .reorder_favorites(workspace_id, user_id, target_ids)
            .await?;
        self.ws().favorites_changed(workspace_id, user_id).await;
        Ok(())
    }

    /// What `target_id` is, where the user may star it: a board, a wiki or a course of
    /// this handle's workspace they read, no template, or a card, a page or a unit of
    /// one, as a reference resolves it. `None` for anything else: a card of a template
    /// is no more starred than the template is, and a view no more than apart from its
    /// board.
    async fn favorite_kind_of(
        &self,
        user_id: &str,
        target_id: &str,
    ) -> Result<Option<FavoriteKind>, AppError> {
        if !is_id(target_id) {
            return Ok(None);
        }
        if self.favorite_container(user_id, target_id).await?.is_some() {
            return Ok(Some(FavoriteKind::Container));
        }
        let Some(linked) = self
            .resolve_references(user_id, &[target_id.to_owned()])
            .await?
            .into_iter()
            .next()
        else {
            return Ok(None);
        };
        Ok(self
            .favorite_container(user_id, &linked.container_id)
            .await?
            .and_then(|_| FavoriteKind::of_linked(linked.kind)))
    }

    /// The board, wiki or course, if it is of this handle's workspace, no template, and
    /// the user reads it.
    async fn favorite_container(
        &self,
        user_id: &str,
        container_id: &str,
    ) -> Result<Option<Container>, AppError> {
        Ok(self
            .container_access(user_id, container_id)
            .await?
            .filter(|access| {
                access.grant.reads()
                    && !access.container.is_template
                    && self.reaches_workspace(&access.container.workspace_id)
            })
            .map(|access| access.container))
    }
}
