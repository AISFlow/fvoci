//! Subscriptions of users to the changes of a block.
//!
//! A subscription is a row while it lasts. It ends when the user ends it, when the
//! block goes for good, and when the user leaves the block's board. A block that
//! goes into the trash takes the subscriptions to it along, and brings them back
//! (`trash/watchers.rs`).

use fvoci_model::{Subscriber, Subscription};

use crate::Store;
use crate::db::{Dialect, Id, Row, Scope, Sql};
use crate::error::StoreError;

/// The columns of a subscription, in the order [`subscription_from_row`] reads them.
const SUBSCRIPTION_COLUMNS: &str = "block_type, block_id, user_id, notified_at, created_at";

/// A subscription, and the board or the wiki its block is on: what decides whether its
/// user may know of it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WatchedBlock {
    /// Who is subscribed to which block, and when they were last told of it.
    pub subscription: Subscription,
    /// The board or the wiki of its block.
    pub container_id: String,
}

impl Store {
    /// Subscribes a user to a block and returns the subscription as it is stored:
    /// created and notified now.
    ///
    /// If the user is subscribed to the block already, that subscription stays as
    /// it is, creation time and block type included, and is marked as notified now; that
    /// one is returned.
    ///
    /// # Errors
    ///
    /// Returns [`StoreError::InvalidSubscription`] if the subscription fails
    /// [`Subscription::validate`].
    pub async fn create_subscription(
        &self,
        subscription: Subscription,
    ) -> Result<Subscription, StoreError> {
        self.transaction(async |scope| create_subscription(scope, subscription).await)
            .await
    }

    /// Ends the subscription of a user to a block.
    ///
    /// # Errors
    ///
    /// Returns [`StoreError::NotFound`] if the user is not subscribed to the block.
    pub async fn delete_subscription(
        &self,
        block_id: &str,
        user_id: &str,
    ) -> Result<(), StoreError> {
        self.with_write_conn(async |scope| {
            let mut sql = scope.sql("DELETE FROM subscriptions WHERE block_id = ");
            sql.bind(Id(block_id))
                .push_bind(" AND user_id = ", Id(user_id));
            if scope.execute(sql).await? == 0 {
                return Err(subscription_not_found(block_id, user_id));
            }
            Ok(())
        })
        .await
    }

    /// Returns the subscription of a user to a block.
    ///
    /// # Errors
    ///
    /// Returns [`StoreError::NotFound`] if there is no such subscription.
    pub async fn get_subscription(
        &self,
        block_id: &str,
        user_id: &str,
    ) -> Result<Subscription, StoreError> {
        self.with_conn(async |scope| {
            let mut sql = select_subscriptions(scope);
            sql.push_bind(" AND block_id = ", Id(block_id))
                .push_bind(" AND user_id = ", Id(user_id));
            fetch_subscriptions(scope, sql)
                .await?
                .into_iter()
                .next()
                .ok_or_else(|| subscription_not_found(block_id, user_id))
        })
        .await
    }

    /// Returns the subscriptions of a user, oldest first.
    ///
    /// # Errors
    ///
    /// Returns [`StoreError::Database`] if the query fails.
    pub async fn get_subscriptions(&self, user_id: &str) -> Result<Vec<Subscription>, StoreError> {
        self.with_conn(async |scope| {
            let mut sql = select_subscriptions(scope);
            sql.push_bind(" AND user_id = ", Id(user_id))
                .push(" ORDER BY created_at, block_id");
            fetch_subscriptions(scope, sql).await
        })
        .await
    }

    /// Returns the subscriptions of a user to blocks of the boards of a workspace,
    /// oldest first, as [`Store::get_subscriptions`] orders them.
    ///
    /// # Errors
    ///
    /// Returns [`StoreError::Database`] if the query fails.
    pub async fn get_subscriptions_in_workspace(
        &self,
        user_id: &str,
        workspace_id: &str,
    ) -> Result<Vec<Subscription>, StoreError> {
        self.with_conn(async |scope| {
            let mut sql = select_subscriptions(scope);
            sql.push_bind(" AND user_id = ", Id(user_id))
                .push(
                    " AND block_id IN (SELECT bl.id FROM blocks AS bl \
                     JOIN containers AS bo ON bo.id = bl.container_id",
                )
                .push(&scope.same_organization(&[("bo", "bl")]))
                .push_bind(" WHERE bo.workspace_id = ", Id(workspace_id))
                .push(") ORDER BY created_at, block_id");
            fetch_subscriptions(scope, sql).await
        })
        .await
    }

    /// Returns the subscriptions of a user, with the board or the wiki of each
    /// block, oldest first as [`Store::get_subscriptions`] orders them: of the boards and
    /// the wikis of `workspace_id` alone, if there is one.
    ///
    /// # Errors
    ///
    /// Returns [`StoreError::Database`] if the query fails.
    pub async fn get_watched_blocks(
        &self,
        user_id: &str,
        workspace_id: Option<&str>,
    ) -> Result<Vec<WatchedBlock>, StoreError> {
        self.with_conn(async |scope| {
            let mut sql = scope.sql(
                "SELECT s.block_type, s.block_id, s.user_id, s.notified_at, s.created_at, \
                 bl.container_id FROM subscriptions AS s \
                 JOIN blocks AS bl ON bl.id = s.block_id",
            );
            sql.push(&scope.same_organization(&[("bl", "s")]))
                .push(" JOIN containers AS bo ON bo.id = bl.container_id")
                .push(&scope.same_organization(&[("bo", "bl")]))
                .push_bind(" WHERE s.user_id = ", Id(user_id));
            if let Some(workspace_id) = workspace_id {
                sql.push_bind(" AND bo.workspace_id = ", Id(workspace_id));
            }
            sql.push(" ORDER BY s.created_at, s.block_id");
            scope
                .fetch_all(sql)
                .await?
                .iter()
                .map(|row| {
                    Ok(WatchedBlock {
                        subscription: subscription_from_row(row)?,
                        container_id: row.id(5)?,
                    })
                })
                .collect()
        })
        .await
    }

    /// Returns the subscribed users of a block, the one notified longest ago first; those
    /// notified at the same time come by identifier.
    ///
    /// # Errors
    ///
    /// Returns [`StoreError::Database`] if the query fails.
    pub async fn get_subscribers_for_block(
        &self,
        block_id: &str,
    ) -> Result<Vec<Subscriber>, StoreError> {
        self.with_conn(async |scope| {
            let mut sql =
                scope.sql("SELECT user_id, notified_at FROM subscriptions WHERE block_id = ");
            sql.bind(Id(block_id))
                .push(" ORDER BY notified_at, user_id");
            scope
                .fetch_all(sql)
                .await?
                .iter()
                .map(|row| {
                    Ok(Subscriber {
                        user_id: row.id(0)?,
                        notified_at: row.int(1)?,
                    })
                })
                .collect()
        })
        .await
    }

    /// Returns the number of subscribed users of a block.
    ///
    /// # Errors
    ///
    /// Returns [`StoreError::Database`] if the query fails.
    pub async fn get_subscribers_count_for_block(&self, block_id: &str) -> Result<i64, StoreError> {
        self.with_conn(async |scope| {
            let mut sql = scope.sql("SELECT COUNT(user_id) FROM subscriptions WHERE block_id = ");
            sql.bind(Id(block_id));
            match scope.fetch_optional(sql).await? {
                Some(row) => row.int(0),
                None => Ok(0),
            }
        })
        .await
    }
}

/// Starts the query for subscriptions; conditions follow with `AND`.
fn select_subscriptions(scope: &Scope<'_>) -> Sql {
    scope.sql([
        "SELECT ",
        SUBSCRIPTION_COLUMNS,
        " FROM subscriptions WHERE 1 = 1",
    ])
}

async fn fetch_subscriptions(
    scope: &mut Scope<'_>,
    sql: Sql,
) -> Result<Vec<Subscription>, StoreError> {
    scope
        .fetch_all(sql)
        .await?
        .iter()
        .map(subscription_from_row)
        .collect()
}

fn subscription_from_row(row: &Row) -> Result<Subscription, StoreError> {
    Ok(Subscription {
        block_type: crate::blocks::block_type_column(row, 0, "block_type")?,
        block_id: row.id(1)?,
        user_id: row.id(2)?,
        notified_at: row.int(3)?,
        created_at: row.int(4)?,
        // A subscription that exists is not ended.
        deleted_at: None,
    })
}

fn subscription_not_found(block_id: &str, user_id: &str) -> StoreError {
    StoreError::not_found(format_args!(
        "subscription BlockID={block_id} UserID={user_id}"
    ))
}

/// Ends every subscription of a user to a block of the board.
pub(crate) async fn end_subscriptions_to_container(
    scope: &mut Scope<'_>,
    container_id: &str,
    user_id: &str,
) -> Result<(), StoreError> {
    let mut sql = scope.sql("DELETE FROM subscriptions WHERE user_id = ");
    sql.bind(Id(user_id))
        .push_bind(
            " AND block_id IN (SELECT id FROM blocks WHERE container_id = ",
            Id(container_id),
        )
        .push(")");
    scope.execute(sql).await?;
    Ok(())
}

pub(crate) async fn create_subscription(
    scope: &mut Scope<'_>,
    subscription: Subscription,
) -> Result<Subscription, StoreError> {
    subscription.validate()?;

    // The notification time starts at now so that the first notification does not
    // report the block's whole history.
    let now = scope.now_millis();
    let subscription = Subscription {
        notified_at: now,
        created_at: now,
        deleted_at: None,
        ..subscription
    };

    // Subscribing again to what one is subscribed to keeps the subscription as it is,
    // creation time and block type included, and marks it as notified now without
    // stepping backward on clock skew.
    let notified_expr = match scope.dialect() {
        Dialect::Postgres => "GREATEST(subscriptions.notified_at, EXCLUDED.notified_at)",
        Dialect::Sqlite => "MAX(subscriptions.notified_at, EXCLUDED.notified_at)",
    };
    let mut sql = scope.sql([
        "INSERT INTO subscriptions (",
        SUBSCRIPTION_COLUMNS,
        ") VALUES (",
    ]);
    sql.bind(subscription.block_type.as_str())
        .push_bind(", ", Id(&subscription.block_id))
        .push_bind(", ", Id(&subscription.user_id))
        .push_bind(", ", subscription.notified_at)
        .push_bind(", ", subscription.created_at)
        .push(") ON CONFLICT (block_id, user_id) DO UPDATE SET notified_at = ")
        .push(notified_expr);
    scope
        .execute(sql)
        .await
        .map_err(|error| error.missing_reference("block or user"))?;

    let mut sql = select_subscriptions(scope);
    sql.push_bind(" AND block_id = ", Id(&subscription.block_id))
        .push_bind(" AND user_id = ", Id(&subscription.user_id));
    fetch_subscriptions(scope, sql)
        .await?
        .into_iter()
        .next()
        .ok_or_else(|| subscription_not_found(&subscription.block_id, &subscription.user_id))
}
