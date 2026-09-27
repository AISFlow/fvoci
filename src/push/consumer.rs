//! `push` outbox consumer (source `listPushMessages` + `deliverPushMessages`).
//!
//! External delivery with its own processed-events key, independent of
//! `notifications` and `mail`: a mail failure neither skips nor repeats push,
//! and a completed push is not rolled back by a later mail error. Recipients,
//! current membership/permissions and prefs come from the same
//! `notify_for_event` rules as the inbox, evaluated when the event is
//! delivered; only rows whose `inApp` pref is on are pushed.
//!
//! Sends are best-effort: up to [`PUSH_CONCURRENCY`] at once, 5 s each,
//! 404/410 delete the endpoint (globally), every other failure is logged with
//! the endpoint origin only and not retried. Only a failure before any send
//! (database) makes the outbox retry the event.

use std::collections::BTreeSet;
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use std::time::Duration;

use futures_util::stream::{self, StreamExt};
use sqlx::{PgPool, Postgres, Transaction};
use uuid::Uuid;

use crate::auth::password::Keyring;
use crate::db::context::{set_system, set_tenant};
use crate::db::notifications::{
    display_id_for, find_prefs_tx, resolved_store_prefs, NotificationInsert,
};
use crate::db::outbox::{is_processed, OutboxEvent};
use crate::integrations::outbound::Outbound;
use crate::notifications::notify_for_event;
use crate::outbox::{DeliveryMode, OutboxConsumer, OutboxProcessError};
use crate::push::db::{list_for_user, remove_by_endpoint, PushSubscriptionRow};
use crate::push::message::{format_person_name, notification_body, push_url};
use crate::push::send::{
    endpoint_origin_for_log, send_push, PushPayload, PushSendOutcome, PushTarget,
};
use crate::push::vapid::{load_vapid_key_pair, VapidKeysError};

pub const PUSH_CONSUMER: &str = "push";

/// Source send concurrency.
pub const PUSH_CONCURRENCY: usize = 8;
/// Wall-clock budget for one event's sends. The dispatcher abandons a batch
/// just before the 30 s lease; sends still pending at this point are dropped
/// (best-effort) so the event is marked instead of re-sent to every device.
const PUSH_EVENT_BUDGET: Duration = Duration::from_secs(20);

pub struct PushConsumer {
    outbound: Outbound,
    encryption_keys: Option<Arc<Keyring>>,
    /// VAPID `sub` (`FVOCI_PUBLIC_ORIGIN`).
    subject: String,
}

impl OutboxConsumer for PushConsumer {
    fn name(&self) -> &str {
        PUSH_CONSUMER
    }

    fn delivery_mode(&self) -> DeliveryMode {
        DeliveryMode::External
    }

    fn deliver<'a>(
        &'a self,
        pool: &'a PgPool,
        _lease_owner: Uuid,
        event: &'a OutboxEvent,
    ) -> Pin<Box<dyn Future<Output = Result<(), OutboxProcessError>> + Send + 'a>> {
        Box::pin(async move { self.deliver_push(pool, event).await })
    }

    /// One event per batch: its sends use the whole [`PUSH_EVENT_BUDGET`].
    fn batch_event_cap(&self) -> usize {
        1
    }
}

/// `outbound` must be the strict policy (`Outbound::without_allow_list`):
/// push endpoints are user-supplied URLs and never use the webhook allow-list.
pub fn push_consumer(
    outbound: Outbound,
    encryption_keys: Option<Arc<Keyring>>,
    public_origin: String,
) -> Arc<dyn OutboxConsumer> {
    Arc::new(PushConsumer {
        outbound,
        encryption_keys,
        subject: public_origin,
    })
}

struct PushMessage {
    subscription: PushSubscriptionRow,
    payload: Arc<PushPayload>,
}

impl PushConsumer {
    async fn deliver_push(
        &self,
        pool: &PgPool,
        event: &OutboxEvent,
    ) -> Result<(), OutboxProcessError> {
        let Some(workspace_id) = event.workspace_id else {
            return Ok(());
        };
        // Source `pushPending`: a replayed event (cursor rebased by
        // `--recover-outbox`, or a lease lost after the mark) is not pushed twice.
        if is_processed(pool, PUSH_CONSUMER, event.id).await? {
            return Ok(());
        }
        let messages = list_push_messages(pool, workspace_id, event).await?;
        if messages.is_empty() {
            return Ok(());
        }
        let vapid = match load_vapid_key_pair(pool, self.encryption_keys.as_deref()).await {
            Ok(Some(pair)) => pair,
            Ok(None) => {
                tracing::warn!(event = "push.skipped", reason = "vapid_keys_missing");
                return Ok(());
            }
            Err(VapidKeysError::Db(err)) => return Err(err.into()),
            Err(err) => {
                tracing::warn!(event = "push.skipped", reason = "vapid_keys_unusable", error = %err);
                return Ok(());
            }
        };

        let total = messages.len();
        let mut sends = stream::iter(messages)
            .map(|message| {
                let vapid = &vapid;
                async move {
                    let target = PushTarget {
                        endpoint: &message.subscription.endpoint,
                        p256dh: &message.subscription.p256dh,
                        auth: &message.subscription.auth,
                    };
                    let outcome = send_push(
                        &self.outbound,
                        vapid,
                        &self.subject,
                        target,
                        &message.payload,
                    )
                    .await;
                    (message.subscription.endpoint, outcome)
                }
            })
            .buffer_unordered(PUSH_CONCURRENCY);

        let deadline = tokio::time::Instant::now() + PUSH_EVENT_BUDGET;
        let mut expired = BTreeSet::new();
        let mut finished = 0usize;
        loop {
            let next = match tokio::time::timeout_at(deadline, sends.next()).await {
                Ok(Some(next)) => next,
                Ok(None) => break,
                Err(_) => {
                    tracing::warn!(
                        event_id = %event.id,
                        abandoned = total - finished,
                        "push.budget_exceeded"
                    );
                    break;
                }
            };
            finished += 1;
            let (endpoint, outcome) = next;
            match outcome {
                Ok(PushSendOutcome::Delivered) => {}
                Ok(PushSendOutcome::ExpiredEndpoint) => {
                    expired.insert(endpoint);
                }
                Ok(PushSendOutcome::Failed(status)) => tracing::warn!(
                    origin = %endpoint_origin_for_log(&endpoint),
                    status,
                    "push.delivery_failed"
                ),
                Err(err) => tracing::warn!(
                    origin = %endpoint_origin_for_log(&endpoint),
                    error = %err,
                    "push.transport_failed"
                ),
            }
        }
        drop(sends);

        if !expired.is_empty() {
            let mut tx = pool.begin().await?;
            set_system(&mut tx).await?;
            for endpoint in &expired {
                remove_by_endpoint(&mut tx, endpoint).await?;
            }
            tx.commit().await?;
        }
        Ok(())
    }
}

/// Source `listPushMessages`: one message per (kept notification row with
/// `inApp` on) x (that user's subscriptions).
async fn list_push_messages(
    pool: &PgPool,
    workspace_id: Uuid,
    event: &OutboxEvent,
) -> Result<Vec<PushMessage>, OutboxProcessError> {
    let mut tx = pool.begin().await?;
    set_system(&mut tx).await?;
    set_tenant(&mut tx, workspace_id).await?;
    let rows = notify_for_event(&mut tx, event).await?;
    let mut out = Vec::new();
    if !rows.is_empty() {
        let workspace: Option<(String, String)> = sqlx::query_as(
            "SELECT slug, name FROM fvoci.workspaces WHERE id = $1 AND deleted_at IS NULL",
        )
        .bind(workspace_id)
        .fetch_optional(&mut *tx)
        .await?;
        if let Some((slug, workspace_name)) = workspace {
            for row in rows {
                let prefs =
                    resolved_store_prefs(find_prefs_tx(&mut tx, workspace_id, row.user_id).await?);
                if !prefs.in_app {
                    continue;
                }
                let subscriptions = list_for_user(&mut tx, row.user_id).await?;
                if subscriptions.is_empty() {
                    continue;
                }
                let payload = Arc::new(push_payload(&mut tx, &slug, &workspace_name, &row).await?);
                out.extend(subscriptions.into_iter().map(|subscription| PushMessage {
                    subscription,
                    payload: payload.clone(),
                }));
            }
        }
    }
    tx.commit().await?;
    Ok(out)
}

async fn push_payload(
    tx: &mut Transaction<'_, Postgres>,
    slug: &str,
    workspace_name: &str,
    row: &NotificationInsert,
) -> Result<PushPayload, sqlx::Error> {
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
        .unwrap_or_else(|| workspace_name.to_string());
    Ok(PushPayload {
        title,
        body: notification_body(&row.verb, &row.payload),
        url: push_url(slug, display_id.as_deref()),
    })
}

async fn actor_name(
    tx: &mut Transaction<'_, Postgres>,
    actor_user_id: Option<Uuid>,
) -> Result<Option<String>, sqlx::Error> {
    let Some(actor_user_id) = actor_user_id else {
        return Ok(None);
    };
    let row: Option<(String, Option<String>, String)> = sqlx::query_as(
        "SELECT given_name, family_name, locale FROM fvoci.users WHERE id = $1 AND deleted_at IS NULL",
    )
    .bind(actor_user_id)
    .fetch_optional(&mut **tx)
    .await?;
    Ok(row
        .map(|(given, family, locale)| format_person_name(&given, family.as_deref(), &locale))
        .filter(|name| !name.is_empty()))
}
