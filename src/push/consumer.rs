//! `push` outbox consumer: fan-out only (source `listPushMessages`); sending
//! is [`crate::push::sender`], the same split as webhooks.
//!
//! In one transaction with its own processed mark and cursor advance
//! (independent of `notifications` and `mail`), each event becomes one
//! `push_deliveries` row per (recipient, subscription). Recipients are the
//! `notify_for_event` rows whose `inApp` pref is on and whose user is active.
//! The sender re-checks all of this right before each POST, so the fan-out
//! only has to be complete, not final.

use std::collections::HashMap;
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;

use sqlx::{PgPool, Postgres, Transaction};
use tokio::sync::Notify;
use uuid::Uuid;

use crate::db::context::{set_system, set_tenant};
use crate::db::notifications::{
    display_id_for, find_prefs_tx, resolved_store_prefs, NotificationInsert,
};
use crate::db::outbox::{advance_cursor_tx, mark_processed_tx, OutboxEvent};
use crate::notifications::notify_for_event;
use crate::outbox::{DeliveryMode, OutboxConsumer, OutboxProcessError};
use crate::push::message::{format_person_name, notification_body, push_url};
use crate::push::send::PushPayload;

pub const PUSH_CONSUMER: &str = "push";

pub struct PushConsumer {
    /// Wakes the sender after a fan-out commit.
    sender_wake: Option<Arc<Notify>>,
}

impl OutboxConsumer for PushConsumer {
    fn name(&self) -> &str {
        PUSH_CONSUMER
    }

    fn delivery_mode(&self) -> DeliveryMode {
        DeliveryMode::PgOnly
    }

    fn deliver<'a>(
        &'a self,
        pool: &'a PgPool,
        lease_owner: Uuid,
        event: &'a OutboxEvent,
    ) -> Pin<Box<dyn Future<Output = Result<(), OutboxProcessError>> + Send + 'a>> {
        Box::pin(async move {
            let queued = fan_out_event(pool, lease_owner, event).await?;
            if queued > 0 {
                if let Some(wake) = &self.sender_wake {
                    wake.notify_one();
                }
            }
            Ok(())
        })
    }
}

pub fn push_consumer(sender_wake: Option<Arc<Notify>>) -> Arc<dyn OutboxConsumer> {
    Arc::new(PushConsumer { sender_wake })
}

async fn fan_out_event(
    pool: &PgPool,
    lease_owner: Uuid,
    event: &OutboxEvent,
) -> Result<u64, OutboxProcessError> {
    let mut tx = pool.begin().await?;
    set_system(&mut tx).await?;
    if let Some(workspace_id) = event.workspace_id {
        set_tenant(&mut tx, workspace_id).await?;
    }
    let mut queued = 0;
    if mark_processed_tx(&mut tx, PUSH_CONSUMER, event.id).await? {
        queued = fan_out(&mut tx, event).await?;
    }
    if !advance_cursor_tx(&mut tx, PUSH_CONSUMER, lease_owner, &event.xact, event.seq).await? {
        tx.rollback().await?;
        return Err(OutboxProcessError::Delivery(
            "advance rejected in pg-only tx".into(),
        ));
    }
    tx.commit().await?;
    Ok(queued)
}

async fn fan_out(
    tx: &mut Transaction<'_, Postgres>,
    event: &OutboxEvent,
) -> Result<u64, sqlx::Error> {
    let Some(workspace_id) = event.workspace_id else {
        return Ok(0);
    };
    // Without a keypair nothing can be sent (source: those pushes are dropped).
    let vapid: Option<String> = sqlx::query_scalar("SELECT fvoci.app_vapid_public_key()")
        .fetch_one(&mut **tx)
        .await?;
    if vapid.is_none() {
        return Ok(0);
    }
    let recipients = push_recipients(tx, event).await?;
    if recipients.is_empty() {
        return Ok(0);
    }
    let users: Vec<Uuid> = recipients.keys().copied().collect();
    Ok(sqlx::query(
        r#"
        INSERT INTO fvoci.push_deliveries (event_id, workspace_id, user_id, endpoint)
        SELECT $1, $2, s.user_id, s.endpoint
        FROM fvoci.push_subscriptions AS s
        WHERE s.user_id = ANY($3)
        ORDER BY s.user_id, s.id
        ON CONFLICT (event_id, user_id, endpoint) DO NOTHING
        "#,
    )
    .bind(event.id)
    .bind(workspace_id)
    .bind(&users)
    .execute(&mut **tx)
    .await?
    .rows_affected())
}

/// Current push recipients of `event`, keyed by user: the in-app notification
/// rows (membership, resource permission and actor rules of
/// `notify_for_event`) with `inApp` on, for users who are neither deleted nor
/// suspended. The caller's transaction must have the event's tenant set.
pub(crate) async fn push_recipients(
    tx: &mut Transaction<'_, Postgres>,
    event: &OutboxEvent,
) -> Result<HashMap<Uuid, NotificationInsert>, sqlx::Error> {
    let Some(workspace_id) = event.workspace_id else {
        return Ok(HashMap::new());
    };
    let mut out = HashMap::new();
    for row in notify_for_event(tx, event).await? {
        let prefs = resolved_store_prefs(find_prefs_tx(tx, workspace_id, row.user_id).await?);
        if !prefs.in_app {
            continue;
        }
        let active: bool = sqlx::query_scalar(
            "SELECT EXISTS (SELECT 1 FROM fvoci.users \
             WHERE id = $1 AND deleted_at IS NULL AND suspended_at IS NULL)",
        )
        .bind(row.user_id)
        .fetch_one(&mut **tx)
        .await?;
        if active {
            out.insert(row.user_id, row);
        }
    }
    Ok(out)
}

/// Source `listPushMessages` payload for one notification row.
pub(crate) async fn push_payload(
    tx: &mut Transaction<'_, Postgres>,
    row: &NotificationInsert,
) -> Result<Option<PushPayload>, sqlx::Error> {
    let workspace: Option<(String, String)> = sqlx::query_as(
        "SELECT slug, name FROM fvoci.workspaces WHERE id = $1 AND deleted_at IS NULL",
    )
    .bind(row.workspace_id)
    .fetch_optional(&mut **tx)
    .await?;
    let Some((slug, workspace_name)) = workspace else {
        return Ok(None);
    };
    let display_id = display_id_for(
        tx,
        row.workspace_id,
        row.target_type.as_deref(),
        row.target_id,
        &row.payload,
    )
    .await?;
    let title = actor_name(tx, row.actor_user_id)
        .await?
        .unwrap_or(workspace_name);
    Ok(Some(PushPayload {
        title,
        body: notification_body(&row.verb, &row.payload),
        url: push_url(&slug, display_id.as_deref()),
    }))
}

async fn actor_name(
    tx: &mut Transaction<'_, Postgres>,
    actor_user_id: Option<Uuid>,
) -> Result<Option<String>, sqlx::Error> {
    let Some(actor_user_id) = actor_user_id else {
        return Ok(None);
    };
    let row: Option<(String, Option<String>, String)> = sqlx::query_as(
        "SELECT given_name, family_name, locale FROM fvoci.users \
         WHERE id = $1 AND deleted_at IS NULL",
    )
    .bind(actor_user_id)
    .fetch_optional(&mut **tx)
    .await?;
    Ok(row
        .map(|(given, family, locale)| format_person_name(&given, family.as_deref(), &locale))
        .filter(|name| !name.is_empty()))
}
