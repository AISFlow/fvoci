//! A new account's or workspace's first board.
//!
//! Shared workspaces copy the built-in template carrying the onboarding mark; personal
//! spaces (`OrgKind::Personal`) copy the dedicated `welcome-personal` built-in template.

use std::time::Duration;

use fvoci_model::DuplicateAs;
use fvoci_model::{
    BlockType, ContainerVisibility, ContainersAndBlocks, OrgKind, date_property_ids, move_dates,
    renew_containers_and_blocks_ids,
};

use super::accept_language::Reader;
use super::board_ops::remap_copied_block_files;
use super::container_copies::{Stored, titles_in};
use super::language::Language;
use super::templates::{
    changes_of_copy, is_onboarding_template, is_personal_onboarding_template, written_in,
};
use super::{App, Changes};
use crate::error::AppError;
use crate::metrics::{ContainerWork, Module};

/// How long somebody who comes in or starts a space waits for their welcome board.
const WELCOME_WAIT: Duration = Duration::from_secs(5);

impl App {
    /// Copies the welcome board into `workspace_id` for `user_id` (`welcome-personal` in
    /// a personal space, `welcome` in a shared workspace) and returns the copy's ID.
    ///
    /// # Errors
    ///
    /// Returns a server error if the welcome board is missing or could not be copied, or
    /// the store's error.
    pub async fn create_welcome_board(
        &self,
        user_id: &str,
        workspace_id: &str,
        accept_language: Option<&str>,
    ) -> Result<String, AppError> {
        let now = crate::auth::now_millis();
        self.create_welcome_board_at(user_id, workspace_id, accept_language, now)
            .await
    }

    /// [`App::create_welcome_board`] at `now`, in milliseconds.
    ///
    /// # Errors
    ///
    /// As [`App::create_welcome_board`].
    pub async fn create_welcome_board_at(
        &self,
        user_id: &str,
        workspace_id: &str,
        accept_language: Option<&str>,
        now: i64,
    ) -> Result<String, AppError> {
        let org_kind = if self.store().is_own_space().await? {
            OrgKind::Personal
        } else {
            OrgKind::Shared
        };
        let reader = self.reader_at(user_id, accept_language, now).await?;
        let template_id = self
            .welcome_template_id_for(reader.language, org_kind)
            .await?;
        let board_id = self
            .copy_welcome_board(&template_id, user_id, workspace_id, &reader)
            .await?;
        if let Err(error) = self
            .store()
            .record_workspace_welcome(workspace_id, now)
            .await
        {
            tracing::warn!(workspace_id, %error, "a workspace's welcome board was not recorded");
        }
        if org_kind == OrgKind::Shared
            && let Err(error) = self.store().record_welcome(user_id, now).await
        {
            tracing::warn!(user_id, %error, "an account's welcome board was not recorded");
        }
        Ok(board_id)
    }

    /// Idempotently creates the welcome board for `workspace_id` (`workspaces.welcomed_at`)
    /// when a new organization or personal space is started, waiting up to
    /// [`WELCOME_WAIT`] so the board is ready on initial load.
    pub(super) async fn welcome_workspace(
        &self,
        org_id: &str,
        user_id: &str,
        workspace_id: &str,
        org_kind: OrgKind,
        accept_language: Option<&str>,
    ) {
        let made = self.spawn_tracked({
            let app = self.clone();
            let org_id = org_id.to_owned();
            let user_id = user_id.to_owned();
            let workspace_id = workspace_id.to_owned();
            let accept_language = accept_language.map(str::to_owned);
            async move {
                let now = crate::auth::now_millis();
                match app
                    .welcome_workspace_once(
                        &org_id,
                        &user_id,
                        &workspace_id,
                        org_kind,
                        accept_language.as_deref(),
                        now,
                    )
                    .await
                {
                    Ok(Some(board_id)) => {
                        tracing::info!(
                            user_id,
                            workspace_id,
                            board_id,
                            is_personal = (org_kind == OrgKind::Personal),
                            "a workspace was given its welcome board"
                        );
                    }
                    Ok(None) => {}
                    Err(error) => {
                        tracing::warn!(
                            user_id,
                            workspace_id,
                            %error,
                            "a workspace's welcome board was not made"
                        );
                    }
                }
            }
        });
        match tokio::time::timeout(WELCOME_WAIT, made).await {
            Ok(Ok(())) => {}
            Ok(Err(error)) => {
                tracing::error!(
                    user_id,
                    workspace_id,
                    %error,
                    "the making of a workspace's welcome board broke off"
                );
            }
            Err(_) => {
                tracing::info!(
                    user_id,
                    workspace_id,
                    "a workspace's welcome board is still being made"
                );
            }
        }
    }

    async fn welcome_workspace_once(
        &self,
        org_id: &str,
        user_id: &str,
        workspace_id: &str,
        org_kind: OrgKind,
        accept_language: Option<&str>,
        now: i64,
    ) -> Result<Option<String>, AppError> {
        if self.store().is_workspace_welcomed(workspace_id).await? {
            return Ok(None);
        }
        let reader = self.reader_at(user_id, accept_language, now).await?;
        let Some(template_id) = self.marked_template_for(reader.language, org_kind).await? else {
            self.restock_templates_soon();
            return Ok(None);
        };
        let (mut copies, board_id) = self
            .prepare_welcome_board_copy(&template_id, user_id, workspace_id, &reader)
            .await?;
        let file_ids: Vec<String> = copies
            .blocks
            .iter()
            .flat_map(fvoci_model::Block::shown_files)
            .collect();
        let (file_map, staged_files, written_places) = self
            .pre_copy_files_to_container(self, &template_id, org_id, &board_id, &file_ids, user_id)
            .await?;
        if let Err(error) = remap_copied_block_files(&mut copies.blocks, &file_map) {
            for place in written_places {
                place.give_up().await;
            }
            return Err(error);
        }
        let committed = self
            .store()
            .commit_workspace_welcome_board(workspace_id, &mut copies, &staged_files, user_id, now)
            .await;
        let members = match committed {
            Ok(Some(members)) => {
                for place in written_places {
                    place.settled_in_transaction();
                }
                members
            }
            Ok(None) => {
                for place in written_places {
                    let _ = self.remove_doomed_now(place.path()).await;
                    place.settled_in_transaction();
                }
                return Ok(None);
            }
            Err(error) => {
                for place in written_places {
                    place.give_up().await;
                }
                return Err(error.into());
            }
        };
        self.finish_workspace_welcome(
            user_id,
            workspace_id,
            org_kind,
            now,
            &board_id,
            copies,
            members,
        )
        .await;
        Ok(Some(board_id))
    }

    async fn prepare_welcome_board_copy(
        &self,
        template_id: &str,
        user_id: &str,
        workspace_id: &str,
        reader: &Reader,
    ) -> Result<(ContainersAndBlocks, String), AppError> {
        let mut template = self.container(template_id).await?;
        let changes = changes_of_copy(&template, reader.today.as_deref());
        self.write_container_bodies(template_id).await?;
        for name in changes.without_properties {
            template.properties.remove(*name);
        }
        template.visibility = ContainerVisibility::Private;
        template.is_template = false;
        template.is_builtin = false;
        user_id.clone_into(&mut template.created_by);
        workspace_id.clone_into(&mut template.workspace_id);

        let mut blocks: Vec<_> = self
            .blocks_for_container(template_id)
            .await?
            .into_iter()
            .filter(|block| block.block_type != BlockType::Comment)
            .collect();
        let dates = date_property_ids(&template.card_properties);
        for block in &mut blocks {
            move_dates(&mut block.fields, &dates, changes.days_later);
        }
        let copies = renew_containers_and_blocks_ids(ContainersAndBlocks {
            containers: vec![template],
            blocks,
        });
        let [board] = copies.containers.as_slice() else {
            return Err(AppError::internal("new welcome board wasn't prepared"));
        };
        let board_id = board.id.clone();
        Ok((copies, board_id))
    }

    #[expect(
        clippy::too_many_arguments,
        reason = "post-commit welcome board sidebar placement, metrics, and broadcasts share state"
    )]
    async fn finish_workspace_welcome(
        &self,
        user_id: &str,
        workspace_id: &str,
        org_kind: OrgKind,
        now: i64,
        board_id: &str,
        copies: ContainersAndBlocks,
        members: Vec<fvoci_model::ContainerMember>,
    ) {
        if let Err(error) = self
            .add_containers_to_default_category(
                user_id,
                workspace_id,
                std::slice::from_ref(&board_id.to_owned()),
            )
            .await
        {
            tracing::warn!(user_id, workspace_id, %error, "welcome board was not placed in default category");
        }
        if org_kind == OrgKind::Shared
            && let Err(error) = self.store().record_welcome(user_id, now).await
        {
            tracing::warn!(user_id, %error, "an account's welcome board was not recorded");
        }
        self.metrics()
            .container_copied(ContainerWork::Welcome, Module::Board);

        let changes = Changes::in_workspace(workspace_id)
            .made(copies.containers, members)
            .copied(copies.blocks);
        self.tell(changes);
    }

    /// Gives somebody who came into the workspace as a member their welcome board, a
    /// private board of their own there, if their account has none yet.
    pub(super) async fn welcome_member(
        &self,
        user_id: &str,
        workspace_id: &str,
        accept_language: Option<&str>,
    ) {
        let made = self.spawn_tracked({
            let app = self.clone();
            let (user_id, workspace_id) = (user_id.to_owned(), workspace_id.to_owned());
            let accept_language = accept_language.map(str::to_owned);
            async move {
                app.welcome_and_log(&user_id, &workspace_id, accept_language.as_deref())
                    .await;
            }
        });
        match tokio::time::timeout(WELCOME_WAIT, made).await {
            Ok(Ok(())) => {}
            Ok(Err(error)) => {
                tracing::error!(
                    user_id,
                    workspace_id,
                    %error,
                    "the making of a new member's welcome board broke off"
                );
            }
            Err(_) => {
                tracing::info!(
                    user_id,
                    workspace_id,
                    "a new member's welcome board is still being made"
                );
            }
        }
    }

    async fn welcome_and_log(
        &self,
        user_id: &str,
        workspace_id: &str,
        accept_language: Option<&str>,
    ) {
        let now = crate::auth::now_millis();
        let made = self
            .welcome_once(user_id, workspace_id, accept_language, now)
            .await;
        match made {
            Ok(Some(board_id)) => {
                tracing::info!(
                    user_id,
                    workspace_id,
                    board_id,
                    "a new member was given their welcome board"
                );
            }
            Ok(None) => {}
            Err(error) => {
                tracing::warn!(
                    user_id,
                    workspace_id,
                    %error,
                    "a new member's welcome board was not made, or not whole"
                );
            }
        }
    }

    /// [`App::welcome_member`] in the default workspace of the handle's organization,
    /// which somebody who signs in through its provider comes into.
    pub(super) async fn welcome_newcomer(&self, user_id: &str, accept_language: Option<&str>) {
        match self.store().organization().await {
            Ok(organization) => {
                self.welcome_member(user_id, &organization.default_workspace_id, accept_language)
                    .await;
            }
            Err(error) => {
                tracing::warn!(user_id, %error, "a new member was not given their welcome board");
            }
        }
    }

    async fn welcome_once(
        &self,
        user_id: &str,
        workspace_id: &str,
        accept_language: Option<&str>,
        now: i64,
    ) -> Result<Option<String>, AppError> {
        let org_kind = if self.store().is_own_space().await? {
            OrgKind::Personal
        } else {
            OrgKind::Shared
        };
        let reader = self.reader_at(user_id, accept_language, now).await?;
        let Some(template_id) = self.marked_template_for(reader.language, org_kind).await? else {
            self.restock_templates_soon();
            return Ok(None);
        };
        let template = self.container(&template_id).await?;
        let changes = changes_of_copy(&template, reader.today.as_deref());
        self.write_container_bodies(&template_id).await?;
        let titles = titles_in(reader.language, template.kind);
        let stored = self
            .store()
            .duplicate_welcome_board(&template_id, user_id, workspace_id, &titles, changes, now)
            .await?;
        let Some((copies, members)) = stored else {
            return Ok(None);
        };
        let [board] = copies.containers.as_slice() else {
            return Err(AppError::internal("new board wasn't created"));
        };
        let board_id = board.id.clone();
        let stored = Stored {
            source_id: &template_id,
            source_is_template: template.is_template,
            user_id,
            to_workspace: workspace_id,
            duplicate_as: DuplicateAs::Copy,
            kind: template.kind,
            work: ContainerWork::Welcome,
            welcomed_at: Some(now),
        };
        self.finish_copy(stored, copies, members).await?;
        let _ = self
            .store()
            .record_workspace_welcome(workspace_id, now)
            .await;
        Ok(Some(board_id))
    }

    async fn copy_welcome_board(
        &self,
        template_id: &str,
        user_id: &str,
        workspace_id: &str,
        reader: &Reader,
    ) -> Result<String, AppError> {
        let (copies, _members) = self
            .copy_container(
                template_id,
                user_id,
                workspace_id,
                DuplicateAs::Copy,
                reader,
                ContainerWork::Welcome,
            )
            .await?;
        match copies.containers.as_slice() {
            [board] => Ok(board.id.clone()),
            _ => Err(AppError::internal("new board wasn't created")),
        }
    }

    async fn welcome_template_id_for(
        &self,
        language: Language,
        org_kind: OrgKind,
    ) -> Result<String, AppError> {
        if let Some(board) = self.marked_template_for(language, org_kind).await? {
            return Ok(board);
        }
        tracing::warn!(
            language = language.tag(),
            is_personal = (org_kind == OrgKind::Personal),
            "no built-in template is the welcome board of the language: storing them again"
        );
        self.init_templates().await?;
        self.marked_template_for(language, org_kind)
            .await?
            .ok_or_else(|| AppError::internal("the built-in templates hold no welcome board"))
    }

    async fn marked_template_for(
        &self,
        language: Language,
        org_kind: OrgKind,
    ) -> Result<Option<String>, AppError> {
        let boards = self.store().get_builtin_templates().await?;
        Ok(boards
            .into_iter()
            .find(|board| {
                let matches_kind = if org_kind == OrgKind::Personal {
                    is_personal_onboarding_template(board)
                } else {
                    is_onboarding_template(board)
                };
                matches_kind && written_in(board) == Some(language)
            })
            .map(|board| board.id))
    }
}
