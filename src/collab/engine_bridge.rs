use std::path::{Path, PathBuf};
use std::sync::mpsc;
use std::thread::{self, JoinHandle};

use collab_engine::limits::Limits;
use collab_engine::outcome::{EngineReport, EngineStatus};
use collab_engine::process::{EngineSession, SpawnRequest};
use collab_engine::protocol::Request;
use tokio::sync::oneshot;

/// Parent-side bridge to one owned native child on a dedicated std thread.
/// Async callers await oneshot responses; native work never blocks the Tokio reactor.
pub struct EngineBridge {
    tx: mpsc::Sender<BridgeJob>,
    engine_bin: PathBuf,
    limits: Limits,
    join: JoinHandle<()>,
}

enum BridgeJob {
    Call {
        request: Request,
        reply: oneshot::Sender<EngineReport>,
    },
    Kill {
        reply: oneshot::Sender<()>,
    },
}

impl EngineBridge {
    pub fn spawn(engine_bin: PathBuf, limits: Limits) -> Result<Self, EngineReport> {
        let (tx, rx) = mpsc::channel();
        let bin = engine_bin.clone();
        let join = thread::Builder::new()
            .name("fvoci-collab-engine".into())
            .spawn(move || worker_loop(rx, engine_bin, limits))
            .map_err(|e| EngineReport::new(EngineStatus::WorkerFailure {
                reason: collab_engine::outcome::WorkerFailureReason::Spawn,
                detail: e.to_string(),
            }))?;
        Ok(Self {
            tx,
            engine_bin: bin,
            limits,
            join,
        })
    }

    pub async fn call(&self, request: Request) -> Result<EngineReport, BridgeError> {
        let (reply_tx, reply_rx) = oneshot::channel();
        self.tx
            .send(BridgeJob::Call {
                request,
                reply: reply_tx,
            })
            .map_err(|_| BridgeError::Dead)?;
        reply_rx.await.map_err(|_| BridgeError::Dead)
    }

    pub async fn kill_and_reap(&self) -> Result<(), BridgeError> {
        let (reply_tx, reply_rx) = oneshot::channel();
        self.tx
            .send(BridgeJob::Kill { reply: reply_tx })
            .map_err(|_| BridgeError::Dead)?;
        reply_rx.await.map_err(|_| BridgeError::Dead)
    }

    pub fn join(self) {
        let _ = self.join.join();
    }

    pub fn engine_bin(&self) -> &Path {
        &self.engine_bin
    }

    pub fn limits(&self) -> Limits {
        self.limits
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BridgeError {
    Dead,
}

fn worker_loop(rx: mpsc::Receiver<BridgeJob>, engine_bin: PathBuf, limits: Limits) {
    let mut session = match spawn_session(&engine_bin, limits) {
        Ok(session) => session,
        Err(_) => return,
    };
    while let Ok(job) = rx.recv() {
        match job {
            BridgeJob::Call { request, reply } => {
                let report = session.call(&request);
                let _ = reply.send(report);
            }
            BridgeJob::Kill { reply } => {
                session.kill_and_reap();
                session = match spawn_session(&engine_bin, limits) {
                    Ok(s) => s,
                    Err(_) => break,
                };
                let _ = reply.send(());
            }
        }
    }
    session.kill_and_reap();
}

fn spawn_session(engine_bin: &Path, limits: Limits) -> Result<EngineSession, EngineReport> {
    EngineSession::spawn(SpawnRequest {
        engine_bin: engine_bin.to_path_buf(),
        limits,
        test_hang_ms: None,
        test_exit_after_read: None,
        test_close_stdout_hang_ms: None,
        test_exit_after_write: None,
    })
}

/// Bounded N2 recovery check: load proposed durable bytes in an isolated fresh child.
pub fn validate_recoverable_blocking(
    engine_bin: &Path,
    limits: Limits,
    snapshot: &[u8],
    tail: &[Vec<u8>],
) -> bool {
    let mut session = match spawn_session(engine_bin, limits) {
        Ok(s) => s,
        Err(_) => return false,
    };
    let report = session.call(&Request::Load {
        snapshot_b64: Some(snapshot.to_vec()),
        tail_b64: tail.to_vec(),
        encoding: 1,
    });
    let ok = matches!(report.outcome, EngineStatus::Ok { applied: true, .. });
    session.kill_and_reap();
    ok
}

pub async fn validate_recoverable(
    engine_bin: PathBuf,
    limits: Limits,
    snapshot: Vec<u8>,
    tail: Vec<Vec<u8>>,
) -> bool {
    tokio::task::spawn_blocking(move || validate_recoverable_blocking(&engine_bin, limits, &snapshot, &tail))
        .await
        .unwrap_or(false)
}

pub async fn fresh_validate_snapshot(
    engine_bin: PathBuf,
    limits: Limits,
    snapshot: Vec<u8>,
) -> bool {
    validate_recoverable(engine_bin, limits, snapshot, Vec::new()).await
}
