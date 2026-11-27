//! The worker process as the server holds it: what it answers, and what becomes of it
//! when it dies, takes too long or is given up half way.

use std::time::{Duration, Instant};

use fvoci_collab::testing::Editor;
use fvoci_server::collab::worker::{
    Blame, CallError, Death, Worker, WorkerCommand, WorkerSettings,
};
use fvoci_server::collab_worker::wire::{Answer, Limits, Refusal, Request, Share, Source};
use serde_json::{Value, json};

const STATE_BYTES: u64 = 16 * 1024 * 1024;

/// The worker these tests start: the server's worker with the request of the tests
/// (`Request::FailNextSettle`), which the server's executable has only when it was last
/// built for the tests.
const TEST_WORKER: &str = env!("CARGO_BIN_EXE_fvoci-test-worker");

/// A request a worker answers at once: closing a document that is not open.
const PING: Request = Request::Close(u64::MAX);

/// Longer than any time limit of these tests: a worker stopped for it is killed.
const STALL: Duration = Duration::from_secs(30);

fn limits(documents: u64) -> Limits {
    Limits {
        update_bytes: 64 * 1024,
        state_bytes: STATE_BYTES,
        body_bytes: 1024 * 1024,
        documents,
        held_bytes: 128 * 1024 * 1024,
    }
}

fn worker(timeout: Duration, documents: u64) -> Worker {
    Worker::new(WorkerSettings {
        command: WorkerCommand::of_executable(TEST_WORKER.into()),
        limits: limits(documents),
        timeout,
        memory_bytes: 1024 * 1024 * 1024,
    })
}

fn roomy() -> Worker {
    worker(Duration::from_secs(10), 100)
}

fn paragraphs(texts: &[&str]) -> Value {
    let content: Vec<Value> = texts
        .iter()
        .map(|text| json!({"type": "paragraph", "content": [{"type": "text", "text": text}]}))
        .collect();
    json!({"type": "doc", "content": content})
}

fn open_body(doc: u64, body: &Value) -> Request {
    Request::Open {
        doc,
        source: Source::Body(Some(serde_json::to_vec(body).expect("JSON"))),
        share: Share::Kept,
    }
}

/// The bytes of an answer that carries them.
fn bytes(answer: Answer) -> Vec<u8> {
    match answer {
        Answer::Bytes(bytes) => bytes,
        other => panic!("no bytes: {other:?}"),
    }
}

async fn body_of(worker: &Worker, generation: u64, doc: u64) -> Value {
    let (_, answer) = worker
        .call(Some(generation), &Request::Body(doc))
        .await
        .expect("a body");
    let Answer::Body(Some(json)) = answer else {
        panic!("a body: {answer:?}");
    };
    serde_json::from_slice(&json).expect("JSON")
}

fn refusal(result: Result<(u64, Answer), CallError>) -> Refusal {
    match result {
        Err(CallError::Refused(refused)) => refused.refusal,
        other => panic!("a refusal: {other:?}"),
    }
}

/// The server's own executable is a worker when it is started as one, whichever build
/// of it was made last: nothing here asks it what only the tests ask.
#[tokio::test]
async fn the_servers_executable_is_a_worker() {
    let worker = Worker::new(WorkerSettings {
        command: WorkerCommand::of_executable(env!("CARGO_BIN_EXE_fvoci-server").into()),
        limits: limits(100),
        timeout: Duration::from_secs(10),
        memory_bytes: 1024 * 1024 * 1024,
    });
    let (generation, _) = worker
        .call(None, &open_body(1, &paragraphs(&["kept"])))
        .await
        .expect("opened");
    assert_eq!(body_of(&worker, generation, 1).await, paragraphs(&["kept"]));
    worker.end().await;
}

#[tokio::test]
async fn two_editors_that_write_at_once_converge_through_the_worker() {
    let worker = roomy();
    let (generation, opened) = worker
        .call(None, &open_body(1, &paragraphs(&["first"])))
        .await
        .expect("opened");
    let Answer::Opened {
        state: Some(state), ..
    } = opened
    else {
        panic!("a document made from a body has its state: {opened:?}");
    };
    let one = Editor::join(101, &state);
    let other = Editor::join(102, &state);
    // Each writes before it has the other's update.
    let from_one = one.append_paragraph("one");
    let from_other = other.append_paragraph("other");
    for update in [&from_one, &from_other] {
        let (_, taken) = worker
            .call(
                Some(generation),
                &Request::Take {
                    doc: 1,
                    update: update.clone(),
                },
            )
            .await
            .expect("taken");
        assert!(
            matches!(taken, Answer::Taken { repair: None, .. }),
            "{taken:?}"
        );
    }
    let merged = body_of(&worker, generation, 1).await;
    assert_eq!(
        merged["content"].as_array().map(Vec::len),
        Some(3),
        "{merged}"
    );
    let (_, own) = worker
        .call(Some(generation), &Request::StateVector(1))
        .await
        .expect("a state vector");
    assert_ne!(
        bytes(own),
        Vec::<u8>::new(),
        "a state vector of the document"
    );
    for editor in [&one, &other] {
        let (_, diff) = worker
            .call(
                Some(generation),
                &Request::Diff {
                    doc: 1,
                    state_vector: editor.state_vector(),
                },
            )
            .await
            .expect("what the editor has not");
        editor.receive(&bytes(diff));
        let body = editor.body().expect("a body");
        assert_eq!(
            body.document(),
            &merged,
            "the editor has what the worker has"
        );
    }
    worker.end().await;
}

/// What a worker that dies leaves: it is reaped, the documents it held are gone, and
/// the next request starts another worker.
#[cfg(unix)]
#[tokio::test]
async fn a_worker_that_is_killed_is_started_again_without_its_documents() {
    let worker = roomy();
    let (generation, _) = worker
        .call(None, &open_body(1, &paragraphs(&["kept"])))
        .await
        .expect("opened");
    let first = worker.pid().await.expect("a worker runs");
    signal(first, "KILL");
    // The worker's task notices the death while nobody asks anything, and reaps it.
    let began = Instant::now();
    while is_alive(first) {
        assert!(
            began.elapsed() < Duration::from_secs(5),
            "no zombie is left of it"
        );
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    let lost = worker.call(Some(generation), &Request::State(1)).await;
    assert_eq!(lost, Err(CallError::Lost), "its documents are gone");
    let (again, _) = worker
        .call(None, &open_body(1, &paragraphs(&["kept"])))
        .await
        .expect("a document opens in the next worker");
    assert_ne!(again, generation, "a new generation");
    let second = worker.pid().await.expect("a worker runs");
    assert_ne!(second, first, "another process");
    assert_eq!(body_of(&worker, again, 1).await, paragraphs(&["kept"]));
    worker.end().await;
}

#[tokio::test]
async fn a_worker_that_takes_too_long_is_killed_and_the_request_fails() {
    let worker = worker(Duration::from_millis(300), 10);
    let (generation, _) = worker.call(None, &PING).await.expect("a stall of nothing");
    let first = worker.pid().await.expect("a worker runs");
    let began = Instant::now();
    let stalled = worker.stall(STALL, Blame::Worker).await;
    assert_eq!(stalled, Err(CallError::Died(Death::TimedOut)));
    assert!(
        began.elapsed() < Duration::from_secs(5),
        "{:?}",
        began.elapsed()
    );
    #[cfg(unix)]
    assert!(!is_alive(first), "killed, and no zombie is left");
    let (next, _) = worker
        .call(None, &PING)
        .await
        .expect("the next worker answers");
    assert_ne!(next, generation);
    assert_ne!(worker.pid().await, Some(first));
    worker.end().await;
}

/// A worker that keeps dying is started again later each time, so that one that cannot
/// run is not started over and over: after six deaths in a row, 1.6 s later. A death
/// put down to the sender of an update ([`Blame::Sender`]), whom the server holds back
/// instead, does not make the next start wait, which every document of the worker would
/// wait for.
#[tokio::test]
async fn a_death_put_down_to_a_sender_does_not_make_the_next_worker_wait() {
    const DEATHS: usize = 6;
    for (blame, waits) in [(Blame::Sender, false), (Blame::Worker, true)] {
        let worker = worker(Duration::from_millis(200), 10);
        for _ in 0..DEATHS {
            let stalled = worker.stall(STALL, blame).await;
            assert_eq!(stalled, Err(CallError::Died(Death::TimedOut)), "{blame:?}");
        }
        let began = Instant::now();
        worker
            .call(None, &PING)
            .await
            .expect("the next worker answers");
        let took = began.elapsed();
        assert_eq!(
            took >= Duration::from_millis(1500),
            waits,
            "{blame:?}: started in {took:?}"
        );
        worker.end().await;
    }
}

/// A caller that gives up waiting leaves the worker to finish the request: the next
/// request is answered by the same worker, with its own answer.
#[tokio::test]
async fn a_call_given_up_half_way_neither_kills_the_worker_nor_puts_the_answers_out_of_step() {
    let worker = roomy();
    let (generation, _) = worker
        .call(None, &open_body(1, &paragraphs(&["before"])))
        .await
        .expect("opened");
    let first = worker.pid().await.expect("a worker runs");
    let given_up = tokio::time::timeout(
        Duration::from_millis(100),
        worker.stall(Duration::from_millis(500), Blame::Worker),
    )
    .await;
    assert!(given_up.is_err(), "given up");
    assert_eq!(
        body_of(&worker, generation, 1).await,
        paragraphs(&["before"]),
        "the same worker, and the answer to this request"
    );
    assert_eq!(worker.pid().await, Some(first));
    worker.end().await;
}

/// A request larger than a frame is refused before it is sent, and the worker lives.
#[tokio::test]
async fn a_request_larger_than_a_frame_is_refused_without_harm_to_the_worker() {
    // Memory for documents as large as a state: a frame is that and a little more.
    let worker = Worker::new(WorkerSettings {
        command: WorkerCommand::of_executable(TEST_WORKER.into()),
        limits: Limits {
            held_bytes: STATE_BYTES,
            ..limits(100)
        },
        timeout: Duration::from_secs(10),
        memory_bytes: 1024 * 1024 * 1024,
    });
    let (generation, _) = worker
        .call(None, &open_body(1, &paragraphs(&["kept"])))
        .await
        .expect("opened");
    let first = worker.pid().await;
    let past_a_frame = usize::try_from(STATE_BYTES).expect("small") + 2048;
    let huge = worker
        .call(
            Some(generation),
            &Request::Take {
                doc: 1,
                update: vec![0; past_a_frame],
            },
        )
        .await;
    assert_eq!(refusal(huge), Refusal::UpdateTooLarge);
    let stored = worker
        .call(
            None,
            &Request::Open {
                doc: 2,
                source: Source::State(vec![0; past_a_frame]),
                share: Share::Kept,
            },
        )
        .await;
    assert_eq!(refusal(stored), Refusal::DocumentTooLarge);
    assert_eq!(worker.pid().await, first, "the same worker");
    assert_eq!(body_of(&worker, generation, 1).await, paragraphs(&["kept"]));
    worker.end().await;
}

/// A worker holds the states of its documents within the bytes it is given for them.
#[tokio::test]
async fn a_worker_holds_no_more_bytes_of_documents_than_it_may() {
    let texts: Vec<String> = (0..100)
        .map(|line| format!("line {line} of a body"))
        .collect();
    let texts: Vec<&str> = texts.iter().map(String::as_str).collect();
    let body = paragraphs(&texts);
    let probe = roomy();
    let (_, opened) = probe
        .call(None, &open_body(1, &body))
        .await
        .expect("opened");
    probe.end().await;
    let Answer::Opened { state_bytes, .. } = opened else {
        panic!("{opened:?}");
    };
    let worker = Worker::new(WorkerSettings {
        command: WorkerCommand::of_executable(TEST_WORKER.into()),
        limits: Limits {
            // Two and no third. A document's state is a few bytes longer or shorter
            // with the client identifier it was made under, which is drawn anew.
            held_bytes: state_bytes * 5 / 2,
            ..limits(100)
        },
        timeout: Duration::from_secs(10),
        memory_bytes: 1024 * 1024 * 1024,
    });
    for doc in 1..=2 {
        worker
            .call(None, &open_body(doc, &body))
            .await
            .expect("within");
    }
    let third = worker.call(None, &open_body(3, &body)).await;
    assert_eq!(refusal(third), Refusal::TooManyDocuments, "past the bytes");
    worker.call(None, &Request::Close(1)).await.expect("closed");
    worker
        .call(None, &open_body(3, &body))
        .await
        .expect("room again once one is closed");
    worker.end().await;
}

#[tokio::test]
async fn what_is_past_the_limits_is_refused() {
    let worker = worker(Duration::from_secs(10), 1);
    let (generation, _) = worker
        .call(
            None,
            &Request::Open {
                doc: 1,
                source: Source::Body(None),
                share: Share::Kept,
            },
        )
        .await
        .expect("opened");
    let second = worker
        .call(
            None,
            &Request::Open {
                doc: 2,
                source: Source::Body(None),
                share: Share::Kept,
            },
        )
        .await;
    assert_eq!(refusal(second), Refusal::TooManyDocuments);
    let large = worker
        .call(
            Some(generation),
            &Request::Take {
                doc: 1,
                update: vec![0; 64 * 1024 + 1],
            },
        )
        .await;
    assert_eq!(refusal(large), Refusal::UpdateTooLarge);
    let garbage = worker
        .call(
            Some(generation),
            &Request::Take {
                doc: 1,
                update: vec![0xff; 16],
            },
        )
        .await;
    assert_eq!(refusal(garbage), Refusal::Unreadable);
    let vector = worker
        .call(
            Some(generation),
            &Request::Diff {
                doc: 1,
                state_vector: vec![0xff, 0xff, 0xff, 0xff, 0x0f],
            },
        )
        .await;
    assert_eq!(
        refusal(vector),
        Refusal::Unreadable,
        "a state vector that claims 2^32 entries"
    );
    let unknown = worker.call(Some(generation), &Request::State(7)).await;
    assert_eq!(refusal(unknown), Refusal::UnknownDocument);
    worker.end().await;
}

/// The worker is held to its limits by the system, has no environment and its own
/// process group.
#[cfg(target_os = "linux")]
#[tokio::test]
async fn the_worker_is_started_with_its_limits() {
    let worker = roomy();
    worker.call(None, &PING).await.expect("started");
    let pid = worker.pid().await.expect("a worker runs");
    let limits = std::fs::read_to_string(format!("/proc/{pid}/limits")).expect("its limits");
    let limit = |name: &str| {
        limits
            .lines()
            .find(|line| line.starts_with(name))
            .map_or_else(
                || panic!("{name} in {limits}"),
                |line| {
                    line.split_whitespace()
                        .rev()
                        .skip(1)
                        .take(2)
                        .collect::<Vec<_>>()
                },
            )
    };
    assert_eq!(limit("Max open files"), ["16", "16"], "{limits}");
    assert_eq!(limit("Max core file size"), ["0", "0"], "{limits}");
    assert_eq!(
        limit("Max address space"),
        ["1073741824", "1073741824"],
        "{limits}"
    );
    let environment = std::fs::read(format!("/proc/{pid}/environ")).expect("its environment");
    assert!(
        environment.is_empty(),
        "nothing of the server's environment"
    );
    let stat = std::fs::read_to_string(format!("/proc/{pid}/stat")).expect("its state");
    let group = stat
        .rsplit(')')
        .next()
        .and_then(|rest| rest.split_whitespace().nth(2))
        .expect("the process group");
    assert_eq!(group, pid.to_string(), "a group of its own");
    worker.end().await;
}

/// The time of a request and its answer, and the memory of a worker that holds 20
/// documents, printed for the report.
#[cfg(target_os = "linux")]
#[tokio::test]
async fn the_cost_of_a_request_and_of_held_documents_is_measured() {
    const ROUNDS: u32 = 1_000;
    let worker = roomy();
    let (generation, _) = worker
        .call(None, &open_body(1, &paragraphs(&["measured"])))
        .await
        .expect("opened");
    let began = Instant::now();
    for _ in 0..ROUNDS {
        worker
            .call(Some(generation), &Request::StateVector(1))
            .await
            .expect("a state vector");
    }
    let round_trip = began.elapsed() / ROUNDS;
    let pid = worker.pid().await.expect("a worker runs");
    let before = resident_kib(pid);
    let texts: Vec<String> = (0..200)
        .map(|line| format!("line {line} of a body"))
        .collect();
    let texts: Vec<&str> = texts.iter().map(String::as_str).collect();
    let body = paragraphs(&texts);
    for doc in 2..22 {
        worker
            .call(None, &open_body(doc, &body))
            .await
            .expect("opened");
    }
    let after = resident_kib(pid);
    let body_bytes = serde_json::to_vec(&body).expect("JSON").len();
    report(&format!(
        "({}) worker round trip {round_trip:?}; resident {before} KiB with 1 document, \
         {after} KiB with 21 (20 of {body_bytes} bytes of JSON each)",
        profile()
    ));
    worker.end().await;
}

/// The time the worker takes over updates: a thousand small ones into one document
/// (without the log), and one of 60 KiB into a document whose body is near the limit
/// of a body; printed for the report and for the limit of an update.
#[cfg(target_os = "linux")]
#[tokio::test]
async fn the_cost_of_taking_updates_is_measured() {
    // Room for a machine that runs much else: a document of 1 MB takes long in a debug
    // build.
    let worker = worker(Duration::from_secs(120), 100);
    let (generation, state) = worker
        .call(None, &open_body(1, &paragraphs(&["measured"])))
        .await
        .expect("opened");
    let Answer::Opened {
        state: Some(state), ..
    } = state
    else {
        panic!("a document made from a body comes with its state: {state:?}");
    };
    let editor = Editor::join(101, &state);
    let take = |update: Vec<u8>, doc: u64| Request::Take { doc, update };
    let began = Instant::now();
    let mut slowest = Duration::ZERO;
    for n in 0..1_000 {
        let update = editor.append_paragraph(&format!("line {n}"));
        let one = Instant::now();
        worker
            .call(Some(generation), &take(update, 1))
            .await
            .expect("taken");
        slowest = slowest.max(one.elapsed());
    }
    let small = began.elapsed();

    let line = "w".repeat(1000);
    let texts: Vec<&str> = (0..960).map(|_| line.as_str()).collect();
    let (_, opened) = worker
        .call(Some(generation), &open_body(2, &paragraphs(&texts)))
        .await
        .expect("opened");
    let Answer::Opened {
        state: Some(state),
        state_bytes,
        ..
    } = opened
    else {
        panic!("a document made from a body comes with its state: {opened:?}");
    };
    let editor = Editor::join(102, &state);
    let update = editor.append_paragraph(&"v".repeat(60 * 1024));
    let large_bytes = update.len();
    let began = Instant::now();
    let answered = worker.call(Some(generation), &take(update, 2)).await;
    let large = began.elapsed();
    assert_eq!(
        refusal(answered),
        Refusal::BodyTooLarge,
        "past the limit of a body"
    );
    // An editor that holds the document without the refused update.
    let editor = Editor::join(103, &state);
    let update = editor.append_paragraph(&"u".repeat(20 * 1024));
    let began = Instant::now();
    worker
        .call(Some(generation), &take(update, 2))
        .await
        .expect("taken");
    let near = began.elapsed();
    report(&format!(
        "({}) 1000 updates taken by the worker in {small:?} ({:?} each, the slowest {slowest:?}); \
         into a document of {state_bytes} bytes (960 paragraphs): an update of {large_bytes} \
         bytes refused in {large:?}, one of 20 KiB taken in {near:?}",
        profile(),
        small / 1_000
    ));
    worker.end().await;
}

#[cfg(target_os = "linux")]
fn resident_kib(pid: u32) -> u64 {
    let status = std::fs::read_to_string(format!("/proc/{pid}/status")).expect("its status");
    status
        .lines()
        .find_map(|line| line.strip_prefix("VmRSS:"))
        .and_then(|value| value.split_whitespace().next())
        .and_then(|kib| kib.parse().ok())
        .expect("its resident memory")
}

/// Whether the tests run in a debug build: the times they print are of that.
#[cfg(target_os = "linux")]
fn profile() -> &'static str {
    if cfg!(debug_assertions) {
        "debug build"
    } else {
        "release build"
    }
}

#[cfg(target_os = "linux")]
#[expect(clippy::print_stderr, reason = "the measurements are for the report")]
fn report(line: &str) {
    eprintln!("measured: {line}");
}

#[cfg(unix)]
fn signal(pid: u32, name: &str) {
    let status = std::process::Command::new("kill")
        .args([format!("-{name}"), pid.to_string()])
        .status()
        .expect("kill runs");
    assert!(status.success(), "kill -{name} {pid}");
}

/// Whether a process of that identifier is there, a zombie included.
#[cfg(unix)]
fn is_alive(pid: u32) -> bool {
    let pid = libc::pid_t::try_from(pid).expect("a process identifier fits pid_t");
    // SAFETY: signal zero only checks whether this process exists; it sends no signal.
    if unsafe { libc::kill(pid, 0) } == 0 {
        return true;
    }
    let error = std::io::Error::last_os_error();
    if error.raw_os_error() == Some(libc::EPERM) {
        return true;
    }
    assert_eq!(
        error.raw_os_error(),
        Some(libc::ESRCH),
        "process {pid}: {error}"
    );
    false
}

/// Updates that push a table past 200 rows or 20 columns are refused over worker IPC
/// with `Refusal::TableTooLarge` while keeping the held document clean for later edits.
#[tokio::test]
async fn oversized_table_updates_return_table_too_large_over_worker_ipc() {
    let worker = roomy();
    let cell = json!({"type": "tableCell", "content": [{"type": "paragraph"}]});
    let row_1 = json!({"type": "tableRow", "content": [&cell]});
    let max_rows = json!({
        "type": "doc",
        "content": [{"type": "table", "content": vec![row_1; usize::from(fvoci_model::TABLE_MAX_ROWS)]}],
    });
    let row_20 = json!({
        "type": "tableRow",
        "content": vec![cell; usize::from(fvoci_model::TABLE_MAX_COLUMNS)],
    });
    let max_cols = json!({
        "type": "doc",
        "content": [{"type": "table", "content": [&row_20, &row_20]}],
    });

    let (generation, opened_rows) = worker
        .call(None, &open_body(1, &max_rows))
        .await
        .expect("opened 200-row doc");
    let Answer::Opened {
        state: Some(rows_state),
        ..
    } = opened_rows
    else {
        panic!("expected initial state");
    };
    let rows_editor = Editor::join(201, &rows_state);
    let rows_refused = worker
        .call(
            Some(generation),
            &Request::Take {
                doc: 1,
                update: rows_editor.add_table_row(1),
            },
        )
        .await;
    assert_eq!(refusal(rows_refused), Refusal::TableTooLarge);
    assert_eq!(
        body_of(&worker, generation, 1).await,
        max_rows,
        "200-row table stays unpolluted"
    );

    let (_, opened_cols) = worker
        .call(Some(generation), &open_body(2, &max_cols))
        .await
        .expect("opened 20-col doc");
    let Answer::Opened {
        state: Some(cols_state),
        ..
    } = opened_cols
    else {
        panic!("expected initial state");
    };
    let cols_editor = Editor::join(202, &cols_state);
    let cols_refused = worker
        .call(
            Some(generation),
            &Request::Take {
                doc: 2,
                update: cols_editor.add_table_column(),
            },
        )
        .await;
    assert_eq!(refusal(cols_refused), Refusal::TableTooLarge);
    assert_eq!(
        body_of(&worker, generation, 2).await,
        max_cols,
        "20-col table stays unpolluted"
    );

    // Subsequent valid edits succeed on both documents.
    worker
        .call(
            Some(generation),
            &Request::Take {
                doc: 1,
                update: Editor::join(203, &rows_state).append_paragraph("ok rows"),
            },
        )
        .await
        .expect("valid edit after 201-row refusal");
    worker
        .call(
            Some(generation),
            &Request::Take {
                doc: 2,
                update: Editor::join(204, &cols_state).append_paragraph("ok cols"),
            },
        )
        .await
        .expect("valid edit after 21-col refusal");
    worker.end().await;
}
