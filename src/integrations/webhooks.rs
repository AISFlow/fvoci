//! Outgoing workspace webhooks (source `core/webhook.ts`, `jobs/webhook.ts`).
//!
//! The `webhooks` outbox consumer is DatabaseAtomic: in the transaction that advances
//! its cursor it fans each workspace event into `webhook_deliveries` for every
//! hook subscribed to the verb whose creator may still manage the workspace and
//! can see the event. A separate sender task ([`spawn_webhook_sender`], source
//! ran webhook jobs on their own queue) sends due rows: signed POST, 2xx
//! delivered, 4xx (and refused targets/redirects) terminal, otherwise retry
//! after 1 / 5 / 15 min up to 5 attempts. The sender keeps at most `batch`
//! requests in flight and claims more as each one finishes, so a slow receiver
//! holds one slot for at most the request timeout and never the outbox
//! dispatcher that serves notifications, mail and search.

use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use std::time::Duration;

use hmac::{Hmac, Mac};
use serde_json::{json, Value};
use sha2::Sha256;
use sqlx::{PgPool, Postgres, Transaction};
use tokio::sync::Notify;
use tokio::task::JoinSet;
use tokio_util::sync::CancellationToken;
use tracing::warn;
use uuid::Uuid;

use crate::auth::password::Keyring;
use crate::db::backend::{Backend, OperationTx};
use crate::db::integrations::{
    claim_due_webhooks_backend, record_webhook_backend, ClaimedWebhookDelivery, DeliveryOutcome,
};
use crate::db::outbox::{
    advance_cursor_backend_tx, mark_processed_backend_tx, BackendOutboxEvent, OutboxEvent,
};
use crate::integrations::outbound::{Outbound, OutboundError};
use crate::outbox::{DeliveryMode, OutboxConsumer, OutboxProcessError};
use crate::projects::ProjectPermission;
use crate::secret_box;

pub const WEBHOOKS_CONSUMER: &str = "webhooks";
pub const WEBHOOK_BACKOFF: [Duration; 3] = [
    Duration::from_secs(60),
    Duration::from_secs(300),
    Duration::from_secs(900),
];
pub const WEBHOOK_MAX_ATTEMPTS: i32 = 5;
pub const WEBHOOK_EVENTS_MAX: usize = 64;
pub const WEBHOOK_USER_AGENT: &str = "FVOCI-Webhook/1";

/// Source `EVENT_CATALOG`: the verbs a webhook may subscribe to.
pub const EVENT_CATALOG: &[&str] = &[
    "instance.setup",
    "auth.login",
    "auth.logout",
    "auth.password_changed",
    "auth.password_reset",
    "auth.mfa_enabled",
    "auth.mfa_disabled",
    "user.name_updated",
    "user.email_changed",
    "user.withdrawn",
    "user.withdraw_cancelled",
    "user.anonymized",
    "consent.recorded",
    "legal.published",
    "admin.instance_admin_set",
    "admin.user_suspended_set",
    "instance_settings.updated",
    "instance.vapid_rotated",
    "workspace.created",
    "workspace.deleted",
    "workspace_member.removed",
    "workspace_member.role_changed",
    "invitation.created",
    "invitation.accepted",
    "identity.linked",
    "identity.unlinked",
    "project.created",
    "project.updated",
    "project.archived",
    "project.unarchived",
    "project.deleted",
    "project.restored",
    "project_member.added",
    "project_member.removed",
    "project_member.role_changed",
    "document.created",
    "document.updated",
    "document.moved",
    "document.trashed",
    "document.restored",
    "document.purged",
    "attachment.completed",
    "attachment.deleted",
    "task.created",
    "task.updated",
    "task.deleted",
    "comment.created",
    "comment.updated",
    "comment.deleted",
    "comment.resolved",
];

/// Source `normalizeEvents`: 1..=64 known verbs, order kept.
pub fn normalize_events(events: &[String]) -> Option<Vec<String>> {
    if events.is_empty() || events.len() > WEBHOOK_EVENTS_MAX {
        return None;
    }
    events
        .iter()
        .map(|verb| EVENT_CATALOG.contains(&verb.as_str()).then(|| verb.clone()))
        .collect()
}

pub fn webhook_secret_context(workspace_id: Uuid, webhook_id: Uuid) -> String {
    format!("webhook:{workspace_id}:{webhook_id}")
}

/// Source `signWebhookBody`: `sha256=<hex HMAC-SHA256(secret, body)>`.
pub fn sign_body(secret: &str, body: &[u8]) -> String {
    let mut mac = Hmac::<Sha256>::new_from_slice(secret.as_bytes()).expect("hmac key");
    mac.update(body);
    format!("sha256={}", hex::encode(mac.finalize().into_bytes()))
}

/// Source `serializeWebhookPayload`; `createdAt` in JS `toISOString` form.
pub fn serialize_payload(event: &OutboxEvent) -> Vec<u8> {
    serialize_payload_backend(&event.clone().into())
}

pub fn serialize_payload_backend(event: &BackendOutboxEvent) -> Vec<u8> {
    serde_json::to_vec(&json!({
        "id": event.id.to_string(),
        "verb": event.verb,
        "workspaceId": event.workspace_id.map(|id| id.to_string()),
        "actorUserId": event.actor_user_id.map(|id| id.to_string()),
        "targetType": event.target_type,
        "targetId": event.target_id.map(|id| id.to_string()),
        "payload": event.payload,
        "channel": event.channel,
        "createdAt": event.created_at.format("%Y-%m-%dT%H:%M:%S%.3fZ").to_string(),
    }))
    .expect("webhook payload json")
}

fn backoff_after(failed_attempt: i32) -> Duration {
    let index = (failed_attempt.max(1) as usize - 1).min(WEBHOOK_BACKOFF.len() - 1);
    WEBHOOK_BACKOFF[index]
}

/// Source `recordDeliveryResult` classification.
pub fn classify(attempt: i32, http_status: Option<u16>) -> DeliveryOutcome {
    match http_status {
        Some(status) if (200..300).contains(&status) => DeliveryOutcome::Delivered,
        Some(status) if (400..500).contains(&status) => DeliveryOutcome::Failed,
        _ if attempt >= WEBHOOK_MAX_ATTEMPTS => DeliveryOutcome::Failed,
        _ => DeliveryOutcome::Retry {
            after: backoff_after(attempt),
        },
    }
}

fn payload_uuid(payload: &Value, key: &str) -> Option<Uuid> {
    payload
        .get(key)
        .and_then(Value::as_str)
        .and_then(|s| Uuid::parse_str(s).ok())
}

enum Scope {
    Project(Uuid),
    Document(Uuid),
    Open,
}

async fn document_scope(
    tx: &mut OperationTx<'_, '_>,
    workspace_id: Uuid,
    document_id: Uuid,
) -> Result<Scope, sqlx::Error> {
    Ok(
        match tx
            .webhook_document_project(workspace_id, document_id)
            .await?
        {
            Some(Some(project_id)) => Scope::Project(project_id),
            Some(None) => Scope::Document(document_id),
            None => Scope::Open,
        },
    )
}

async fn task_scope(
    tx: &mut OperationTx<'_, '_>,
    workspace_id: Uuid,
    task_id: Uuid,
) -> Result<Scope, sqlx::Error> {
    Ok(tx
        .webhook_task_project(workspace_id, task_id)
        .await?
        .map_or(Scope::Open, Scope::Project))
}

/// Source `eventVisibleTo`: the event's project (from the payload, its
/// document, task, attachment or comment) must be viewable by `user_id`.
/// Events outside any project are visible; a workspace document outside a
/// project additionally follows its own document permission.
pub(crate) async fn event_visible_to(
    tx: &mut Transaction<'_, Postgres>,
    user_id: Uuid,
    event: &OutboxEvent,
) -> Result<bool, sqlx::Error> {
    event_visible_to_backend(
        &mut OperationTx::Postgres(tx),
        user_id,
        &event.clone().into(),
    )
    .await
}

pub(crate) async fn event_visible_to_backend(
    tx: &mut OperationTx<'_, '_>,
    user_id: Uuid,
    event: &BackendOutboxEvent,
) -> Result<bool, sqlx::Error> {
    let Some(workspace_id) = event.workspace_id else {
        return Ok(true);
    };
    let payload = &event.payload;
    let scope = if let Some(project_id) = payload_uuid(payload, "projectId") {
        Scope::Project(project_id)
    } else if let Some(document_id) = payload_uuid(payload, "documentId") {
        document_scope(tx, workspace_id, document_id).await?
    } else if let Some(task_id) = payload_uuid(payload, "taskId") {
        task_scope(tx, workspace_id, task_id).await?
    } else {
        match (event.target_type.as_deref(), event.target_id) {
            (Some("task"), Some(id)) => task_scope(tx, workspace_id, id).await?,
            (Some("document"), Some(id)) => document_scope(tx, workspace_id, id).await?,
            (Some("attachment"), Some(id)) => {
                let parent = tx.webhook_attachment_parent(workspace_id, id).await?;
                match parent {
                    Some((Some(document_id), _)) => {
                        document_scope(tx, workspace_id, document_id).await?
                    }
                    Some((None, Some(task_id))) => task_scope(tx, workspace_id, task_id).await?,
                    _ => Scope::Open,
                }
            }
            (Some("comment"), Some(id)) => {
                let parent = tx.webhook_comment_parent(workspace_id, id).await?;
                match parent {
                    Some((Some(document_id), _)) => {
                        document_scope(tx, workspace_id, document_id).await?
                    }
                    Some((None, Some(task_id))) => task_scope(tx, workspace_id, task_id).await?,
                    _ => Scope::Open,
                }
            }
            _ => Scope::Open,
        }
    };
    Ok(match scope {
        Scope::Open => true,
        Scope::Project(project_id) => tx
            .project_permission_by_id(workspace_id, user_id, project_id)
            .await?
            .is_some_and(|level| level.at_least(ProjectPermission::View)),
        Scope::Document(document_id) => tx
            .document_permission(workspace_id, user_id, document_id, false)
            .await?
            .at_least(ProjectPermission::View),
    })
}

/// Source `enqueueWebhookDeliveries`, plus the creator's current manage right.
pub async fn fan_out(
    tx: &mut Transaction<'_, Postgres>,
    event: &OutboxEvent,
) -> Result<usize, sqlx::Error> {
    fan_out_backend(&mut OperationTx::Postgres(tx), &event.clone().into()).await
}

pub(crate) async fn fan_out_backend(
    tx: &mut OperationTx<'_, '_>,
    event: &BackendOutboxEvent,
) -> Result<usize, sqlx::Error> {
    let Some(workspace_id) = event.workspace_id else {
        return Ok(0);
    };
    let hooks = tx.webhook_subscriptions(workspace_id, &event.verb).await?;
    let mut queued = 0;
    for (webhook_id, created_by) in hooks {
        if !tx
            .webhook_creator_can_manage(workspace_id, created_by)
            .await?
        {
            continue;
        }
        if !event_visible_to_backend(tx, created_by, event).await? {
            continue;
        }
        tx.webhook_enqueue(workspace_id, webhook_id, event.id)
            .await?;
        queued += 1;
    }
    Ok(queued)
}

#[derive(Debug, Clone)]
pub struct WebhookDeliverySettings {
    /// Per-request budget (source 10 s), name resolution included.
    pub request_timeout: Duration,
    /// Requests in flight at once (source concurrency 20).
    pub batch: i64,
    /// Claim lease (source 240 s): a crashed sender's rows return after this.
    pub claim_lease: Duration,
    /// Idle poll for newly due rows.
    pub poll_interval: Duration,
}

impl Default for WebhookDeliverySettings {
    fn default() -> Self {
        Self {
            request_timeout: Duration::from_secs(10),
            batch: 20,
            claim_lease: Duration::from_secs(240),
            poll_interval: Duration::from_secs(1),
        }
    }
}

/// Fan-out only; sending is [`spawn_webhook_sender`].
pub struct WebhooksConsumer;

pub fn webhooks_consumer() -> Arc<dyn OutboxConsumer> {
    Arc::new(WebhooksConsumer)
}

impl OutboxConsumer for WebhooksConsumer {
    fn name(&self) -> &str {
        WEBHOOKS_CONSUMER
    }

    fn delivery_mode(&self) -> DeliveryMode {
        DeliveryMode::DatabaseAtomic
    }

    fn deliver<'a>(
        &'a self,
        pool: &'a PgPool,
        lease_owner: Uuid,
        event: &'a OutboxEvent,
    ) -> Pin<Box<dyn Future<Output = Result<(), OutboxProcessError>> + Send + 'a>> {
        Box::pin(async move { fan_out_event(pool, lease_owner, event).await })
    }
    fn deliver_backend<'a>(
        &'a self,
        backend: &'a Backend,
        lease_owner: Uuid,
        event: &'a BackendOutboxEvent,
    ) -> Pin<Box<dyn Future<Output = Result<(), OutboxProcessError>> + Send + 'a>> {
        Box::pin(async move { fan_out_event_backend(backend, lease_owner, event).await })
    }
}

async fn fan_out_event(
    pool: &PgPool,
    lease_owner: Uuid,
    event: &OutboxEvent,
) -> Result<(), OutboxProcessError> {
    fan_out_event_backend(
        &Backend::Postgres(pool.clone()),
        lease_owner,
        &event.clone().into(),
    )
    .await
}

pub(crate) async fn fan_out_event_backend(
    backend: &Backend,
    lease_owner: Uuid,
    event: &BackendOutboxEvent,
) -> Result<(), OutboxProcessError> {
    let mut tx = backend.begin_write().await?;
    let previous = tx.operation().set_system().await?;
    if let Some(workspace) = event.workspace_id {
        tx.operation().set_tenant(workspace).await?;
    }
    let current = tx
        .operation()
        .outbox_event_by_id(event.id)
        .await?
        .ok_or_else(|| OutboxProcessError::Delivery("webhook event missing".into()))?;
    if current.workspace_id != event.workspace_id || current.cursor() != event.cursor() {
        tx.rollback().await?;
        return Err(OutboxProcessError::Delivery(
            "webhook event scope or cursor changed".into(),
        ));
    }
    if mark_processed_backend_tx(&mut tx, WEBHOOKS_CONSUMER, current.id).await? {
        fan_out_backend(&mut tx.operation(), &current).await?;
    }
    if !advance_cursor_backend_tx(&mut tx, WEBHOOKS_CONSUMER, lease_owner, &current.cursor())
        .await?
    {
        tx.rollback().await?;
        return Err(OutboxProcessError::Delivery(
            "advance rejected in webhook transaction".into(),
        ));
    }
    tx.operation().restore_system(previous).await?;
    tx.commit()
        .await
        .map_err(|e| OutboxProcessError::Db(sqlx::Error::AnyDriverError(Box::new(e))))?;
    // The sender polls independently; any future wake must follow this confirmed commit.
    Ok(())
}

enum Prepared {
    Dead(&'static str),
    Send {
        url: String,
        body: Vec<u8>,
        signature: String,
    },
}

async fn prepare(
    backend: &Backend,
    keys: Option<&Keyring>,
    claim: &ClaimedWebhookDelivery,
) -> Result<Prepared, sqlx::Error> {
    let due = &claim.due;
    let mut tx = backend.begin_read().await?;
    tx.operation().set_tenant(due.workspace_id).await?;
    let previous = tx.operation().set_system().await?;
    if !tx.operation().webhook_claim_is_current(claim).await? {
        tx.rollback().await?;
        return Ok(Prepared::Dead("claim_replaced"));
    }
    let Some(event) = tx.operation().outbox_event_by_id(due.event_id).await? else {
        tx.rollback().await?;
        return Ok(Prepared::Dead("event_missing"));
    };
    if event.workspace_id != Some(due.workspace_id) {
        tx.rollback().await?;
        return Ok(Prepared::Dead("event_wrong_workspace"));
    }
    let Some(target) = tx.operation().webhook_target(due).await? else {
        tx.rollback().await?;
        return Ok(Prepared::Dead("webhook_missing"));
    };
    if !tx
        .operation()
        .webhook_creator_can_manage(due.workspace_id, target.created_by)
        .await?
    {
        tx.rollback().await?;
        return Ok(Prepared::Dead("creator_not_manager"));
    }
    if !event_visible_to_backend(&mut tx.operation(), target.created_by, &event).await? {
        tx.rollback().await?;
        return Ok(Prepared::Dead("event_not_visible"));
    }
    tx.operation().restore_system(previous).await?;
    tx.commit()
        .await
        .map_err(|e| sqlx::Error::AnyDriverError(Box::new(e)))?;
    let Some(keys) = keys else {
        return Ok(Prepared::Dead("encryption_keys_unset"));
    };
    let secret = match secret_box::open(
        keys,
        &target.sealed_secret,
        &webhook_secret_context(due.workspace_id, due.webhook_id),
    ) {
        Ok(secret) => secret,
        Err(_) => return Ok(Prepared::Dead("secret_unavailable")),
    };
    let body = serialize_payload_backend(&event);
    let signature = sign_body(&secret, &body);
    Ok(Prepared::Send {
        url: target.url,
        body,
        signature,
    })
}

/// Sends one claimed row and records the attempt. Logs carry ids, attempt,
/// status and an error class only: never the URL, secret, signature or body.
async fn deliver_one(
    backend: Backend,
    outbound: Outbound,
    keys: Option<Arc<Keyring>>,
    claim: ClaimedWebhookDelivery,
    timeout: Duration,
) -> Result<(), sqlx::Error> {
    let due = &claim.due;
    let attempt = due
        .attempt
        .checked_add(1)
        .ok_or_else(|| sqlx::Error::Protocol("webhook attempt overflow".into()))?;
    let (outcome, http_status, error) = match prepare(&backend, keys.as_deref(), &claim).await? {
        Prepared::Dead(reason) => (DeliveryOutcome::Failed, None, Some(reason.to_string())),
        Prepared::Send {
            url,
            body,
            signature,
        } => {
            let headers = [
                ("content-type", "application/json".to_string()),
                ("x-fvoci-signature", signature),
                ("user-agent", WEBHOOK_USER_AGENT.to_string()),
            ];
            match outbound.post(&url, &headers, body, timeout).await {
                Ok(status) => (classify(attempt, Some(status)), Some(status), None),
                // Source maps a refused target or redirect to 400: terminal.
                Err(OutboundError::Rejected(rejected)) => (
                    DeliveryOutcome::Failed,
                    None,
                    Some(format!("refused: {rejected}")),
                ),
                Err(OutboundError::Transport(kind)) => (classify(attempt, None), None, Some(kind)),
            }
        }
    };
    let recorded = record_webhook_backend(&backend, &claim, outcome, http_status).await?;
    if outcome != DeliveryOutcome::Delivered || !recorded {
        warn!(
            delivery_id = %due.id,
            webhook_id = %due.webhook_id,
            attempt,
            http_status,
            outcome = ?outcome,
            recorded,
            error = error.as_deref().unwrap_or(""),
            "webhook.deliver_failed"
        );
    }
    Ok(())
}

pub struct WebhookSenderHandle {
    cancel: CancellationToken,
    join: tokio::task::JoinHandle<()>,
    pub wake: Arc<Notify>,
}

impl WebhookSenderHandle {
    pub fn request_shutdown(&self) {
        self.cancel.cancel();
    }

    pub async fn join(self) -> Result<(), String> {
        self.join
            .await
            .map_err(|err| format!("webhook sender task join failed: {err}"))
    }
}

/// Sends due `webhook_deliveries` rows on its own task. Shutdown aborts the
/// requests in flight; their rows return after the claim lease and the
/// attempt fence keeps a late record from counting twice.
pub fn spawn_webhook_sender(
    pool: PgPool,
    outbound: Outbound,
    keys: Option<Arc<Keyring>>,
    settings: WebhookDeliverySettings,
) -> WebhookSenderHandle {
    spawn_webhook_sender_backend(Backend::Postgres(pool), outbound, keys, settings)
}

pub fn spawn_webhook_sender_backend(
    backend: Backend,
    outbound: Outbound,
    keys: Option<Arc<Keyring>>,
    settings: WebhookDeliverySettings,
) -> WebhookSenderHandle {
    let cancel = CancellationToken::new();
    let wake = Arc::new(Notify::new());
    let join = tokio::spawn(run_sender(
        backend,
        outbound,
        keys,
        settings,
        cancel.child_token(),
        wake.clone(),
    ));
    WebhookSenderHandle { cancel, join, wake }
}

fn log_send_result(joined: Result<Result<(), sqlx::Error>, tokio::task::JoinError>) {
    match joined {
        Ok(Ok(())) => {}
        Ok(Err(err)) => warn!(error = %err, "webhook.record_failed"),
        Err(err) if err.is_cancelled() => {}
        Err(err) => warn!(error = %err, "webhook.send_task_failed"),
    }
}

async fn run_sender(
    backend: Backend,
    outbound: Outbound,
    keys: Option<Arc<Keyring>>,
    settings: WebhookDeliverySettings,
    cancel: CancellationToken,
    wake: Arc<Notify>,
) {
    let slots = settings.batch.max(1) as usize;
    let mut sends: JoinSet<Result<(), sqlx::Error>> = JoinSet::new();
    while !cancel.is_cancelled() {
        while let Some(joined) = sends.try_join_next() {
            log_send_result(joined);
        }
        let free = slots.saturating_sub(sends.len());
        if free > 0 {
            match claim_due_webhooks_backend(&backend, free as i64, settings.claim_lease).await {
                Ok(due) => {
                    for row in due {
                        sends.spawn(deliver_one(
                            backend.clone(),
                            outbound.clone(),
                            keys.clone(),
                            row,
                            settings.request_timeout,
                        ));
                    }
                }
                Err(err) => warn!(error = %err, "webhook.claim_failed"),
            }
        }
        // A finished send frees a slot and claims again at once.
        tokio::select! {
            () = cancel.cancelled() => break,
            () = wake.notified() => {}
            () = tokio::time::sleep(settings.poll_interval) => {}
            Some(joined) = sends.join_next(), if !sends.is_empty() => log_send_result(joined),
        }
    }
    sends.shutdown().await;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn classify_follows_source_retry_rules() {
        assert_eq!(classify(1, Some(204)), DeliveryOutcome::Delivered);
        assert_eq!(classify(1, Some(410)), DeliveryOutcome::Failed);
        assert_eq!(
            classify(1, Some(500)),
            DeliveryOutcome::Retry {
                after: Duration::from_secs(60)
            }
        );
        assert_eq!(
            classify(2, None),
            DeliveryOutcome::Retry {
                after: Duration::from_secs(300)
            }
        );
        assert_eq!(
            classify(4, Some(503)),
            DeliveryOutcome::Retry {
                after: Duration::from_secs(900)
            }
        );
        assert_eq!(classify(5, Some(503)), DeliveryOutcome::Failed);
        assert_eq!(classify(5, None), DeliveryOutcome::Failed);
    }

    #[test]
    fn signature_matches_known_hmac() {
        // RFC 4231 test case 2.
        let sig = sign_body("Jefe", b"what do ya want for nothing?");
        assert_eq!(
            sig,
            "sha256=5bdcc146bf60754e6a042426089575c75a003f089d2739839dec58b964ec3843"
        );
    }

    #[test]
    fn events_must_be_known_and_bounded() {
        assert_eq!(
            normalize_events(&["task.created".into(), "comment.created".into()]),
            Some(vec!["task.created".into(), "comment.created".into()])
        );
        assert_eq!(normalize_events(&[]), None);
        assert_eq!(normalize_events(&["task.nope".into()]), None);
        assert_eq!(
            normalize_events(&vec!["task.created".to_string(); WEBHOOK_EVENTS_MAX + 1]),
            None
        );
    }
}

#[cfg(test)]
mod backend_regressions {
    use super::*;
    use crate::db::integrations::webhook_family_fixture::*;
    use crate::db::outbox::{
        ensure_consumer_backend, fetch_cursor_backend, is_processed_backend,
        lease_consumer_backend, OutboxCursor,
    };
    use crate::integrations::outbound::OutboundPolicy;
    use axum::{
        body::Bytes,
        extract::State,
        http::{HeaderMap, StatusCode},
        routing::post,
        Router,
    };
    use std::sync::{
        atomic::{AtomicBool, Ordering},
        Mutex,
    };

    #[derive(Clone, Default)]
    struct Capture {
        rows: Arc<Mutex<Vec<(HeaderMap, Vec<u8>)>>>,
        received: Arc<Notify>,
        unknown: Arc<AtomicBool>,
    }
    async fn receive(State(c): State<Capture>, headers: HeaderMap, body: Bytes) -> StatusCode {
        c.rows.lock().unwrap().push((headers, body.to_vec()));
        c.received.notify_one();
        if c.unknown.load(Ordering::SeqCst) {
            std::future::pending::<()>().await;
        }
        StatusCode::NO_CONTENT
    }
    struct Receiver {
        capture: Capture,
        url: String,
        cancel: CancellationToken,
        join: tokio::task::JoinHandle<()>,
    }
    impl Receiver {
        async fn new() -> Self {
            let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
            let address = listener.local_addr().unwrap();
            let capture = Capture::default();
            let app = Router::new()
                .route("/synthetic", post(receive))
                .with_state(capture.clone());
            let cancel = CancellationToken::new();
            let shutdown = cancel.clone();
            let join = tokio::spawn(async move {
                axum::serve(listener, app)
                    .with_graceful_shutdown(shutdown.cancelled_owned())
                    .await
                    .unwrap();
            });
            Self {
                capture,
                url: format!("http://{address}/synthetic"),
                cancel,
                join,
            }
        }
        fn outbound(&self) -> Outbound {
            Outbound::system(OutboundPolicy::parse_allow_list("127.0.0.1").unwrap())
        }
        fn count(&self) -> usize {
            self.capture.rows.lock().unwrap().len()
        }
        async fn finish(self) {
            self.cancel.cancel();
            self.join.abort();
            assert!(self.join.await.unwrap_err().is_cancelled());
        }
    }
    async fn lease(f: &Fixture) -> Uuid {
        ensure_consumer_backend(&f.backend, WEBHOOKS_CONSUMER)
            .await
            .unwrap();
        let owner = Uuid::now_v7();
        assert!(
            lease_consumer_backend(&f.backend, WEBHOOKS_CONSUMER, owner, 30)
                .await
                .unwrap()
        );
        owner
    }
    async fn marker_queue(f: &Fixture, event: Uuid) -> (i64, i64) {
        let m = sqlx::query_scalar(
            "SELECT count(*) FROM processed_events WHERE consumer='webhooks' AND event_id=?1",
        )
        .bind(event.as_bytes().as_slice())
        .fetch_one(&f.pool)
        .await
        .unwrap();
        let q = sqlx::query_scalar("SELECT count(*) FROM webhook_deliveries WHERE event_id=?1")
            .bind(event.as_bytes().as_slice())
            .fetch_one(&f.pool)
            .await
            .unwrap();
        (m, q)
    }
    async fn make_due(f: &Fixture, id: Uuid) {
        sqlx::query("UPDATE webhook_deliveries SET next_attempt_at=(unixepoch()*1000000+CAST(substr(strftime('%f','now'),4,3) AS INTEGER)*1000)-1 WHERE id=?1 AND status='pending'").bind(id.as_bytes().as_slice()).execute(&f.pool).await.unwrap();
    }
    async fn append(
        f: &Fixture,
        verb: &str,
        target_type: Option<&str>,
        target: Option<Uuid>,
        payload: Value,
    ) -> BackendOutboxEvent {
        let id = Uuid::now_v7();
        let mut tx = f.backend.begin_write().await.unwrap();
        tx.operation().set_tenant(f.workspace).await.unwrap();
        let previous = tx.operation().set_system().await.unwrap();
        tx.operation()
            .append_event(crate::db::identity::EventAppend {
                id,
                workspace_id: Some(f.workspace),
                actor_user_id: Some(f.actor),
                verb: verb.into(),
                target_type: target_type.map(str::to_string),
                target_id: target,
                payload,
            })
            .await
            .unwrap();
        tx.operation().restore_system(previous).await.unwrap();
        tx.commit().await.unwrap();
        crate::db::outbox::fetch_event_by_id_backend(&f.backend, id)
            .await
            .unwrap()
            .unwrap()
    }

    #[tokio::test]
    async fn actual_backend_registered_dispatcher_sends_signed_committed_source_and_fresh_row() {
        let f = Fixture::new().await;
        let receiver = Receiver::new().await;
        let hook_id = hook(&f, &receiver.url).await;
        let event = append(
            &f,
            "document.updated",
            Some("document"),
            Some(f.document),
            json!({"title":"한글🙂","null":null,"version":"9007199254740993"}),
        )
        .await;
        let sender = spawn_webhook_sender_backend(
            f.backend.clone(),
            receiver.outbound(),
            Some(keys()),
            WebhookDeliverySettings {
                poll_interval: Duration::from_millis(10),
                ..Default::default()
            },
        );
        let dispatcher = crate::outbox::spawn_outbox_dispatcher_backend(
            crate::outbox::OutboxDispatcherSettings {
                poll_interval: Duration::from_millis(10),
                ..Default::default()
            },
            f.backend.clone(),
            vec![webhooks_consumer()],
        )
        .unwrap();
        tokio::time::timeout(Duration::from_secs(3),async {
            loop {let delivered:bool=sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM webhook_deliveries WHERE webhook_id=?1 AND event_id=?2 AND status='delivered' AND attempt=1 AND http_status=204)").bind(hook_id.as_bytes().as_slice()).bind(event.id.as_bytes().as_slice()).fetch_one(&f.pool).await.unwrap();if delivered{break}tokio::task::yield_now().await;}
        }).await.unwrap();
        dispatcher.request_shutdown();
        sender.request_shutdown();
        dispatcher.join().await.unwrap();
        sender.join().await.unwrap();
        assert_eq!(receiver.count(), 1);
        let capture = receiver.capture.rows.lock().unwrap()[0].clone();
        assert_eq!(capture.1, serialize_payload_backend(&event));
        assert_eq!(
            capture.0["x-fvoci-signature"],
            sign_body(SECRET, &capture.1)
        );
        assert_eq!(capture.0["user-agent"], WEBHOOK_USER_AGENT);
        assert_eq!(marker_queue(&f, event.id).await, (1, 1));
        assert!(
            is_processed_backend(&f.backend, WEBHOOKS_CONSUMER, event.id)
                .await
                .unwrap()
        );
        assert_eq!(
            fetch_cursor_backend(&f.backend, WEBHOOKS_CONSUMER)
                .await
                .unwrap(),
            Some(OutboxCursor::SqliteFamily { seq: event.seq })
        );
        receiver.finish().await;
        f.finish().await;
    }

    #[tokio::test]
    async fn actual_backend_atomic_failed_cursor_duplicate_no_hook_and_wrong_tenant() {
        let f = Fixture::new().await;
        let _hook = hook(&f, "http://127.0.0.1:1/unused").await;
        let owner = lease(&f).await;
        let event = f.append_comment_event("comment.created").await;
        assert!(fan_out_event_backend(&f.backend, Uuid::now_v7(), &event)
            .await
            .is_err());
        assert_eq!(marker_queue(&f, event.id).await, (0, 0));
        assert_eq!(
            fetch_cursor_backend(&f.backend, WEBHOOKS_CONSUMER)
                .await
                .unwrap(),
            Some(OutboxCursor::SqliteFamily { seq: 0 })
        );
        let mut forged = event.clone();
        forged.workspace_id = Some(f.other_workspace);
        assert!(fan_out_event_backend(&f.backend, owner, &forged)
            .await
            .is_err());
        assert_eq!(marker_queue(&f, event.id).await, (0, 0));
        // Duplicate mark and UNIQUE queue controls use the same actual owner
        // writer before its valid cursor advance, matching DatabaseAtomic.
        let mut tx = f.backend.begin_write().await.unwrap();
        tx.operation().set_tenant(f.workspace).await.unwrap();
        let previous = tx.operation().set_system().await.unwrap();
        let current = tx
            .operation()
            .outbox_event_by_id(event.id)
            .await
            .unwrap()
            .unwrap();
        assert!(
            mark_processed_backend_tx(&mut tx, WEBHOOKS_CONSUMER, current.id)
                .await
                .unwrap()
        );
        assert_eq!(
            fan_out_backend(&mut tx.operation(), &current)
                .await
                .unwrap(),
            1
        );
        assert!(
            !mark_processed_backend_tx(&mut tx, WEBHOOKS_CONSUMER, current.id)
                .await
                .unwrap()
        );
        assert_eq!(
            fan_out_backend(&mut tx.operation(), &current)
                .await
                .unwrap(),
            1
        );
        let mut operation = tx.operation();
        let OperationTx::SqliteFamily(family) = &mut operation else {
            unreachable!()
        };
        let duplicate_counts=family.query("SELECT (SELECT count(*) FROM processed_events WHERE consumer='webhooks' AND event_id=?1),(SELECT count(*) FROM webhook_deliveries WHERE event_id=?1)",&[crate::db::codec::Cell::uuid(current.id)]).await.unwrap();
        assert_eq!(duplicate_counts[0].cell(0).unwrap().integer().unwrap(), 1);
        assert_eq!(duplicate_counts[0].cell(1).unwrap().integer().unwrap(), 1);
        assert!(
            advance_cursor_backend_tx(&mut tx, WEBHOOKS_CONSUMER, owner, &current.cursor())
                .await
                .unwrap()
        );
        tx.operation().restore_system(previous).await.unwrap();
        tx.commit().await.unwrap();
        // Already-passed callbacks have no successful advance contract absent
        // a genuine requeue marker. Rejection must preserve the first effects.
        assert!(fan_out_event_backend(&f.backend, owner, &event)
            .await
            .is_err());
        assert_eq!(marker_queue(&f, event.id).await, (1, 1));
        assert_eq!(
            fetch_cursor_backend(&f.backend, WEBHOOKS_CONSUMER)
                .await
                .unwrap(),
            Some(OutboxCursor::SqliteFamily { seq: event.seq })
        );
        sqlx::query("UPDATE memberships SET role='member' WHERE workspace_id=?1 AND user_id=?2")
            .bind(f.workspace.as_bytes().as_slice())
            .bind(f.user.as_bytes().as_slice())
            .execute(&f.pool)
            .await
            .unwrap();
        let no_hook = f.append_comment_event("comment.created").await;
        fan_out_event_backend(&f.backend, owner, &no_hook)
            .await
            .unwrap();
        assert_eq!(marker_queue(&f, no_hook.id).await, (1, 0));
        assert_eq!(
            fetch_cursor_backend(&f.backend, WEBHOOKS_CONSUMER)
                .await
                .unwrap(),
            Some(OutboxCursor::SqliteFamily { seq: no_hook.seq })
        );
        // Actual core dead-letter skip and explicit requeue are the supported
        // replay seam; neither a cursor rewind nor an invented core policy.
        sqlx::query("UPDATE memberships SET role='admin' WHERE workspace_id=?1 AND user_id=?2")
            .bind(f.workspace.as_bytes().as_slice())
            .bind(f.user.as_bytes().as_slice())
            .execute(&f.pool)
            .await
            .unwrap();
        let replay = f.append_comment_event("comment.created").await;
        for attempt in 1..=crate::db::outbox::OUTBOX_MAX_ATTEMPTS {
            assert_eq!(
                crate::db::outbox::record_failure_backend(
                    &f.backend,
                    WEBHOOKS_CONSUMER,
                    owner,
                    replay.id,
                    "synthetic whole-rollback error",
                    crate::db::outbox::OUTBOX_FAILURE_BACKOFF_MS,
                    crate::db::outbox::OUTBOX_MAX_ATTEMPTS
                )
                .await
                .unwrap(),
                attempt
            );
        }
        assert!(crate::db::outbox::advance_cursor_backend(
            &f.backend,
            WEBHOOKS_CONSUMER,
            owner,
            &replay.cursor()
        )
        .await
        .unwrap());
        assert_eq!(marker_queue(&f, replay.id).await, (0, 0));
        assert!(
            crate::db::outbox::requeue_backend(&f.backend, WEBHOOKS_CONSUMER, replay.id)
                .await
                .unwrap()
        );
        fan_out_event_backend(&f.backend, owner, &replay)
            .await
            .unwrap();
        assert_eq!(marker_queue(&f, replay.id).await, (1, 1));
        assert!(crate::db::outbox::fetch_failure_state_backend(
            &f.backend,
            WEBHOOKS_CONSUMER,
            replay.id
        )
        .await
        .unwrap()
        .is_none());
        assert_eq!(
            fetch_cursor_backend(&f.backend, WEBHOOKS_CONSUMER)
                .await
                .unwrap(),
            Some(OutboxCursor::SqliteFamily { seq: replay.seq })
        );
        f.finish().await;
    }

    #[tokio::test]
    async fn actual_backend_current_creator_workspace_secret_and_event_scope_refuse_transport() {
        let f = Fixture::new().await;
        let receiver = Receiver::new().await;
        let hook_id = hook(&f, &receiver.url).await;
        let owner = lease(&f).await;
        let changes = [
            ("member", 0),
            ("admin", 1),
            ("admin", 2),
            ("admin", 5),
            ("admin", 6),
            ("admin", 3),
            ("admin", 4),
        ];
        for (role, case) in changes {
            let event = f.append_comment_event("comment.created").await;
            fan_out_event_backend(&f.backend, owner, &event)
                .await
                .unwrap();
            let claim = claim(&f).await;
            match case {
                0 => {
                    sqlx::query(
                        "UPDATE memberships SET role=?3 WHERE workspace_id=?1 AND user_id=?2",
                    )
                    .bind(f.workspace.as_bytes().as_slice())
                    .bind(f.user.as_bytes().as_slice())
                    .bind(role)
                    .execute(&f.pool)
                    .await
                    .unwrap();
                }
                1 => {
                    sqlx::query("UPDATE users SET suspended_at=1 WHERE id=?1")
                        .bind(f.user.as_bytes().as_slice())
                        .execute(&f.pool)
                        .await
                        .unwrap();
                }
                2 => {
                    sqlx::query("UPDATE workspaces SET deleted_at=1 WHERE id=?1")
                        .bind(f.workspace.as_bytes().as_slice())
                        .execute(&f.pool)
                        .await
                        .unwrap();
                }
                5 => {
                    sqlx::query("UPDATE users SET deleted_at=1 WHERE id=?1")
                        .bind(f.user.as_bytes().as_slice())
                        .execute(&f.pool)
                        .await
                        .unwrap();
                }
                6 => {
                    sqlx::query("DELETE FROM memberships WHERE workspace_id=?1 AND user_id=?2")
                        .bind(f.workspace.as_bytes().as_slice())
                        .bind(f.user.as_bytes().as_slice())
                        .execute(&f.pool)
                        .await
                        .unwrap();
                }
                3 => {}
                4 => {
                    sqlx::query("UPDATE webhooks SET secret='enc:v2:wrong' WHERE id=?1")
                        .bind(hook_id.as_bytes().as_slice())
                        .execute(&f.pool)
                        .await
                        .unwrap();
                }
                _ => unreachable!(),
            }
            let key = if case == 3 { None } else { Some(keys()) };
            let expected_reason = match case {
                0 | 1 | 5 | 6 => "creator_not_manager",
                2 => "webhook_missing",
                3 => "encryption_keys_unset",
                4 => "secret_unavailable",
                _ => unreachable!(),
            };
            assert!(
                matches!(prepare(&f.backend,key.as_deref(),&claim).await.unwrap(),Prepared::Dead(reason) if reason==expected_reason),
                "case {case} must reject at its current authority/secret boundary"
            );
            deliver_one(
                f.backend.clone(),
                receiver.outbound(),
                key,
                claim.clone(),
                Duration::from_secs(1),
            )
            .await
            .unwrap();
            assert_eq!(row(&f, claim.due.id).await.1, "failed");
            assert_eq!(receiver.count(), 0);
            sqlx::query("INSERT INTO memberships(workspace_id,user_id,role) VALUES(?1,?2,'admin') ON CONFLICT(workspace_id,user_id) DO UPDATE SET role='admin'")
                .bind(f.workspace.as_bytes().as_slice())
                .bind(f.user.as_bytes().as_slice())
                .execute(&f.pool)
                .await
                .unwrap();
            sqlx::query("UPDATE users SET suspended_at=NULL,deleted_at=NULL WHERE id=?1")
                .bind(f.user.as_bytes().as_slice())
                .execute(&f.pool)
                .await
                .unwrap();
            sqlx::query("UPDATE workspaces SET deleted_at=NULL WHERE id=?1")
                .bind(f.workspace.as_bytes().as_slice())
                .execute(&f.pool)
                .await
                .unwrap();
        }
        // Same receiver and normal positive signed transport after all refusals.
        let sealed = crate::secret_box::seal(
            &keys(),
            SECRET,
            &webhook_secret_context(f.workspace, hook_id),
        )
        .unwrap();
        sqlx::query("UPDATE webhooks SET secret=?2 WHERE id=?1")
            .bind(hook_id.as_bytes().as_slice())
            .bind(sealed)
            .execute(&f.pool)
            .await
            .unwrap();
        let event = f.append_comment_event("comment.created").await;
        fan_out_event_backend(&f.backend, owner, &event)
            .await
            .unwrap();
        deliver_one(
            f.backend.clone(),
            receiver.outbound(),
            Some(keys()),
            claim(&f).await,
            Duration::from_secs(1),
        )
        .await
        .unwrap();
        assert_eq!(receiver.count(), 1);
        let removed_event = f.append_comment_event("comment.created").await;
        fan_out_event_backend(&f.backend, owner, &removed_event)
            .await
            .unwrap();
        let removed_claim = claim(&f).await;
        sqlx::query("DELETE FROM webhooks WHERE id=?1")
            .bind(hook_id.as_bytes().as_slice())
            .execute(&f.pool)
            .await
            .unwrap();
        assert!(matches!(
            prepare(&f.backend, Some(&keys()), &removed_claim)
                .await
                .unwrap(),
            Prepared::Dead("claim_replaced")
        ));
        deliver_one(
            f.backend.clone(),
            receiver.outbound(),
            Some(keys()),
            removed_claim,
            Duration::from_secs(1),
        )
        .await
        .unwrap();
        assert_eq!(receiver.count(), 1);
        receiver.finish().await;
        f.finish().await;
    }

    #[tokio::test]
    async fn actual_backend_private_project_historical_payload_and_send_time_revocation() {
        let f = Fixture::new().await;
        let receiver = Receiver::new().await;
        let _hook = hook(&f, &receiver.url).await;
        let owner = lease(&f).await;
        let project = Uuid::now_v7();
        sqlx::query("INSERT INTO projects(id,workspace_id,key,name,visibility,created_by) VALUES(?1,?2,'PRIVATE','private','private',?3)").bind(project.as_bytes().as_slice()).bind(f.workspace.as_bytes().as_slice()).bind(f.actor.as_bytes().as_slice()).execute(&f.pool).await.unwrap();
        let payload = json!({"projectId":project,"title":"historical private 한글🙂","number":7,"null":null,"version":"9007199254740993"});
        let event = append(
            &f,
            "task.deleted",
            Some("task"),
            Some(Uuid::now_v7()),
            payload.clone(),
        )
        .await;
        fan_out_event_backend(&f.backend, owner, &event)
            .await
            .unwrap();
        assert_eq!(marker_queue(&f, event.id).await, (1, 0));
        sqlx::query("INSERT INTO project_members(id,workspace_id,project_id,user_id,role) VALUES(?1,?2,?3,?4,'viewer')").bind(Uuid::now_v7().as_bytes().as_slice()).bind(f.workspace.as_bytes().as_slice()).bind(project.as_bytes().as_slice()).bind(f.user.as_bytes().as_slice()).execute(&f.pool).await.unwrap();
        let pending = append(
            &f,
            "task.deleted",
            Some("task"),
            Some(Uuid::now_v7()),
            payload.clone(),
        )
        .await;
        fan_out_event_backend(&f.backend, owner, &pending)
            .await
            .unwrap();
        let c = claim(&f).await;
        sqlx::query(
            "DELETE FROM project_members WHERE workspace_id=?1 AND project_id=?2 AND user_id=?3",
        )
        .bind(f.workspace.as_bytes().as_slice())
        .bind(project.as_bytes().as_slice())
        .bind(f.user.as_bytes().as_slice())
        .execute(&f.pool)
        .await
        .unwrap();
        deliver_one(
            f.backend.clone(),
            receiver.outbound(),
            Some(keys()),
            c.clone(),
            Duration::from_secs(1),
        )
        .await
        .unwrap();
        assert_eq!(receiver.count(), 0);
        assert_eq!(row(&f, c.due.id).await.1, "failed");
        sqlx::query("INSERT INTO project_members(id,workspace_id,project_id,user_id,role) VALUES(?1,?2,?3,?4,'viewer')").bind(Uuid::now_v7().as_bytes().as_slice()).bind(f.workspace.as_bytes().as_slice()).bind(project.as_bytes().as_slice()).bind(f.user.as_bytes().as_slice()).execute(&f.pool).await.unwrap();
        let visible = append(
            &f,
            "task.deleted",
            Some("task"),
            Some(Uuid::now_v7()),
            payload,
        )
        .await;
        // This fixture pins persisted UTC microseconds so received timestamp
        // assertions do not obtain their expected value from the serializer.
        sqlx::query("UPDATE events SET created_at=?1 WHERE id=?2")
            .bind(1_700_000_000_123_456_i64)
            .bind(visible.id.as_bytes().as_slice())
            .execute(&f.pool)
            .await
            .unwrap();
        let visible = crate::db::outbox::fetch_event_by_id_backend(&f.backend, visible.id)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(visible.created_at.timestamp_micros(), 1_700_000_000_123_456);
        fan_out_event_backend(&f.backend, owner, &visible)
            .await
            .unwrap();
        deliver_one(
            f.backend.clone(),
            receiver.outbound(),
            Some(keys()),
            claim(&f).await,
            Duration::from_secs(1),
        )
        .await
        .unwrap();
        assert_eq!(receiver.count(), 1);
        let captured = receiver.capture.rows.lock().unwrap()[0].clone();
        let body: Value = serde_json::from_slice(&captured.1).unwrap();
        assert_eq!(
            body,
            json!({
                "id": visible.id.to_string(),
                "verb": "task.deleted",
                "workspaceId": f.workspace.to_string(),
                "actorUserId": f.actor.to_string(),
                "targetType": "task",
                "targetId": visible.target_id.unwrap().to_string(),
                "payload": {
                    "projectId": project.to_string(),
                    "title": "historical private 한글🙂",
                    "number": 7,
                    "null": null,
                    "version": "9007199254740993"
                },
                "channel": "web",
                "createdAt": "2023-11-14T22:13:20.123Z"
            })
        );
        assert!(body["payload"]["title"].is_string());
        assert!(body["payload"]["number"].is_number());
        assert!(body["payload"]["null"].is_null());
        assert!(body["payload"]["version"].is_string());
        assert_eq!(
            captured.0["x-fvoci-signature"],
            sign_body(SECRET, &captured.1)
        );
        assert_eq!(
            receiver.capture.rows.lock().unwrap()[0].1,
            serialize_payload_backend(&visible)
        );
        receiver.finish().await;
        f.finish().await;
    }

    #[tokio::test]
    async fn actual_backend_unknown_response_retries_after_restart_and_preserves_fence() {
        let f = Fixture::new().await;
        let receiver = Receiver::new().await;
        let _hook = hook(&f, &receiver.url).await;
        let owner = lease(&f).await;
        let event = f.append_comment_event("comment.created").await;
        fan_out_event_backend(&f.backend, owner, &event)
            .await
            .unwrap();
        receiver.capture.unknown.store(true, Ordering::SeqCst);
        let first = claim(&f).await;
        deliver_one(
            f.backend.clone(),
            receiver.outbound(),
            Some(keys()),
            first.clone(),
            Duration::from_millis(100),
        )
        .await
        .unwrap();
        let state = row(&f, first.due.id).await;
        assert_eq!((state.0, state.1, state.2), (1, "pending".into(), None));
        assert_eq!(receiver.count(), 1);
        let now: i64 = sqlx::query_scalar(
            "SELECT (unixepoch()*1000000+CAST(substr(strftime('%f','now'),4,3) AS INTEGER)*1000)",
        )
        .fetch_one(&f.pool)
        .await
        .unwrap();
        assert!(state.3.unwrap() > now + 59_000_000 && state.3.unwrap() <= now + 60_000_000);
        make_due(&f, first.due.id).await;
        receiver.capture.unknown.store(false, Ordering::SeqCst);
        let restarted = claim(&f).await;
        assert_eq!(restarted.due.attempt, 1);
        deliver_one(
            f.backend.clone(),
            receiver.outbound(),
            Some(keys()),
            restarted.clone(),
            Duration::from_secs(1),
        )
        .await
        .unwrap();
        assert_eq!(
            row(&f, restarted.due.id).await,
            (2, "delivered".into(), Some(204), None)
        );
        assert_eq!(receiver.count(), 2);
        assert!(
            !record_webhook_backend(&f.backend, &first, DeliveryOutcome::Failed, None)
                .await
                .unwrap()
        );
        for (_, body) in receiver.capture.rows.lock().unwrap().iter() {
            assert_eq!(body, &serialize_payload_backend(&event));
        }
        receiver.finish().await;
        f.finish().await;
    }

    #[tokio::test]
    async fn actual_backend_sender_cancel_joins_requests_and_leaves_reclaimable_row() {
        let f = Fixture::new().await;
        let receiver = Receiver::new().await;
        let _hook = hook(&f, &receiver.url).await;
        let owner = lease(&f).await;
        let event = f.append_comment_event("comment.created").await;
        fan_out_event_backend(&f.backend, owner, &event)
            .await
            .unwrap();
        receiver.capture.unknown.store(true, Ordering::SeqCst);
        let sender = spawn_webhook_sender_backend(
            f.backend.clone(),
            receiver.outbound(),
            Some(keys()),
            WebhookDeliverySettings {
                poll_interval: Duration::from_millis(10),
                ..Default::default()
            },
        );
        tokio::time::timeout(Duration::from_secs(2), receiver.capture.received.notified())
            .await
            .unwrap();
        sender.request_shutdown();
        sender.join().await.unwrap();
        let id: Vec<u8> = sqlx::query_scalar("SELECT id FROM webhook_deliveries WHERE event_id=?1")
            .bind(event.id.as_bytes().as_slice())
            .fetch_one(&f.pool)
            .await
            .unwrap();
        let id = Uuid::from_slice(&id).unwrap();
        let state = row(&f, id).await;
        assert_eq!((state.0, state.1, state.2), (0, "pending".into(), None));
        assert!(state.3.is_some());
        assert!(
            claim_due_webhooks_backend(&f.backend, 1, Duration::from_secs(240))
                .await
                .unwrap()
                .is_empty()
        );
        make_due(&f, id).await;
        receiver.capture.unknown.store(false, Ordering::SeqCst);
        deliver_one(
            f.backend.clone(),
            receiver.outbound(),
            Some(keys()),
            claim(&f).await,
            Duration::from_secs(1),
        )
        .await
        .unwrap();
        assert_eq!(row(&f, id).await, (1, "delivered".into(), Some(204), None));
        assert_eq!(receiver.count(), 2);
        receiver.finish().await;
        f.finish().await;
    }
}
