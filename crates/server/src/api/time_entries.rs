//! Personal time tracking endpoints under `/users/me/time`.
//!
//! Every operation acts strictly on the calling user's own entries and obeys the
//! session's organization walls (`sso_required` and personal space password confirmation).

use std::num::IntErrorKind;

use axum::body::Bytes;
use axum::extract::{Path, State};
use fvoci_model::{
    ActiveTimeEntry, CreateTimeEntryRequest, MAX_TIME_MILLIS, StartTimeEntryRequest,
    StartedTimeEntry, TimeEntry, TimeEntryKind, TimeEntryList, TimeEntryPatch,
};
use serde::Deserialize;
use utoipa_axum::router::OpenApiRouter;
use utoipa_axum::routes;

use super::contract::params::{
    Page, PerPage, TimeBlockId, TimeContainerId, TimeEntryId, TimeFrom, TimeKind, TimeTo,
};
use super::contract::tag;
use super::query::{QueryParams, non_empty_opt, paging_from_strings};
use super::{ApiResult, RequiredSession, created, decode_object, json_response, no_content};
use crate::app::{App, TimeEntryQuery};
use crate::error::AppError;

#[derive(Debug, Default, Deserialize)]
#[serde(default, rename_all = "camelCase", deny_unknown_fields)]
struct TimeEntriesListQuery {
    from: String,
    to: String,
    kind: String,
    block_id: String,
    container_id: String,
    page: String,
    per_page: String,
}

pub(super) fn routes() -> OpenApiRouter<App> {
    OpenApiRouter::new()
        .routes(routes!(handle_get_active_time_entry))
        .routes(routes!(handle_start_time_entry))
        .routes(routes!(handle_stop_time_entry))
        .routes(routes!(handle_list_time_entries, handle_create_time_entry))
        .routes(routes!(handle_patch_time_entry, handle_delete_time_entry))
}

/// The caller's currently running timer
///
/// Returns `{ "entry": TimeEntry }` if the user of the session has a running timer in an
/// organization the session opens, or `{ "entry": null }` otherwise. Timers running in an
/// organization closed to this session (`sso_required` or unconfirmed personal space) are
/// hidden.
#[utoipa::path(
    get,
    path = "/users/me/time/active",
    operation_id = "getActiveTimeEntry",
    tag = tag::TIME,
    responses((status = 200, description = "The currently running timer, or null.", body = ActiveTimeEntry)),
)]
async fn handle_get_active_time_entry(
    State(app): State<App>,
    RequiredSession(session): RequiredSession,
) -> ApiResult {
    let entry = app.active_time_entry(&session).await?;
    Ok(json_response(&ActiveTimeEntry { entry }))
}

/// Start a live timer
///
/// Starts a running timer for the user of the session. At most one timer may run per
/// account across all organizations: if another timer is already running (in the same
/// organization or another), it is stopped automatically in the same transaction and
/// returned in `stopped` when its organization is open to this session.
///
/// Optional `workspaceId`, `containerId` and `blockId` link the entry to a readable
/// non-template card or course unit (or board/course container). Passing `workspaceId`
/// alone creates a workspace-scoped entry with `targetKind: null` (refused with 404 for
/// workspace guests). Linking to an unreadable, trashed, template, or non-card/non-unit
/// block answers 404. Linking to a target in an organization closed to this session
/// answers 401 (`sso_required` or `password_sign_in_required`). Targetless timers are
/// stored in the user's own space.
#[utoipa::path(
    post,
    path = "/users/me/time/start",
    operation_id = "startTimeEntry",
    tag = tag::TIME,
    request_body = StartTimeEntryRequest,
    responses((status = 201, description = "The newly started timer and any previously running timer that was stopped.", body = StartedTimeEntry)),
)]
async fn handle_start_time_entry(
    State(app): State<App>,
    RequiredSession(session): RequiredSession,
    body: Bytes,
) -> ApiResult {
    let request: StartTimeEntryRequest = decode_object(&body)?;
    let started = app.start_time_entry(&session, &request).await?;
    Ok(created(&started))
}

/// Stop the caller's currently running timer
///
/// Stops the running timer visible to this session (`endedAt` set to now) and returns the
/// completed entry. Answers 404 if no timer is running in an organization open to this
/// session.
#[utoipa::path(
    post,
    path = "/users/me/time/stop",
    operation_id = "stopTimeEntry",
    tag = tag::TIME,
    responses((status = 200, description = "The stopped time entry.", body = TimeEntry)),
)]
async fn handle_stop_time_entry(
    State(app): State<App>,
    RequiredSession(session): RequiredSession,
) -> ApiResult {
    let stopped = app.stop_time_entry(&session).await?;
    Ok(json_response(&stopped))
}

/// List the caller's time entries and totals
///
/// Returns one page of the user's own time entries across every organization open to this
/// session, newest first (`startedAt DESC, id DESC`), together with `summary` totals over
/// every matching entry across all pages. Completed entries contribute `endedAt - startedAt`
/// to `summary`; a running entry (`endedAt` is `null`) is counted in `summary.count` and
/// contributes `0` milliseconds until stopped.
///
/// Optional filters: `from` (`startedAt >= from`), `to` (`startedAt <= to`), `kind`,
/// `blockId`, `containerId`, plus standard `page` and `perPage` pagination.
#[utoipa::path(
    get,
    path = "/users/me/time/entries",
    operation_id = "listTimeEntries",
    tag = tag::TIME,
    params(TimeFrom, TimeTo, TimeKind, TimeBlockId, TimeContainerId, Page, PerPage),
    responses((status = 200, description = "Matching entries and aggregate summary.", body = TimeEntryList)),
)]
async fn handle_list_time_entries(
    State(app): State<App>,
    RequiredSession(session): RequiredSession,
    QueryParams(query): QueryParams<TimeEntriesListQuery>,
) -> ApiResult {
    let page = paging_from_strings(&query.page, &query.per_page)?;
    let from = parse_opt_time(&query.from, "from")?;
    let to = parse_opt_time(&query.to, "to")?;
    if let (Some(from_ms), Some(to_ms)) = (from, to)
        && to_ms < from_ms
    {
        return Err(
            AppError::bad_request("`to` must be greater than or equal to `from`")
                .with_parameter("to"),
        );
    }
    let kind = match non_empty_opt(&query.kind) {
        Some(text) => Some(text.parse::<TimeEntryKind>().map_err(|_| {
            AppError::bad_request("invalid `kind` parameter: must be lecture, study, work or other")
                .with_parameter("kind")
        })?),
        None => None,
    };
    let block_id = non_empty_opt(&query.block_id);
    let container_id = non_empty_opt(&query.container_id);
    let filter = TimeEntryQuery {
        from,
        to,
        kind,
        block_id: block_id.as_deref(),
        container_id: container_id.as_deref(),
    };
    let list = app.list_time_entries(&session, &filter, page).await?;
    Ok(json_response(&list))
}

/// Manually log a completed time entry
///
/// Creates a completed time entry (`endedAt >= startedAt`) for the user of the session
/// without stopping or modifying any currently running timer. Target validation and
/// organization session walls work the same way as `startTimeEntry`.
#[utoipa::path(
    post,
    path = "/users/me/time/entries",
    operation_id = "createTimeEntry",
    tag = tag::TIME,
    request_body = CreateTimeEntryRequest,
    responses((status = 201, description = "The created time entry.", body = TimeEntry)),
)]
async fn handle_create_time_entry(
    State(app): State<App>,
    RequiredSession(session): RequiredSession,
    body: Bytes,
) -> ApiResult {
    let request: CreateTimeEntryRequest = decode_object(&body)?;
    let entry = app.create_time_entry(&session, &request).await?;
    Ok(created(&entry))
}

/// Edit one of the caller's time entries
///
/// Updates `kind`, `startedAt`, `endedAt` and/or `note` on one of the user's own entries
/// in an organization open to this session. Setting `endedAt` on a running timer stops it.
/// Answers 400 if `endedAt < startedAt` or any field is invalid, and 404 for another
/// user's entry, a missing identifier, or an entry in an organization closed to this
/// session.
#[utoipa::path(
    patch,
    path = "/users/me/time/entries/{entryId}",
    operation_id = "patchTimeEntry",
    tag = tag::TIME,
    params(TimeEntryId),
    request_body = TimeEntryPatch,
    responses((status = 200, description = "The updated time entry.", body = TimeEntry)),
)]
async fn handle_patch_time_entry(
    State(app): State<App>,
    RequiredSession(session): RequiredSession,
    Path(entry_id): Path<String>,
    body: Bytes,
) -> ApiResult {
    let patch: TimeEntryPatch = decode_object(&body)?;
    let updated = app.patch_time_entry(&session, &entry_id, &patch).await?;
    Ok(json_response(&updated))
}

/// Delete one of the caller's time entries
///
/// Permanently deletes one of the user's own time entries in an organization open to this
/// session. Answers 404 for another user's entry, a missing identifier, or an entry in an
/// organization closed to this session.
#[utoipa::path(
    delete,
    path = "/users/me/time/entries/{entryId}",
    operation_id = "deleteTimeEntry",
    tag = tag::TIME,
    params(TimeEntryId),
    responses((status = 204, description = "The time entry was deleted.")),
)]
async fn handle_delete_time_entry(
    State(app): State<App>,
    RequiredSession(session): RequiredSession,
    Path(entry_id): Path<String>,
) -> ApiResult {
    app.delete_time_entry(&session, &entry_id).await?;
    Ok(no_content())
}

fn parse_opt_time(raw: &str, name: &str) -> Result<Option<i64>, AppError> {
    if raw.is_empty() {
        return Ok(None);
    }
    let value: i64 = raw.parse().map_err(|error: std::num::ParseIntError| {
        let reason = match error.kind() {
            IntErrorKind::PosOverflow | IntErrorKind::NegOverflow => "out of range",
            _ => "not a whole number",
        };
        AppError::bad_request(format_args!("invalid `{name}` parameter: {reason}"))
            .with_parameter(name)
    })?;
    if !(0..=MAX_TIME_MILLIS).contains(&value) {
        return Err(AppError::bad_request(format_args!(
            "invalid `{name}` parameter: out of range"
        ))
        .with_parameter(name));
    }
    Ok(Some(value))
}
