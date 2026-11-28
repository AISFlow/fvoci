//! What the user of a session does with their notifications besides reading them
//! (`notification_triage.rs` of the store): marking one unread again, putting one away
//! and taking it back, and putting away every one that was read. Each tells the user's
//! connections that what they were told changed (`notificationsChanged`), of the
//! organizations it changed in; what an organization the session does not open told
//! them is not reached, as for reading.

use super::App;
use super::notifications::reader;
use super::sso::Asker;
use crate::auth::now_millis;
use crate::error::AppError;

impl App {
    /// Marks a notification of the asker unread again, out of the archive if it was put
    /// away.
    ///
    /// # Errors
    ///
    /// Returns a not-found error for a notification that is not the asker's, as for one
    /// that is not there, and for one of an organization their session does not open.
    pub async fn unread_notification(
        &self,
        asker: Asker<'_>,
        notification_id: &str,
    ) -> Result<(), AppError> {
        let changed = self
            .store()
            .unread_notification(reader(asker), notification_id)
            .await?;
        self.told_of_one(asker, changed).await
    }

    /// Puts a notification of the asker away, reading it if it was not.
    ///
    /// # Errors
    ///
    /// As [`App::unread_notification`].
    pub async fn archive_notification(
        &self,
        asker: Asker<'_>,
        notification_id: &str,
    ) -> Result<(), AppError> {
        let changed = self
            .store()
            .archive_notification(reader(asker), notification_id, now_millis())
            .await?;
        self.told_of_one(asker, changed).await
    }

    /// Takes a notification of the asker out of the archive; it stays read.
    ///
    /// # Errors
    ///
    /// As [`App::unread_notification`].
    pub async fn unarchive_notification(
        &self,
        asker: Asker<'_>,
        notification_id: &str,
    ) -> Result<(), AppError> {
        let changed = self
            .store()
            .unarchive_notification(reader(asker), notification_id)
            .await?;
        self.told_of_one(asker, changed).await
    }

    /// Puts away every notification of the asker that was read, in the organizations
    /// their session opens.
    ///
    /// # Errors
    ///
    /// Returns the store's error if the statement fails.
    pub async fn archive_read_notifications(&self, asker: Asker<'_>) -> Result<(), AppError> {
        let archived = self
            .store()
            .archive_read_notifications(reader(asker), now_millis())
            .await?;
        let org_ids: Vec<&str> = archived.keys().map(String::as_str).collect();
        self.ws()
            .notifications_changed(asker.user_id, &org_ids)
            .await;
        Ok(())
    }

    /// Tells the asker's connections of the change of one notification of the
    /// organization `changed`; 404 where there was none.
    async fn told_of_one(&self, asker: Asker<'_>, changed: Option<String>) -> Result<(), AppError> {
        let Some(org_id) = changed else {
            return Err(AppError::not_found("notification"));
        };
        self.ws()
            .notifications_changed(asker.user_id, &[&org_id])
            .await;
        Ok(())
    }
}
