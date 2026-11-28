//! What a person is told inside the application: that somebody mentioned them, gave
//! them a card, or commented on what they watch.

use std::fmt;
use std::str::FromStr;

use serde::{Deserialize, Serialize};

use crate::{ContainerKind, SearchKind};

/// What a notification tells of.
#[derive(
    Debug,
    Clone,
    Copy,
    PartialEq,
    Eq,
    PartialOrd,
    Ord,
    Hash,
    Serialize,
    Deserialize,
    utoipa::ToSchema,
)]
#[serde(rename_all = "snake_case")]
pub enum NotificationKind {
    /// A body, of a card, a page, a unit or a comment, names the person.
    Mention,
    /// A person property of a card took the person.
    Assignment,
    /// A card, a page or a unit the person watches has a new comment.
    Comment,
    /// A scheduled personal review of a course unit is due today.
    ReviewDue,
    /// A course study meeting is starting soon.
    MeetingUpcoming,
    /// An incomplete card assigned to the person is due soon or due today.
    DueSoon,
    /// Somebody added the person to a board, wiki, or course.
    AddedToContainer,
}

impl NotificationKind {
    /// The word of the kind, as the API and the database write it.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Mention => "mention",
            Self::Assignment => "assignment",
            Self::Comment => "comment",
            Self::ReviewDue => "review_due",
            Self::MeetingUpcoming => "meeting_upcoming",
            Self::DueSoon => "due_soon",
            Self::AddedToContainer => "added_to_container",
        }
    }
}

impl fmt::Display for NotificationKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// A word that is no kind of notification.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("unknown kind of notification {0:?}")]
pub struct UnknownNotificationKind(pub String);

impl FromStr for NotificationKind {
    type Err = UnknownNotificationKind;

    fn from_str(text: &str) -> Result<Self, Self::Err> {
        match text {
            "mention" => Ok(Self::Mention),
            "assignment" => Ok(Self::Assignment),
            "comment" => Ok(Self::Comment),
            "review_due" => Ok(Self::ReviewDue),
            "meeting_upcoming" => Ok(Self::MeetingUpcoming),
            "due_soon" => Ok(Self::DueSoon),
            "added_to_container" => Ok(Self::AddedToContainer),
            other => Err(UnknownNotificationKind(other.to_owned())),
        }
    }
}

/// What a notification is about, for a reader who reads it now.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, utoipa::ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct NotificationSubject {
    /// What happened.
    pub kind: NotificationKind,
    /// Who did the latest of it; empty where nobody is known to have. The identifier
    /// alone, as a card says who wrote it: the name is asked of the workspace's
    /// people, where its reader is told the names they may see.
    pub actor_id: String,
    /// How many times it happened: what happens to the same card or page, of the same
    /// kind, within an hour of the first of it, is told in one notification.
    pub times: i64,
    /// How many people did it, `actorId` among them; 0 where nobody is known to have.
    /// A hundred are counted at most.
    pub people: i64,
    /// The workspace of the card or the page.
    pub workspace_id: String,
    /// The board the card is on, or the wiki the page is in.
    pub container_id: String,
    /// What that container is: a board, or a wiki.
    pub container_kind: ContainerKind,
    /// The card or the page it is about: what the reader opens.
    pub block_id: String,
    /// What that is.
    pub block_kind: SearchKind,
    /// Its title as it is now.
    pub title: String,
    /// The comment the latest of it happened in, where that is still there; empty
    /// where it happened in the card or the page itself, or the comment went.
    pub comment_id: String,
    /// The name of the personal API token through which an agent of `actorId` did the
    /// latest of it; left out where `actorId` did it themselves. A token that tells its
    /// owner (`notifySelf`) names them as `actorId` of their own notifications.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agent_token_name: Option<String>,
}

/// One thing a person was told.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, utoipa::ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct Notification {
    /// Its identifier.
    pub id: String,
    /// When it was made: when the first of it happened.
    pub created_at: i64,
    /// When the latest of it happened; the notifications are listed by it, the
    /// newest first.
    pub updated_at: i64,
    /// When its reader read it; 0 until then, and again once more of it happens.
    pub read_at: i64,
    /// When its reader put it away (`archiveNotification`); 0 while it is in the
    /// inbox, and again once more of it happens. One put away is read.
    pub archived_at: i64,
    /// What it is about; `null` where its reader does not read that now (it is gone,
    /// or theirs no more), and is told nothing of it: not what, not where, not by
    /// whom.
    pub subject: Option<NotificationSubject>,
}

/// How many notifications a person has not read.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, utoipa::ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct UnreadNotifications {
    /// How many.
    pub count: i64,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_kind_reads_back_from_its_word() {
        for kind in [
            NotificationKind::Mention,
            NotificationKind::Assignment,
            NotificationKind::Comment,
            NotificationKind::ReviewDue,
            NotificationKind::MeetingUpcoming,
            NotificationKind::DueSoon,
            NotificationKind::AddedToContainer,
        ] {
            assert_eq!(kind.as_str().parse(), Ok(kind));
            assert_eq!(
                serde_json::to_value(kind).expect("JSON"),
                serde_json::json!(kind.as_str())
            );
        }
        assert!("mentions".parse::<NotificationKind>().is_err());
    }
}

/// Which of a reader's notifications are listed (`getNotifications`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct NotificationFilter {
    /// Only those not read.
    pub unread: bool,
    /// Only those that tell of this.
    pub kind: Option<NotificationKind>,
    /// Those put away instead of those in the inbox.
    pub archived: bool,
}

impl NotificationFilter {
    /// The inbox: every notification that was not put away.
    pub const INBOX: Self = Self {
        unread: false,
        kind: None,
        archived: false,
    };
}
