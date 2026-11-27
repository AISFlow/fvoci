//! Subscriptions of users to changes of a block.

use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::block::BlockType;
use crate::refusal::{RefusalReason, Refusals};

/// Why a [`Subscription`] is rejected before it is stored: every member that is
/// refused.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
#[error("{0}")]
pub struct SubscriptionError(Refusals);

impl SubscriptionError {
    /// The members that were refused.
    pub fn into_refusals(self) -> Refusals {
        self.0
    }
}

/// A subscription to the changes of a block.
#[derive(Debug, Default, Clone, PartialEq, Eq, Serialize, Deserialize, utoipa::ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct Subscription {
    /// The type of the block subscribed to.
    pub block_type: BlockType,
    /// The identifier of the block subscribed to.
    #[serde(default)]
    pub block_id: String,
    /// The identifier of the subscribed user.
    #[serde(default)]
    pub user_id: String,
    /// The time of the last notification sent for this subscription, in milliseconds
    /// since the Unix epoch.
    #[serde(default)]
    pub notified_at: i64,
    /// Creation time in milliseconds since the Unix epoch.
    #[serde(default)]
    pub created_at: i64,
    /// Deletion time in milliseconds since the Unix epoch; absent while not deleted.
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "crate::serde_util::positive_millis"
    )]
    #[schema(nullable = false)]
    pub deleted_at: Option<i64>,
}

impl Subscription {
    /// Checks that the subscription may be stored.
    ///
    /// # Errors
    ///
    /// Returns a [`SubscriptionError`] with every one of the block identifier and the
    /// user identifier that is empty.
    pub fn validate(&self) -> Result<(), SubscriptionError> {
        let mut refused = Refusals::new();
        if self.block_id.is_empty() {
            refused.refuse("blockId", RefusalReason::Required, "missing block id");
        }
        if self.user_id.is_empty() {
            refused.refuse("userId", RefusalReason::Required, "missing user id");
        }
        refused.into_result().map_err(SubscriptionError)
    }
}

/// A user subscribed to a block.
#[derive(Debug, Default, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Subscriber {
    /// The identifier of the subscribed user.
    #[serde(default)]
    pub user_id: String,
    /// The time the user was last notified, in milliseconds since the Unix epoch.
    #[serde(default)]
    pub notified_at: i64,
}
