use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{mpsc, Arc};
use std::thread::{self, JoinHandle};

use collab_engine::limits::Limits;
use collab_engine::outcome::{EngineReport, EngineStatus, WorkerFailureReason};
use collab_engine::process::{EngineSession, SpawnRequest};
use collab_engine::protocol::Request;
use tokio::sync::oneshot;

/// Parent-side bridge to one owned native child on a dedicated std thread.
/// Async callers await oneshot responses; native work never blocks the Tokio reactor.
///
/// The thread starts with no child. Only [`EngineBridge::recycle`] spawns one,
/// so a room's child is always fresh and loaded by the caller right after; a
/// call with no child fails instead of spawning an unloaded one. A failed
/// spawn is reported and the thread keeps serving: the next recycle retries.
pub struct EngineBridge {
    tx: Option<mpsc::Sender<BridgeJob>>,
    engine_bin: PathBuf,
    limits: Limits,
    ops_used: Arc<AtomicU32>,
    join: JoinHandle<()>,
}

enum BridgeJob {
    Call {
        request: Request,
        reply: oneshot::Sender<EngineReport>,
    },
    /// Kill the current child, if any, and spawn a fresh one (room load or
    /// reload after rejection). Replies with the spawn failure, if any.
    Recycle {
        reply: oneshot::Sender<Result<(), EngineReport>>,
    },
    /// Kill the current child and terminate the worker thread (room shutdown).
    Stop { reply: oneshot::Sender<()> },
}

impl EngineBridge {
    #[allow(clippy::result_large_err)]
    pub fn spawn(engine_bin: PathBuf, limits: Limits) -> Result<Self, EngineReport> {
        let (tx, rx) = mpsc::channel();
        let bin = engine_bin.clone();
        let ops_used = Arc::new(AtomicU32::new(0));
        let ops_tracker = ops_used.clone();
        let join = thread::Builder::new()
            .name("fvoci-collab-engine".into())
            .spawn(move || worker_loop(rx, engine_bin, limits, ops_tracker))
            .map_err(|e| {
                EngineReport::new(EngineStatus::WorkerFailure {
                    reason: WorkerFailureReason::Spawn,
                    detail: e.to_string(),
                })
            })?;
        Ok(Self {
            tx: Some(tx),
            engine_bin: bin,
            limits,
            ops_used,
            join,
        })
    }

    pub async fn call(&self, request: Request) -> Result<EngineReport, BridgeError> {
        let tx = self.tx.as_ref().ok_or(BridgeError::Dead)?;
        let (reply_tx, reply_rx) = oneshot::channel();
        tx.send(BridgeJob::Call {
            request,
            reply: reply_tx,
        })
        .map_err(|_| BridgeError::Dead)?;
        reply_rx.await.map_err(|_| BridgeError::Dead)
    }

    pub async fn recycle(&self) -> Result<(), RecycleError> {
        let tx = self.tx.as_ref().ok_or(RecycleError::Dead)?;
        let (reply_tx, reply_rx) = oneshot::channel();
        tx.send(BridgeJob::Recycle { reply: reply_tx })
            .map_err(|_| RecycleError::Dead)?;
        reply_rx
            .await
            .map_err(|_| RecycleError::Dead)?
            .map_err(|report| RecycleError::Spawn(Box::new(report)))
    }

    pub async fn stop(mut self) -> Result<(), BridgeError> {
        let mut stop_err = None;
        if let Some(tx) = self.tx.take() {
            let (reply_tx, reply_rx) = oneshot::channel();
            let stop_ok =
                tx.send(BridgeJob::Stop { reply: reply_tx }).is_ok() && reply_rx.await.is_ok();
            if !stop_ok {
                stop_err = Some(BridgeError::Dead);
            }
        }
        let join = self.join;
        tokio::task::spawn_blocking(move || join.join())
            .await
            .map_err(|_| BridgeError::Dead)?
            .map_err(|_| BridgeError::Dead)?;
        if let Some(err) = stop_err {
            return Err(err);
        }
        Ok(())
    }

    pub fn engine_bin(&self) -> &Path {
        &self.engine_bin
    }

    pub fn limits(&self) -> Limits {
        self.limits
    }

    /// Child op count since the last recycle (each engine request counts as one).
    pub fn ops_used(&self) -> u32 {
        self.ops_used.load(Ordering::Relaxed)
    }

    /// True when the next op would hit the child's hard cap.
    pub fn needs_recycle(&self) -> bool {
        let max = self.limits.max_ops;
        max > 0 && self.ops_used() >= max - 1
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BridgeError {
    Dead,
}

/// Why [`EngineBridge::recycle`] left the bridge without a child.
#[derive(Debug, Clone)]
pub enum RecycleError {
    /// The worker thread is gone (it panicked or the bridge was stopped).
    Dead,
    /// The helper did not spawn (slot cap, fork/exec, missing binary). The
    /// bridge has no child until a later recycle succeeds.
    Spawn(Box<EngineReport>),
}

fn worker_loop(
    rx: mpsc::Receiver<BridgeJob>,
    engine_bin: PathBuf,
    limits: Limits,
    ops_used: Arc<AtomicU32>,
) {
    // Spawned and reaped only on this thread: the helper's PDEATHSIG is tied
    // to it, and it outlives every session it holds.
    let mut session: Option<EngineSession> = None;
    while let Ok(job) = rx.recv() {
        match job {
            BridgeJob::Call { request, reply } => {
                let report = match session.as_mut() {
                    Some(session) => {
                        let report = session.call(&request);
                        ops_used.fetch_add(1, Ordering::Relaxed);
                        report
                    }
                    None => EngineReport::new(EngineStatus::WorkerFailure {
                        reason: WorkerFailureReason::SessionDead,
                        detail: "no helper; recycle required".into(),
                    }),
                };
                warn_once_if_oom_backstop_missing();
                let _ = reply.send(report);
            }
            BridgeJob::Recycle { reply } => {
                if let Some(mut old) = session.take() {
                    old.kill_and_reap();
                }
                // The caller logs a spawn failure, with the room's ids.
                let spawned = spawn_session(&engine_bin, limits).map(|fresh| {
                    session = Some(fresh);
                    ops_used.store(0, Ordering::Relaxed);
                });
                let _ = reply.send(spawned);
            }
            BridgeJob::Stop { reply } => {
                if let Some(mut old) = session.take() {
                    old.kill_and_reap();
                }
                let _ = reply.send(());
                break;
            }
        }
    }
    if let Some(mut old) = session.take() {
        old.kill_and_reap();
    }
}

/// The helper reports a denied `oom_score_adj` only after its first reply, so
/// check after calls; warn once per process.
fn warn_once_if_oom_backstop_missing() {
    static WARNED: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
    if collab_engine::process::oom_backstop_missing()
        && !WARNED.swap(true, std::sync::atomic::Ordering::Relaxed)
    {
        tracing::warn!(
            "collab helper OOM backstop unavailable: oom_score_adj was not applied (container profile); container mem_limit and per-helper limits remain the bound"
        );
    }
}

#[allow(clippy::result_large_err)]
fn spawn_session(engine_bin: &Path, limits: Limits) -> Result<EngineSession, EngineReport> {
    EngineSession::spawn(SpawnRequest {
        engine_bin: engine_bin.to_path_buf(),
        limits,
        slot_kind: collab_engine::process::ChildSlotKind::Primary,
        slot_wait: None,
        test_hang_ms: None,
        test_exit_after_read: None,
        test_close_stdout_hang_ms: None,
        test_exit_after_write: None,
    })
}

/// Log why the engine did not apply a request. Only the failure variant and its
/// detail are logged; `Ok` payloads can carry document content.
pub(crate) fn warn_engine_not_applied(
    site: &'static str,
    workspace_id: Option<uuid::Uuid>,
    document_id: Option<uuid::Uuid>,
    outcome: &collab_engine::EngineStatus,
) {
    use collab_engine::EngineStatus;
    let (status, detail) = match outcome {
        EngineStatus::Ok { .. } => ("ok_not_applied", ""),
        EngineStatus::Malformed { detail } => ("malformed", detail.as_str()),
        EngineStatus::Unsupported { detail, .. } => ("unsupported", detail.as_str()),
        EngineStatus::ResourceLimit { detail, .. } => ("resource_limit", detail.as_str()),
        EngineStatus::WorkerFailure { detail, .. } => ("worker_failure", detail.as_str()),
    };
    tracing::warn!(
        site,
        workspace_id = ?workspace_id,
        document_id = ?document_id,
        status,
        detail,
        "collab engine request not applied"
    );
}
