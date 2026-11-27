//! Diaries: listing and making them, and their entries.
//!
//! A diary is a board of the kind `diary`, in the workspace of a user's own space and in
//! no other. What it shares with a board is asked under `/containers/{containerId}`
//! (`InContainer`); here is what only a diary has, and the handle these routes work with
//! reaches diaries and nothing else, in that workspace alone (`InDiary`).

use axum::body::Bytes;
use axum::extract::Path;
use fvoci_model::{
    Block, Container, ContainerKind, ContainerVisibility, DAYS_PER_LIST, DeletedDiaryEntry,
    DiaryDate, DiaryDay, DiaryEntry, DiaryEntryPatch, DiaryMaking, DiaryRun, MonthDay,
    NewDiaryEntry,
};
use serde::Deserialize;
use utoipa::ToSchema;
use utoipa_axum::router::OpenApiRouter;
use utoipa_axum::routes;

use super::body::decode_diary_entry_patch;
use super::contract::params::Page as PageNumber;
use super::contract::params::{
    DaysFrom, DaysTo, DiaryId, EntryId, MonthDayOfYear, OwnWorkspaceId, PerPage,
};
use super::contract::tag;
use super::query::{PagingQuery, QueryParams};
use super::workspace::{InDiary, InWorkspace};
use super::{
    ApiResult, created, decode_object, json_response, no_content, require_container_permission,
};
use crate::app::{App, ChangeNotification, EntryAnswer};
use crate::auth::now_millis;
use crate::error::AppError;
use crate::permissions::ContainerPermission;

#[derive(Debug, Default, Deserialize)]
#[serde(default, rename_all = "camelCase", deny_unknown_fields)]
struct DiaryDaysQuery {
    from: String,
    to: String,
}

pub(super) fn routes() -> OpenApiRouter<App> {
    OpenApiRouter::new()
        .routes(routes!(handle_get_diaries, handle_create_diary))
        .routes(routes!(handle_get_diary_days))
        .routes(routes!(handle_get_diary_runs))
        .routes(routes!(handle_get_diary_entries_on))
        .routes(routes!(handle_create_diary_entry))
        .routes(routes!(
            handle_get_diary_entry,
            handle_patch_diary_entry,
            handle_delete_diary_entry
        ))
        .routes(routes!(handle_get_deleted_diary_entries))
        .routes(routes!(handle_restore_diary_entry))
}

/// A diary to make.
#[derive(Debug, Default, Deserialize, ToSchema)]
#[serde(default, rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct NewDiary {
    /// The diary's title. Left out, empty: a client shows its own word for a diary.
    title: String,
    /// The diary's description. Left out, empty.
    description: String,
    /// The diary's icon, usually one emoji. Left out, none.
    icon: String,
    /// Whether the diary is made only if the space has none: with `true`, a space that
    /// has a diary is answered with its oldest (200) and nothing is made. For what
    /// opens "the diary" of a space that may have none yet, without asking for a name:
    /// two such requests at once make one diary. Left out, `false`: one more diary.
    if_none: bool,
}

/// The diaries of the user's own space
///
/// Every diary of the workspace, oldest first: each is a board of the `kind` `diary`,
/// private (`visibility` `private`), with the user as its only member. Those in the
/// trash are not listed (`getDeletedContainers`).
#[utoipa::path(
    get,
    path = "/w/{workspaceId}/diaries",
    operation_id = "getDiaries",
    tag = tag::DIARIES,
    params(OwnWorkspaceId),
    responses((status = 200, description = "The diaries.", body = Vec<Container>)),
)]
async fn handle_get_diaries(InDiary(ws): InDiary) -> ApiResult {
    let diaries = ws
        .app
        .workspace_containers(&ws.caller.user_id, &ws.workspace_id, ws.role)
        .await?;
    Ok(json_response(&diaries))
}

/// Make a diary
///
/// The user becomes the diary's admin, and the diary is filed under the user's default
/// category, as a board is. A space has at most 20 diaries, those in the trash not
/// counted: one more is 400 with the reason `too_many_diaries` and the `limit`. With
/// `ifNone`, a space that has a diary is answered with its oldest, and nothing is made.
/// A member the request does not know is 400.
#[utoipa::path(
    post,
    path = "/w/{workspaceId}/diaries",
    operation_id = "createDiary",
    tag = tag::DIARIES,
    params(OwnWorkspaceId),
    request_body = NewDiary,
    responses(
        (status = 201, description = "The diary that was made.", body = Container),
        (status = 200, description = "The oldest diary of the space, which had one: `ifNone` was sent, and nothing was made.", body = Container),
    ),
)]
async fn handle_create_diary(InDiary(ws): InDiary, body: Bytes) -> ApiResult {
    ws.require_container_maker("access denied to create diary")?;
    let NewDiary {
        title,
        description,
        icon,
        if_none,
    } = decode_object(&body)?;
    let diary = Container {
        workspace_id: ws.workspace_id.clone(),
        visibility: ContainerVisibility::Private,
        kind: ContainerKind::Diary,
        title,
        description,
        icon,
        ..Container::default()
    };
    let making = if if_none {
        DiaryMaking::IfNone
    } else {
        DiaryMaking::Another
    };
    let made = ws
        .app
        .create_diary(diary, &ws.caller.user_id, making)
        .await?;
    Ok(match made.made {
        Some(_) => created(&made.diary),
        None => json_response(&made.diary),
    })
}

/// What a member of the workspace who may not read the diary is refused an entry with.
pub(super) const READ_DENIED: &str = "access denied to fetch entry";

/// Returns the entry of the diary for a handler of
/// `/diaries/{diaryId}/entries/{entryId}`, if the user has the permission on the diary:
/// its block, read once. A block that is no entry of this diary is no entry.
pub(super) async fn entry_with_permission(
    ws: &InWorkspace,
    diary_id: &str,
    entry_id: &str,
    permission: ContainerPermission,
    denial: &str,
) -> Result<Block, AppError> {
    require_container_permission(&ws.app, &ws.caller.user_id, diary_id, permission, denial).await?;
    ws.app.diary_entry_of(diary_id, entry_id).await
}

/// The day a query names, which it must.
fn day_param(raw: &str, name: &str) -> Result<DiaryDate, AppError> {
    match raw {
        "" => Err(AppError::bad_request(format_args!(
            "the days of a diary are listed from a day to a day: `{name}` is left out"
        ))
        .with_parameter(name)),
        day => day
            .parse()
            .map_err(|error| AppError::bad_request(error).with_parameter(name)),
    }
}

/// The days of a diary that are written
///
/// Each day of the diary that has an entry, from `from` to `to` (each included), the
/// earliest first: its `date`, its entry (`entryId`) and the entry's `mood`, and nothing
/// else of the entry. For a calendar: a month, or as much as a year (366 days) at once.
/// 400 at the parameter for a range that is wider or has no `from` or `to`; one that
/// ends before it begins lists nothing. Entries in the trash have no day.
#[utoipa::path(
    get,
    path = "/w/{workspaceId}/diaries/{diaryId}/days",
    operation_id = "getDiaryDays",
    tag = tag::DIARIES,
    params(OwnWorkspaceId, DiaryId, DaysFrom, DaysTo),
    responses((status = 200, description = "The days that are written.", body = Vec<DiaryDay>)),
)]
async fn handle_get_diary_days(
    InDiary(ws): InDiary,
    Path((_, diary_id)): Path<(String, String)>,
    QueryParams(query): QueryParams<DiaryDaysQuery>,
) -> ApiResult {
    require_container_permission(
        &ws.app,
        &ws.caller.user_id,
        &diary_id,
        ContainerPermission::View,
        "access denied to fetch the days of the diary",
    )
    .await?;
    let (from, to) = (day_param(&query.from, "from")?, day_param(&query.to, "to")?);
    if !from.lists_to(to) {
        return Err(AppError::bad_request(format_args!(
            "at most {DAYS_PER_LIST} days of a diary are listed at once"
        ))
        .with_parameter("to")
        .with_limit(u64::from(DAYS_PER_LIST)));
    }
    let days = ws.app.diary_days(&diary_id, from, to).await?;
    Ok(json_response(&days))
}

/// The runs of a diary's written days
///
/// Every run of days of the diary written one after the other, the earliest first: its
/// first day (`from`), its last (`to`) and how many days it has (`days`), and nothing
/// of the entries. What a client counts the days written in a row by, the longest and
/// the one that reaches its today, which the server does not say. A diary without
/// entries has no runs; entries in the trash have no day. There are no more runs than
/// the diary has entries.
#[utoipa::path(
    get,
    path = "/w/{workspaceId}/diaries/{diaryId}/runs",
    operation_id = "getDiaryRuns",
    tag = tag::DIARIES,
    params(OwnWorkspaceId, DiaryId),
    responses((status = 200, description = "The runs of written days.", body = Vec<DiaryRun>)),
)]
async fn handle_get_diary_runs(
    InDiary(ws): InDiary,
    Path((_, diary_id)): Path<(String, String)>,
) -> ApiResult {
    require_container_permission(
        &ws.app,
        &ws.caller.user_id,
        &diary_id,
        ContainerPermission::View,
        "access denied to fetch the days of the diary",
    )
    .await?;
    let runs = ws.app.diary_runs(&diary_id).await?;
    Ok(json_response(&runs))
}

/// The entries of a month and day in every year
///
/// The entries of the diary whose day is `monthDay` in any year, each with its body,
/// the latest year first: what a diary shows on one page, today's entry above those of
/// the same day in the years before. `02-29` lists the entries of leap years alone. A
/// day nothing was written on lists nothing.
#[utoipa::path(
    get,
    path = "/w/{workspaceId}/diaries/{diaryId}/days/{monthDay}",
    operation_id = "getDiaryEntriesOn",
    tag = tag::DIARIES,
    params(OwnWorkspaceId, DiaryId, MonthDayOfYear),
    responses((status = 200, description = "The entries of the day in every year.", body = Vec<DiaryEntry>)),
)]
async fn handle_get_diary_entries_on(
    InDiary(ws): InDiary,
    Path((_, diary_id, month_day)): Path<(String, String, String)>,
) -> ApiResult {
    require_container_permission(
        &ws.app,
        &ws.caller.user_id,
        &diary_id,
        ContainerPermission::View,
        READ_DENIED,
    )
    .await?;
    let at: MonthDay = month_day
        .parse()
        .map_err(|error| AppError::bad_request(error).with_parameter("monthDay"))?;
    Ok(json_response(
        &ws.app.diary_entries_on(&diary_id, at).await?,
    ))
}

/// Make the entry of a day
///
/// A diary has one entry a day. The entry of `date` is made (201) with its mood, weather
/// and place; where the day has an entry, that entry is answered as it is (200) and
/// nothing of the request is stored, so of two requests that make the entry of one day
/// at once, one makes it and the other is answered with it. The server chooses the
/// identifier, the creator and the times. The body of the entry is written by
/// `patchDiaryEntry`, or together in its room (`joinDoc`).
///
/// `date` is the day as the writer's own calendar has it, which the server does not
/// know: it must be a day of the calendar (`2026-02-30` is 400 at `/date`), and no more
/// than two days after the server's own day in UTC (400 at `/date`), which every time
/// zone's today is within. A day in the past is written as any other. The day of an
/// entry never changes afterwards.
///
/// A diary has at most 40,000 entries, those in the trash not counted: one more is 400
/// with the reason `too_many_entries` and the `limit`. An entry in the trash holds no
/// day: its day takes a new entry.
///
/// The user's other sessions are told with `blockChanged`, whose block of the `type`
/// `entry` has the day as its `title` and the day, mood, weather and place among its
/// `fields`.
#[utoipa::path(
    post,
    path = "/w/{workspaceId}/diaries/{diaryId}/entries",
    operation_id = "createDiaryEntry",
    tag = tag::DIARIES,
    params(OwnWorkspaceId, DiaryId),
    request_body = NewDiaryEntry,
    responses(
        (status = 201, description = "The entry that was made.", body = DiaryEntry),
        (status = 200, description = "The entry the day had: nothing was made or changed.", body = DiaryEntry),
    ),
)]
async fn handle_create_diary_entry(
    InDiary(ws): InDiary,
    Path((_, diary_id)): Path<(String, String)>,
    body: Bytes,
) -> ApiResult {
    let user_id = ws.caller.user_id.as_str();
    require_container_permission(
        &ws.app,
        user_id,
        &diary_id,
        ContainerPermission::ManageCards,
        "access denied to create entry",
    )
    .await?;
    let new: NewDiaryEntry = decode_object(&body)?;
    let today = DiaryDate::of_utc_millis(now_millis())
        .ok_or_else(|| AppError::internal("the clock of the server tells no day"))?;
    let date = new.check_request(today)?;
    let (entry, answer) = ws
        .app
        .create_diary_entry(&diary_id, date, new, user_id)
        .await?;
    tracing::debug!(diary_id, entry_id = %entry.id, user_id, ?answer, "CreateDiaryEntry");
    Ok(match answer {
        EntryAnswer::Made => created(&entry),
        EntryAnswer::ThereAlready => json_response(&entry),
    })
}

/// An entry
///
/// The entry with its body.
#[utoipa::path(
    get,
    path = "/w/{workspaceId}/diaries/{diaryId}/entries/{entryId}",
    operation_id = "getDiaryEntry",
    tag = tag::DIARIES,
    params(OwnWorkspaceId, DiaryId, EntryId),
    responses((status = 200, description = "The entry.", body = DiaryEntry)),
)]
async fn handle_get_diary_entry(
    InDiary(ws): InDiary,
    Path((_, diary_id, entry_id)): Path<(String, String, String)>,
) -> ApiResult {
    let block = entry_with_permission(
        &ws,
        &diary_id,
        &entry_id,
        ContainerPermission::View,
        READ_DENIED,
    )
    .await?;
    Ok(json_response(&DiaryEntry::from(block)))
}

/// Change an entry
///
/// Its mood, its weather, its place, its whole body (`body`), or blocks appended after
/// its current body (`appendBody`). `body` and `appendBody` cannot be sent together.
/// A whole-body replacement (`body`) is sent with `readAt`, the `updatedAt` of the entry
/// as it was read: `readAt` is compared against when the entry's body last changed, so
/// metadata-only patches (`mood`, `weather`, `place`) do not cause 409 conflicts while
/// any change to the body does. `appendBody` appends inside one server transaction
/// without requiring `readAt`, refusing (400 at `/appendBody`) if the combined body
/// exceeds the body limits (`BODY_MAX_BYTES`, `BODY_MAX_NODES`, `BODY_MAX_DEPTH`).
/// Where the body is being edited together (the same user in two windows), what was
/// typed is written to the entry first, so a writer who read it before is refused (for
/// `body`) or appended after the latest text (for `appendBody`). The day of the entry
/// does not change: the patch has no member for it.
#[utoipa::path(
    patch,
    path = "/w/{workspaceId}/diaries/{diaryId}/entries/{entryId}",
    operation_id = "patchDiaryEntry",
    tag = tag::DIARIES,
    params(OwnWorkspaceId, DiaryId, EntryId),
    request_body = DiaryEntryPatch,
    responses((status = 200, description = "The entry as it is now.", body = DiaryEntry)),
)]
async fn handle_patch_diary_entry(
    InDiary(ws): InDiary,
    Path((_, diary_id, entry_id)): Path<(String, String, String)>,
    body: Bytes,
) -> ApiResult {
    let user_id = ws.caller.user_id.as_str();
    let entry = entry_with_permission(
        &ws,
        &diary_id,
        &entry_id,
        ContainerPermission::ManageCards,
        "access denied to patch entry",
    )
    .await?;
    let patch = decode_diary_entry_patch(&body)?;
    patch.check_request()?;
    if let Some(body) = patch.body.new_body() {
        ws.app.check_body_files(&diary_id, Some(body)).await?;
    } else if let Some(extra) = patch.append_body.as_ref()
        && !ws.app.has_body_files(&diary_id, Some(extra)).await?
    {
        return Err(
            AppError::bad_request(fvoci_model::BlockError::BodyFileNotOfContainer)
                .with_field("appendBody"),
        );
    }
    let patched = ws
        .app
        .patch_diary_entry(
            &diary_id,
            &entry.id,
            &patch,
            user_id,
            ChangeNotification::Send,
        )
        .await?;
    tracing::debug!(diary_id, entry_id, user_id, "PatchDiaryEntry");
    Ok(json_response(&DiaryEntry::from(patched)))
}

/// Delete an entry
///
/// An entry with a body or any note (mood, weather or place) goes to the trash of its
/// diary, where it holds no day so the day takes a new entry; it is restored by
/// `restoreDiaryEntry`, and gone for good when the trash is emptied of it. A completely
/// empty entry (no body, no mood, no weather and no place) is removed for good without
/// entering the trash.
#[utoipa::path(
    delete,
    path = "/w/{workspaceId}/diaries/{diaryId}/entries/{entryId}",
    operation_id = "deleteDiaryEntry",
    tag = tag::DIARIES,
    params(OwnWorkspaceId, DiaryId, EntryId),
    responses((status = 204, description = "The entry is deleted.")),
)]
async fn handle_delete_diary_entry(
    InDiary(ws): InDiary,
    Path((_, diary_id, entry_id)): Path<(String, String, String)>,
) -> ApiResult {
    let user_id = ws.caller.user_id.as_str();
    let entry = entry_with_permission(
        &ws,
        &diary_id,
        &entry_id,
        ContainerPermission::ManageCards,
        "access denied to delete entry",
    )
    .await?;
    ws.app
        .delete_diary_entry(&diary_id, &entry.id, user_id, ChangeNotification::Send)
        .await?;
    tracing::debug!(diary_id, entry_id, user_id, "DeleteDiaryEntry");
    Ok(no_content())
}

/// The deleted entries of a diary
///
/// The entries of the diary that are in the trash, the last deleted first: each with its
/// day and its mood, when it was deleted and by whom. One page of the list, as
/// `getDeletedCards` pages its own.
#[utoipa::path(
    get,
    path = "/w/{workspaceId}/diaries/{diaryId}/trash/entries",
    operation_id = "getDeletedDiaryEntries",
    tag = tag::DIARIES,
    params(OwnWorkspaceId, DiaryId, PageNumber, PerPage),
    responses((status = 200, description = "The deleted entries of the diary.", body = Vec<DeletedDiaryEntry>)),
)]
async fn handle_get_deleted_diary_entries(
    InDiary(ws): InDiary,
    Path((_, diary_id)): Path<(String, String)>,
    QueryParams(query): QueryParams<PagingQuery>,
) -> ApiResult {
    require_container_permission(
        &ws.app,
        &ws.caller.user_id,
        &diary_id,
        ContainerPermission::ManageCards,
        "access denied to the trash of the diary",
    )
    .await?;
    let page = query.into_list_page()?;
    Ok(json_response(
        &ws.app.deleted_diary_entries(&diary_id, page).await?,
    ))
}

/// Restore a deleted entry
///
/// The entry comes back to its day with its body and the versions of it. Where the day
/// has an entry since, it does not: 409 with the reason `day_taken`, and nothing is
/// changed (deleting the entry of the day makes room). Where the diary has as many
/// entries as one may, 400 with the reason `too_many_entries` and the `limit`. 404 if
/// the trash of the diary has no such entry.
#[utoipa::path(
    post,
    path = "/w/{workspaceId}/diaries/{diaryId}/entries/{entryId}/restore",
    operation_id = "restoreDiaryEntry",
    tag = tag::DIARIES,
    params(OwnWorkspaceId, DiaryId, EntryId),
    responses((status = 200, description = "The entry as it is again.", body = DiaryEntry)),
)]
async fn handle_restore_diary_entry(
    InDiary(ws): InDiary,
    Path((_, diary_id, entry_id)): Path<(String, String, String)>,
) -> ApiResult {
    let user_id = ws.caller.user_id.as_str();
    require_container_permission(
        &ws.app,
        user_id,
        &diary_id,
        ContainerPermission::ManageCards,
        "access denied to undelete entry",
    )
    .await?;
    let entry = ws
        .app
        .undelete_diary_entry(&diary_id, &entry_id, user_id)
        .await?;
    tracing::debug!(diary_id, entry_id, user_id, "UndeleteDiaryEntry");
    Ok(json_response(&entry))
}
