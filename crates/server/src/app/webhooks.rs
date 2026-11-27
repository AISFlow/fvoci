//! The one door to the server operator's webhooks (`block_change_webhooks`): what is
//! posted there leaves the server, to an address its operator chose. A block of an
//! organization people work in is posted when it changes; nothing of an account's own
//! space is, which is its owner's alone.
//!
//! Outbound notifications flow through a bounded [`mpsc`] channel drained by at most one
//! background worker task, preventing slow webhook endpoints from spawning
//! unbounded tasks or exhausting memory during bulk mutations and archive imports.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use fvoci_model::{Block, OrgKind};
use fvoci_store::Store;
use tokio::sync::{OnceCell, mpsc};
use tokio_util::task::TaskTracker;

use super::App;
use crate::webhook::Webhook;

/// Maximum queued block-change notifications held in memory before newer items are dropped.
const WEBHOOK_QUEUE_CAPACITY: usize = 1_024;

#[derive(Debug)]
struct QueuedWebhook {
    store: Store,
    org_kind: Arc<OnceCell<OrgKind>>,
    block: Block,
}

/// Bounded FIFO queue and single-worker dispatcher for outbound block-change webhooks.
#[derive(Debug)]
pub(super) struct WebhookQueue {
    tx: mpsc::Sender<QueuedWebhook>,
    rx: Arc<tokio::sync::Mutex<mpsc::Receiver<QueuedWebhook>>>,
    active: Arc<AtomicBool>,
}

impl Default for WebhookQueue {
    fn default() -> Self {
        let (tx, rx) = mpsc::channel(WEBHOOK_QUEUE_CAPACITY);
        Self {
            tx,
            rx: Arc::new(tokio::sync::Mutex::new(rx)),
            active: Arc::new(AtomicBool::new(false)),
        }
    }
}

impl WebhookQueue {
    fn enqueue(&self, item: QueuedWebhook, webhook: &Webhook, background: &TaskTracker) {
        if self.tx.try_send(item).is_err() {
            tracing::warn!("webhook queue is full; dropping block-change notification");
            return;
        }
        self.ensure_worker(webhook, background);
    }

    fn ensure_worker(&self, webhook: &Webhook, background: &TaskTracker) {
        if self
            .active
            .compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst)
            .is_err()
        {
            return;
        }
        let rx = Arc::clone(&self.rx);
        let active = Arc::clone(&self.active);
        let webhook = webhook.clone();
        background.spawn(async move {
            loop {
                let next = {
                    let mut guard = rx.lock().await;
                    guard.try_recv().ok()
                };
                if let Some(item) = next {
                    deliver(&webhook, item).await;
                    continue;
                }
                active.store(false, Ordering::SeqCst);
                let has_more = {
                    let guard = rx.lock().await;
                    !guard.is_empty()
                };
                if !has_more
                    || active
                        .compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst)
                        .is_err()
                {
                    break;
                }
            }
        });
    }
}

async fn deliver(webhook: &Webhook, item: QueuedWebhook) {
    let QueuedWebhook {
        store,
        org_kind,
        mut block,
    } = item;
    let kind = org_kind
        .get_or_try_init(|| async {
            let own = store.is_own_space().await?;
            Ok::<_, fvoci_store::StoreError>(if own {
                OrgKind::Personal
            } else {
                OrgKind::Shared
            })
        })
        .await;
    match kind {
        Ok(OrgKind::Shared) => {
            if !block.takes_number() && block.number.is_some() {
                block.hide_inactive_number();
            }
            webhook.notify_update(&block).await;
        }
        Ok(OrgKind::Personal) => {}
        Err(error) => {
            tracing::warn!(
                block_id = %block.id, %error,
                "a change was not posted to the webhooks: whose it is could not be read"
            );
        }
    }
}

impl App {
    /// Queues the block, which is of the organization this handle works in, for the
    /// operator's webhooks on the bounded single-worker channel; nothing is posted if the
    /// organization is an account's own space, or if the server cannot tell whose the
    /// block is.
    pub(crate) fn tell_webhooks(&self, block: &Block) {
        self.enqueue_webhook(block);
    }

    /// Enqueues `block` onto the bounded webhook channel without spawning a per-block task.
    pub(crate) fn enqueue_webhook(&self, block: &Block) {
        if self.config().block_change_webhooks.is_empty() {
            return;
        }
        // Fast-path skip when this handle is already known to be a personal space.
        if self.org_kind.get() == Some(&OrgKind::Personal) {
            return;
        }
        self.inner.webhook_queue.enqueue(
            QueuedWebhook {
                store: self.store.clone(),
                org_kind: Arc::clone(&self.org_kind),
                block: block.clone(),
            },
            &self.inner.webhook,
            &self.inner.background,
        );
    }
}
