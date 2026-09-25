use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{mpsc, Arc};
use std::thread::{self, JoinHandle};

use collab_engine::limits::Limits;
use collab_engine::outcome::{EngineReport, EngineStatus};
use collab_engine::process::{EngineSession, SpawnRequest};
use collab_engine::protocol::Request;
use tokio::sync::oneshot;

/// Parent-side bridge to one owned native child on a dedicated std thread.
/// Async callers await oneshot responses; native work never blocks the Tokio reactor.
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
    /// Kill the current child and spawn a fresh one (room reload after rejection).
    Recycle { reply: oneshot::Sender<()> },
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
                    reason: collab_engine::outcome::WorkerFailureReason::Spawn,
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

    pub async fn recycle(&self) -> Result<(), BridgeError> {
        let tx = self.tx.as_ref().ok_or(BridgeError::Dead)?;
        let (reply_tx, reply_rx) = oneshot::channel();
        tx.send(BridgeJob::Recycle { reply: reply_tx })
            .map_err(|_| BridgeError::Dead)?;
        reply_rx.await.map_err(|_| BridgeError::Dead)
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

fn worker_loop(
    rx: mpsc::Receiver<BridgeJob>,
    engine_bin: PathBuf,
    limits: Limits,
    ops_used: Arc<AtomicU32>,
) {
    let mut session = match spawn_session(&engine_bin, limits) {
        Ok(session) => session,
        Err(_) => return,
    };
    ops_used.store(0, Ordering::Relaxed);
    while let Ok(job) = rx.recv() {
        match job {
            BridgeJob::Call { request, reply } => {
                let report = session.call(&request);
                ops_used.fetch_add(1, Ordering::Relaxed);
                let _ = reply.send(report);
            }
            BridgeJob::Recycle { reply } => {
                session.kill_and_reap();
                session = match spawn_session(&engine_bin, limits) {
                    Ok(s) => s,
                    Err(_) => break,
                };
                ops_used.store(0, Ordering::Relaxed);
                let _ = reply.send(());
            }
            BridgeJob::Stop { reply } => {
                session.kill_and_reap();
                let _ = reply.send(());
                break;
            }
        }
    }
    session.kill_and_reap();
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
