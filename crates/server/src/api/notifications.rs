//! What the user of the session was told: that they were mentioned, that a card was
//! given to them, and that what they watch has a new comment; and the inbox sorted:
//! marked unread again, put away and taken back.

use axum::extract::{Path, State};
use fvoci_model::{Notification, NotificationFilter, NotificationKind, UnreadNotifications};
use serde::Deserialize;
use utoipa_axum::router::OpenApiRouter;
use utoipa_axum::routes;

use super::contract::params::{
    ArchivedOnly, NotificationId, NotificationKindOnly, Page, PerPage, ReadThrough, UnreadOnly,
};
use super::contract::tag;
use super::query::{QueryParams, int_param, paging_from_strings};
use super::{ApiResult, RequiredSession, json_response, no_content};
use crate::app::App;
use crate::auth::now_millis;
use crate::error::AppError;

#[derive(Debug, Default, Deserialize)]
#[serde(default, rename_all = "camelCase", deny_unknown_fields)]
struct ReadNotificationsQuery {
    through: String,
}

#[derive(Debug, Default, Deserialize)]
#[serde(default, rename_all = "camelCase", deny_unknown_fields)]
struct NotificationsQuery {
    page: String,
    per_page: String,
    unread: String,
    kind: String,
    archived: String,
}

impl NotificationsQuery {
    /// Which of the user's notifications the query lists.
    fn filter(&self) -> Result<NotificationFilter, AppError> {
        let kind = if self.kind.is_empty() {
            None
        } else {
            Some(self.kind.parse::<NotificationKind>().map_err(|_| {
                AppError::bad_request("invalid `kind` parameter: no kind of notification")
                    .with_parameter("kind")
            })?)
        };
        Ok(NotificationFilter {
            unread: flag(&self.unread, "unread")?,
            kind,
            archived: flag(&self.archived, "archived")?,
        })
    }
}

/// A parameter that is `true` or `false`; left out, `false`.
fn flag(text: &str, name: &str) -> Result<bool, AppError> {
    match text {
        "" | "false" => Ok(false),
        "true" => Ok(true),
        _ => Err(AppError::bad_request(format_args!(
            "invalid `{name}` parameter: `true` or `false`"
        ))
        .with_parameter(name)),
    }
}

pub(super) fn routes() -> OpenApiRouter<App> {
    OpenApiRouter::new()
        .routes(routes!(handle_get_notifications))
        .routes(routes!(handle_get_unread_notifications))
        .routes(routes!(handle_read_notifications))
        .routes(routes!(handle_read_notification))
        .routes(routes!(handle_unread_notification))
        .routes(routes!(
            handle_archive_notification,
            handle_unarchive_notification
        ))
        .routes(routes!(handle_archive_read_notifications))
}

/// The notifications of the user
///
/// One page of what the user of the session was told, in every organization the
/// session opens, the newest first (by `updatedAt`): that somebody mentioned them in a
/// card, a page, a unit or a comment (`mention`), gave them a card (`assignment`), or
/// commented on a card, a page or a unit they watch (`comment`; one named in that comment
/// is told of the mention alone). Nobody is told of their own doing, nor of anything in a
/// user's own space, nor of a kind they switched off (`notifyMentions`,
/// `notifyAssignments`, `notifyComments` of `patchUserSettings`): what is off is not made
/// at all.
///
/// Whoever writes a card, a page or a unit, is given a card, or comments on one watches
/// it (`createSubscription`), until they end that (`deleteSubscription`). What happens to
/// one card, page or unit, of one kind, within an hour of the first of it, is one
/// notification: `times` and `people` count it, `actorId` and `commentId` say the
/// latest, and it is unread again. One person makes a hundred new ones for another
/// within an hour at most; past that, what they do joins only those there.
///
/// A notification is made only for somebody who reads the card, page or unit at that
/// time, and each one listed is asked again: its `subject` says what it is about, as
/// that is now, only where the user reads it now with this session. Where they do not
/// (it is in the trash or gone, or they left the container or the workspace) `subject` is
/// `null`, and nothing says what it was about, where or by whom. Such a one is still
/// listed, and counted while unread.
///
/// The inbox is listed (those put away left out) unless `archived` asks for the archive:
/// what the user put away (`archiveNotification`), read. `unread` and `kind` narrow
/// either.
///
/// What an organization that takes only sessions made through its provider told the
/// user is left out for a session made otherwise, as `getMyMemberships` leaves its
/// boards out: it is not listed, not counted, and not marked as read by such a
/// session, and it is there again for a session made through the provider.
///
/// A notification that was read is kept for ninety days, and a user has ten thousand
/// at most: past that the oldest go, those that were read first.
#[utoipa::path(
    get,
    path = "/notifications",
    operation_id = "getNotifications",
    tag = tag::NOTIFICATIONS,
    params(Page, PerPage, UnreadOnly, NotificationKindOnly, ArchivedOnly),
    responses((status = 200, description = "The notifications of the page, the newest first.", body = Vec<Notification>)),
)]
async fn handle_get_notifications(
    State(app): State<App>,
    RequiredSession(session): RequiredSession,
    QueryParams(query): QueryParams<NotificationsQuery>,
) -> ApiResult {
    let page = paging_from_strings(&query.page, &query.per_page)?;
    let filter = query.filter()?;
    Ok(json_response(
        &app.notifications(&session, filter, page).await?,
    ))
}

/// How many notifications the user has not read
///
/// The number of notifications of the user of the session that are not read, in every
/// organization the session opens (`getNotifications`): those whose `subject` is
/// `null` among them.
#[utoipa::path(
    get,
    path = "/notifications/unread",
    operation_id = "getUnreadNotifications",
    tag = tag::NOTIFICATIONS,
    responses((status = 200, description = "How many.", body = UnreadNotifications)),
)]
async fn handle_get_unread_notifications(
    State(app): State<App>,
    RequiredSession(session): RequiredSession,
) -> ApiResult {
    let count = app.unread_notifications((&session).into()).await?;
    Ok(json_response(&UnreadNotifications { count }))
}

/// Mark every notification as read
///
/// Marks as read every notification of the user of the session whose latest happened
/// up to `through`, or up to now where that is left out: a client gives the `updatedAt`
/// of the newest it has shown, so that one that came or grew since stays unread. Those of an
/// organization the session does not open (`getNotifications`) stay as they are.
#[utoipa::path(
    post,
    path = "/notifications/read",
    operation_id = "readNotifications",
    tag = tag::NOTIFICATIONS,
    params(ReadThrough),
    responses((status = 204, description = "They are read.")),
)]
async fn handle_read_notifications(
    State(app): State<App>,
    RequiredSession(session): RequiredSession,
    QueryParams(query): QueryParams<ReadNotificationsQuery>,
) -> ApiResult {
    let through = int_param(&query.through, "through", now_millis())?;
    app.read_notifications((&session).into(), through).await?;
    Ok(no_content())
}

/// Mark a notification as read
///
/// Marks one notification of the user of the session as read; one that was read stays
/// read since when it was. 404 for a notification of somebody else, as for one that
/// is not there, and for one of an organization the session does not open
/// (`getNotifications`).
#[utoipa::path(
    post,
    path = "/notifications/{notificationId}/read",
    operation_id = "readNotification",
    tag = tag::NOTIFICATIONS,
    params(NotificationId),
    responses((status = 204, description = "It is read.")),
)]
async fn handle_read_notification(
    State(app): State<App>,
    RequiredSession(session): RequiredSession,
    Path(notification_id): Path<String>,
) -> ApiResult {
    app.read_notification((&session).into(), &notification_id)
        .await?;
    Ok(no_content())
}

/// Mark a notification as unread
///
/// Marks one notification of the user of the session as not read, as more of it would:
/// it is counted again (`getUnreadNotifications`), and one that was put away
/// (`archiveNotification`) is back in the inbox. 404 for a notification of somebody
/// else, as for one that is not there, and for one of an organization the session does
/// not open (`getNotifications`).
#[utoipa::path(
    post,
    path = "/notifications/{notificationId}/unread",
    operation_id = "unreadNotification",
    tag = tag::NOTIFICATIONS,
    params(NotificationId),
    responses((status = 204, description = "It is unread.")),
)]
async fn handle_unread_notification(
    State(app): State<App>,
    RequiredSession(session): RequiredSession,
    Path(notification_id): Path<String>,
) -> ApiResult {
    app.unread_notification((&session).into(), &notification_id)
        .await?;
    Ok(no_content())
}

/// Put a notification away
///
/// Puts one notification of the user of the session away, out of the inbox and into
/// the archive (`getNotifications` with `archived=true`): one that was not read is read
/// with it, and one put away stays put away since when it was. One whose `subject` is
/// `null` is put away as any other. More of it within the hour brings it back into the
/// inbox, unread. 404 as for `unreadNotification`.
#[utoipa::path(
    post,
    path = "/notifications/{notificationId}/archive",
    operation_id = "archiveNotification",
    tag = tag::NOTIFICATIONS,
    params(NotificationId),
    responses((status = 204, description = "It is put away.")),
)]
async fn handle_archive_notification(
    State(app): State<App>,
    RequiredSession(session): RequiredSession,
    Path(notification_id): Path<String>,
) -> ApiResult {
    app.archive_notification((&session).into(), &notification_id)
        .await?;
    Ok(no_content())
}

/// Take a notification out of the archive
///
/// Brings one notification of the user of the session back into the inbox; it stays
/// read. One in the inbox stays there. 404 as for `unreadNotification`.
#[utoipa::path(
    delete,
    path = "/notifications/{notificationId}/archive",
    operation_id = "unarchiveNotification",
    tag = tag::NOTIFICATIONS,
    params(NotificationId),
    responses((status = 204, description = "It is in the inbox.")),
)]
async fn handle_unarchive_notification(
    State(app): State<App>,
    RequiredSession(session): RequiredSession,
    Path(notification_id): Path<String>,
) -> ApiResult {
    app.unarchive_notification((&session).into(), &notification_id)
        .await?;
    Ok(no_content())
}

/// Put away every notification that was read
///
/// Puts away every notification of the user of the session that was read and is in the
/// inbox, in every organization the session opens (`getNotifications`); those not read
/// stay. Those of an organization the session does not open stay as they are.
#[utoipa::path(
    post,
    path = "/notifications/archive",
    operation_id = "archiveReadNotifications",
    tag = tag::NOTIFICATIONS,
    responses((status = 204, description = "They are put away.")),
)]
async fn handle_archive_read_notifications(
    State(app): State<App>,
    RequiredSession(session): RequiredSession,
) -> ApiResult {
    app.archive_read_notifications((&session).into()).await?;
    Ok(no_content())
}
