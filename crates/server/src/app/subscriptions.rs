//! Subscriptions of users to changes on a block.
//!
//! Subscriptions are stored and queried directly; subscription mutations do not emit
//! websocket broadcasts.

use std::collections::HashMap;

use fvoci_model::Subscription;
use fvoci_store::WatchedBlock;

use super::App;
use crate::auth::now_millis;
use crate::error::AppError;

impl App {
    /// Subscribes a user to a block and returns the subscription, created and last
    /// notified now.
    ///
    /// Subscribing again revives the stored subscription: it keeps its creation time and
    /// block type, and updates its notification time to now.
    ///
    /// # Errors
    ///
    /// Returns a bad request if the subscription has no block identifier, block type or
    /// user identifier.
    pub async fn create_subscription(
        &self,
        subscription: Subscription,
    ) -> Result<Subscription, AppError> {
        Ok(self.store().create_subscription(subscription).await?)
    }

    /// Ends the subscription of a user to a block and returns it, marked as deleted now.
    /// For a handle of one workspace (`App::in_workspace`) only one to a block of a board
    /// of that workspace is there.
    ///
    /// # Errors
    ///
    /// Returns a not-found error if the user has no subscription to the block, or has
    /// ended it already.
    pub async fn delete_subscription(
        &self,
        block_id: &str,
        user_id: &str,
    ) -> Result<Subscription, AppError> {
        // One to a block the user no longer reads is ended all the same.
        if let Some(workspace_id) = self.workspace.as_deref()
            && !self
                .store()
                .get_subscriptions_in_workspace(user_id, workspace_id)
                .await?
                .iter()
                .any(|subscription| subscription.block_id == block_id)
        {
            return Err(AppError::not_found("subscription"));
        }
        let mut subscription = self.store().get_subscription(block_id, user_id).await?;
        self.store().delete_subscription(block_id, user_id).await?;
        subscription.deleted_at = Some(now_millis());
        Ok(subscription)
    }

    /// Returns the subscriptions of a user that have not ended, oldest first: for a
    /// handle of one workspace (`App::in_workspace`), those to blocks of its boards.
    /// Only those to a block of a board or a wiki the user reads now, or may join to
    /// read (`Grant::reads_on_joining`), as they are told of it: of any other they are
    /// not to know even the identifier.
    ///
    /// # Errors
    ///
    /// Returns the store's error if a query fails.
    pub async fn subscriptions(&self, user_id: &str) -> Result<Vec<Subscription>, AppError> {
        let watched = self
            .store()
            .get_watched_blocks(user_id, self.workspace.as_deref())
            .await?;
        let mut reads: HashMap<String, bool> = HashMap::new();
        let mut kept = Vec::with_capacity(watched.len());
        for WatchedBlock {
            subscription,
            container_id,
        } in watched
        {
            let reads_it = if let Some(reads_it) = reads.get(&container_id) {
                *reads_it
            } else {
                let reads_it = self
                    .container_access(user_id, &container_id)
                    .await?
                    .is_some_and(|access| access.grant.reads_on_joining());
                reads.insert(container_id, reads_it);
                reads_it
            };
            if reads_it {
                kept.push(subscription);
            }
        }
        Ok(kept)
    }
}
