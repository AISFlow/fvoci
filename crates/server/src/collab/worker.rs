//! A worker process as the server sees it: started when it is first needed, asked one
//! request at a time, and killed and started again when it dies, breaks a frame or
//! takes longer than the time limit.
//!
//! The process is held by a task of its own, which takes the requests in turn from a
//! queue and answers each on a channel of its caller's. A caller that gives up waiting
//! leaves the task to finish the request, so that the next one is not answered with
//! what was meant for it; and the task reaps a worker that dies while nobody asks
//! anything.
//!
//! Every start is a new generation. What a worker held is gone with it: a request
//! about a document opened in an earlier generation is not sent ([`CallError::Lost`]),
//! and whoever holds the document opens it again from what it stored. A worker that
//! dies again and again is started again after a wait that doubles, up to a ceiling.

use std::ffi::OsString;
use std::path::PathBuf;
use std::process::Stdio;
use std::sync::{Arc, OnceLock};
use std::time::Duration;

use thiserror::Error;
use tokio::io::{AsyncReadExt, AsyncWriteExt, BufReader};
use tokio::process::{Child, ChildStdin, ChildStdout, Command};
use tokio::sync::{mpsc, oneshot};
use tokio::time::{Instant, sleep_until, timeout};
use tokio_util::sync::CancellationToken;

use crate::collab_worker::WORKER_ARGUMENT;
use crate::collab_worker::frame::{HEADER_BYTES, header, length_of};
#[cfg(feature = "test-util")]
use crate::collab_worker::wire::DocNumber;
use crate::collab_worker::wire::{
    Answer, Limits, Refusal, Refused, Request, Response, decode_response,
};

/// The longest wait before a worker that keeps dying is started again.
pub const LONGEST_RESTART_WAIT: Duration = Duration::from_secs(10);

/// The wait before the second start in a row that follows a death; it doubles with
/// each further one.
const FIRST_RESTART_WAIT: Duration = Duration::from_millis(100);

/// How long a worker that is asked to end is given before it is killed.
const END_WAIT: Duration = Duration::from_secs(2);

/// How many requests may wait for the worker before a caller waits to put one in.
const QUEUE: usize = 256;

/// How a worker process is started: a program and its arguments.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorkerCommand {
    /// The executable to start: the server's own, or the tests' worker.
    pub program: PathBuf,
    /// Its arguments.
    pub args: Vec<OsString>,
}

impl WorkerCommand {
    /// The executable the server runs in, as the worker. On Linux it is the file the
    /// server was started from even if another was put at its path since (an upgrade
    /// while it runs): the two speak the same frames, which carry no version. Elsewhere,
    /// and without `/proc`, it is the file at the path the server was started from.
    ///
    /// # Errors
    ///
    /// Returns why the path of the running executable is not known.
    pub fn this_executable() -> std::io::Result<Self> {
        let running = std::path::Path::new("/proc/self/exe");
        if cfg!(target_os = "linux") && running.exists() {
            return Ok(Self::of_executable(running.to_owned()));
        }
        Ok(Self::of_executable(std::env::current_exe()?))
    }

    /// The server's executable at `program`, as the worker.
    #[must_use]
    pub fn of_executable(program: PathBuf) -> Self {
        Self {
            program,
            args: vec![WORKER_ARGUMENT.into()],
        }
    }
}

/// How the workers are started and held.
#[derive(Debug, Clone)]
pub struct WorkerSettings {
    /// How a worker is started.
    pub command: WorkerCommand,
    /// What a worker holds what it is sent to.
    pub limits: Limits,
    /// How long an answer may take.
    pub timeout: Duration,
    /// The address space of a worker, where the system limits it.
    pub memory_bytes: u64,
}

/// Why a request got no answer.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum CallError {
    /// The document was opened by a worker that is gone.
    #[error("the worker that held the document is gone")]
    Lost,
    /// The worker died, broke a frame or took too long, and was killed; it is started
    /// again for the next request.
    #[error("the worker died: {0}")]
    Died(Death),
    /// No worker could be started.
    #[error("no worker could be started: {0}")]
    Unavailable(String),
    /// The worker refused the request, or it was refused before it was sent: it is
    /// larger than a frame.
    #[error(transparent)]
    Refused(#[from] Refused),
}

/// Whom a death of the worker over a request is put down to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Blame {
    /// The worker: it is started again after the wait that its deaths in a row call for,
    /// so that one that cannot run is not started over and over.
    Worker,
    /// The sender of the update the request takes, whose updates the server refuses for
    /// a while: the worker is started again without a wait for it, which every
    /// document of the worker would wait for.
    Sender,
}

/// How a worker ended.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum Death {
    /// It did not answer within the time limit.
    #[error("it did not answer in time")]
    TimedOut,
    /// A frame could not be sent or read: the process ended, or wrote what is no
    /// answer.
    #[error("{0}")]
    Broken(String),
}

/// How many workers ended, for the tests of whom a death is put down to.
#[cfg(feature = "test-util")]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Ended {
    /// The workers that ran and died or were ended (one that did not take its limits
    /// never ran).
    pub workers: u64,
    /// The deaths put down to the worker since its last answer: the next one waits for
    /// them to start ([`Blame::Worker`]).
    pub in_a_row: u32,
}

/// Which request a test has the process of a worker stopped at ([`Worker::stop_at`]).
#[cfg(feature = "test-util")]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StopAt {
    /// The next request, whatever it is about.
    NextRequest,
    /// The next request about the document of this number.
    Document(DocNumber),
}

#[cfg(feature = "test-util")]
impl StopAt {
    fn names(self, request: &Request) -> bool {
        match self {
            Self::NextRequest => true,
            Self::Document(doc) => document_of(request) == Some(doc),
        }
    }
}

/// The document a request is about, if it is about one.
#[cfg(feature = "test-util")]
fn document_of(request: &Request) -> Option<DocNumber> {
    match request {
        Request::Open { doc, .. }
        | Request::Take { doc, .. }
        | Request::Retake { doc, .. }
        | Request::Append { doc, .. }
        | Request::Diff { doc, .. }
        | Request::StateVector(doc)
        | Request::State(doc)
        | Request::Body(doc)
        | Request::Close(doc)
        | Request::FailNextSettle(doc) => Some(*doc),
        Request::Configure(_) => None,
    }
}

/// A worker process and the task that holds it. Dropping it ends the task, and with it
/// the process.
#[derive(Debug)]
pub struct Worker {
    settings: Arc<WorkerSettings>,
    queue: OnceLock<mpsc::Sender<Job>>,
    /// Cancelled when the worker is to end now, whatever request it is on.
    stopped: CancellationToken,
    /// Where the process is to be stopped, and for how long ([`Worker::stop_at`]).
    #[cfg(feature = "test-util")]
    stops: std::sync::Mutex<Vec<(StopAt, Duration)>>,
}

/// What the task of a worker is asked.
#[derive(Debug)]
enum Job {
    Call {
        generation: Option<u64>,
        frame: Vec<u8>,
        blame: Blame,
        /// How long the process is stopped as it is sent the request.
        #[cfg(feature = "test-util")]
        stop: Option<Duration>,
        answer: oneshot::Sender<Result<(u64, Answer), CallError>>,
    },
    Pid(oneshot::Sender<Option<u32>>),
    #[cfg(feature = "test-util")]
    Ended(oneshot::Sender<Ended>),
    End(oneshot::Sender<()>),
}

impl Worker {
    /// A worker that is started when it is first asked something. Its task begins
    /// then, on the runtime of the caller.
    pub fn new(settings: WorkerSettings) -> Self {
        Self {
            settings: Arc::new(settings),
            queue: OnceLock::new(),
            stopped: CancellationToken::new(),
            #[cfg(feature = "test-util")]
            stops: std::sync::Mutex::new(Vec::new()),
        }
    }

    /// The queue of the task, which is begun on first use.
    fn queue(&self) -> &mpsc::Sender<Job> {
        self.queue.get_or_init(|| {
            let (queue, jobs) = mpsc::channel(QUEUE);
            tokio::spawn(hold(Arc::clone(&self.settings), jobs, self.stopped.clone()));
            queue
        })
    }

    async fn ask<T>(&self, job: impl FnOnce(oneshot::Sender<T>) -> Job) -> Option<T> {
        let (answer, answered) = oneshot::channel();
        self.queue().send(job(answer)).await.ok()?;
        answered.await.ok()
    }

    /// Sends a request and waits for its answer, starting a worker first if none
    /// runs. `generation` is that of the worker that opened the document the request
    /// is about; `None` for a request that is about no open document. Gives the
    /// generation of the worker that answered.
    ///
    /// # Errors
    ///
    /// Returns [`CallError::Lost`] if the worker of `generation` is gone,
    /// [`CallError::Refused`] if the worker refused the request or it is larger than
    /// a frame, and [`CallError::Died`] or [`CallError::Unavailable`] if it got no
    /// answer.
    pub async fn call(
        &self,
        generation: Option<u64>,
        request: &Request,
    ) -> Result<(u64, Answer), CallError> {
        self.call_blaming(generation, request, Blame::Worker).await
    }

    /// [`Worker::call`], with whom a death of the worker over the request is put down
    /// to: [`Blame::Sender`] for an update whose sender the server knows and holds
    /// back, so that its death does not make the next worker wait.
    ///
    /// # Errors
    ///
    /// As [`Worker::call`].
    pub async fn call_blaming(
        &self,
        generation: Option<u64>,
        request: &Request,
        blame: Blame,
    ) -> Result<(u64, Answer), CallError> {
        let frame = request.encode();
        if header(frame.len(), self.settings.limits.frame_bytes()).is_err() {
            let refusal = if matches!(request, Request::Open { .. }) {
                Refusal::DocumentTooLarge
            } else {
                Refusal::UpdateTooLarge
            };
            return Err(CallError::Refused(Refused {
                refusal,
                message: "the request is larger than a frame".to_owned(),
                rolled_back: false,
            }));
        }
        #[cfg(feature = "test-util")]
        let stop = self.stop_for(request);
        self.ask(|answer| Job::Call {
            generation,
            frame,
            blame,
            #[cfg(feature = "test-util")]
            stop,
            answer,
        })
        .await
        .unwrap_or_else(|| Err(CallError::Unavailable("the worker's task ended".to_owned())))
    }

    /// The process identifier of the running worker, if one runs.
    pub async fn pid(&self) -> Option<u32> {
        self.ask(Job::Pid).await.flatten()
    }

    /// How many workers ended, and how many deaths in a row the next one waits for.
    #[cfg(feature = "test-util")]
    pub async fn ended(&self) -> Option<Ended> {
        self.ask(Job::Ended).await
    }

    /// Has the process stopped (`SIGSTOP`) as it is sent the request `at` names, and
    /// go on `stop` later: for the tests of a worker that does not answer in time. The
    /// request is sent all the same, and the process reads it as it goes on, unless the
    /// time limit kills it first ([`Death::TimedOut`]). The request is the server's
    /// own and the process the worker's, whatever it was built with. Unix only.
    ///
    /// # Panics
    ///
    /// If a thread panicked while it held the stops.
    #[cfg(feature = "test-util")]
    pub fn stop_at(&self, at: StopAt, stop: Duration) {
        let mut stops = self.stops.lock().expect("not poisoned");
        stops.push((at, stop));
    }

    /// Has the worker take `stop` over a request about no document: its process is
    /// stopped as it is sent one ([`Worker::stop_at`]). Gives what the request gave.
    ///
    /// # Errors
    ///
    /// As [`Worker::call`]: [`CallError::Died`] with [`Death::TimedOut`] if `stop` is
    /// past the time limit.
    ///
    /// # Panics
    ///
    /// If a thread panicked while it held the stops.
    #[cfg(feature = "test-util")]
    pub async fn stall(&self, stop: Duration, blame: Blame) -> Result<(u64, Answer), CallError> {
        self.stop_at(StopAt::NextRequest, stop);
        // No document is open under the largest number: closing it answers `Done`.
        self.call_blaming(None, &Request::Close(DocNumber::MAX), blame)
            .await
    }

    /// How long the process is to be stopped as it is sent `request`: the first stop
    /// that names it, which is then used up.
    #[cfg(feature = "test-util")]
    fn stop_for(&self, request: &Request) -> Option<Duration> {
        let mut stops = self.stops.lock().expect("not poisoned");
        let at = stops.iter().position(|(at, _)| at.names(request))?;
        Some(stops.remove(at).1)
    }

    /// Ends the worker: closes its input, which ends it, and kills it if it does not
    /// end in time. The next request starts another.
    pub async fn end(&self) {
        let _ = self.ask(Job::End).await;
    }

    /// Ends the worker now: the request it is on, if any, ends as a death, and so do
    /// those after it, without a worker; and the worker is killed. For a server that
    /// stops and cannot wait.
    pub async fn end_now(&self) {
        self.stopped.cancel();
        self.end().await;
    }
}

/// What the task of a worker knows.
#[derive(Debug)]
struct State {
    settings: Arc<WorkerSettings>,
    /// The worker is to end now ([`Worker::end_now`]).
    stopped: CancellationToken,
    running: Option<Running>,
    /// The generation of the running worker, or of the next one if none runs.
    generation: u64,
    /// The deaths since the last answer.
    deaths: u32,
    /// When the next worker may be started.
    next_start: Instant,
}

#[derive(Debug)]
struct Running {
    child: Child,
    input: ChildStdin,
    output: BufReader<ChildStdout>,
}

/// The task of a worker: takes the jobs in turn until every handle is dropped, and
/// reaps a worker that dies between them.
async fn hold(
    settings: Arc<WorkerSettings>,
    mut jobs: mpsc::Receiver<Job>,
    stopped: CancellationToken,
) {
    let mut state = State {
        settings,
        stopped,
        running: None,
        generation: 0,
        deaths: 0,
        next_start: Instant::now(),
    };
    loop {
        let job = match state.running.as_mut() {
            // A worker that ended is buried before the next job is asked of it.
            Some(running) => tokio::select! {
                biased;
                status = running.child.wait() => {
                    let death = Death::Broken(format!("it ended by itself: {status:?}"));
                    state.bury(&death, Blame::Worker).await;
                    continue;
                }
                job = jobs.recv() => job,
            },
            None => jobs.recv().await,
        };
        let Some(job) = job else { break };
        match job {
            Job::Call {
                generation,
                frame,
                blame,
                #[cfg(feature = "test-util")]
                stop,
                answer,
            } => {
                let answered = state
                    .call(
                        generation,
                        &frame,
                        blame,
                        #[cfg(feature = "test-util")]
                        stop,
                    )
                    .await;
                let _ = answer.send(answered);
            }
            Job::Pid(answer) => {
                let pid = state
                    .running
                    .as_ref()
                    .and_then(|running| running.child.id());
                let _ = answer.send(pid);
            }
            #[cfg(feature = "test-util")]
            Job::Ended(answer) => {
                let _ = answer.send(Ended {
                    workers: state.generation,
                    in_a_row: state.deaths,
                });
            }
            Job::End(answer) => {
                state.end().await;
                let _ = answer.send(());
            }
        }
    }
    state.end().await;
}

impl State {
    async fn call(
        &mut self,
        generation: Option<u64>,
        frame: &[u8],
        blame: Blame,
        #[cfg(feature = "test-util")] stop: Option<Duration>,
    ) -> Result<(u64, Answer), CallError> {
        // A worker that ended while the job waited took its documents with it: the
        // request is not sent to it, and is not counted against them.
        if let Some(running) = self.running.as_mut()
            && let Ok(Some(status)) = running.child.try_wait()
        {
            let death = Death::Broken(format!("it ended by itself: {status:?}"));
            self.bury(&death, Blame::Worker).await;
        }
        if generation.is_some_and(|generation| generation != self.generation) {
            return Err(CallError::Lost);
        }
        if self.stopped.is_cancelled() {
            return Err(CallError::Unavailable("the worker was ended".to_owned()));
        }
        let generation = self.generation;
        if self.running.is_none() {
            self.running = Some(self.start().await?);
        }
        let limit = self.settings.limits.frame_bytes();
        let running = self.running.as_mut().expect("a worker runs");
        #[cfg(feature = "test-util")]
        let pid = running.child.id();
        let exchange = running.exchange(frame, limit);
        #[cfg(feature = "test-util")]
        let exchange = stopped_over(pid, stop, exchange);
        let answered = tokio::select! {
            answered = timeout(self.settings.timeout, exchange) => answered,
            () = self.stopped.cancelled() => Ok(Err("the worker was ended under it".to_owned())),
        };
        let death = match answered {
            Ok(Ok(response)) => {
                self.deaths = 0;
                return response
                    .map(|answer| (generation, answer))
                    .map_err(CallError::Refused);
            }
            Ok(Err(broken)) => Death::Broken(broken),
            Err(_) => Death::TimedOut,
        };
        self.bury(&death, blame).await;
        Err(CallError::Died(death))
    }

    /// Starts a worker and gives it its limits, after the wait that the deaths before
    /// it call for.
    async fn start(&mut self) -> Result<Running, CallError> {
        let ended = || CallError::Unavailable("the worker was ended".to_owned());
        tokio::select! {
            () = sleep_until(self.next_start) => {}
            () = self.stopped.cancelled() => return Err(ended()),
        }
        let started = spawn(&self.settings).map_err(|error| error.to_string());
        let mut running = match started {
            Ok(running) => running,
            Err(error) => {
                self.count_death();
                tracing::error!(%error, "cannot start a collab worker");
                return Err(CallError::Unavailable(error));
            }
        };
        let configure = Request::Configure(self.settings.limits).encode();
        let limit = self.settings.limits.frame_bytes();
        let configured = tokio::select! {
            configured = timeout(self.settings.timeout, running.exchange(&configure, limit)) => configured,
            () = self.stopped.cancelled() => Ok(Err("the worker was ended".to_owned())),
        };
        match configured {
            Ok(Ok(Ok(Answer::Done))) => {
                tracing::info!(pid = running.child.id(), "started a collab worker");
                Ok(running)
            }
            other => {
                let _ = running.child.kill().await;
                self.count_death();
                let error = format!("the worker did not take its limits: {other:?}");
                tracing::error!(%error, "cannot start a collab worker");
                Err(CallError::Unavailable(error))
            }
        }
    }

    /// Kills the running worker and waits for its end, so that it leaves no zombie;
    /// what it held is gone with it. A death put down to the worker makes the next one
    /// wait ([`restart_wait`]).
    async fn bury(&mut self, death: &Death, blame: Blame) {
        self.generation += 1;
        if blame == Blame::Worker {
            self.count_death();
        }
        if let Some(mut running) = self.running.take() {
            let _ = running.child.start_kill();
            let status = running.child.wait().await;
            tracing::warn!(%death, ?status, ?blame, "a collab worker died; the next request starts another");
        }
    }

    /// Ends the running worker: closes its input and kills it if it does not end in
    /// time.
    async fn end(&mut self) {
        if let Some(mut running) = self.running.take() {
            self.generation += 1;
            drop(running.input);
            if timeout(END_WAIT, running.child.wait()).await.is_err() {
                let _ = running.child.kill().await;
            }
        }
    }

    fn count_death(&mut self) {
        self.deaths = self.deaths.saturating_add(1);
        self.next_start = Instant::now() + restart_wait(self.deaths);
    }
}

/// Runs `exchange` with the process `pid` stopped for `stop` first, if there is a stop:
/// the process goes on when it is up, unless the time limit drops `exchange` before
/// then, and the process is killed stopped.
#[cfg(feature = "test-util")]
async fn stopped_over<T>(
    pid: Option<u32>,
    stop: Option<Duration>,
    exchange: impl Future<Output = T>,
) -> T {
    let (Some(pid), Some(stop)) = (pid, stop) else {
        return exchange.await;
    };
    #[cfg(unix)]
    super::child::signal(pid, super::child::Signal::Stop);
    #[cfg(not(unix))]
    let _ = pid;
    let mut exchange = std::pin::pin!(exchange);
    tokio::select! {
        exchanged = &mut exchange => return exchanged,
        () = tokio::time::sleep(stop) => {}
    }
    #[cfg(unix)]
    super::child::signal(pid, super::child::Signal::Continue);
    exchange.await
}

fn spawn(settings: &WorkerSettings) -> std::io::Result<Running> {
    let WorkerCommand { program, args } = &settings.command;
    let mut command = Command::new(program);
    command
        .args(args)
        // The worker needs nothing from the environment, which may hold the
        // database's password.
        .env_clear()
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::inherit())
        .kill_on_drop(true);
    #[cfg(unix)]
    super::child::limit(&mut command, settings.memory_bytes);
    let mut child = command.spawn()?;
    let input = child.stdin.take().ok_or_else(no_pipe)?;
    let output = child.stdout.take().ok_or_else(no_pipe)?;
    Ok(Running {
        child,
        input,
        output: BufReader::new(output),
    })
}

fn no_pipe() -> std::io::Error {
    std::io::Error::other("the worker's pipe was not made")
}

/// The wait before a worker is started after `deaths` deaths in a row: none after the
/// first, then [`FIRST_RESTART_WAIT`], doubling up to [`LONGEST_RESTART_WAIT`].
fn restart_wait(deaths: u32) -> Duration {
    if deaths <= 1 {
        return Duration::ZERO;
    }
    let doublings = deaths.saturating_sub(2).min(16);
    FIRST_RESTART_WAIT
        .saturating_mul(1 << doublings)
        .min(LONGEST_RESTART_WAIT)
}

impl Running {
    /// Writes a request and reads its answer.
    async fn exchange(&mut self, frame: &[u8], limit: u64) -> Result<Response, String> {
        let head = header(frame.len(), limit).map_err(|error| error.to_string())?;
        let write = async {
            self.input.write_all(&head).await?;
            self.input.write_all(frame).await?;
            self.input.flush().await
        };
        write
            .await
            .map_err(|error| format!("cannot write a request: {error}"))?;
        let mut head = [0; HEADER_BYTES];
        self.output
            .read_exact(&mut head)
            .await
            .map_err(|error| format!("cannot read an answer: {error}"))?;
        let length = length_of(head, limit).map_err(|error| error.to_string())?;
        let mut answer = vec![0; length];
        self.output
            .read_exact(&mut answer)
            .await
            .map_err(|error| format!("cannot read an answer: {error}"))?;
        decode_response(&answer).map_err(|error| error.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_wait_before_a_restart_doubles_up_to_its_ceiling() {
        assert_eq!(restart_wait(0), Duration::ZERO);
        assert_eq!(
            restart_wait(1),
            Duration::ZERO,
            "a first death costs no wait"
        );
        assert_eq!(restart_wait(2), FIRST_RESTART_WAIT);
        assert_eq!(restart_wait(3), FIRST_RESTART_WAIT * 2);
        assert_eq!(restart_wait(9), LONGEST_RESTART_WAIT, "12.8 s is past it");
        assert_eq!(restart_wait(u32::MAX), LONGEST_RESTART_WAIT);
    }
}
