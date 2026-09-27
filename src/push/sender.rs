//! Sends `push_deliveries` rows (source `deliverPushMessages`), shaped like
//! the webhook sender: claim a batch with a lease, send, record.
//!
//! One batch is one transaction. Right before the POSTs it re-checks every
//! row against current state: the event's `notify_for_event` recipients with
//! `inApp` on (membership, resource permission, prefs), an active user, and
//! the subscription row itself, read `FOR SHARE` and held until the attempt
//! is recorded. A logout, rotation or cleanup that deletes the subscription
//! therefore waits for an in-flight send and, once committed, no later send
//! reaches that browser. Bytes already handed to a push service cannot be
//! recalled.
//!
//! Every row gets one attempt with the per-request timeout (no tail drop);
//! 404/410 delete the endpoint for every user, other failures are logged with
//! the endpoint origin only. The attempt and the row deletes commit together.
//! If the process stops or the record fails after the POSTs, the batch's rows
//! return after the claim lease and are sent again: delivery is at least once
//! per device, and never a replay of the whole event.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use futures_util::future::join_all;
use sqlx::PgPool;
use tokio::sync::Notify;
use tokio_util::sync::CancellationToken;
use tracing::warn;
use uuid::Uuid;

use crate::auth::password::Keyring;
use crate::db::context::{set_system, set_tenant};
use crate::db::outbox::fetch_event_by_id;
use crate::integrations::outbound::Outbound;
use crate::push::consumer::{push_payload, push_recipients};
use crate::push::db::remove_by_endpoint;
use crate::push::send::{
    endpoint_origin_for_log, send_push, PushPayload, PushSendOutcome, PushTarget, PUSH_TIMEOUT,
};
use crate::push::vapid::{load_vapid_key_pair, VapidKeysError};

#[derive(Debug, Clone)]
pub struct PushSenderSettings {
    /// Requests in flight at once (source concurrency 8).
    pub batch: i64,
    /// Per-request budget, name resolution included (source 5 s).
    pub request_timeout: Duration,
    /// A crashed sender's claimed rows return after this.
    pub claim_lease: Duration,
    /// Idle poll for rows queued by another replica.
    pub poll_interval: Duration,
    /// Rows older than the push TTL are dropped unsent.
    pub max_age: Duration,
}

impl Default for PushSenderSettings {
    fn default() -> Self {
        Self {
            batch: 8,
            request_timeout: PUSH_TIMEOUT,
            claim_lease: Duration::from_secs(60),
            poll_interval: Duration::from_secs(1),
            max_age: Duration::from_secs(86_400),
        }
    }
}

pub struct PushSenderHandle {
    cancel: CancellationToken,
    join: tokio::task::JoinHandle<()>,
    pub wake: Arc<Notify>,
}

impl PushSenderHandle {
    pub fn request_shutdown(&self) {
        self.cancel.cancel();
    }

    pub async fn join(self) -> Result<(), String> {
        self.join
            .await
            .map_err(|err| format!("push sender task join failed: {err}"))
    }
}

/// `outbound` must be the strict policy (`Outbound::without_allow_list`):
/// endpoints are user-supplied URLs and never use the webhook allow-list.
/// `subject` is the VAPID `sub` (`FVOCI_PUBLIC_ORIGIN`). Shutdown abandons
/// the batch in flight; its rows return after the claim lease.
pub fn spawn_push_sender(
    pool: PgPool,
    outbound: Outbound,
    keys: Option<Arc<Keyring>>,
    subject: String,
    settings: PushSenderSettings,
) -> PushSenderHandle {
    let cancel = CancellationToken::new();
    let wake = Arc::new(Notify::new());
    let sender = Sender {
        pool,
        outbound,
        keys,
        subject,
        settings,
    };
    let join = tokio::spawn(sender.run(cancel.child_token(), wake.clone()));
    PushSenderHandle { cancel, join, wake }
}

struct Sender {
    pool: PgPool,
    outbound: Outbound,
    keys: Option<Arc<Keyring>>,
    subject: String,
    settings: PushSenderSettings,
}

struct Claimed {
    id: Uuid,
    event_id: Uuid,
    user_id: Uuid,
    endpoint: String,
    stale: bool,
}

struct Ready {
    endpoint: String,
    p256dh: String,
    auth: String,
    payload: Arc<PushPayload>,
}

impl Sender {
    async fn run(self, cancel: CancellationToken, wake: Arc<Notify>) {
        while !cancel.is_cancelled() {
            let worked = tokio::select! {
                () = cancel.cancelled() => break,
                result = self.send_batch() => match result {
                    Ok(worked) => worked,
                    Err(err) => {
                        warn!(error = %err, "push.batch_failed");
                        false
                    }
                },
            };
            if worked {
                continue;
            }
            tokio::select! {
                () = cancel.cancelled() => break,
                () = wake.notified() => {}
                () = tokio::time::sleep(self.settings.poll_interval) => {}
            }
        }
    }

    /// Claims, re-checks, sends and records one batch. `Ok(false)`: nothing due.
    async fn send_batch(&self) -> Result<bool, sqlx::Error> {
        let claimed = self.claim().await?;
        if claimed.is_empty() {
            return Ok(false);
        }
        let vapid = match load_vapid_key_pair(&self.pool, self.keys.as_deref()).await {
            Ok(Some(pair)) => Some(pair),
            Ok(None) => {
                warn!(event = "push.skipped", reason = "vapid_keys_missing");
                None
            }
            Err(VapidKeysError::Db(err)) => return Err(err),
            Err(err) => {
                warn!(event = "push.skipped", reason = "vapid_keys_unusable", error = %err);
                None
            }
        };

        let mut tx = self.pool.begin().await?;
        set_system(&mut tx).await?;
        let mut ready = Vec::new();
        if let Some(vapid) = &vapid {
            let mut by_event: HashMap<Uuid, Vec<&Claimed>> = HashMap::new();
            for row in claimed.iter().filter(|row| !row.stale) {
                by_event.entry(row.event_id).or_default().push(row);
            }
            for (event_id, rows) in by_event {
                let Some(event) = fetch_event_by_id(&self.pool, event_id).await? else {
                    continue;
                };
                let Some(workspace_id) = event.workspace_id else {
                    continue;
                };
                set_tenant(&mut tx, workspace_id).await?;
                let recipients = push_recipients(&mut tx, &event).await?;
                let mut payloads: HashMap<Uuid, Option<Arc<PushPayload>>> = HashMap::new();
                for row in rows {
                    let Some(notification) = recipients.get(&row.user_id) else {
                        continue;
                    };
                    let keys: Option<(String, String)> = sqlx::query_as(
                        "SELECT p256dh, auth FROM fvoci.push_subscriptions \
                         WHERE user_id = $1 AND endpoint = $2 FOR SHARE",
                    )
                    .bind(row.user_id)
                    .bind(&row.endpoint)
                    .fetch_optional(&mut *tx)
                    .await?;
                    let Some((p256dh, auth)) = keys else {
                        continue;
                    };
                    let payload = match payloads.get(&row.user_id) {
                        Some(payload) => payload.clone(),
                        None => {
                            let payload = push_payload(&mut tx, notification).await?.map(Arc::new);
                            payloads.insert(row.user_id, payload.clone());
                            payload
                        }
                    };
                    if let Some(payload) = payload {
                        ready.push(Ready {
                            endpoint: row.endpoint.clone(),
                            p256dh,
                            auth,
                            payload,
                        });
                    }
                }
            }
            let outcomes = join_all(ready.iter().map(|message| {
                send_push(
                    &self.outbound,
                    vapid,
                    &self.subject,
                    PushTarget {
                        endpoint: &message.endpoint,
                        p256dh: &message.p256dh,
                        auth: &message.auth,
                    },
                    &message.payload,
                    self.settings.request_timeout,
                )
            }))
            .await;
            for (message, outcome) in ready.iter().zip(outcomes) {
                let origin = endpoint_origin_for_log(&message.endpoint);
                match outcome {
                    Ok(PushSendOutcome::Delivered) => {}
                    Ok(PushSendOutcome::ExpiredEndpoint) => {
                        remove_by_endpoint(&mut tx, &message.endpoint).await?;
                    }
                    Ok(PushSendOutcome::Failed(status)) => {
                        warn!(%origin, status, "push.delivery_failed");
                    }
                    Err(err) => warn!(%origin, error = %err, "push.transport_failed"),
                }
            }
        }
        let stale = claimed.iter().filter(|row| row.stale).count();
        if stale > 0 {
            warn!(stale, "push.expired_unsent");
        }
        let ids: Vec<Uuid> = claimed.iter().map(|row| row.id).collect();
        sqlx::query("DELETE FROM fvoci.push_deliveries WHERE id = ANY($1)")
            .bind(&ids)
            .execute(&mut *tx)
            .await?;
        tx.commit().await?;
        Ok(true)
    }

    async fn claim(&self) -> Result<Vec<Claimed>, sqlx::Error> {
        let mut tx = self.pool.begin().await?;
        set_system(&mut tx).await?;
        let rows: Vec<(Uuid, Uuid, Uuid, String, bool)> = sqlx::query_as(
            r#"
            UPDATE fvoci.push_deliveries AS d
            SET claimed_until = now() + make_interval(secs => $2::double precision)
            WHERE d.id IN (
                SELECT id FROM fvoci.push_deliveries
                WHERE claimed_until IS NULL OR claimed_until < now()
                ORDER BY id
                LIMIT $1
                FOR UPDATE SKIP LOCKED
            )
            RETURNING d.id, d.event_id, d.user_id, d.endpoint,
                d.created_at < now() - make_interval(secs => $3::double precision)
            "#,
        )
        .bind(self.settings.batch.max(1))
        .bind(self.settings.claim_lease.as_secs_f64())
        .bind(self.settings.max_age.as_secs_f64())
        .fetch_all(&mut *tx)
        .await?;
        tx.commit().await?;
        let mut claimed: Vec<Claimed> = rows
            .into_iter()
            .map(|(id, event_id, user_id, endpoint, stale)| Claimed {
                id,
                event_id,
                user_id,
                endpoint,
                stale,
            })
            .collect();
        claimed.sort_by_key(|row| row.id);
        Ok(claimed)
    }
}
