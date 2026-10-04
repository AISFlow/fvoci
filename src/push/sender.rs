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
//!    404/410 remove the endpoint for every user only if its current delivery
//!    was acknowledged. Stale attempts cannot trigger endpoint cleanup.
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
use crate::db::backend::Backend;
use crate::db::context::{set_system, set_tenant};
use crate::db::outbox::fetch_event_by_id;
use crate::db::push::PushClaim as Claimed;
use crate::integrations::outbound::Outbound;
use crate::push::consumer::{
    push_payload, push_payload_backend, push_recipients, push_recipients_backend,
};
use crate::push::db::remove_by_endpoint;
use crate::push::send::{
    endpoint_origin_for_log, send_push, PushPayload, PushSendOutcome, PushTarget, PUSH_TIMEOUT,
};
use crate::push::vapid::{load_vapid_key_pair_backend, VapidKeysError};

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
    spawn_push_sender_backend(Backend::Postgres(pool), outbound, keys, subject, settings)
}

/// Selected-backend sender; transport policy and shutdown semantics are identical.
pub fn spawn_push_sender_backend(
    backend: Backend,
    outbound: Outbound,
    keys: Option<Arc<Keyring>>,
    subject: String,
    settings: PushSenderSettings,
) -> PushSenderHandle {
    let cancel = CancellationToken::new();
    let wake = Arc::new(Notify::new());
    let sender = Sender {
        backend,
        outbound,
        keys,
        subject,
        settings,
    };
    let join = tokio::spawn(sender.run(cancel.child_token(), wake.clone()));
    PushSenderHandle { cancel, join, wake }
}

struct Sender {
    backend: Backend,
    outbound: Outbound,
    keys: Option<Arc<Keyring>>,
    subject: String,
    settings: PushSenderSettings,
}

struct Ready {
    claim: Claimed,
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
        let vapid = match load_vapid_key_pair_backend(&self.backend, self.keys.as_deref()).await {
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
                Ok(PushSendOutcome::ExpiredEndpoint) => expired.push(message),
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
        let mut tx = self.backend.begin_write().await?;
        let previous = tx.operation().set_system().await?;
        let mut claimed = tx
            .operation()
            .claim_push_deliveries(self.settings.batch, self.settings.claim_lease)
            .await?;
        tx.operation().restore_system(previous).await?;
        tx.commit()
            .await
            .map_err(|e| sqlx::Error::AnyDriverError(Box::new(e)))?;
        claimed.sort_by_key(|r| r.id);
        Ok(claimed)
    }

    async fn final_check(&self, claimed: &[Claimed]) -> Result<Vec<Ready>, sqlx::Error> {
        if let Backend::Postgres(_) = &self.backend {
            return self.final_check_pg(claimed).await;
        }
        let mut by_event: HashMap<Uuid, Vec<&Claimed>> = HashMap::new();
        for row in claimed {
            by_event.entry(row.event_id).or_default().push(row);
        }
        let mut ready = Vec::new();
        for (event_id, rows) in by_event {
            // Tenant cannot change inside a family writer, so each event owns
            // a short reservation through its authorization/handoff commit.
            let mut tx = self.backend.begin_write().await?;
            let previous = tx.operation().set_system().await?;
            let event = tx.operation().outbox_event_by_id(event_id).await?;
            if let Some(event) = event.filter(|e| e.workspace_id.is_some()) {
                let workspace = event.workspace_id.ok_or(sqlx::Error::RowNotFound)?;
                tx.operation().set_tenant(workspace).await?;
                let recipients = push_recipients_backend(&mut tx.operation(), &event).await?;
                let mut payloads: HashMap<Uuid, Option<Arc<PushPayload>>> = HashMap::new();
                for row in rows {
                    let mut message = None;
                    if let Some(notification) = recipients.get(&row.user_id) {
                        if let Some((endpoint, p256dh, auth)) = tx
                            .operation()
                            .push_claim_subscription(row, workspace)
                            .await?
                        {
                            let payload = match payloads.get(&row.user_id) {
                                Some(p) => p.clone(),
                                None => {
                                    let p = push_payload_backend(&mut tx.operation(), notification)
                                        .await?
                                        .map(Arc::new);
                                    payloads.insert(row.user_id, p.clone());
                                    p
                                }
                            };
                            if let Some(payload) = payload {
                                if tx.operation().hand_off_push_claim(row, workspace).await? {
                                    message = Some(Ready {
                                        claim: *row,
                                        endpoint,
                                        p256dh,
                                        auth,
                                        payload,
                                    });
                                }
                            }
                        }
                    }
                    if let Some(message) = message {
                        ready.push(message);
                    } else {
                        tx.operation().delete_push_claim(row).await?;
                    }
                }
            } else {
                for row in rows {
                    tx.operation().delete_push_claim(row).await?;
                }
            }
            tx.operation().restore_system(previous).await?;
            tx.commit()
                .await
                .map_err(|e| sqlx::Error::AnyDriverError(Box::new(e)))?;
        }
        Ok(ready)
    }

    /// The handoff: re-checks every claimed row against committed state,
    /// deletes the rows that no longer qualify, marks the rest handed off and
    /// commits before any POST.
    async fn final_check_pg(&self, claimed: &[Claimed]) -> Result<Vec<Ready>, sqlx::Error> {
        let pool = self.backend.postgres("push final authorization")?;
        let mut tx = pool.begin().await?;
        set_system(&mut tx).await?;
        let mut ready = Vec::new();
        let mut passed = Vec::new();
        let mut by_event: HashMap<Uuid, Vec<&Claimed>> = HashMap::new();
        for row in claimed {
            by_event.entry(row.event_id).or_default().push(row);
        }
        for (event_id, rows) in by_event {
            let Some(event) = fetch_event_by_id(pool, event_id).await? else {
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
                    claim: *row,
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
    async fn ack(&self, claimed: &[Claimed], expired: &[&Ready]) -> Result<(), sqlx::Error> {
        if let Backend::Postgres(_) = &self.backend {
            return self.ack_pg(claimed, expired).await;
        }
        let mut tx = self.backend.begin_write().await?;
        let previous = tx.operation().set_system().await?;
        let mut acknowledged = Vec::new();
        for row in claimed {
            if tx.operation().delete_push_claim(row).await? {
                acknowledged.push((row.id, row.attempt));
            }
        }
        for message in expired {
            if acknowledged.contains(&(message.claim.id, message.claim.attempt)) {
                tx.operation()
                    .remove_expired_push_endpoint(&message.endpoint)
                    .await?;
            }
        }
        tx.operation().restore_system(previous).await?;
        tx.commit()
            .await
            .map_err(|e| sqlx::Error::AnyDriverError(Box::new(e)))
    }

    async fn ack_pg(&self, claimed: &[Claimed], expired: &[&Ready]) -> Result<(), sqlx::Error> {
        let mut tx = self
            .backend
            .postgres("push acknowledgment")?
            .begin()
            .await?;
        set_system(&mut tx).await?;
        let (ids, attempts): (Vec<Uuid>, Vec<i32>) =
            claimed.iter().map(|row| (row.id, row.attempt)).unzip();
        let acknowledged: Vec<(Uuid, i32)> = sqlx::query_as(
            r#"
            DELETE FROM fvoci.push_deliveries AS d
            USING unnest($1::uuid[], $2::int[]) AS c (id, attempt)
            WHERE d.id = c.id AND d.attempt = c.attempt
            RETURNING d.id, d.attempt
            "#,
        )
        .bind(&ids)
        .bind(&attempts)
        .fetch_all(&mut *tx)
        .await?;
        for message in expired {
            if acknowledged.contains(&(message.claim.id, message.claim.attempt)) {
                remove_by_endpoint(&mut tx, &message.endpoint).await?;
            }
        }
        tx.commit().await
    }
}

#[cfg(test)]
mod backend_regressions {
    use super::*;
    use crate::db::notifications::family_runtime_fixture::Fixture;
    use crate::db::outbox::{
        ensure_consumer_backend, lease_consumer_backend, release_consumer_backend,
    };
    use crate::integrations::outbound::OutboundPolicy;
    use crate::push::consumer::{push_consumer, PUSH_CONSUMER};
    use base64::engine::general_purpose::URL_SAFE_NO_PAD;
    use base64::Engine;
    use std::sync::atomic::{AtomicUsize, Ordering};

    fn sender(f: &Fixture) -> Sender {
        Sender {
            backend: f.backend.clone(),
            outbound: Outbound::system(OutboundPolicy::default()),
            keys: None,
            subject: "https://fixture.example.invalid".into(),
            settings: PushSenderSettings::default(),
        }
    }
    fn keys() -> Arc<Keyring> {
        Arc::new(Keyring {
            active_id: "fixture".into(),
            keys: HashMap::from([("fixture".into(), vec![0x11; 32])]),
        })
    }
    async fn subscription(f: &Fixture, user: Uuid, session: Uuid, endpoint: &str) -> Uuid {
        let secret = web_push_native::p256::SecretKey::random(
            &mut web_push_native::p256::elliptic_curve::rand_core::OsRng,
        );
        let p256dh = URL_SAFE_NO_PAD
            .encode(web_push_native::p256::EncodedPoint::from(secret.public_key()).as_bytes());
        let auth = URL_SAFE_NO_PAD.encode([7u8; 16]);
        let id = Uuid::now_v7();
        sqlx::query("INSERT INTO push_subscriptions(id,user_id,session_id,endpoint,p256dh,auth) VALUES(?1,?2,?3,?4,?5,?6)")
            .bind(id.as_bytes().as_slice()).bind(user.as_bytes().as_slice()).bind(session.as_bytes().as_slice()).bind(endpoint).bind(p256dh).bind(auth).execute(&f.pool).await.unwrap();
        id
    }
    async fn queue(f: &Fixture) -> crate::db::outbox::BackendOutboxEvent {
        f.grant_wiki().await;
        sqlx::query("UPDATE instance_config SET vapid_public_key='fixture-public' WHERE vapid_public_key IS NULL").execute(&f.pool).await.unwrap();
        ensure_consumer_backend(&f.backend, PUSH_CONSUMER)
            .await
            .unwrap();
        let owner = Uuid::now_v7();
        assert!(lease_consumer_backend(&f.backend, PUSH_CONSUMER, owner, 60)
            .await
            .unwrap());
        let event = f.append_comment_event("comment.created").await;
        push_consumer(None)
            .deliver_backend(&f.backend, owner, &event)
            .await
            .unwrap();
        release_consumer_backend(&f.backend, PUSH_CONSUMER, owner)
            .await
            .unwrap();
        event
    }
    async fn delivery_count(f: &Fixture) -> i64 {
        sqlx::query_scalar("SELECT COUNT(*) FROM push_deliveries")
            .fetch_one(&f.pool)
            .await
            .unwrap()
    }
    async fn subscription_count(f: &Fixture) -> i64 {
        sqlx::query_scalar("SELECT COUNT(*) FROM push_subscriptions")
            .fetch_one(&f.pool)
            .await
            .unwrap()
    }
    async fn expire(f: &Fixture) {
        sqlx::query("UPDATE push_deliveries SET claimed_until=unixepoch()*1000000-1000000")
            .execute(&f.pool)
            .await
            .unwrap();
    }

    #[tokio::test]
    async fn actual_claim_current_auth_handoff_ack_and_claim_response_loss() {
        let f = Fixture::new().await;
        subscription(
            &f,
            f.user,
            f.credential,
            "https://push.example.invalid/positive",
        )
        .await;
        queue(&f).await;
        let s = sender(&f);
        let claim = s.claim().await.unwrap();
        assert_eq!(claim.len(), 1);
        assert_eq!(claim[0].attempt, 1);
        // Losing a confirmed claim's response does not immediately reclaim it.
        assert!(s.claim().await.unwrap().is_empty());
        let ready = s.final_check(&claim).await.unwrap();
        assert_eq!(ready.len(), 1);
        assert_eq!(ready[0].payload.url, "/w/notify-main/WIKI-1");
        let handoff: Option<i64> = sqlx::query_scalar("SELECT handed_off_at FROM push_deliveries")
            .fetch_one(&f.pool)
            .await
            .unwrap();
        assert!(handoff.unwrap() > 0);
        s.ack(&claim, &[]).await.unwrap();
        assert_eq!(delivery_count(&f).await, 0);
        assert_eq!(subscription_count(&f).await, 1);
        assert!(s.claim().await.unwrap().is_empty());
        f.finish().await;
    }

    #[tokio::test]
    async fn revocation_before_handoff_checks_current_target_role_user_session_and_prefs() {
        for denial in [
            "role",
            "target",
            "revoked",
            "expired",
            "foreign_session",
            "deleted",
            "suspended",
            "prefs",
            "workspace",
        ] {
            let f = Fixture::new().await;
            let id = subscription(
                &f,
                f.user,
                f.credential,
                "https://push.example.invalid/revoke",
            )
            .await;
            queue(&f).await;
            let s = sender(&f);
            let claim = s.claim().await.unwrap();
            assert_eq!(claim.len(), 1);
            match denial {
                "role" => {
                    sqlx::query("DELETE FROM memberships WHERE workspace_id=?1 AND user_id=?2")
                        .bind(f.workspace.as_bytes().as_slice())
                        .bind(f.user.as_bytes().as_slice())
                        .execute(&f.pool)
                        .await
                        .unwrap();
                }
                "target" => {
                    sqlx::query("DELETE FROM group_members WHERE workspace_id=?1 AND user_id=?2")
                        .bind(f.workspace.as_bytes().as_slice())
                        .bind(f.user.as_bytes().as_slice())
                        .execute(&f.pool)
                        .await
                        .unwrap();
                }
                "revoked" => {
                    sqlx::query("UPDATE sessions SET revoked_at=1 WHERE id=?1")
                        .bind(f.credential.as_bytes().as_slice())
                        .execute(&f.pool)
                        .await
                        .unwrap();
                }
                "expired" => {
                    sqlx::query("UPDATE sessions SET expires_at=unixepoch()*1000000 WHERE id=?1")
                        .bind(f.credential.as_bytes().as_slice())
                        .execute(&f.pool)
                        .await
                        .unwrap();
                }
                "foreign_session" => {
                    sqlx::query("UPDATE push_subscriptions SET session_id=?2 WHERE id=?1")
                        .bind(id.as_bytes().as_slice())
                        .bind(f.other_credential.as_bytes().as_slice())
                        .execute(&f.pool)
                        .await
                        .unwrap();
                }
                "deleted" | "suspended" => {
                    let q = if denial == "deleted" {
                        "UPDATE users SET deleted_at=1 WHERE id=?1"
                    } else {
                        "UPDATE users SET suspended_at=1 WHERE id=?1"
                    };
                    sqlx::query(q)
                        .bind(f.user.as_bytes().as_slice())
                        .execute(&f.pool)
                        .await
                        .unwrap();
                }
                "prefs" => {
                    sqlx::query("INSERT INTO notification_prefs(workspace_id,user_id,in_app,mail_immediate,mail_digest) VALUES(?1,?2,0,1,1)").bind(f.workspace.as_bytes().as_slice()).bind(f.user.as_bytes().as_slice()).execute(&f.pool).await.unwrap();
                }
                "workspace" => {
                    sqlx::query("UPDATE workspaces SET deleted_at=1 WHERE id=?1")
                        .bind(f.workspace.as_bytes().as_slice())
                        .execute(&f.pool)
                        .await
                        .unwrap();
                }
                _ => unreachable!(),
            }
            assert!(
                s.final_check(&claim).await.unwrap().is_empty(),
                "denial {denial}"
            );
            assert_eq!(delivery_count(&f).await, 0);
            assert_eq!(subscription_count(&f).await, 1);
            f.finish().await;
        }
    }

    #[tokio::test]
    async fn reclaimed_attempt_rejects_old_handoff_ack_and_expired_endpoint_cleanup() {
        let f = Fixture::new().await;
        let endpoint = "https://push.example.invalid/shared-expired";
        subscription(&f, f.user, f.credential, endpoint).await;
        subscription(&f, f.other_user, f.other_credential, endpoint).await;
        subscription(
            &f,
            f.other_user,
            f.other_credential,
            "https://push.example.invalid/unrelated",
        )
        .await;
        queue(&f).await;
        let s = sender(&f);
        let old = s.claim().await.unwrap();
        let ready = s.final_check(&old).await.unwrap();
        assert_eq!(ready.len(), 1);
        expire(&f).await;
        let current = s.claim().await.unwrap();
        assert_eq!(current.len(), 1);
        assert_eq!(current[0].attempt, 2);
        assert!(s.final_check(&old).await.unwrap().is_empty());
        let original_fenced_delete =
            sqlx::query("DELETE FROM push_deliveries WHERE id=?1 AND attempt=?2")
                .bind(old[0].id.as_bytes().as_slice())
                .bind(old[0].attempt)
                .execute(&f.pool)
                .await
                .unwrap();
        assert_eq!(original_fenced_delete.rows_affected(), 0);
        assert_eq!(delivery_count(&f).await, 1);
        // The unchanged oracle rejects original unconditional cleanup after
        // a zero-row fenced DELETE; only current Ready receipts may clean up.
        s.ack(&old, &[&ready[0]]).await.unwrap();
        assert_eq!(
            delivery_count(&f).await,
            1,
            "stale ack must retain new attempt"
        );
        assert_eq!(
            subscription_count(&f).await,
            3,
            "stale ack must not clean any endpoint"
        );
        let current_ready = s.final_check(&current).await.unwrap();
        assert_eq!(current_ready.len(), 1);
        s.ack(&current, &[&current_ready[0]]).await.unwrap();
        assert_eq!(delivery_count(&f).await, 0);
        assert_eq!(
            subscription_count(&f).await,
            1,
            "genuine gone endpoint is global; unrelated account endpoint survives"
        );
        f.finish().await;
    }

    #[tokio::test]
    async fn failed_ack_restart_rechecks_current_auth_and_preserves_handoff_gap() {
        let f = Fixture::new().await;
        subscription(
            &f,
            f.user,
            f.credential,
            "https://push.example.invalid/restart",
        )
        .await;
        queue(&f).await;
        let s = sender(&f);
        let old = s.claim().await.unwrap();
        let ready = s.final_check(&old).await.unwrap();
        assert_eq!(ready.len(), 1);
        sqlx::raw_sql("CREATE TRIGGER reject_push_ack BEFORE DELETE ON push_deliveries BEGIN SELECT RAISE(ABORT,'injected ack failure'); END;").execute(&f.pool).await.unwrap();
        assert!(s.ack(&old, &[]).await.is_err());
        assert_eq!(delivery_count(&f).await, 1);
        sqlx::query("DROP TRIGGER reject_push_ack")
            .execute(&f.pool)
            .await
            .unwrap();
        sqlx::query("UPDATE sessions SET revoked_at=1 WHERE id=?1")
            .bind(f.credential.as_bytes().as_slice())
            .execute(&f.pool)
            .await
            .unwrap();
        // Already approved memory cannot be recalled by this later revocation.
        assert_eq!(ready[0].payload.title, "한글🙂");
        expire(&f).await;
        let restarted = sender(&f);
        let current = restarted.claim().await.unwrap();
        assert_eq!(current[0].attempt, 2);
        assert!(restarted.final_check(&current).await.unwrap().is_empty());
        assert_eq!(delivery_count(&f).await, 0);
        f.finish().await;
    }

    #[tokio::test]
    async fn named_claim_requires_system_writer_and_handoff_tenant_and_rolls_back() {
        let f = Fixture::new().await;
        subscription(
            &f,
            f.user,
            f.credential,
            "https://push.example.invalid/authority",
        )
        .await;
        queue(&f).await;
        let mut tx = f.backend.begin_write().await.unwrap();
        assert!(tx
            .operation()
            .claim_push_deliveries(8, Duration::from_secs(30))
            .await
            .is_err());
        tx.operation().set_system().await.unwrap();
        let c = tx
            .operation()
            .claim_push_deliveries(8, Duration::from_secs(30))
            .await
            .unwrap();
        assert_eq!(c.len(), 1);
        assert!(tx
            .operation()
            .hand_off_push_claim(&c[0], f.workspace)
            .await
            .is_err());
        tx.rollback().await.unwrap();
        let mut tx = f.backend.begin_read().await.unwrap();
        tx.operation().set_system().await.unwrap();
        assert!(tx
            .operation()
            .claim_push_deliveries(8, Duration::from_secs(30))
            .await
            .is_err());
        tx.rollback().await.unwrap();
        let s = sender(&f);
        let c = s.claim().await.unwrap();
        assert_eq!(c[0].attempt, 1);
        s.ack(&c, &[]).await.unwrap();
        f.finish().await;
    }

    #[tokio::test]
    async fn no_due_no_key_damaged_key_and_cancelled_idle_are_real_results() {
        let f = Fixture::new().await;
        let mut s = sender(&f);
        assert!(!s.send_batch().await.unwrap());
        subscription(
            &f,
            f.user,
            f.credential,
            "https://push.example.invalid/no-key",
        )
        .await;
        queue(&f).await;
        assert!(s.send_batch().await.unwrap());
        assert_eq!(delivery_count(&f).await, 0);
        queue(&f).await;
        sqlx::query("UPDATE instance_config SET vapid_private_key='enc:v2:damaged' ")
            .execute(&f.pool)
            .await
            .unwrap();
        s.keys = Some(keys());
        assert!(s.send_batch().await.unwrap());
        assert_eq!(delivery_count(&f).await, 0);
        let handle = spawn_push_sender_backend(
            f.backend.clone(),
            s.outbound.clone(),
            None,
            s.subject.clone(),
            s.settings.clone(),
        );
        handle.request_shutdown();
        handle.join().await.unwrap();
        f.finish().await;
    }

    #[tokio::test]
    async fn local_synthetic_delivery_uses_maintained_transport_and_terminal_status_cleanup() {
        let f = Fixture::new().await;
        let k = keys();
        crate::push::vapid::ensure_vapid_keys_backend(&f.backend, Some(&k))
            .await
            .unwrap();
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let base = format!("http://{}", listener.local_addr().unwrap());
        let seen = Arc::new(AtomicUsize::new(0));
        let count = seen.clone();
        let app =
            axum::Router::new().fallback(move |uri: axum::http::Uri, body: axum::body::Bytes| {
                let count = count.clone();
                async move {
                    assert!(!body.is_empty());
                    count.fetch_add(1, Ordering::SeqCst);
                    match uri.path() {
                        "/gone" => axum::http::StatusCode::GONE,
                        "/missing" => axum::http::StatusCode::NOT_FOUND,
                        "/error" => axum::http::StatusCode::INTERNAL_SERVER_ERROR,
                        _ => axum::http::StatusCode::CREATED,
                    }
                }
            });
        let cancel = CancellationToken::new();
        let stopped = cancel.clone();
        let receiver = tokio::spawn(async move {
            axum::serve(listener, app)
                .with_graceful_shutdown(stopped.cancelled_owned())
                .await
                .unwrap()
        });
        for path in ["/ok", "/gone", "/missing", "/error"] {
            subscription(&f, f.user, f.credential, &format!("{base}{path}")).await;
        }
        subscription(
            &f,
            f.other_user,
            f.other_credential,
            &format!("{base}/gone"),
        )
        .await;
        let mut s = sender(&f);
        s.keys = Some(k);
        s.outbound = Outbound::system(OutboundPolicy::parse_allow_list("127.0.0.1").unwrap());
        queue(&f).await;
        assert!(s.send_batch().await.unwrap());
        assert_eq!(seen.load(Ordering::SeqCst), 4);
        assert_eq!(delivery_count(&f).await, 0);
        assert_eq!(subscription_count(&f).await, 2);
        assert!(!s.send_batch().await.unwrap());
        cancel.cancel();
        receiver.await.unwrap();
        f.finish().await;
    }
    #[tokio::test]
    async fn cancellation_after_received_request_retains_claim_until_reclaim_and_rechecks() {
        let f = Fixture::new().await;
        let k = keys();
        crate::push::vapid::ensure_vapid_keys_backend(&f.backend, Some(&k))
            .await
            .unwrap();
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let endpoint = format!("http://{}/held", listener.local_addr().unwrap());
        let arrived = Arc::new(Notify::new());
        let notice = arrived.clone();
        let release = CancellationToken::new();
        let unblock = release.clone();
        let app = axum::Router::new().fallback(move || {
            let notice = notice.clone();
            let unblock = unblock.clone();
            async move {
                notice.notify_one();
                unblock.cancelled().await;
                axum::http::StatusCode::CREATED
            }
        });
        let stop = CancellationToken::new();
        let stopped = stop.clone();
        let receiver = tokio::spawn(async move {
            axum::serve(listener, app)
                .with_graceful_shutdown(stopped.cancelled_owned())
                .await
                .unwrap()
        });
        subscription(&f, f.user, f.credential, &endpoint).await;
        queue(&f).await;
        let outbound = Outbound::system(OutboundPolicy::parse_allow_list("127.0.0.1").unwrap());
        let handle = spawn_push_sender_backend(
            f.backend.clone(),
            outbound,
            Some(k),
            "https://fixture.example.invalid".into(),
            PushSenderSettings::default(),
        );
        tokio::time::timeout(PUSH_TIMEOUT, arrived.notified())
            .await
            .unwrap();
        handle.request_shutdown();
        handle.join().await.unwrap();
        assert_eq!(
            delivery_count(&f).await,
            1,
            "unknown external result cannot be acked on cancellation"
        );
        assert!(sender(&f).claim().await.unwrap().is_empty());
        sqlx::query("UPDATE sessions SET revoked_at=1 WHERE id=?1")
            .bind(f.credential.as_bytes().as_slice())
            .execute(&f.pool)
            .await
            .unwrap();
        expire(&f).await;
        let s = sender(&f);
        let c = s.claim().await.unwrap();
        assert_eq!(c[0].attempt, 2);
        assert!(s.final_check(&c).await.unwrap().is_empty());
        assert_eq!(delivery_count(&f).await, 0);
        release.cancel();
        stop.cancel();
        receiver.await.unwrap();
        f.finish().await;
    }
}
