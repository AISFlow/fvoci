//! What a person is told inside the application: their list, as they read it now, and
//! the working off of whom a change may be told to.
//!
//! A notification is made, and listed, only for somebody who reads what it is about
//! at that moment, or may join its open board to read it ([`App::places_read_by`]):
//! once when the list of the store is worked off ([`tend`]), and again each time its
//! reader lists it ([`App::notifications`]). Who is given a card comes to watch it
//! then, if they read it ([`settle`]).
//! What they do not read then, because it is gone, in the trash, another's now or
//! behind a sign-in their session does not open, is listed as something they cannot
//! see: no title, no place, no name. Nobody is told of what they switched off
//! (`notifyMentions` and the others of their settings), and each connection of theirs
//! is told when what it sees of their notifications changed (`Hub::notifications_changed`).

use std::collections::{HashMap, HashSet};

use fvoci_model::{
    ContainerKind, Notification, NotificationKind, NotificationSubject, SearchKind, Session,
};
use fvoci_store::{
    ContainerNotificationPlace, MeetingNotificationPlace, NotificationEvent, NotificationLimits,
    NotificationPlace, NotificationReader, StoredNotification, Told,
};

use super::sso::Asker;
use super::{App, ListPage};
use crate::auth::now_millis;
use crate::error::AppError;
use crate::metrics::NotificationSettled;
use crate::permissions::ContainerPermission;
use crate::scheduler::RecurringTask;

/// An hour, in milliseconds.
const HOUR_MILLIS: i64 = 60 * 60 * 1000;
/// The bounds a notification is made within: what happens to one card or page, of one
/// kind, within an hour of the first of it is one notification (a mention taken out
/// and put back, a card given again, the comments of a busy card); a hundred new ones
/// from one person to another within the hour, the rest joining only those; and ten
/// thousand to a person.
const LIMITS: NotificationLimits = NotificationLimits {
    same_within_millis: HOUR_MILLIS,
    most_made_by_one: 100,
    made_within_millis: HOUR_MILLIS,
    most_of_a_person: 10_000,
};
/// How long a notification is kept after it was read: ninety days.
const READ_KEPT_MILLIS: i64 = 90 * 24 * 60 * 60 * 1000;
/// How often the list of whom to tell is worked off.
const UPKEEP_INTERVAL: std::time::Duration = std::time::Duration::from_secs(1);
/// How many rows of the list one round reads, and how many rounds one run works off
/// at most: the rest is the next run's, a second later.
const ROUND: i64 = 200;
/// See [`ROUND`].
const ROUNDS_A_RUN: usize = 5;
/// How long a row that could not be worked off waits for its next try, at first and
/// at most; doubled with every failure in a row.
const FIRST_RETRY_MILLIS: i64 = 5_000;
/// See [`FIRST_RETRY_MILLIS`].
const LONGEST_RETRY_MILLIS: i64 = 10 * 60 * 1000;

/// What a notification, or a row of the list, names.
#[derive(Debug, Clone, Copy)]
struct Named<'a> {
    /// The card or the page it is about.
    block_id: &'a str,
    /// Where it happened: that block, or a comment on it.
    source_id: &'a str,
}

/// What a reader reads of what a notification names, now.
#[derive(Debug, Clone)]
struct Seen {
    /// Where the card or the page is, and what it is called.
    place: NotificationPlace,
    /// The comment it happened in, where that is there on the card or the page; empty
    /// where it happened in the card or the page itself, or the comment went.
    comment_id: String,
}

impl Seen {
    /// Whether what `named` happened in is there: the card or the page, or the
    /// comment on it.
    fn holds(&self, named: &Named<'_>) -> bool {
        named.source_id == named.block_id || !self.comment_id.is_empty()
    }
}

/// On whose word a person is let to what is an organization's.
#[derive(Debug, Clone, Copy)]
enum Entry<'a> {
    /// A session's: its user enters each workspace as any request of it does, and one
    /// its sign-in does not open is closed to it.
    Session(Asker<'a>),
    /// Nobody's: what the user is there, whatever session they may come with later.
    Standing,
}

impl App {
    /// Where each of `named` is now, for a user who reads it now; `None` for what the
    /// user does not read. For a handle behind the wall of the organization the rows
    /// are of.
    ///
    /// What is read is a card that is no template of its board, or a page, that is
    /// not in the trash, on a board or in a wiki that is no template, in a workspace
    /// the user enters (`entry`), which the user reads there, or may join to read
    /// (`Grant::reads_on_joining`: an open board of their workspace they did not join,
    /// whose title and way to join they are shown), as [`App::container_access`] says,
    /// asked once a container. Where it happened in a comment, [`Seen::comment_id`]
    /// says whether that is there, on that card or page.
    async fn places_read_by(
        &self,
        user_id: &str,
        entry: Entry<'_>,
        named: &[Named<'_>],
    ) -> Result<Vec<Option<Seen>>, AppError> {
        let block_ids: Vec<&str> = named.iter().map(|named| named.block_id).collect();
        let places: HashMap<String, NotificationPlace> = self
            .store()
            .notification_places(&block_ids)
            .await?
            .into_iter()
            .map(|place| (place.block_id.clone(), place))
            .collect();
        let comment_ids: Vec<&str> = named
            .iter()
            .filter(|named| named.source_id != named.block_id)
            .map(|named| named.source_id)
            .collect();
        let comments: HashMap<String, String> = self
            .store()
            .comments_with_parents(&comment_ids)
            .await?
            .into_iter()
            .collect();
        let mut entered: HashMap<&str, Option<Self>> = HashMap::new();
        let mut reads: HashMap<&str, bool> = HashMap::new();
        let mut read = Vec::with_capacity(named.len());
        for named in named {
            let Some(place) = places.get(named.block_id) else {
                read.push(None);
                continue;
            };
            let workspace_id = place.workspace_id.as_str();
            if !entered.contains_key(workspace_id) {
                let app = self.entered_workspace(entry, workspace_id).await?;
                entered.insert(workspace_id, app);
            }
            let Some(app) = entered.get(workspace_id).and_then(Option::as_ref) else {
                read.push(None);
                continue;
            };
            let container_id = place.container_id.as_str();
            let reads_it = if let Some(reads_it) = reads.get(container_id) {
                *reads_it
            } else {
                let reads_it = app
                    .container_access(user_id, container_id)
                    .await?
                    .is_some_and(|access| {
                        access.grant.reads_on_joining()
                            && !access.container.is_template
                            && access.container.workspace_id == workspace_id
                    });
                reads.insert(container_id, reads_it);
                reads_it
            };
            let comment_there = named.source_id != named.block_id
                && comments.get(named.source_id).map(String::as_str) == Some(named.block_id);
            read.push(reads_it.then(|| Seen {
                place: place.clone(),
                comment_id: if comment_there {
                    named.source_id.to_owned()
                } else {
                    String::new()
                },
            }));
        }
        Ok(read)
    }

    /// Where each course meeting among `meeting_ids` is now, for an enrolled course
    /// member whose session enters its workspace and reads the course.
    async fn meeting_places_read_by(
        &self,
        user_id: &str,
        entry: Entry<'_>,
        meeting_ids: &[&str],
    ) -> Result<Vec<Option<Seen>>, AppError> {
        let places: HashMap<String, MeetingNotificationPlace> = self
            .store()
            .meeting_notification_places(meeting_ids)
            .await?
            .into_iter()
            .map(|place| (place.meeting_id.clone(), place))
            .collect();
        let mut entered: HashMap<&str, Option<Self>> = HashMap::new();
        let mut reads: HashMap<&str, bool> = HashMap::new();
        let mut read = Vec::with_capacity(meeting_ids.len());
        for meeting_id in meeting_ids {
            let Some(place) = places.get(*meeting_id) else {
                read.push(None);
                continue;
            };
            let workspace_id = place.workspace_id.as_str();
            if !entered.contains_key(workspace_id) {
                let app = self.entered_workspace(entry, workspace_id).await?;
                entered.insert(workspace_id, app);
            }
            let Some(app) = entered.get(workspace_id).and_then(Option::as_ref) else {
                read.push(None);
                continue;
            };
            let container_id = place.container_id.as_str();
            let reads_it = if let Some(reads_it) = reads.get(container_id) {
                *reads_it
            } else {
                let reads_it = app
                    .container_access(user_id, container_id)
                    .await?
                    .is_some_and(|access| {
                        access.has_membership()
                            && access.grant.allows(ContainerPermission::View)
                            && !access.container.is_template
                            && access.container.workspace_id == workspace_id
                    });
                reads.insert(container_id, reads_it);
                reads_it
            };
            read.push(reads_it.then(|| Seen {
                place: NotificationPlace {
                    block_id: place.meeting_id.clone(),
                    block_kind: SearchKind::Unit,
                    title: place.title.clone(),
                    container_id: place.container_id.clone(),
                    container_kind: ContainerKind::Course,
                    workspace_id: place.workspace_id.clone(),
                },
                comment_id: String::new(),
            }));
        }
        Ok(read)
    }

    /// Where each container among `container_ids` is now, for a user who reads it or may
    /// join it (`AddedToContainer`).
    async fn container_places_read_by(
        &self,
        user_id: &str,
        entry: Entry<'_>,
        container_ids: &[&str],
    ) -> Result<Vec<Option<Seen>>, AppError> {
        let places: HashMap<String, ContainerNotificationPlace> = self
            .store()
            .container_notification_places(container_ids)
            .await?
            .into_iter()
            .map(|place| (place.container_id.clone(), place))
            .collect();
        let mut entered: HashMap<&str, Option<Self>> = HashMap::new();
        let mut reads: HashMap<&str, bool> = HashMap::new();
        let mut read = Vec::with_capacity(container_ids.len());
        for container_id in container_ids {
            let Some(place) = places.get(*container_id) else {
                read.push(None);
                continue;
            };
            let workspace_id = place.workspace_id.as_str();
            if !entered.contains_key(workspace_id) {
                let app = self.entered_workspace(entry, workspace_id).await?;
                entered.insert(workspace_id, app);
            }
            let Some(app) = entered.get(workspace_id).and_then(Option::as_ref) else {
                read.push(None);
                continue;
            };
            let cid = place.container_id.as_str();
            let reads_it = if let Some(reads_it) = reads.get(cid) {
                *reads_it
            } else {
                let reads_it = app
                    .container_access(user_id, cid)
                    .await?
                    .is_some_and(|access| {
                        access.grant.reads_on_joining()
                            && !access.container.is_template
                            && access.container.workspace_id == workspace_id
                    });
                reads.insert(cid, reads_it);
                reads_it
            };
            read.push(reads_it.then(|| Seen {
                place: NotificationPlace {
                    block_id: String::new(),
                    block_kind: search_kind_of_container(place.container_kind),
                    title: place.title.clone(),
                    container_id: place.container_id.clone(),
                    container_kind: place.container_kind,
                    workspace_id: place.workspace_id.clone(),
                },
                comment_id: String::new(),
            }));
        }
        Ok(read)
    }

    /// This handle for what is in the workspace, as `entry` lets its user in; `None`
    /// for a workspace a session does not enter: one its user is no member of, and
    /// one its sign-in does not open.
    async fn entered_workspace(
        &self,
        entry: Entry<'_>,
        workspace_id: &str,
    ) -> Result<Option<Self>, AppError> {
        let Entry::Session(asker) = entry else {
            return Ok(Some(self.clone()));
        };
        Ok(self
            .enter_workspace_quietly(asker, workspace_id)
            .await?
            .map(|entered| entered.app))
    }

    /// A page of what the session's user was told, in every organization the session
    /// opens, the newest first. Each is asked again whether the user reads what it is
    /// about, as the session opens it: one they do not read now has no subject.
    ///
    /// # Errors
    ///
    /// Returns the store's error if a fact cannot be read: that is no "cannot see".
    pub async fn notifications(
        &self,
        session: &Session,
        page: ListPage,
    ) -> Result<Vec<Notification>, AppError> {
        let user_id = session.user_id.as_str();
        let stored = self
            .store()
            .notifications_of_user(reader(session.into()), page.paging())
            .await?;
        let mut places: HashMap<&str, Seen> = HashMap::new();
        let org_ids: HashSet<&str> = stored.iter().map(|told| told.org_id.as_str()).collect();
        for org_id in org_ids {
            let org_app = self.in_organization(org_id)?;
            let of_blocks: Vec<&StoredNotification> = stored
                .iter()
                .filter(|told| {
                    told.org_id == org_id
                        && !matches!(
                            told.kind,
                            NotificationKind::MeetingUpcoming | NotificationKind::AddedToContainer
                        )
                })
                .collect();
            if !of_blocks.is_empty() {
                let named: Vec<Named<'_>> = of_blocks
                    .iter()
                    .map(|told| Named {
                        block_id: &told.block_id,
                        source_id: &told.source_id,
                    })
                    .collect();
                let read = org_app
                    .places_read_by(user_id, Entry::Session(session.into()), &named)
                    .await?;
                for (told, place) in of_blocks.into_iter().zip(read) {
                    if let Some(place) = place {
                        places.insert(told.id.as_str(), place);
                    }
                }
            }
            let of_meetings: Vec<&StoredNotification> = stored
                .iter()
                .filter(|told| {
                    told.org_id == org_id && told.kind == NotificationKind::MeetingUpcoming
                })
                .collect();
            if !of_meetings.is_empty() {
                let meeting_ids: Vec<&str> = of_meetings
                    .iter()
                    .map(|told| told.block_id.as_str())
                    .collect();
                let read = org_app
                    .meeting_places_read_by(user_id, Entry::Session(session.into()), &meeting_ids)
                    .await?;
                for (told, place) in of_meetings.into_iter().zip(read) {
                    if let Some(place) = place {
                        places.insert(told.id.as_str(), place);
                    }
                }
            }
            let of_containers: Vec<&StoredNotification> = stored
                .iter()
                .filter(|told| {
                    told.org_id == org_id && told.kind == NotificationKind::AddedToContainer
                })
                .collect();
            if !of_containers.is_empty() {
                let container_ids: Vec<&str> = of_containers
                    .iter()
                    .map(|told| told.container_id.as_str())
                    .collect();
                let read = org_app
                    .container_places_read_by(
                        user_id,
                        Entry::Session(session.into()),
                        &container_ids,
                    )
                    .await?;
                for (told, place) in of_containers.into_iter().zip(read) {
                    if let Some(place) = place {
                        places.insert(told.id.as_str(), place);
                    }
                }
            }
        }
        Ok(stored
            .iter()
            .map(|told| Notification {
                id: told.id.clone(),
                created_at: told.created_at,
                updated_at: told.updated_at,
                read_at: told.read_at,
                subject: places
                    .remove(told.id.as_str())
                    .map(|seen| subject(told, seen)),
            })
            .collect())
    }

    /// How many notifications the asker has not read, in every organization their
    /// session opens: those they cannot see any more among them, until they are read.
    ///
    /// # Errors
    ///
    /// Returns the store's error if the query fails.
    pub async fn unread_notifications(&self, asker: Asker<'_>) -> Result<i64, AppError> {
        Ok(self
            .store()
            .unread_notifications_of_user(reader(asker))
            .await?)
    }

    /// Marks a notification of the asker as read.
    ///
    /// # Errors
    ///
    /// Returns a not-found error for a notification that is not the asker's, as for
    /// one that is not there, and for one of an organization their session does not
    /// open.
    pub async fn read_notification(
        &self,
        asker: Asker<'_>,
        notification_id: &str,
    ) -> Result<(), AppError> {
        let Some(org_id) = self
            .store()
            .read_notification(reader(asker), notification_id, now_millis())
            .await?
        else {
            return Err(AppError::not_found("notification"));
        };
        self.ws()
            .notifications_changed(asker.user_id, &[&org_id])
            .await;
        Ok(())
    }

    /// Marks as read every notification of the asker whose latest happened up to
    /// `through`, in the organizations their session opens.
    ///
    /// # Errors
    ///
    /// Returns the store's error if the statement fails.
    pub async fn read_notifications(&self, asker: Asker<'_>, through: i64) -> Result<(), AppError> {
        let read = self
            .store()
            .read_notifications_of_user(reader(asker), through, now_millis())
            .await?;
        let org_ids: Vec<&str> = read.keys().map(String::as_str).collect();
        self.ws()
            .notifications_changed(asker.user_id, &org_ids)
            .await;
        Ok(())
    }

    /// Forgets the notifications of every organization that were read more than
    /// ninety days ago. It takes the housekeeping's reach itself; what fails is
    /// logged, and left for the next run.
    pub async fn forget_old_notifications(&self) {
        let app = self.housekeeping();
        if let Err(error) = app.require_server_lock() {
            tracing::warn!(%error, "the notifications read long ago are not forgotten");
            return;
        }
        let read_before = now_millis().saturating_sub(READ_KEPT_MILLIS);
        match app.store().forget_read_notifications(read_before).await {
            Ok(0) => {}
            Ok(forgotten) => tracing::info!(forgotten, "forgot notifications read long ago"),
            Err(error) => tracing::error!(%error, "unable to forget notifications read long ago"),
        }
    }
}

/// What a notification is about, for a reader who reads what `seen` says now.
fn subject(told: &StoredNotification, seen: Seen) -> NotificationSubject {
    let Seen { place, comment_id } = seen;
    NotificationSubject {
        kind: told.kind,
        actor_id: told.actor_id.clone(),
        times: told.times,
        people: told.people,
        workspace_id: place.workspace_id,
        container_id: place.container_id,
        container_kind: place.container_kind,
        block_id: place.block_id,
        block_kind: place.block_kind,
        title: place.title,
        comment_id,
        agent_token_name: told.actor_token_name.clone(),
    }
}

/// The asker as the store reads their notifications: with the session they ask by.
fn reader(asker: Asker<'_>) -> NotificationReader<'_> {
    NotificationReader {
        user_id: asker.user_id,
        making: asker.making,
    }
}

/// When a row that failed for the `attempts`-th time in a row is tried again.
fn retry_at(now: i64, attempts: i64) -> i64 {
    let doublings = u32::try_from(attempts.clamp(0, 16)).unwrap_or(16);
    let wait = FIRST_RETRY_MILLIS
        .saturating_mul(2_i64.saturating_pow(doublings))
        .min(LONGEST_RETRY_MILLIS);
    now.saturating_add(wait)
}

const fn search_kind_of_container(kind: ContainerKind) -> SearchKind {
    match kind {
        ContainerKind::Board => SearchKind::Card,
        ContainerKind::Wiki => SearchKind::Page,
        ContainerKind::Course => SearchKind::Unit,
        ContainerKind::Diary => SearchKind::Entry,
    }
}

/// Whether the person of `event` reads now what it happened in, as anybody they may
/// come with later ([`Entry::Standing`]). `app` reaches every organization.
async fn reads_it(app: &App, event: &NotificationEvent) -> Result<bool, AppError> {
    if event.kind == NotificationKind::AddedToContainer {
        return Ok(app
            .in_organization(&event.org_id)?
            .container_places_read_by(&event.recipient_id, Entry::Standing, &[&event.container_id])
            .await?
            .first()
            .and_then(Option::as_ref)
            .is_some());
    }
    let named = Named {
        block_id: &event.block_id,
        source_id: &event.source_id,
    };
    Ok(app
        .in_organization(&event.org_id)?
        .places_read_by(&event.recipient_id, Entry::Standing, &[named])
        .await?
        .first()
        .and_then(Option::as_ref)
        .is_some_and(|seen| seen.holds(&named)))
}

/// Whether the person of `event`, who reads what it happened in, is told of it:
/// somebody else did it, or an agent of theirs through a token that tells them (only
/// such a token's rows name them as its doer), and the person did not switch its kind
/// off.
async fn tells(app: &App, event: &NotificationEvent) -> Result<bool, AppError> {
    if event.actor_id == event.recipient_id && event.actor_token_name.is_none() {
        return Ok(false);
    }
    let settings = app.store().get_user_settings(&event.recipient_id).await?;
    Ok(settings.notifies(event.kind))
}

/// Works off one row of the list: the person is told if they read what it is about
/// ([`reads_it`]) and are to be told ([`tells`]), and the row is dropped otherwise.
/// Who reads a card they were given watches it from then on, told of it or not: nobody
/// comes to watch, or to know the identifier of, what they do not read. Each
/// connection of theirs is told of what it sees change.
async fn settle(app: &App, event: &NotificationEvent) -> Result<(), AppError> {
    let reads = reads_it(app, event).await?;
    if reads && event.kind == NotificationKind::Assignment {
        app.in_organization(&event.org_id)?
            .store()
            .watch_given(&event.block_id, &event.recipient_id)
            .await?;
    }
    let settled = if reads && tells(app, event).await? {
        match app.store().tell_of(event, now_millis(), LIMITS).await? {
            Told::Made(_) => Some(NotificationSettled::Made),
            Told::Joined(_) => Some(NotificationSettled::Joined),
            Told::Counted(_) => Some(NotificationSettled::Counted),
            Told::OverTheLimit => Some(NotificationSettled::OverTheLimit),
            Told::NotedAgain => None,
        }
    } else if app.store().drop_notification_event(event).await? {
        Some(NotificationSettled::NotTold)
    } else {
        None
    };
    let Some(settled) = settled else {
        return Ok(());
    };
    app.metrics().notification_settled(settled);
    if matches!(
        settled,
        NotificationSettled::Made | NotificationSettled::Joined
    ) {
        app.ws()
            .notifications_changed(&event.recipient_id, &[&event.org_id])
            .await;
    }
    Ok(())
}

/// Works off the rows that are due, a few rounds of them. A row that fails is tried
/// again later and later, and the others are worked off all the same.
async fn work_off(app: &App) -> Result<(), AppError> {
    for _ in 0..ROUNDS_A_RUN {
        let now = now_millis();
        let events = app.store().due_notification_events(now, ROUND).await?;
        if events.is_empty() {
            break;
        }
        for event in &events {
            if let Err(error) = settle(app, event).await {
                tracing::warn!(
                    %error,
                    attempts = event.attempts.saturating_add(1),
                    "a person could not be told of a change; it is tried again later"
                );
                app.store()
                    .notification_event_failed(event, retry_at(now, event.attempts))
                    .await?;
            }
        }
    }
    Ok(())
}

/// Works off whom the changes so far may be told to, once, now: what the server's
/// task does every interval, for a server that holds the lock. One run at a time: a
/// second one waits for the first. The server does it by itself while it serves;
/// this is for its tests.
pub async fn tend(app: &App) {
    let _turn = app.inner.telling.lock().await;
    let app = app.housekeeping();
    if !app.holds_server_lock() {
        return;
    }
    if let Err(error) = work_off(&app).await {
        tracing::error!(%error, "the list of whom to tell could not be worked off");
    }
}

/// Starts the task that works off whom to tell ([`tend`]); `None` where it is worked
/// off only when it is called.
pub(crate) fn start_upkeep(app: &App) -> Option<RecurringTask> {
    if app.config().notifications_by_hand {
        return None;
    }
    let app = app.clone();
    Some(RecurringTask::spawn_starting_now(
        "tellPeople",
        UPKEEP_INTERVAL,
        move || {
            let app = app.clone();
            async move { tend(&app).await }
        },
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_row_that_failed_waits_twice_as_long_each_time_up_to_ten_minutes() {
        assert_eq!(retry_at(1000, 0), 6000);
        assert_eq!(retry_at(1000, 1), 11_000);
        assert_eq!(retry_at(0, 20), LONGEST_RETRY_MILLIS);
        assert_eq!(retry_at(0, -1), FIRST_RETRY_MILLIS);
    }
}
