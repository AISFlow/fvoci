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
