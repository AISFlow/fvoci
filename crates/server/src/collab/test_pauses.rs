//! Test-only pause and crash-simulation hooks for [`Collab`].

use std::sync::Arc;

use super::Collab;

impl Collab {
    /// Has the next write of a document's body to its card wait, after it read the card
    /// and before it writes it, until the second is notified; the first is notified when
    /// it waits.
    #[doc(hidden)]
    #[must_use]
    pub fn pause_next_projection(&self) -> (Arc<tokio::sync::Notify>, Arc<tokio::sync::Notify>) {
        let pause = (
            Arc::new(tokio::sync::Notify::new()),
            Arc::new(tokio::sync::Notify::new()),
        );
        *self
            .inner
            .projection_pause
            .lock()
            .expect("the pause is not poisoned") = Some(pause.clone());
        pause
    }

    /// Waits where [`Collab::pause_next_projection`] asked.
    pub(super) async fn paused_before_projection(&self) {
        let pause = self
            .inner
            .projection_pause
            .lock()
            .expect("the pause is not poisoned")
            .take();
        if let Some((waiting, go)) = pause {
            waiting.notify_one();
            go.notified().await;
        }
    }

    /// Has the next update wait, after the worker took it and before it is logged,
    /// until the second is notified; the first is notified when it waits.
    #[doc(hidden)]
    #[must_use]
    pub fn pause_next_log(&self) -> (Arc<tokio::sync::Notify>, Arc<tokio::sync::Notify>) {
        let pause = (
            Arc::new(tokio::sync::Notify::new()),
            Arc::new(tokio::sync::Notify::new()),
        );
        *self
            .inner
            .log_pause
            .lock()
            .expect("the pause is not poisoned") = Some(pause.clone());
        pause
    }

    /// Waits where [`Collab::pause_next_log`] asked.
    pub(super) async fn paused_before_log(&self) {
        let pause = self
            .inner
            .log_pause
            .lock()
            .expect("the pause is not poisoned")
            .take();
        if let Some((waiting, go)) = pause {
            waiting.notify_one();
            go.notified().await;
        }
    }

    /// Forgets every open document without writing its body, and ends the workers.
    #[doc(hidden)]
    pub async fn abandon(&self) {
        for slot in self.all_slots() {
            let (tenant, container_id) = {
                let guard = slot.lock().await;
                (guard.tenant, guard.container_id.clone())
            };
            let _gate = self.reading(tenant, &container_id).await;
            let mut slot = slot.lock_owned().await;
            self.lost(&mut slot);
            self.retire(&mut slot);
        }
        for worker in &self.inner.workers {
            worker.end().await;
        }
    }
}
