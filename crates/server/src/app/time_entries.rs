//! Personal time tracking operations and single active timer orchestration.
//!
//! Every entry belongs strictly to the calling user (`session.user_id`) and is never
//! broadcast to websockets, posted to webhooks, indexed in search, or written to audit
//! logs. Linking an entry to a workspace, board, course, card, or unit goes through
//! [`App::enter_workspace`] and [`App::container_access`], enforcing both organization
//! SSO/password session walls and block read permissions.

use fvoci_model::{
    BlockType, ContainerKind, CreateTimeEntryRequest, Session, StartTimeEntryRequest,
    StartedTimeEntry, TimeEntry, TimeEntryKind, TimeEntryList, TimeEntryPatch, TimeTargetKind,
    WorkspaceRole,
};
use fvoci_store::{NewStoredTimeEntry, TimeEntryFilter, TimeEntryPatchOutcome, TimeReader};

use super::sso::Asker;
use super::{App, ListPage};
use crate::auth::now_millis;
use crate::error::{AppError, OrMissing};

/// Filters passed from the HTTP layer to [`App::list_time_entries`].
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct TimeEntryQuery<'a> {
    /// Inclusive lower bound on `started_at` (epoch ms).
    pub from: Option<i64>,
    /// Inclusive upper bound on `started_at` (epoch ms).
    pub to: Option<i64>,
    /// Optional activity kind filter.
    pub kind: Option<TimeEntryKind>,
    /// Optional linked block identifier filter.
    pub block_id: Option<&'a str>,
    /// Optional linked container identifier filter.
    pub container_id: Option<&'a str>,
}

/// Verified target resolved behind its organization's wall.
struct ResolvedTarget {
    org_id: String,
    workspace_id: Option<String>,
    container_id: Option<String>,
    block_id: Option<String>,
    target_kind: Option<TimeTargetKind>,
    target_title: Option<String>,
    /// Organization-scoped handle used by [`App::create_time_entry`] so manual inserts run
    /// directly under the target organization's RLS tenant (`start_time_entry` uses `org_id`
    /// with the cross-organization atomic switch helper instead).
    org_app: App,
}

impl App {
    /// Returns the caller's currently active timer, if one is running in an organization
    /// open to this session.
    ///
    /// # Errors
    ///
    /// Returns the store's error if the query fails.
    pub async fn active_time_entry(
        &self,
        session: &Session,
    ) -> Result<Option<TimeEntry>, AppError> {
        Ok(self
            .store()
            .active_time_entry(reader(session.into()))
            .await?)
    }

    /// Validates the optional target, atomically stops any existing active timer of the
    /// user (across organizations), and starts a new active timer.
    ///
    /// # Errors
    ///
    /// Returns 400 for invalid request fields, 401 (`sso_required` or
    /// `password_sign_in_required`) if the session does not open the target organization,
    /// or 404 if the target does not exist, is not readable, or is not a non-template
    /// card, unit, board, or course.
    pub async fn start_time_entry(
        &self,
        session: &Session,
        request: &StartTimeEntryRequest,
    ) -> Result<StartedTimeEntry, AppError> {
        request.check_request()?;
        let target = self
            .resolve_time_target(
                session,
                request.workspace_id.as_deref(),
                request.container_id.as_deref(),
                request.block_id.as_deref(),
            )
            .await?;
        let new = NewStoredTimeEntry {
            workspace_id: target.workspace_id.as_deref(),
            container_id: target.container_id.as_deref(),
            block_id: target.block_id.as_deref(),
            target_kind: target.target_kind,
            target_title: target.target_title.as_deref(),
            kind: request.kind,
            note: &request.note,
        };
        Ok(self
            .store()
            .start_time_entry(reader(session.into()), &target.org_id, &new, now_millis())
            .await?)
    }

    /// Stops the caller's currently active timer in an organization open to this session.
    ///
    /// # Errors
    ///
    /// Returns 404 if no active timer is running in an open organization.
    pub async fn stop_time_entry(&self, session: &Session) -> Result<TimeEntry, AppError> {
        self.store()
            .stop_time_entry(reader(session.into()), now_millis())
            .await?
            .ok_or_else(|| AppError::not_found("active time entry"))
    }

    /// Lists one page of the caller's time entries across all organizations open to this
    /// session, along with the aggregate summary across all matching entries.
    ///
    /// # Errors
    ///
    /// Returns the store's error if the query fails.
    pub async fn list_time_entries(
        &self,
        session: &Session,
        query: &TimeEntryQuery<'_>,
        page: ListPage,
    ) -> Result<TimeEntryList, AppError> {
        let filter = TimeEntryFilter {
            from: query.from,
            to: query.to,
            kind: query.kind,
            block_id: query.block_id,
            container_id: query.container_id,
        };
        Ok(self
            .store()
            .list_time_entries(reader(session.into()), &filter, page.paging())
            .await?)
    }

    /// Manually logs a completed time entry without affecting any active timer.
    ///
    /// # Errors
    ///
    /// Returns 400 for invalid timestamps or note, 401 if the session does not open the
    /// target organization, or 404 if the target is missing or not readable.
    pub async fn create_time_entry(
        &self,
        session: &Session,
        request: &CreateTimeEntryRequest,
    ) -> Result<TimeEntry, AppError> {
        request.check_request()?;
        let target = self
            .resolve_time_target(
                session,
                request.workspace_id.as_deref(),
                request.container_id.as_deref(),
                request.block_id.as_deref(),
            )
            .await?;
        let new = NewStoredTimeEntry {
            workspace_id: target.workspace_id.as_deref(),
            container_id: target.container_id.as_deref(),
            block_id: target.block_id.as_deref(),
            target_kind: target.target_kind,
            target_title: target.target_title.as_deref(),
            kind: request.kind,
            note: &request.note,
        };
        Ok(target
            .org_app
            .store()
            .create_time_entry(
                &session.user_id,
                &new,
                request.started_at,
                request.ended_at,
                now_millis(),
            )
            .await?)
    }

    /// Edits one of the caller's time entries (`kind`, `startedAt`, `endedAt`, `note`).
    ///
    /// # Errors
    ///
    /// Returns 400 if fields are invalid or `endedAt < startedAt`, or 404 if no such
    /// entry belongs to the caller in an open organization.
    pub async fn patch_time_entry(
        &self,
        session: &Session,
        entry_id: &str,
        patch: &TimeEntryPatch,
    ) -> Result<TimeEntry, AppError> {
        patch.check_request()?;
        match self
            .store()
            .patch_time_entry(reader(session.into()), entry_id, patch, now_millis())
            .await?
        {
            TimeEntryPatchOutcome::Updated(entry) => Ok(entry),
            TimeEntryPatchOutcome::NotFound => Err(AppError::not_found("time entry")),
            TimeEntryPatchOutcome::EndedBeforeStarted => {
                let field = if patch.ended_at.is_some() {
                    "endedAt"
                } else {
                    "startedAt"
                };
                Err(
                    AppError::bad_request("endedAt must be greater than or equal to startedAt")
                        .with_field(field),
                )
            }
        }
    }

    /// Deletes one of the caller's time entries in an open organization.
    ///
    /// # Errors
    ///
    /// Returns 404 if no such entry belongs to the caller in an open organization.
    pub async fn delete_time_entry(
        &self,
        session: &Session,
        entry_id: &str,
    ) -> Result<(), AppError> {
        if self
            .store()
            .delete_time_entry(reader(session.into()), entry_id)
            .await?
        {
            Ok(())
        } else {
            Err(AppError::not_found("time entry"))
        }
    }

    /// Resolves and authorizes an optional `(workspace_id, container_id, block_id)` target.
    async fn resolve_time_target(
        &self,
        session: &Session,
        workspace_id: Option<&str>,
        container_id: Option<&str>,
        block_id: Option<&str>,
    ) -> Result<ResolvedTarget, AppError> {
        if workspace_id.is_none() && container_id.is_none() && block_id.is_none() {
            return self.resolve_own_space_target(session).await;
        }
        let ws_id = match workspace_id {
            Some(ws) => ws.to_owned(),
            None => self
                .store()
                .workspace_of_time_target(&session.user_id, container_id, block_id)
                .await?
                .ok_or_else(|| AppError::not_found("target"))?,
        };
        let entered = self
            .enter_workspace(session, &ws_id)
            .await?
            .ok_or_else(|| AppError::not_found("target"))?;
        let org_id = entered
            .app
            .store()
            .tenant()
            .org_id()
            .ok_or_else(|| AppError::internal("workspace handle stands in no organization"))?;
        let ws_app = entered.app.in_workspace(&ws_id);
        if let Some(bid) = block_id {
            return resolve_block_target(
                ws_app,
                entered.app,
                org_id,
                ws_id,
                &session.user_id,
                container_id,
                bid,
            )
            .await;
        }
        if let Some(cid) = container_id {
            return resolve_container_target(
                ws_app,
                entered.app,
                org_id,
                ws_id,
                &session.user_id,
                cid,
            )
            .await;
        }
        if entered.role == WorkspaceRole::Guest {
            return Err(AppError::not_found("target"));
        }
        Ok(ResolvedTarget {
            org_id,
            workspace_id: Some(ws_id),
            container_id: None,
            block_id: None,
            target_kind: None,
            target_title: None,
            org_app: entered.app,
        })
    }

    async fn resolve_own_space_target(
        &self,
        session: &Session,
    ) -> Result<ResolvedTarget, AppError> {
        let own_ws = self
            .store()
            .own_space_workspace_of(&session.user_id)
            .await?
            .ok_or_else(|| AppError::not_found("target"))?;
        let entered = self
            .enter_workspace(session, &own_ws)
            .await?
            .ok_or_else(|| AppError::not_found("target"))?;
        let org_id = entered
            .app
            .store()
            .tenant()
            .org_id()
            .ok_or_else(|| AppError::internal("own space handle stands in no organization"))?;
        Ok(ResolvedTarget {
            org_id,
            workspace_id: None,
            container_id: None,
            block_id: None,
            target_kind: None,
            target_title: None,
            org_app: entered.app,
        })
    }
}

async fn resolve_block_target(
    ws_app: App,
    org_app: App,
    org_id: String,
    workspace_id: String,
    user_id: &str,
    container_id: Option<&str>,
    block_id: &str,
) -> Result<ResolvedTarget, AppError> {
    let block = ws_app.block(block_id).await.or_missing("target")?;
    if container_id.is_some_and(|cid| block.container_id != cid) {
        return Err(AppError::not_found("target"));
    }
    let (target_kind, expected_kind) = match block.block_type {
        BlockType::Card if !block.is_template() => (TimeTargetKind::Card, ContainerKind::Board),
        BlockType::Unit => (TimeTargetKind::Unit, ContainerKind::Course),
        _ => return Err(AppError::not_found("target")),
    };
    let Some(access) = ws_app
        .container_access(user_id, &block.container_id)
        .await?
    else {
        return Err(AppError::not_found("target"));
    };
    if access.container.kind != expected_kind
        || access.container.is_template
        || access.container.is_builtin
        || !access.grant.reads()
    {
        return Err(AppError::not_found("target"));
    }
    Ok(ResolvedTarget {
        org_id,
        workspace_id: Some(workspace_id),
        container_id: Some(block.container_id),
        block_id: Some(block.id),
        target_kind: Some(target_kind),
        target_title: Some(block.title),
        org_app,
    })
}

async fn resolve_container_target(
    ws_app: App,
    org_app: App,
    org_id: String,
    workspace_id: String,
    user_id: &str,
    container_id: &str,
) -> Result<ResolvedTarget, AppError> {
    let Some(access) = ws_app.container_access(user_id, container_id).await? else {
        return Err(AppError::not_found("target"));
    };
    let target_kind = match access.container.kind {
        ContainerKind::Board => TimeTargetKind::Board,
        ContainerKind::Course => TimeTargetKind::Course,
        _ => return Err(AppError::not_found("target")),
    };
    if access.container.is_template || access.container.is_builtin || !access.grant.reads() {
        return Err(AppError::not_found("target"));
    }
    Ok(ResolvedTarget {
        org_id,
        workspace_id: Some(workspace_id),
        container_id: Some(access.container.id),
        block_id: None,
        target_kind: Some(target_kind),
        target_title: Some(access.container.title),
        org_app,
    })
}

fn reader(asker: Asker<'_>) -> TimeReader<'_> {
    TimeReader {
        user_id: asker.user_id,
        making: asker.making,
    }
}
