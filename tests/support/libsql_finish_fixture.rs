//! Actual libsql 0.9.30 -> forwarding proxy -> official sqld fixture.
//!
//! The proxy does not execute, rewrite, or emulate SQL. Its only fault is
//! withholding/dropping the downstream response after recording the complete
//! official upstream response. COMMIT rejection must come from real database
//! constraints prepared by the caller. This local protocol fixture is neither
//! the pinned SQLite 3.53.4 application nor proof of a Turso primary.

use std::collections::HashSet;
use std::error::Error;
use std::net::{Ipv4Addr, SocketAddr};
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use axum::body::{Body, Bytes};
use axum::extract::{Request, State};
use axum::http::{header, Response, StatusCode};
use axum::Router;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};
use tokio::process::{Child, Command};
use tokio::sync::Notify;
use tokio::task::JoinHandle;
use tokio_util::sync::CancellationToken;

type FixtureResult<T> = Result<T, Box<dyn Error + Send + Sync>>;

/// Official libsql-server-v0.24.32 x86_64 GNU binary, pinned by the root's
/// upstream-download-proof.json. This upstream reports SQLite 3.45.1.
pub const SQLD_SHA256: &str = "0863c3fbe68ac9714bca2cec1330def7a0ba5e4a29f199bf60ef46fa0c95b895";
const FIXTURE_TOKEN: &str = "isolated-protocol-fixture-only";
const START_DEADLINE: Duration = Duration::from_secs(10);
const FINISH_DEADLINE: Duration = Duration::from_secs(10);
const MAX_REQUEST_BYTES: usize = 64 * 1024 * 1024;

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Exchange {
    pub ordinal: usize,
    pub path: String,
    /// Synthetic fixture traffic only; headers and credentials are excluded.
    pub request: Value,
    pub has_commit: bool,
    pub has_close: bool,
    pub upstream_status: Option<u16>,
    /// Full bytes are retained even for a cursor response, without parsing it.
    pub upstream_body: Vec<u8>,
    pub upstream_error: Option<String>,
    pub reply_lost: bool,
}

/// Pause only after the official upstream response has been fully received.
/// The test can inspect actual effects/ACKs while the original SDK awaits its
/// response, then release the gate to deliver a real transport error.
pub struct CommitReplyGate(Arc<ReplyGate>);
struct ReplyGate {
    received: AtomicBool,
    released: AtomicBool,
    response: Notify,
    release: Notify,
    upstream_error: Mutex<Option<String>>,
}
impl CommitReplyGate {
    pub async fn wait_upstream_response(&self) -> Result<(), String> {
        loop {
            let notified = self.0.response.notified();
            tokio::pin!(notified);
            notified.as_mut().enable();
            if self.0.received.load(Ordering::SeqCst) {
                return match self
                    .0
                    .upstream_error
                    .lock()
                    .expect("fixture response lock")
                    .as_ref()
                {
                    Some(error) => Err(error.clone()),
                    None => Ok(()),
                };
            }
            notified.await;
        }
    }
    pub fn release_lost_reply(&self) {
        self.0.released.store(true, Ordering::SeqCst);
        self.0.release.notify_waiters();
    }
}
impl Drop for CommitReplyGate {
    fn drop(&mut self) {
        self.release_lost_reply();
    }
}

#[derive(Clone)]
struct Forwarder {
    upstream: SocketAddr,
    client: reqwest::Client,
    exchanges: Arc<Mutex<Vec<Exchange>>>,
    armed: Arc<Mutex<Option<Arc<ReplyGate>>>>,
    cancel: CancellationToken,
}

pub struct LibsqlFinishFixture {
    root: PathBuf,
    upstream: SocketAddr,
    proxy: SocketAddr,
    child: Option<Child>,
    sqld_pid: u32,
    proxy_join: Option<JoinHandle<Result<(), std::io::Error>>>,
    forwarder: Forwarder,
}

#[derive(Debug, Serialize)]
pub struct FinishReceipt {
    pub root: PathBuf,
    pub sqld_pid: u32,
    pub sqld_exit: String,
    pub sqld_reaped: bool,
    pub proxy_joined: bool,
    pub upstream: SocketAddr,
    pub proxy: SocketAddr,
    pub upstream_closed: bool,
    pub proxy_closed: bool,
    pub exchanges: Vec<Exchange>,
}

impl LibsqlFinishFixture {
    /// `root` must be a new caller-owned run directory. No installed service,
    /// account, global environment, or production credential is touched.
    pub async fn start(sqld: &Path, root: &Path) -> FixtureResult<Self> {
        if hex::encode(Sha256::digest(tokio::fs::read(sqld).await?)) != SQLD_SHA256 {
            return Err("official sqld fixture binary digest mismatch".into());
        }
        tokio::fs::create_dir(root).await?;
        let log = std::fs::File::create(root.join("sqld.log"))?;
        let mut child = Command::new(sqld)
            .args([
                "--db-path",
                root.join("data.sqld")
                    .to_str()
                    .ok_or("non-UTF8 fixture path")?,
                "--http-listen-addr",
                "127.0.0.1:0",
                "--disable-metrics",
                "--no-welcome",
            ])
            .env_clear()
            .env("PATH", "/usr/bin:/bin")
            .env("LANG", "C.UTF-8")
            .env("RUST_LOG", "libsql_server=info")
            .stdin(Stdio::null())
            .stdout(Stdio::from(log.try_clone()?))
            .stderr(Stdio::from(log))
            .kill_on_drop(true)
            .spawn()?;
        let setup = async {
            let upstream = discover_owned_listener(&mut child).await?;
            let listener = tokio::net::TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).await?;
            let proxy = listener.local_addr()?;
            let forwarder = Forwarder {
                upstream,
                client: reqwest::Client::builder()
                    .redirect(reqwest::redirect::Policy::none())
                    .no_proxy()
                    .build()?,
                exchanges: Arc::new(Mutex::new(Vec::new())),
                armed: Arc::new(Mutex::new(None)),
                cancel: CancellationToken::new(),
            };
            let shutdown = forwarder.cancel.clone();
            let router = Router::new()
                .fallback(forward)
                .with_state(forwarder.clone());
            let proxy_join = tokio::spawn(async move {
                axum::serve(listener, router)
                    .with_graceful_shutdown(shutdown.cancelled_owned())
                    .await
            });
            Ok::<_, Box<dyn Error + Send + Sync>>((upstream, proxy, forwarder, proxy_join))
        }
        .await;
        match setup {
            Ok((upstream, proxy, forwarder, proxy_join)) => Ok(Self {
                root: root.to_path_buf(),
                upstream,
                proxy,
                sqld_pid: child.id().ok_or("fixture child pid missing")?,
                child: Some(child),
                proxy_join: Some(proxy_join),
                forwarder,
            }),
            Err(error) => {
                // Keep the failed run/log. A startup error never silently
                // abandons the child or claims kill_on_drop is a reap receipt.
                let cleanup = match child.try_wait() {
                    Ok(Some(_)) => Ok(()),
                    Ok(None) => child.kill().await,
                    Err(error) => Err(error),
                };
                match cleanup {
                    Ok(()) => Err(error),
                    Err(cleanup) => Err(format!(
                        "fixture startup failed: {error}; child cleanup failed: {cleanup}"
                    )
                    .into()),
                }
            }
        }
    }

    /// The real pinned public remote builder. Loopback HTTP is confined to
    /// this fixture; production RemoteDatabase::connect still requires TLS.
    pub async fn database(&self) -> FixtureResult<libsql::Database> {
        Ok(
            libsql::Builder::new_remote(format!("http://{}", self.proxy), FIXTURE_TOKEN.into())
                .build()
                .await?,
        )
    }

    pub fn exchanges(&self) -> Vec<Exchange> {
        self.forwarder
            .exchanges
            .lock()
            .expect("fixture exchange lock")
            .clone()
    }

    pub fn arm_commit_response_loss(&self) -> CommitReplyGate {
        let gate = Arc::new(ReplyGate {
            received: AtomicBool::new(false),
            released: AtomicBool::new(false),
            response: Notify::new(),
            release: Notify::new(),
            upstream_error: Mutex::new(None),
        });
        let mut armed = self.forwarder.armed.lock().expect("fixture fault lock");
        assert!(
            armed.is_none(),
            "only one caller-owned COMMIT gate may be armed"
        );
        *armed = Some(gate.clone());
        CommitReplyGate(gate)
    }

    /// Explicitly join the proxy and reap the official child, preserve raw
    /// exchanges, and verify both port-zero listeners are actually retired.
    pub async fn finish(mut self) -> FixtureResult<FinishReceipt> {
        self.forwarder.cancel.cancel();
        if let Some(gate) = self
            .forwarder
            .armed
            .lock()
            .expect("fixture fault lock")
            .take()
        {
            gate.released.store(true, Ordering::SeqCst);
            gate.release.notify_waiters();
        }
        let proxy_result = match self.proxy_join.take() {
            Some(mut join) => match tokio::time::timeout(FINISH_DEADLINE, &mut join).await {
                Ok(result) => result
                    .map_err(|error| error.to_string())
                    .and_then(|result| result.map_err(|error| error.to_string())),
                Err(_) => {
                    join.abort();
                    let _ = join.await;
                    Err("fixture proxy exceeded its cleanup deadline; aborted and joined".into())
                }
            },
            None => Err("fixture proxy join missing".into()),
        };
        let mut child = self.child.take().ok_or("fixture child missing")?;
        let pid = self.sqld_pid;
        let status = match child.try_wait()? {
            Some(status) => status,
            None => {
                child.start_kill()?;
                tokio::time::timeout(FINISH_DEADLINE, child.wait()).await??
            }
        };
        let receipt = FinishReceipt {
            root: self.root.clone(),
            sqld_pid: pid,
            sqld_exit: status.to_string(),
            sqld_reaped: true,
            proxy_joined: proxy_result.is_ok(),
            upstream: self.upstream,
            proxy: self.proxy,
            upstream_closed: tokio::net::TcpStream::connect(self.upstream).await.is_err(),
            proxy_closed: tokio::net::TcpStream::connect(self.proxy).await.is_err(),
            exchanges: self.exchanges(),
        };
        tokio::fs::write(
            self.root.join("finish-receipt.json"),
            serde_json::to_vec_pretty(&receipt)?,
        )
        .await?;
        if let Err(error) = proxy_result {
            return Err(error.into());
        }
        if !receipt.upstream_closed || !receipt.proxy_closed {
            return Err("fixture listeners remain open after explicit cleanup".into());
        }
        Ok(receipt)
    }
}

impl Drop for LibsqlFinishFixture {
    fn drop(&mut self) {
        self.forwarder.cancel.cancel();
        // Exceptional test unwind requests retirement. Only finish() returns
        // explicit process/port receipts; Drop cannot claim joined cleanup.
        if let Some(child) = self.child.as_mut() {
            let _ = child.start_kill();
        }
    }
}

async fn forward(State(state): State<Forwarder>, request: Request) -> Response<Body> {
    let (parts, body) = request.into_parts();
    let body = match axum::body::to_bytes(body, MAX_REQUEST_BYTES).await {
        Ok(body) => body,
        Err(_) => return response(StatusCode::PAYLOAD_TOO_LARGE, Body::empty()),
    };
    let path = parts
        .uri
        .path_and_query()
        .map(|path| path.as_str())
        .unwrap_or("/")
        .to_string();
    // Inspect only this fixture's fixed control marker; all SQL and native
    // cursor/page bytes are forwarded unchanged to the maintained upstream.
    let inspected: Value = serde_json::from_slice(&body).unwrap_or(Value::Null);
    let (has_commit, has_close) = fixed_finish_markers(&inspected);
    let fault = if has_commit {
        state.armed.lock().expect("fixture fault lock").take()
    } else {
        None
    };
    let ordinal = {
        let mut exchanges = state.exchanges.lock().expect("fixture exchange lock");
        let ordinal = exchanges.len();
        exchanges.push(Exchange {
            ordinal,
            path: path.clone(),
            request: inspected,
            has_commit,
            has_close,
            upstream_status: None,
            upstream_body: Vec::new(),
            upstream_error: None,
            reply_lost: fault.is_some(),
        });
        ordinal
    };
    let mut upstream = state
        .client
        .request(parts.method, format!("http://{}{path}", state.upstream))
        .body(body);
    for name in [header::CONTENT_TYPE, header::AUTHORIZATION] {
        if let Some(value) = parts.headers.get(&name) {
            upstream = upstream.header(name, value.clone());
        }
    }
    let upstream = async {
        let upstream = upstream.send().await?;
        let status = upstream.status();
        let content_type = upstream.headers().get(header::CONTENT_TYPE).cloned();
        let bytes = upstream.bytes().await?;
        Ok::<_, reqwest::Error>((status, content_type, bytes))
    }
    .await;
    let (status, content_type, bytes) = match upstream {
        Ok(upstream) => upstream,
        Err(error) => {
            state.exchanges.lock().expect("fixture exchange lock")[ordinal].upstream_error =
                Some(error.to_string());
            if let Some(gate) = fault {
                *gate.upstream_error.lock().expect("fixture response lock") =
                    Some(error.to_string());
                gate.received.store(true, Ordering::SeqCst);
                gate.response.notify_waiters();
            }
            return response(StatusCode::BAD_GATEWAY, Body::empty());
        }
    };
    {
        let mut exchanges = state.exchanges.lock().expect("fixture exchange lock");
        exchanges[ordinal].upstream_status = Some(status.as_u16());
        exchanges[ordinal].upstream_body = bytes.to_vec();
    }
    let output = if let Some(gate) = fault {
        gate.received.store(true, Ordering::SeqCst);
        gate.response.notify_waiters();
        loop {
            let notified = gate.release.notified();
            tokio::pin!(notified);
            notified.as_mut().enable();
            if gate.released.load(Ordering::SeqCst) {
                break;
            }
            tokio::select! {
                () = notified => {},
                () = state.cancel.cancelled() => break,
            }
        }
        Body::from_stream(futures_util::stream::once(async {
            Err::<Bytes, _>(std::io::Error::new(
                std::io::ErrorKind::UnexpectedEof,
                "fixture cut reply after actual upstream response",
            ))
        }))
    } else {
        Body::from(bytes)
    };
    let mut output = response(status, output);
    if let Some(content_type) = content_type {
        output
            .headers_mut()
            .insert(header::CONTENT_TYPE, content_type);
    }
    output
}

fn response(status: StatusCode, body: Body) -> Response<Body> {
    let mut response = Response::new(body);
    *response.status_mut() = status;
    response
}

fn fixed_finish_markers(request: &Value) -> (bool, bool) {
    let Some(requests) = request.get("requests").and_then(Value::as_array) else {
        return (false, false);
    };
    let mut commit = false;
    let mut close = false;
    for request in requests {
        close |= request.get("type").and_then(Value::as_str) == Some("close");
        if let Some(steps) = request
            .get("batch")
            .and_then(|batch| batch.get("steps"))
            .and_then(Value::as_array)
        {
            commit |= steps.iter().any(|step| {
                step.get("stmt")
                    .and_then(|stmt| stmt.get("sql"))
                    .and_then(Value::as_str)
                    .is_some_and(|sql| sql.trim() == "COMMIT")
            });
        }
    }
    (commit, close)
}

/// Discover the port-zero listener from this exact unreaped child's owned
/// socket FDs. This is process inspection, not a database wire parser.
async fn discover_owned_listener(child: &mut Child) -> FixtureResult<SocketAddr> {
    let pid = child.id().ok_or("sqld pid missing")?;
    let deadline = tokio::time::Instant::now() + START_DEADLINE;
    loop {
        if let Some(status) = child.try_wait()? {
            return Err(format!("official sqld exited before listen: {status}").into());
        }
        let mut owned = HashSet::new();
        for entry in std::fs::read_dir(format!("/proc/{pid}/fd"))? {
            if let Ok(target) = std::fs::read_link(entry?.path()) {
                if let Some(inode) = target
                    .to_str()
                    .and_then(|value| value.strip_prefix("socket:["))
                    .and_then(|value| value.strip_suffix(']'))
                {
                    owned.insert(inode.to_string());
                }
            }
        }
        let mut ports = Vec::new();
        for line in tokio::fs::read_to_string("/proc/net/tcp")
            .await?
            .lines()
            .skip(1)
        {
            let fields: Vec<_> = line.split_ascii_whitespace().collect();
            if fields.len() > 9 && fields[3] == "0A" && owned.contains(fields[9]) {
                if let Some(port) = fields[1].strip_prefix("0100007F:") {
                    ports.push(u16::from_str_radix(port, 16)?);
                }
            }
        }
        if ports.len() == 1 {
            return Ok(SocketAddr::from((Ipv4Addr::LOCALHOST, ports[0])));
        }
        if ports.len() > 1 {
            return Err("official sqld has ambiguous owned listeners".into());
        }
        if tokio::time::Instant::now() >= deadline {
            return Err("official sqld startup deadline exceeded".into());
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
}
