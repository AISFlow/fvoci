//! Sends `push_deliveries` rows (source `deliverPushMessages`), shaped like
//! the webhook sender: claim a batch with a lease, check, send, acknowledge.
//!
//! Per batch:
//! 1. **Claim** up to `batch` rows (`FOR UPDATE SKIP LOCKED`), set the lease
//!    and bump `attempt`, which fences the later writes of this claim.
//! 2. **Final check**, one short transaction: for every row, the event's
//!    current `notify_for_event` recipients with `inApp` on (membership,
//!    resource access, prefs), an active user, and the subscription row with
//!    its bound session still live. Failing rows are deleted, passing rows are
//!    marked `handed_off_at`; then it commits. This is the handoff point: a
//!    logout, revocation, suspension or cleanup committed before the check
//!    statement means no send; one committed after it cannot stop that send.
//! 3. **POST** outside any transaction, at most `batch` at once, each bounded
//!    by the request timeout. Nothing is locked while waiting on the network.
//! 4. **Ack**, one transaction fenced on `attempt`: delete the rows, and for
//!    404/410 remove the endpoint for every user.
//!
//! Every row gets one attempt (no tail drop, no age cutoff). If the process
//! stops or the ack fails after the POSTs, the claimed rows return after the
//! lease and go through the final check again: delivery is at least once per
//! device within one claimed batch, never a replay of the whole event. A
//! message already accepted by a push service cannot be recalled.

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
    /// Claimed rows of a stopped sender return after this. Must exceed the
    /// check + request timeout + ack.
    pub claim_lease: Duration,
    /// Idle poll for rows queued by another replica.
    pub poll_interval: Duration,
}

impl Default for PushSenderSettings {
    fn default() -> Self {
        Self {
            batch: 8,
            request_timeout: PUSH_TIMEOUT,
            claim_lease: Duration::from_secs(30),
            poll_interval: Duration::from_secs(1),
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
    subscription_id: Uuid,
    attempt: i32,
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

    /// One claim → check → send → ack round. `Ok(false)`: nothing due.
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
        let Some(vapid) = vapid else {
            // Source: without a keypair these notifications are dropped.
            self.ack(&claimed, &[]).await?;
            return Ok(true);
        };
        let ready = self.final_check(&claimed).await?;
        let outcomes = join_all(ready.iter().map(|message| {
            send_push(
                &self.outbound,
                &vapid,
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
        let mut expired = Vec::new();
        for (message, outcome) in ready.iter().zip(outcomes) {
            let origin = endpoint_origin_for_log(&message.endpoint);
            match outcome {
                Ok(PushSendOutcome::Delivered) => {}
                Ok(PushSendOutcome::ExpiredEndpoint) => expired.push(message.endpoint.as_str()),
                Ok(PushSendOutcome::Failed(status)) => {
                    warn!(%origin, status, "push.delivery_failed");
                }
                Err(err) => warn!(%origin, error = %err, "push.transport_failed"),
            }
        }
        if let Err(err) = self.ack(&claimed, &expired).await {
            // The rows return after the lease and are checked and sent again.
            warn!(error = %err, rows = claimed.len(), "push.ack_failed");
        }
        Ok(true)
    }

    async fn claim(&self) -> Result<Vec<Claimed>, sqlx::Error> {
        let mut tx = self.pool.begin().await?;
        set_system(&mut tx).await?;
        let rows: Vec<(Uuid, Uuid, Uuid, Uuid, i32)> = sqlx::query_as(
            r#"
            UPDATE fvoci.push_deliveries AS d
            SET claimed_until = now() + make_interval(secs => $2::double precision),
                attempt = d.attempt + 1
            WHERE d.id IN (
                SELECT id FROM fvoci.push_deliveries
                WHERE claimed_until IS NULL OR claimed_until < now()
                ORDER BY id
                LIMIT $1
                FOR UPDATE SKIP LOCKED
            )
            RETURNING d.id, d.event_id, d.user_id, d.subscription_id, d.attempt
            "#,
        )
        .bind(self.settings.batch.max(1))
        .bind(self.settings.claim_lease.as_secs_f64())
        .fetch_all(&mut *tx)
        .await?;
        tx.commit().await?;
        let mut claimed: Vec<Claimed> = rows
            .into_iter()
            .map(
                |(id, event_id, user_id, subscription_id, attempt)| Claimed {
                    id,
                    event_id,
                    user_id,
                    subscription_id,
                    attempt,
                },
            )
            .collect();
        claimed.sort_by_key(|row| row.id);
        Ok(claimed)
    }

    /// The handoff: re-checks every claimed row against committed state,
    /// deletes the rows that no longer qualify, marks the rest handed off and
    /// commits before any POST.
    async fn final_check(&self, claimed: &[Claimed]) -> Result<Vec<Ready>, sqlx::Error> {
        let mut tx = self.pool.begin().await?;
        set_system(&mut tx).await?;
        let mut ready = Vec::new();
        let mut passed = Vec::new();
        let mut by_event: HashMap<Uuid, Vec<&Claimed>> = HashMap::new();
        for row in claimed {
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
                let subscription: Option<(String, String, String)> = sqlx::query_as(
                    r#"
                    SELECT s.endpoint, s.p256dh, s.auth
                    FROM fvoci.push_subscriptions AS s
                    INNER JOIN fvoci.sessions AS se ON se.id = s.session_id
                    WHERE s.id = $1 AND s.user_id = $2
                      AND se.revoked_at IS NULL
                      AND se.expires_at > clock_timestamp()
                    "#,
                )
                .bind(row.subscription_id)
                .bind(row.user_id)
                .fetch_optional(&mut *tx)
                .await?;
                let Some((endpoint, p256dh, auth)) = subscription else {
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
                let Some(payload) = payload else {
                    continue;
                };
                passed.push(row.id);
                ready.push(Ready {
                    endpoint,
                    p256dh,
                    auth,
                    payload,
                });
            }
        }
        let (ids, attempts): (Vec<Uuid>, Vec<i32>) =
            claimed.iter().map(|row| (row.id, row.attempt)).unzip();
        // Fenced: a claim that lost its lease to another sender changes nothing.
        sqlx::query(
            r#"
            DELETE FROM fvoci.push_deliveries AS d
            USING unnest($1::uuid[], $2::int[]) AS c (id, attempt)
            WHERE d.id = c.id AND d.attempt = c.attempt AND NOT (d.id = ANY($3))
            "#,
        )
        .bind(&ids)
        .bind(&attempts)
        .bind(&passed)
        .execute(&mut *tx)
        .await?;
        let handed_off: Vec<Uuid> = sqlx::query_scalar(
            r#"
            UPDATE fvoci.push_deliveries AS d
            SET handed_off_at = now()
            FROM unnest($1::uuid[], $2::int[]) AS c (id, attempt)
            WHERE d.id = c.id AND d.attempt = c.attempt AND d.id = ANY($3)
            RETURNING d.id
            "#,
        )
        .bind(&ids)
        .bind(&attempts)
        .bind(&passed)
        .fetch_all(&mut *tx)
        .await?;
        tx.commit().await?;
        // Only rows this claim still owns are sent.
        Ok(passed
            .iter()
            .zip(ready)
            .filter(|(id, _)| handed_off.contains(id))
            .map(|(_, message)| message)
            .collect())
    }

    /// Terminal cleanup for this claim (fenced), plus 404/410 endpoints.
    async fn ack(&self, claimed: &[Claimed], expired: &[&str]) -> Result<(), sqlx::Error> {
        let mut tx = self.pool.begin().await?;
        set_system(&mut tx).await?;
        let (ids, attempts): (Vec<Uuid>, Vec<i32>) =
            claimed.iter().map(|row| (row.id, row.attempt)).unzip();
        sqlx::query(
            r#"
            DELETE FROM fvoci.push_deliveries AS d
            USING unnest($1::uuid[], $2::int[]) AS c (id, attempt)
            WHERE d.id = c.id AND d.attempt = c.attempt
            "#,
        )
        .bind(&ids)
        .bind(&attempts)
        .execute(&mut *tx)
        .await?;
        for endpoint in expired {
            remove_by_endpoint(&mut tx, endpoint).await?;
        }
        tx.commit().await
    }
}
